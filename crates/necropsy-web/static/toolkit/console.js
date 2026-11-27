// SPDX-License-Identifier: AGPL-3.0-or-later
// The request console. The player writes a curl command (or builds one from
// blocks); it is turned into the raw request curl would send, and the bytes go
// over a WebSocket to the player's own instance, which answers with the raw
// response. See docs/toolkit.md.

import {
  parseCurl, rawRequest, splitResponse, terminalOutput, blocksToCurl, curlToBlocks, blockLabel, CurlError,
} from "./curl.js";

let ws;
let els;
let socket = null;
let pending = null;
let mode = "text";
// Block mode's command: a path and blocks in the order they were added.
const model = { path: "/", blocks: [], flags: { include: true, verbose: false } };

export function init(shared, pane) {
  ws = shared;
  els = {
    target: pane.querySelector("#console-target"),
    toggle: pane.querySelector("#console-blocks"),
    builder: pane.querySelector("#console-builder"),
    form: pane.querySelector("#console-form"),
    cmd: pane.querySelector("#console-cmd"),
    msg: pane.querySelector("#console-msg"),
    term: pane.querySelector("#console-term"),
    req: pane.querySelector("#console-request"),
    res: pane.querySelector("#console-response"),
  };
  document.addEventListener("necropsy:instance", renderTarget);
  renderTarget();
  els.form.addEventListener("submit", (e) => {
    e.preventDefault();
    send();
  });
  els.cmd.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      send();
    }
  });
  els.toggle.addEventListener("change", () => setMode(els.toggle.checked ? "blocks" : "text"));
  els.cmd.addEventListener("input", () => {
    if (mode === "blocks") setMode("text", true);
  });
}

/** Another tool hands over a command. */
export function receive(payload) {
  if (payload && payload.curl) {
    setMode("text");
    els.cmd.value = payload.curl;
  }
}

function base() {
  return ws.instance ? ws.instance.url.replace(/\/+$/, "") : "https://<instance>";
}

function renderTarget() {
  if (!ws.signedIn) {
    els.target.textContent = "Sign in and start the instance to send requests.";
  } else if (ws.instance) {
    els.target.textContent = `Sends to ${ws.instance.host}.`;
    if (!els.cmd.value) els.cmd.value = `curl -i ${base()}/`;
  } else {
    els.target.textContent = "Start the instance to send requests.";
  }
  if (mode === "blocks") renderBlocks();
}

function say(text, err = false) {
  els.msg.textContent = text;
  els.msg.classList.toggle("err-text", err);
}

// ---- block mode ----------------------------------------------------------------

function setMode(next, quiet = false) {
  if (next === "blocks") {
    // Text to blocks works when the command uses only what blocks can show.
    const text = els.cmd.value.trim();
    if (text) {
      try {
        const b = curlToBlocks(text, base());
        model.path = b.path;
        model.blocks = b.blocks;
        model.flags = b.flags;
      } catch (e) {
        els.toggle.checked = false;
        say(`${e.message}`, true);
        return;
      }
    }
    mode = "blocks";
    els.toggle.checked = true;
    els.builder.classList.remove("hidden");
    els.cmd.readOnly = true;
    renderBlocks();
    if (!quiet) say("");
  } else {
    mode = "text";
    els.toggle.checked = false;
    els.builder.classList.add("hidden");
    els.cmd.readOnly = false;
  }
}

function input(value, label, onInput, width = 24) {
  const i = document.createElement("input");
  i.type = "text";
  i.value = value;
  i.size = Math.max(width, value.length + 2);
  i.setAttribute("aria-label", label);
  i.autocomplete = "off";
  i.spellcheck = false;
  i.addEventListener("input", () => {
    onInput(i.value);
    i.size = Math.max(width, i.value.length + 2);
    els.cmd.value = blocksToCurl(base(), model.path, model.blocks, model.flags);
  });
  return i;
}

function renderBlocks() {
  const b = els.builder;
  b.replaceChildren();
  const line = document.createElement("div");
  line.className = "block-line";
  const head = document.createElement("span");
  head.className = "block-fixed";
  head.textContent = `curl ${base()}`;
  line.append(head, input(model.path, "Path", (v) => (model.path = v), 12));
  b.append(line);

  model.blocks.forEach((blk, i) => {
    const row = document.createElement("div");
    row.className = "block-line block-added";
    const flag = document.createElement("span");
    flag.className = "block-fixed";
    flag.textContent = blockLabel(blk.kind);
    const placeholder = { method: "POST", header: "Name: value", data: "name=value", query: "name=value" }[blk.kind];
    const field = input(blk.value, `${blockLabel(blk.kind)} value`, (v) => (blk.value = v));
    field.placeholder = placeholder;
    const rm = document.createElement("button");
    rm.type = "button";
    rm.className = "link-button";
    rm.textContent = "Remove";
    rm.setAttribute("aria-label", `Remove ${blockLabel(blk.kind)} ${blk.value}`);
    rm.addEventListener("click", () => {
      model.blocks.splice(i, 1);
      renderBlocks();
    });
    row.append(flag, field, rm);
    b.append(row);
  });

  const add = document.createElement("div");
  add.className = "tool-row";
  for (const [kind, label] of [
    ["method", "Add method (-X)"],
    ["header", "Add header (-H)"],
    ["data", "Add body (--data)"],
    ["query", "Add query parameter"],
  ]) {
    if (kind === "method" && model.blocks.some((x) => x.kind === "method")) continue;
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "button button-quiet";
    btn.textContent = label;
    btn.addEventListener("click", () => {
      model.blocks.push({ kind, value: "" });
      renderBlocks();
      const fields = b.querySelectorAll(".block-added input");
      fields[fields.length - 1]?.focus();
    });
    add.append(btn);
  }
  for (const [flag, label] of [
    ["include", "Show headers (-i)"],
    ["verbose", "Verbose (-v)"],
  ]) {
    const l = document.createElement("label");
    l.className = "check";
    const c = document.createElement("input");
    c.type = "checkbox";
    c.checked = !!model.flags[flag];
    c.addEventListener("change", () => {
      model.flags[flag] = c.checked;
      els.cmd.value = blocksToCurl(base(), model.path, model.blocks, model.flags);
    });
    l.append(c, ` ${label}`);
    add.append(l);
  }
  b.append(add);
  els.cmd.value = blocksToCurl(base(), model.path, model.blocks, model.flags);
}

// ---- sending -------------------------------------------------------------------

function connect() {
  if (socket && socket.readyState <= 1 && socket.url === ws.instance.wsUrl) return Promise.resolve(socket);
  return new Promise((resolve, reject) => {
    const s = new WebSocket(ws.instance.wsUrl);
    s.binaryType = "arraybuffer";
    s.onopen = () => {
      socket = s;
      resolve(s);
    };
    s.onerror = () => reject(new Error("Could not connect to the instance. Is it still running?"));
    s.onclose = () => {
      if (socket === s) socket = null;
      if (pending) {
        pending.reject(new Error("The connection closed. Send again."));
        pending = null;
      }
    };
    s.onmessage = (e) => {
      if (!pending) return;
      const p = pending;
      pending = null;
      if (typeof e.data === "string") {
        let text = e.data;
        try {
          text = JSON.parse(e.data).error || text;
        } catch {}
        p.reject(new Error(text));
      } else {
        p.resolve(new Uint8Array(e.data));
      }
    };
  });
}

/** Sends raw request bytes to the instance and resolves with the raw response. */
export async function exchange(raw) {
  if (!ws.instance) await ws.refreshInstance();
  if (!ws.instance) throw new Error("Start the instance first.");
  const s = await connect();
  if (pending) throw new Error("A request is already on its way.");
  return new Promise((resolve, reject) => {
    pending = { resolve, reject };
    s.send(typeof raw === "string" ? new TextEncoder().encode(raw) : raw);
  });
}

function showBytes(pre, bytes) {
  // Text as text; anything that is not valid UTF-8 is shown with escapes.
  try {
    pre.textContent = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    pre.textContent = [...bytes]
      .map((b) => (b === 10 || b === 13 || (b >= 32 && b < 127) ? String.fromCharCode(b) : `\\x${b.toString(16).padStart(2, "0")}`))
      .join("");
  }
}

async function send() {
  say("");
  let p;
  try {
    p = parseCurl(els.cmd.value);
  } catch (e) {
    say(e instanceof CurlError ? e.message : "That command could not be read.", true);
    return;
  }
  if (!ws.instance) await ws.refreshInstance();
  if (!ws.instance) {
    say(ws.signedIn ? "Start the instance first." : "Sign in and start the instance first.", true);
    return;
  }
  if (p.host !== ws.instance.host.toLowerCase()) {
    say(`The console only reaches the instance, ${ws.instance.host}. Use that host in the URL.`, true);
    return;
  }
  const req = rawRequest(p);
  els.req.textContent = req.text;
  els.res.textContent = "";
  els.term.textContent = "";
  say("Sending...");
  try {
    const bytes = await exchange(req.text);
    const r = splitResponse(bytes);
    showBytes(els.res, bytes);
    els.term.textContent = terminalOutput(p, req.text, r);
    say(`${r.status} ${r.reason}`);
  } catch (e) {
    say(e.message, true);
  }
}
