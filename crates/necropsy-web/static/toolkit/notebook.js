// SPDX-License-Identifier: AGPL-3.0-or-later
// The notebook: Python cells run in a worker (Pyodide), with `await` allowed at
// the top level. Cells are kept in this browser, per puzzle.

let ws;
let els;
let worker = null;
let seq = 0;
const waiting = new Map();
let ready = null;
let key = "";

function call(op, args = {}) {
  return new Promise((resolve, reject) => {
    const id = ++seq;
    waiting.set(id, { resolve, reject });
    worker.postMessage({ id, op, ...args });
  });
}

function say(text, err = false) {
  els.msg.textContent = text;
  els.msg.classList.toggle("err-text", err);
}

function starter() {
  const f = ws.files[0];
  const cells = [];
  if (f) cells.push(`from necropsy import file\n\ndata = await file(${JSON.stringify(f.path)})\nprint(len(data), "bytes")\nprint(data[:200])`);
  if (ws.live) {
    cells.push(
      `from necropsy import request\n\nr = await request("GET", "/", headers={"User-Agent": "Mozilla/5.0"})\nprint(r.status, r.reason)\nprint(r.headers)\nprint(r.text[:500])`,
    );
  }
  if (!cells.length) cells.push(`import base64\n\nprint(base64.b64decode("bmVjcm9wc3k=").decode())`);
  return cells;
}

function load() {
  try {
    const saved = JSON.parse(localStorage.getItem(key) || "null");
    if (Array.isArray(saved) && saved.length) return saved;
  } catch {}
  return starter();
}

function persist() {
  try {
    const cells = [...els.cells.querySelectorAll("textarea")].map((t) => t.value);
    localStorage.setItem(key, JSON.stringify(cells));
  } catch {}
}

export async function init(shared, pane) {
  ws = shared;
  key = `necropsy:notebook:${ws.id}`;
  els = {
    msg: pane.querySelector("#notebook-msg"),
    cells: pane.querySelector("#notebook-cells"),
    add: pane.querySelector("#notebook-add"),
    runAll: pane.querySelector("#notebook-run-all"),
    cli: pane.querySelector("#cli-notebook"),
  };
  els.cli.textContent = 'python3 script.py  (request() is requests.request(), file() is open(name, "rb").read())';
  for (const c of load()) addCell(c);
  els.add.addEventListener("click", () => addCell("").querySelector("textarea").focus());
  els.runAll.addEventListener("click", async () => {
    for (const cell of els.cells.querySelectorAll(".nb-cell")) {
      if (!(await run(cell))) break;
    }
  });
  if (!ws.tools.pyodide) {
    say("Python is not installed on this server.", true);
    return;
  }
  worker = new Worker("/static/toolkit/py-worker.js", { type: "module" });
  worker.onmessage = (e) => {
    const w = waiting.get(e.data.id);
    if (!w) return;
    waiting.delete(e.data.id);
    if (e.data.ok) w.resolve(e.data.result);
    else w.reject(new Error(e.data.error));
  };
  worker.onerror = (e) => say(`Python stopped: ${e.message || "error"}`, true);
  ready = (async () => {
    say("Loading Python (about 10 MB, once)...");
    const helper = await (await fetch("/static/toolkit/necropsy.py")).text();
    await call("init", { base: ws.tools.pyodide, files: `/p/${encodeURIComponent(ws.id)}/files`, helper });
    await sendInstance();
    say("Python is ready. Shift+Enter runs a cell.");
  })().catch((e) => say(`Python did not load: ${e.message}`, true));
  document.addEventListener("necropsy:instance", sendInstance);
}

async function sendInstance() {
  if (!worker) return;
  const i = ws.instance;
  await call("instance", { url: i ? i.wsUrl : null, host: i ? i.host : null });
}

export function receive(payload) {
  if (payload && payload.file) {
    const cell = addCell(`from necropsy import file\n\ndata = await file(${JSON.stringify(payload.file)})\nprint(len(data), "bytes")`);
    cell.scrollIntoView({ block: "nearest" });
    cell.querySelector("textarea").focus();
  }
}

function addCell(code) {
  const cell = document.createElement("div");
  cell.className = "nb-cell";
  const ta = document.createElement("textarea");
  ta.value = code;
  ta.rows = Math.max(3, code.split("\n").length);
  ta.spellcheck = false;
  ta.autocapitalize = "off";
  ta.setAttribute("aria-label", "Python code");
  ta.addEventListener("input", () => {
    ta.rows = Math.max(3, ta.value.split("\n").length);
    persist();
  });
  ta.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && (e.shiftKey || e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      run(cell);
    } else if (e.key === "Tab" && !e.shiftKey) {
      e.preventDefault();
      ta.setRangeText("    ", ta.selectionStart, ta.selectionEnd, "end");
    }
  });
  const bar = document.createElement("div");
  bar.className = "tool-row";
  const runBtn = document.createElement("button");
  runBtn.type = "button";
  runBtn.className = "button button-quiet";
  runBtn.textContent = "Run";
  runBtn.addEventListener("click", () => run(cell));
  const del = document.createElement("button");
  del.type = "button";
  del.className = "link-button";
  del.textContent = "Delete";
  del.addEventListener("click", () => {
    cell.remove();
    persist();
  });
  bar.append(runBtn, del);
  const out = document.createElement("pre");
  out.className = "nb-out hidden";
  out.tabIndex = 0;
  cell.append(ta, bar, out);
  els.cells.append(cell);
  persist();
  return cell;
}

async function run(cell) {
  const out = cell.querySelector(".nb-out");
  const ta = cell.querySelector("textarea");
  out.classList.remove("hidden", "err-text");
  if (!ready) {
    out.textContent = "Python is not available.";
    return false;
  }
  out.textContent = "Running...";
  await ready;
  try {
    const r = await call("run", { code: ta.value });
    const parts = [];
    if (r.output) parts.push(r.output);
    if (r.value !== null && r.value !== undefined) parts.push(r.value);
    if (r.error) parts.push(r.error);
    out.textContent = parts.join("\n") || "(no output)";
    out.classList.toggle("err-text", !!r.error);
    return !r.error;
  } catch (e) {
    out.textContent = e.message;
    out.classList.add("err-text");
    return false;
  }
}
