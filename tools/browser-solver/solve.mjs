#!/usr/bin/env node
// SPDX-License-Identifier: AGPL-3.0-or-later
// The browser solver (docs/browser-solver.md): proves a pack can be solved
// with the in-browser workspace alone. It starts a site holding only that
// pack, then for each seed opens the workspace as a new signed-out player
// (a new browser, so a new player and new generated files), runs the pack's
// solve.browser.js against the page, types the answer it returns into the
// answer form and checks the site says "Correct.".
//
//   node tools/browser-solver/solve.mjs --pack <pack-dir> --site-bin <necropsy-web>
//        [--toolkit <dir>] [--seeds N] [--timeout SECONDS] [--keep]
//
// The script only gets the page and the helpers below: never the seed, the
// server secret or the pack's other files. Chromium comes from Playwright, or
// NECROPSY_CHROMIUM if set.

import { spawn } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

export const SCRIPT = "solve.browser.js";

function args(argv) {
  const out = { seeds: 5, timeout: 120, keep: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const val = () => {
      if (i + 1 >= argv.length) throw new Error(`${a} needs a value`);
      return argv[++i];
    };
    if (a === "--pack") out.pack = resolve(val());
    else if (a === "--site-bin") out.siteBin = resolve(val());
    else if (a === "--toolkit") out.toolkit = resolve(val());
    else if (a === "--seeds") out.seeds = Number(val());
    else if (a === "--timeout") out.timeout = Number(val());
    else if (a === "--keep") out.keep = true;
    else throw new Error(`unknown option ${a}`);
  }
  if (!out.pack || !out.siteBin) throw new Error("--pack and --site-bin are required");
  if (!Number.isInteger(out.seeds) || out.seeds < 1 || out.seeds > 50) throw new Error("--seeds is 1 to 50");
  if (!(out.timeout > 0)) throw new Error("--timeout is a number of seconds");
  return out;
}

/** The pack's `date` line set to yesterday (UTC), so the copy is a released
 * daily that every timezone can open. Seeds do not depend on the date. */
export function releasedCopy(toml, today = new Date()) {
  const y = new Date(Date.UTC(today.getUTCFullYear(), today.getUTCMonth(), today.getUTCDate() - 1));
  const date = y.toISOString().slice(0, 10);
  const lines = toml.split("\n").filter((l) => !/^\s*date\s*=/.test(l));
  const at = lines.findIndex((l) => /^\s*track\s*=/.test(l));
  lines.splice(at + 1, 0, `date = "${date}"`);
  return lines.join("\n");
}

function freePort() {
  return new Promise((res, rej) => {
    const s = createServer();
    s.once("error", rej);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => res(port));
    });
  });
}

async function waitFor(url, site, ms) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (site.exitCode !== null) throw new Error(`the site stopped (exit ${site.exitCode})`);
    try {
      const r = await fetch(url);
      if (r.ok) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`the site did not answer ${url}`);
}

/** The helpers a solve script gets. Each drives the workspace as a player would. */
export function helpers(page) {
  const t = {
    page,
    /** Opens a workspace tab: files, pcap, decoder, console, notebook, hex. */
    async tool(name) {
      await page.click(`#tab-${name}`);
      await page.waitForSelector(`#pane-${name}:not(.hidden)`);
    },
    /** The puzzle's files, as the files pane lists them. */
    async files() {
      await t.tool("files");
      return page.$$eval("#files-list .file-item a", (as) => as.map((a) => a.textContent));
    },
    /** Opens a text file in the files pane's viewer, as clicking its name does,
     * and returns its contents. */
    async view(path) {
      await t.tool("files");
      await page.locator("#files-list .file-item > a", { hasText: path }).first().click();
      await page.waitForFunction(
        (p) =>
          document.querySelector("#file-view-name")?.textContent === p &&
          document.querySelector("#file-view-body")?.textContent !== "Loading...",
        path,
      );
      return page.textContent("#file-view-body");
    },
    /** Types `code` into a new notebook cell, runs it, and returns what it printed. */
    async python(code, { timeout = 90_000 } = {}) {
      await t.tool("notebook");
      await page.waitForFunction(
        () => /ready|not load|not installed/i.test(document.querySelector("#notebook-msg")?.textContent || ""),
        null,
        { timeout },
      );
      const msg = await page.textContent("#notebook-msg");
      if (!/ready/i.test(msg)) throw new Error(`the notebook: ${msg}`);
      const before = await page.$$eval("#notebook-cells .nb-cell", (c) => c.length);
      await page.click("#notebook-add");
      const cell = page.locator("#notebook-cells .nb-cell").nth(before);
      await cell.locator("textarea").fill(code);
      await cell.getByRole("button", { name: "Run" }).click();
      const out = cell.locator(".nb-out");
      await page.waitForFunction(
        (el) => el && !el.classList.contains("hidden") && el.textContent !== "Running...",
        await out.elementHandle(),
        { timeout },
      );
      const text = await out.textContent();
      if (await out.evaluate((el) => el.classList.contains("err-text"))) throw new Error(`the notebook cell failed:\n${text}`);
      return text;
    },
  };
  return t;
}

async function solveOne(browser, base, id, solve, timeout) {
  const context = await browser.newContext();
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  try {
    await page.goto(`${base}/w/${encodeURIComponent(id)}`);
    const work = (async () => String(await solve(helpers(page))).trim())();
    let timer;
    const answer = await Promise.race([
      work,
      new Promise((_, rej) => {
        timer = setTimeout(() => rej(new Error(`no answer within ${timeout} s`)), timeout * 1000);
      }),
    ]).finally(() => clearTimeout(timer));
    if (!answer) throw new Error("the script returned no answer");
    await page.fill("#answer", answer);
    await page.click("#answer-form button[type=submit]");
    await page.waitForFunction(() => /\S/.test(document.querySelector("#answer-msg")?.textContent || ""));
    const verdict = (await page.textContent("#answer-msg")).trim();
    return { answer, ok: verdict === "Correct.", verdict, errors };
  } finally {
    await context.close();
  }
}

export async function run(opts, log = console.log) {
  const toml = join(opts.pack, "pack.toml");
  const script = join(opts.pack, SCRIPT);
  if (!existsSync(toml)) throw new Error(`${opts.pack} has no pack.toml`);
  if (!existsSync(script)) throw new Error(`${opts.pack} has no ${SCRIPT}`);
  const id = (readFileSync(toml, "utf8").match(/^\s*id\s*=\s*"([^"]+)"/m) || [])[1] || basename(opts.pack);

  const work = mkdtempSync(join(tmpdir(), "necropsy-solve-"));
  const packs = join(work, "packs");
  mkdirSync(packs);
  // The site gets the pack without its solve script, as players do.
  cpSync(opts.pack, join(packs, id), { recursive: true, filter: (src) => basename(src) !== SCRIPT });
  writeFileSync(join(packs, id, "pack.toml"), releasedCopy(readFileSync(toml, "utf8")));
  for (const d of ["learn", "sealed"]) mkdirSync(join(work, d));

  const port = await freePort();
  const base = `http://127.0.0.1:${port}`;
  const env = {
    PATH: process.env.PATH,
    HOME: work,
    NECROPSY_ADDR: `127.0.0.1:${port}`,
    NECROPSY_BASE_URL: base,
    NECROPSY_DB: join(work, "necropsy.db"),
    NECROPSY_PACKS: packs,
    NECROPSY_LEARN: join(work, "learn"),
    NECROPSY_SEALED: join(work, "sealed"),
    RUST_LOG: "warn",
  };
  if (opts.toolkit) env.NECROPSY_TOOLKIT = opts.toolkit;
  const site = spawn(opts.siteBin, [], { env, cwd: work, stdio: ["ignore", "pipe", "pipe"] });
  let siteLog = "";
  site.stdout.on("data", (d) => (siteLog += d));
  site.stderr.on("data", (d) => (siteLog += d));

  let browser;
  const results = [];
  try {
    await waitFor(`${base}/healthz`, site, 30_000);
    const { chromium } = require("playwright");
    browser = await chromium.launch(process.env.NECROPSY_CHROMIUM ? { executablePath: process.env.NECROPSY_CHROMIUM } : {});
    const solve = (await import(pathToFileURL(script).href)).default;
    if (typeof solve !== "function") throw new Error(`${SCRIPT} must export a default async function`);
    for (let i = 0; i < opts.seeds; i++) {
      const started = Date.now();
      let r;
      try {
        r = await solveOne(browser, base, id, solve, opts.timeout);
      } catch (e) {
        r = { ok: false, answer: null, verdict: e.message.split("\n").slice(0, 6).join("\n"), errors: [] };
      }
      r.seconds = (Date.now() - started) / 1000;
      results.push(r);
      log(`${id}: player ${i + 1}: ${r.ok ? "solved" : "FAILED"} in ${r.seconds.toFixed(1)} s` + (r.ok ? "" : `: ${r.answer === null ? "" : `answered ${JSON.stringify(r.answer)}, `}${r.verdict}`));
      for (const e of r.errors) log(`  page error: ${e}`);
    }
  } catch (e) {
    if (siteLog) log(siteLog.trim().split("\n").slice(-20).join("\n"));
    throw e;
  } finally {
    if (browser) await browser.close();
    site.kill("SIGTERM");
    if (!opts.keep) rmSync(work, { recursive: true, force: true });
  }
  const answers = new Set(results.filter((r) => r.ok).map((r) => r.answer));
  return { id, results, solved: results.filter((r) => r.ok).length, distinct: answers.size };
}

if (import.meta.url === pathToFileURL(process.argv[1] || "").href) {
  let opts;
  try {
    opts = args(process.argv.slice(2));
  } catch (e) {
    console.error(`solve.mjs: ${e.message}`);
    process.exit(2);
  }
  run(opts)
    .then((r) => {
      const all = r.solved === r.results.length;
      console.log(`${r.id}: ${all ? "ok" : "failed"}, ${r.solved} of ${r.results.length} players solved it in the browser (${r.distinct} different answers)`);
      process.exit(all ? 0 : 1);
    })
    .catch((e) => {
      console.error(`solve.mjs: ${e.message}`);
      process.exit(1);
    });
}
