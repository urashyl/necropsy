// SPDX-License-Identifier: AGPL-3.0-or-later
// Wireshark's dissectors (Wiregasm, compiled to WebAssembly) in a worker, so a
// large capture does not freeze the page. Messages: { id, op, ... } in,
// { id, ok, result | error } out. See docs/toolkit.md.
"use strict";

let lib = null;
let session = null;

/** An embind vector as an array; the vector is freed. */
function arr(v) {
  try {
    const out = [];
    for (let i = 0; i < v.size(); i++) out.push(v.get(i));
    return out;
  } finally {
    if (v.delete) v.delete();
  }
}

function tree(nodes) {
  return arr(nodes).map((n) => ({ label: n.label, filter: n.filter, tree: tree(n.tree) }));
}

const ops = {
  async init({ base }) {
    if (lib) return true;
    importScripts(`${base}/wiregasm.js`);
    // eslint-disable-next-line no-undef
    lib = await loadWiregasm({
      locateFile: (f) => `${base}/${f}`,
      print: () => {},
      printErr: () => {},
      handleStatus: () => {},
    });
    if (!lib.init()) throw new Error("Wireshark did not start.");
    return true;
  },
  load({ name, bytes }) {
    if (session) session.delete();
    const path = `${lib.getUploadDirectory()}/${name.replace(/[^A-Za-z0-9._-]/g, "_")}`;
    lib.FS.writeFile(path, bytes);
    session = new lib.DissectSession(path);
    const r = session.load();
    if (r.code !== 0) {
      session.delete();
      session = null;
      throw new Error("This file is not a capture Wireshark can read.");
    }
    return { packets: r.summary.packet_count };
  },
  check({ filter }) {
    const r = lib.checkFilter(filter);
    return { ok: r.ok, error: r.error };
  },
  frames({ filter, limit }) {
    const r = session.getFrames(filter, 0, limit);
    return {
      matched: r.matched,
      frames: arr(r.frames).map((f) => ({ number: f.number, columns: arr(f.columns) })),
    };
  },
  frame({ number }) {
    const f = session.getFrame(number);
    const follow = arr(f.follow).map((x) => arr(x));
    const out = { number: f.number, tree: tree(f.tree), follow };
    if (f.data_sources && f.data_sources.delete) f.data_sources.delete();
    if (f.comments && f.comments.delete) f.comments.delete();
    return out;
  },
  follow({ kind, filter }) {
    const r = session.follow(kind, filter);
    return {
      client: `${r.chost}:${r.cport}`,
      server: `${r.shost}:${r.sport}`,
      payloads: arr(r.payloads).map((p) => ({ server: !!p.server, data: p.data })),
    };
  },
};

self.onmessage = async (e) => {
  const { id, op, ...args } = e.data;
  try {
    if (op !== "init" && !lib) throw new Error("Wireshark is not loaded.");
    if (!["init", "load", "check"].includes(op) && !session) throw new Error("No capture is open.");
    postMessage({ id, ok: true, result: await ops[op](args) });
  } catch (err) {
    postMessage({ id, ok: false, error: err && err.message ? err.message : String(err) });
  }
};
