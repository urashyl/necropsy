// SPDX-License-Identifier: AGPL-3.0-or-later
// The workspace page: tabs, the files pane, and what the tools share (the
// puzzle's files, the instance, moving data between tools). Each tool's script
// is loaded the first time its tab opens. The answer form, hints and clock on
// this page are app.js's, as on the puzzle page.

const root = document.getElementById("puzzle");
const id = root.dataset.id;
const files = JSON.parse(root.dataset.files || "[]");

/** What the tools share. */
export const ws = {
  id,
  files,
  live: root.dataset.live === "true",
  signedIn: root.dataset.signedIn === "true",
  tools: {
    wiregasm: root.dataset.wiregasm || null,
    pyodide: root.dataset.pyodide || null,
    cyberchef: root.dataset.cyberchef || null,
  },
  fileUrl: (path) => `/p/${encodeURIComponent(id)}/files/${path.split("/").map(encodeURIComponent).join("/")}`,
  cache: new Map(),
  /** A puzzle file's bytes, fetched once. */
  async fileBytes(path) {
    if (!this.cache.has(path)) {
      this.cache.set(
        path,
        fetch(this.fileUrl(path), { credentials: "same-origin" }).then(async (r) => {
          if (!r.ok) throw new Error(`Could not load ${path}.`);
          return new Uint8Array(await r.arrayBuffer());
        }),
      );
    }
    return this.cache.get(path);
  },
  /** The running instance: { url, host, wsUrl } or null. */
  instance: null,
  async refreshInstance() {
    if (!this.live || !this.signedIn) return null;
    try {
      const r = await fetch(`/api/p/${encodeURIComponent(id)}/instance`, { credentials: "same-origin" });
      const v = r.ok ? await r.json() : { active: false };
      if (v.active && v.url) {
        const host = new URL(v.url).host;
        const label = host.split(".")[0];
        const scheme = location.protocol === "https:" ? "wss" : "ws";
        this.instance = { url: v.url, host, wsUrl: `${scheme}://${location.host}/api/live/${label}/ws` };
      } else {
        this.instance = null;
      }
    } catch {
      this.instance = null;
    }
    document.dispatchEvent(new CustomEvent("necropsy:instance", { detail: this.instance }));
    return this.instance;
  },
  show: (tool) => show(tool),
  /** Hands data to another tool and switches to it. */
  async send(tool, payload) {
    await show(tool);
    const mod = await loaded.get(tool);
    if (mod && mod.receive) mod.receive(payload);
  },
};

const tabs = [...document.querySelectorAll(".ws-tab")];
const loaders = {
  files: async () => ({ init: initFiles }),
  pcap: () => import("./pcap.js"),
  decoder: () => import("./decoder.js"),
  console: () => import("./console.js"),
  notebook: () => import("./notebook.js"),
  hex: () => import("./hex.js"),
};
const loaded = new Map();

async function load(tool) {
  if (!loaded.has(tool)) {
    loaded.set(
      tool,
      loaders[tool]().then(async (mod) => {
        await mod.init(ws, document.getElementById(`pane-${tool}`));
        return mod;
      }),
    );
  }
  return loaded.get(tool);
}

async function show(tool) {
  const tab = document.getElementById(`tab-${tool}`);
  if (!tab) return;
  for (const t of tabs) {
    const on = t === tab;
    t.setAttribute("aria-selected", String(on));
    t.tabIndex = on ? 0 : -1;
    document.getElementById(t.getAttribute("aria-controls")).classList.toggle("hidden", !on);
  }
  try {
    await load(tool);
  } catch (e) {
    const pane = document.getElementById(`pane-${tool}`);
    const p = document.createElement("p");
    p.className = "notice";
    p.textContent = `This tool did not load: ${e.message}`;
    pane.prepend(p);
  }
}

for (const t of tabs) {
  t.addEventListener("click", () => show(t.dataset.tool));
  t.addEventListener("keydown", (e) => {
    const i = tabs.indexOf(t);
    const next = { ArrowRight: i + 1, ArrowLeft: i - 1, Home: 0, End: tabs.length - 1 }[e.key];
    if (next === undefined) return;
    e.preventDefault();
    const target = tabs[(next + tabs.length) % tabs.length];
    target.focus();
    show(target.dataset.tool);
  });
}

// ---- files pane ----------------------------------------------------------------

function initFiles(ws, pane) {
  const list = pane.querySelector("#files-list");
  const cli = pane.querySelector("#cli-files");
  if (!files.length) {
    const li = document.createElement("li");
    li.className = "muted";
    li.textContent = "This puzzle has no files.";
    list.append(li);
  }
  for (const f of files) {
    const li = document.createElement("li");
    li.className = "file-item";
    // The name opens the file here; downloading is its own action.
    const a = document.createElement("a");
    a.href = `#file=${encodeURIComponent(f.path)}`;
    a.textContent = f.path;
    a.addEventListener("click", (e) => {
      e.preventDefault();
      openFile(f);
    });
    const size = document.createElement("span");
    size.className = "muted";
    size.textContent = ` ${f.size} bytes`;
    const actions = document.createElement("span");
    actions.className = "file-actions";
    const button = (label, fn) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "link-button";
      b.textContent = label;
      b.addEventListener("click", fn);
      actions.append(b);
    };
    if (f.kind === "text") button("View", () => openFile(f));
    if (f.kind === "pcap") button("Open in the pcap viewer", () => ws.send("pcap", { path: f.path }));
    if (f.kind === "text") {
      button("Send to the decoder", async () => {
        const bytes = await ws.fileBytes(f.path);
        ws.send("decoder", { input: new TextDecoder().decode(bytes) });
      });
    }
    button("Open in the hex viewer", () => ws.send("hex", { path: f.path }));
    button("Use in the notebook", () => ws.send("notebook", { file: f.path }));
    const dl = document.createElement("a");
    dl.href = ws.fileUrl(f.path);
    dl.download = f.path.split("/").pop();
    dl.className = "link-button file-download";
    dl.textContent = "Download";
    actions.append(dl);
    li.append(a, size, actions);
    list.append(li);
  }
  pane.querySelector("#file-view-close").addEventListener("click", () => {
    pane.querySelector("#file-view").classList.add("hidden");
  });
  const first = files[0];
  cli.textContent = first ? `curl -O ${location.origin}${ws.fileUrl(first.path)}` : "";
}

/** Opens a file in the browser: a capture in the pcap viewer, text in the files
 * pane, anything else in the hex viewer. */
export async function openFile(f) {
  if (f.kind === "pcap") return ws.send("pcap", { path: f.path });
  if (f.kind !== "text") return ws.send("hex", { path: f.path });
  await show("files");
  const view = document.getElementById("file-view");
  const body = document.getElementById("file-view-body");
  document.getElementById("file-view-name").textContent = f.path;
  view.classList.remove("hidden");
  body.textContent = "Loading...";
  try {
    body.textContent = new TextDecoder().decode(await ws.fileBytes(f.path));
  } catch (e) {
    body.textContent = e.message;
  }
  body.scrollTop = 0;
  view.scrollIntoView({ block: "nearest" });
}

// ---- start ---------------------------------------------------------------------

document.addEventListener("necropsy:instance-started", () => ws.refreshInstance());
ws.refreshInstance();

// A link such as /w/<id>#decoder=<CyberChef recipe> (from a hint) opens that tool.
function fromHash() {
  const h = new URLSearchParams(location.hash.slice(1));
  const file = h.has("file") && files.find((f) => f.path === h.get("file"));
  if (file) {
    openFile(file);
  } else if (h.has("decoder")) {
    ws.send("decoder", { recipe: h.get("decoder"), input: h.get("input") || undefined });
  } else if (h.has("tool") && loaders[h.get("tool")]) {
    show(h.get("tool"));
  } else {
    show(tabs[0]?.dataset.tool || "files");
  }
}
window.addEventListener("hashchange", fromHash);
fromHash();
