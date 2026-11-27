// SPDX-License-Identifier: AGPL-3.0-or-later
// The pcap viewer: packet list with Wireshark display filters, the protocol
// tree of a packet, and Follow TCP/HTTP stream. Each view shows the tshark
// command that gives the same result in a terminal.

let ws;
let els;
let worker = null;
let seq = 0;
const waiting = new Map();
let current = null; // { path, name }
let filter = "";
let stream = null; // { kind, filter, text }

function call(op, args = {}, transfer = []) {
  return new Promise((resolve, reject) => {
    const id = ++seq;
    waiting.set(id, { resolve, reject });
    worker.postMessage({ id, op, ...args }, transfer);
  });
}

function say(text, err = false) {
  els.msg.textContent = text;
  els.msg.classList.toggle("err-text", err);
}

function q(s) {
  return /^[A-Za-z0-9_.\/-]+$/.test(s) ? s : `'${s.replace(/'/g, `'\\''`)}'`;
}

function renderCli() {
  const name = current ? current.name : "capture.pcap";
  if (stream) {
    const n = /tcp\.stream eq (\d+)/.exec(stream.filter);
    els.cli.textContent = `tshark -r ${q(name)} -q -z follow,${stream.kind.toLowerCase()},ascii,${n ? n[1] : 0}`;
  } else {
    els.cli.textContent = `tshark -r ${q(name)}${filter ? ` -Y ${q(filter)}` : ""}`;
  }
}

export async function init(shared, pane) {
  ws = shared;
  els = {
    file: pane.querySelector("#pcap-file"),
    form: pane.querySelector("#pcap-filter-form"),
    filter: pane.querySelector("#pcap-filter"),
    msg: pane.querySelector("#pcap-msg"),
    body: pane.querySelector("#pcap-table tbody"),
    detail: pane.querySelector("#pcap-detail"),
    tree: pane.querySelector("#pcap-tree"),
    stream: pane.querySelector("#pcap-stream"),
    followTcp: pane.querySelector("#pcap-follow-tcp"),
    followHttp: pane.querySelector("#pcap-follow-http"),
    toDecoder: pane.querySelector("#pcap-to-decoder"),
    cli: pane.querySelector("#cli-pcap"),
  };
  const pcaps = ws.files.filter((f) => f.kind === "pcap");
  for (const f of pcaps) els.file.append(new Option(f.path, f.path));
  renderCli();
  if (!pcaps.length) {
    els.file.disabled = true;
    say("This puzzle has no capture.");
    els.form.querySelector("button").disabled = true;
    return;
  }
  if (!ws.tools.wiregasm) {
    say("The pcap viewer is not installed on this server.", true);
    return;
  }
  worker = new Worker("/static/toolkit/pcap-worker.js");
  worker.onmessage = (e) => {
    const w = waiting.get(e.data.id);
    if (!w) return;
    waiting.delete(e.data.id);
    if (e.data.ok) w.resolve(e.data.result);
    else w.reject(new Error(e.data.error));
  };
  worker.onerror = (e) => say(`The pcap viewer stopped: ${e.message || "error"}`, true);
  say("Loading Wireshark (about 20 MB, once)...");
  await call("init", { base: ws.tools.wiregasm });
  els.file.addEventListener("change", () => open(els.file.value));
  els.form.addEventListener("submit", (e) => {
    e.preventDefault();
    applyFilter(els.filter.value.trim());
  });
  els.followTcp.addEventListener("click", () => follow("TCP"));
  els.followHttp.addEventListener("click", () => follow("HTTP"));
  els.toDecoder.addEventListener("click", () => {
    if (stream) ws.send("decoder", { input: stream.text });
  });
  await open(pcaps[0].path);
}

export async function receive(payload) {
  if (!payload || !payload.path || !worker) return;
  els.file.value = payload.path;
  await open(payload.path);
}

async function open(path) {
  try {
    say(`Opening ${path}...`);
    const bytes = (await ws.fileBytes(path)).slice();
    const r = await call("load", { name: path.split("/").pop(), bytes }, [bytes.buffer]);
    current = { path, name: path.split("/").pop() };
    say(`${r.packets} packets.`);
    await applyFilter(els.filter.value.trim());
  } catch (e) {
    say(e.message, true);
  }
}

async function applyFilter(f) {
  if (!current) return;
  if (f) {
    const c = await call("check", { filter: f });
    if (!c.ok) {
      say(`Not a valid filter: ${c.error}`, true);
      return;
    }
  }
  filter = f;
  stream = null;
  renderCli();
  const LIMIT = 5000;
  const r = await call("frames", { filter: f, limit: LIMIT });
  els.body.replaceChildren();
  for (const fr of r.frames) {
    const tr = document.createElement("tr");
    tr.tabIndex = 0;
    for (const c of fr.columns.slice(0, 7)) {
      const td = document.createElement("td");
      td.textContent = c;
      tr.append(td);
    }
    const pick = () => select(fr.number, tr);
    tr.addEventListener("click", pick);
    tr.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        pick();
      }
    });
    els.body.append(tr);
  }
  els.detail.classList.add("hidden");
  say(
    f
      ? `${r.matched} packets match.${r.matched > LIMIT ? ` Showing the first ${LIMIT}.` : ""}`
      : `${r.matched} packets.${r.matched > LIMIT ? ` Showing the first ${LIMIT}.` : ""}`,
  );
}

let followable = [];

async function select(number, row) {
  for (const r of els.body.querySelectorAll("tr")) r.classList.toggle("selected", r === row);
  const f = await call("frame", { number });
  followable = f.follow;
  els.followTcp.disabled = !followable.some(([p]) => p === "TCP");
  els.followHttp.disabled = !followable.some(([p]) => p === "HTTP");
  els.stream.classList.add("hidden");
  els.toDecoder.classList.add("hidden");
  stream = null;
  renderCli();
  els.tree.replaceChildren(renderTree(f.tree));
  els.detail.classList.remove("hidden");
}

function renderTree(nodes) {
  const ul = document.createElement("ul");
  for (const n of nodes) {
    const li = document.createElement("li");
    if (n.tree.length) {
      const d = document.createElement("details");
      const s = document.createElement("summary");
      s.textContent = n.label;
      d.append(s, renderTree(n.tree));
      li.append(d);
    } else {
      li.textContent = n.label;
    }
    if (n.filter) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "link-button tree-filter";
      b.textContent = "Filter";
      b.title = `Apply ${n.filter} as a display filter`;
      b.addEventListener("click", (e) => {
        e.preventDefault();
        els.filter.value = n.filter;
        applyFilter(n.filter);
      });
      (li.querySelector("summary") || li).append(" ", b);
    }
    ul.append(li);
  }
  return ul;
}

async function follow(kind) {
  const cand = followable.find(([p]) => p === kind);
  if (!cand) return;
  const r = await call("follow", { kind, filter: cand[1] });
  const dec = new TextDecoder();
  const text = r.payloads
    .map((p) => {
      const bin = atob(p.data);
      const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0));
      return dec.decode(bytes);
    })
    .join("");
  stream = { kind, filter: cand[1], text };
  els.stream.textContent = text || "(empty)";
  els.stream.classList.remove("hidden");
  els.toDecoder.classList.remove("hidden");
  // The stream's packets, as Wireshark shows them after Follow.
  els.filter.value = cand[1];
  filter = cand[1];
  renderCli();
  say(`${kind} stream ${r.client} ↔ ${r.server}.`);
}
