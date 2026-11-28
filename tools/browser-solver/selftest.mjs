#!/usr/bin/env node
// SPDX-License-Identifier: AGPL-3.0-or-later
// Checks the browser solver refuses what it must: a wrong answer, a script
// that throws, one that never answers, and one that answers what the pack's
// own files gave a different player. Runs against a real site and Chromium.
//
//   node tools/browser-solver/selftest.mjs <necropsy-web> <toolkit-dir>

import assert from "node:assert/strict";
import { cpSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { run, SCRIPT } from "./solve.mjs";

const [siteBin, toolkit] = process.argv.slice(2).map((p) => resolve(p));
if (!siteBin || !toolkit) {
  console.error("usage: selftest.mjs <necropsy-web> <toolkit-dir>");
  process.exit(2);
}
const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const work = mkdtempSync(join(tmpdir(), "necropsy-selftest-"));
const quiet = () => {};

/** A copy of the kerberoast pack with `script` as its solve script. */
function packWith(name, script) {
  const dir = join(work, name, "kerberoast");
  cpSync(join(repo, "packs/kerberoast"), dir, { recursive: true });
  writeFileSync(join(dir, SCRIPT), script);
  return dir;
}

const cases = [
  ["a wrong answer", 'export default async () => "nobody";', /Not it\./],
  ["a script that throws", 'export default async () => { throw new Error("broken"); };', /broken/],
  ["no answer in time", "export default () => new Promise(() => {});", /no answer within 3 s/],
  ["an empty answer", 'export default async () => "  ";', /returned no answer/],
];

let failed = 0;
try {
  for (const [name, script, why] of cases) {
    const r = await run({ pack: packWith(name.replace(/\W+/g, "-"), script), siteBin, toolkit, seeds: 1, timeout: 3 }, quiet);
    try {
      assert.equal(r.solved, 0, `${name}: counted as solved`);
      assert.match(r.results[0].verdict, why);
      console.log(`ok: ${name} is refused`);
    } catch (e) {
      failed++;
      console.log(`FAILED: ${e.message}`);
    }
  }
  // Clicking a file's name shows it in the page; it does not download it.
  const viewing = packWith(
    "viewing",
    `import real from "${join(repo, "packs/kerberoast", SCRIPT).replace(/\\/g, "/")}";
export default async (t) => {
  let downloads = 0;
  t.page.on("download", () => downloads++);
  const text = await t.view("4769.csv");
  if (!text.startsWith("TimeCreated,")) throw new Error("the viewer shows " + JSON.stringify(text.slice(0, 40)));
  await t.page.waitForTimeout(500);
  if (downloads) throw new Error("clicking the file downloaded it");
  return real(t);
};`,
  );
  const v = await run({ pack: viewing, siteBin, toolkit, seeds: 1, timeout: 120 }, quiet);
  try {
    assert.equal(v.solved, 1, v.results[0].verdict);
    console.log("ok: a file opens in the page, without a download");
  } catch (e) {
    failed++;
    console.log(`FAILED: viewing a file: ${e.message}`);
  }
  // One player's answer is not another's: a script that remembers the first
  // answer it found solves only the first player.
  const sticky = packWith(
    "sticky",
    `import real from "${join(repo, "packs/kerberoast", SCRIPT).replace(/\\/g, "/")}";
let first;
export default async (t) => (first ??= await real(t));`,
  );
  const r = await run({ pack: sticky, siteBin, toolkit, seeds: 2, timeout: 120 }, quiet);
  try {
    assert.deepEqual(r.results.map((x) => x.ok), [true, false]);
    console.log("ok: each player has their own answer");
  } catch (e) {
    failed++;
    console.log(`FAILED: another player's answer: ${e.message}`);
  }
} finally {
  rmSync(work, { recursive: true, force: true });
}
process.exit(failed ? 1 : 0);
