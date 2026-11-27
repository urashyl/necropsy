// SPDX-License-Identifier: AGPL-3.0-or-later
// Python (Pyodide, compiled to WebAssembly) in a worker, for the notebook.
// Messages: { id, op, ... } in, { id, ok, result | error } out. The necropsy
// module's request() goes over a WebSocket to the player's own instance, the
// same transport as the request console. See docs/toolkit.md.

let py = null;
let fileBase = "";
let wsUrl = null;
let instanceHost = null;
let sock = null;
let pending = null;

function connect() {
  if (sock && sock.readyState <= 1 && sock.url === wsUrl) return Promise.resolve(sock);
  return new Promise((resolve, reject) => {
    const s = new WebSocket(wsUrl);
    s.binaryType = "arraybuffer";
    s.onopen = () => {
      sock = s;
      resolve(s);
    };
    s.onerror = () => reject(new Error("Could not connect to the instance. Is it still running?"));
    s.onclose = () => {
      if (sock === s) sock = null;
      if (pending) {
        pending.reject(new Error("The connection closed."));
        pending = null;
      }
    };
    s.onmessage = (e) => {
      const p = pending;
      pending = null;
      if (!p) return;
      if (typeof e.data === "string") {
        let text = e.data;
        try {
          text = JSON.parse(e.data).error || text;
        } catch {}
        p.reject(new Error(text));
      } else p.resolve(new Uint8Array(e.data));
    };
  });
}

const bridge = {
  instance_host: () => instanceHost,
  async request(raw) {
    if (!wsUrl) throw new Error("Start the instance first.");
    const s = await connect();
    while (pending) await new Promise((r) => setTimeout(r, 20));
    return new Promise((resolve, reject) => {
      pending = { resolve, reject };
      s.send(raw);
    });
  },
  async file(name) {
    const url = `${fileBase}/${String(name).split("/").map(encodeURIComponent).join("/")}`;
    const r = await fetch(url, { credentials: "same-origin" });
    if (!r.ok) throw new Error(`This puzzle has no file ${name}.`);
    return new Uint8Array(await r.arrayBuffer());
  },
};

let out = [];

const ops = {
  async init({ base, files, helper }) {
    if (py) return true;
    fileBase = files;
    const { loadPyodide } = await import(`${base}/pyodide.mjs`);
    py = await loadPyodide({
      indexURL: `${base}/`,
      stdout: (s) => out.push(s),
      stderr: (s) => out.push(s),
    });
    py.registerJsModule("necropsy_bridge", bridge);
    py.FS.writeFile("/home/pyodide/necropsy.py", helper);
    py.runPython("import sys; sys.path.insert(0, '/home/pyodide')");
    return true;
  },
  instance({ url, host }) {
    wsUrl = url;
    instanceHost = host;
    return true;
  },
  async run({ code }) {
    out = [];
    try {
      const v = await py.runPythonAsync(code);
      let shown = null;
      if (v !== undefined && v !== null) {
        shown = py.globals.get("repr")(v);
        if (v.destroy) v.destroy();
      }
      return { output: out.join("\n"), value: shown };
    } catch (e) {
      // Python's traceback, without Pyodide's own frames.
      const msg = String(e.message || e);
      const lines = msg.split("\n");
      const user = lines.findIndex((l) => l.includes('File "<exec>"'));
      const tb = user > 0 ? ["Traceback (most recent call last):", ...lines.slice(user)] : lines;
      return { output: out.join("\n"), error: tb.join("\n").trim() };
    }
  },
};

self.onmessage = async (e) => {
  const { id, op, ...args } = e.data;
  try {
    if (op !== "init" && !py) throw new Error("Python is not loaded.");
    postMessage({ id, ok: true, result: await ops[op](args) });
  } catch (err) {
    postMessage({ id, ok: false, error: err && err.message ? err.message : String(err) });
  }
};
