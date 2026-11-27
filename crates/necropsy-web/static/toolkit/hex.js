// SPDX-License-Identifier: AGPL-3.0-or-later
// The hex and strings viewer for binary files and string tables. The view
// functions are pure, so node tests them.

const PAGE = 64 * 1024;

/** `xxd`-style lines: offset, 16 bytes in hex, the printable ones as text. */
export function hexdump(bytes, start = 0, length = PAGE) {
  const end = Math.min(bytes.length, start + length);
  const lines = [];
  for (let off = start - (start % 16); off < end; off += 16) {
    const row = bytes.subarray(off, Math.min(off + 16, bytes.length));
    let hex = "";
    for (let i = 0; i < 16; i++) {
      hex += i < row.length ? row[i].toString(16).padStart(2, "0") : "  ";
      if (i % 2 === 1) hex += " ";
    }
    const text = [...row].map((b) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".")).join("");
    lines.push(`${off.toString(16).padStart(8, "0")}: ${hex} ${text}`);
  }
  return lines.join("\n");
}

/** Runs of at least `min` printable ASCII characters, with their offsets, like `strings -t x -n min`. */
export function strings(bytes, min = 6) {
  const out = [];
  let start = -1;
  for (let i = 0; i <= bytes.length; i++) {
    const b = bytes[i];
    const printable = i < bytes.length && ((b >= 0x20 && b < 0x7f) || b === 0x09);
    if (printable && start < 0) start = i;
    if (!printable && start >= 0) {
      if (i - start >= min) {
        let s = "";
        for (let j = start; j < i; j++) s += String.fromCharCode(bytes[j]);
        out.push({ offset: start, text: s });
      }
      start = -1;
    }
  }
  return out;
}

/** An offset typed as decimal or 0x hex; null if it is neither. */
export function parseOffset(s) {
  const t = String(s).trim().toLowerCase();
  if (t === "") return 0;
  if (/^0x[0-9a-f]+$/.test(t)) return parseInt(t, 16);
  if (/^[0-9]+$/.test(t)) return parseInt(t, 10);
  return null;
}

let ws;
let els;
let bytes = new Uint8Array();
let name = "file";
let shown = 0;
let start = 0;

export function init(shared, pane) {
  ws = shared;
  els = {
    file: pane.querySelector("#hex-file"),
    mode: pane.querySelector("#hex-mode"),
    form: pane.querySelector("#hex-jump-form"),
    offset: pane.querySelector("#hex-offset"),
    min: pane.querySelector("#hex-min"),
    msg: pane.querySelector("#hex-msg"),
    view: pane.querySelector("#hex-view"),
    more: pane.querySelector("#hex-more"),
    cli: pane.querySelector("#cli-hex"),
  };
  for (const f of ws.files) els.file.append(new Option(f.path, f.path));
  if (!ws.files.length) {
    els.msg.textContent = "This puzzle has no files.";
    return;
  }
  els.file.addEventListener("change", () => open(els.file.value));
  els.mode.addEventListener("change", render);
  els.form.addEventListener("submit", (e) => {
    e.preventDefault();
    const o = parseOffset(els.offset.value);
    if (o === null || o > bytes.length) {
      els.msg.textContent = `Give an offset from 0 to ${bytes.length}, as a number or 0x hex.`;
      return;
    }
    start = o;
    render();
  });
  els.more.addEventListener("click", () => {
    shown += PAGE;
    els.view.textContent += `\n${hexdump(bytes, start + shown, PAGE)}`;
    els.more.classList.toggle("hidden", start + shown + PAGE >= bytes.length);
  });
  open(ws.files[0].path);
}

export function receive(payload) {
  if (payload && payload.path) {
    els.file.value = payload.path;
    open(payload.path);
  }
}

async function open(path) {
  try {
    bytes = await ws.fileBytes(path);
    name = path.split("/").pop();
    start = 0;
    els.offset.value = "";
    render();
  } catch (e) {
    els.msg.textContent = e.message;
  }
}

function render() {
  shown = 0;
  const q = /^[A-Za-z0-9_.-]+$/.test(name) ? name : `'${name}'`;
  if (els.mode.value === "strings") {
    const min = Math.min(Math.max(Number(els.min.value) || 6, 2), 64);
    const list = strings(bytes, min).filter((s) => s.offset >= start);
    els.view.textContent = list.map((s) => `${`0x${s.offset.toString(16)}`.padStart(10, " ")}  ${s.text}`).join("\n");
    els.more.classList.add("hidden");
    els.msg.textContent = `${list.length} strings of ${min} or more characters.`;
    els.cli.textContent = `strings -t x -n ${min} ${q}`;
  } else {
    els.view.textContent = hexdump(bytes, start, PAGE);
    els.more.classList.toggle("hidden", start + PAGE >= bytes.length);
    els.msg.textContent = `${bytes.length} bytes.`;
    els.cli.textContent = start ? `xxd -s 0x${start.toString(16)} ${q} | less` : `xxd ${q} | less`;
  }
}
