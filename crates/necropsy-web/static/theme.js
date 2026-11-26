// SPDX-License-Identifier: AGPL-3.0-or-later
// Runs before first paint so a saved theme never flashes the wrong colors.
(() => {
  try {
    const t = localStorage.getItem("necropsy-theme");
    if (t === "light" || t === "dark") document.documentElement.dataset.theme = t;
  } catch {}
})();

// Tells the server which timezone this browser is in, so "today" is the viewer's own
// calendar day. The server limits it to the real timezones, so this cannot unlock a
// puzzle more than a few hours early. When it changes, the page is loaded once more so
// what was rendered matches (once per session and offset, so a browser that refuses
// cookies does not loop).
(() => {
  try {
    const m = Math.min(840, Math.max(-720, -new Date().getTimezoneOffset()));
    const had = document.cookie.match(/(?:^|; )necropsy_tz=(-?\d+)/);
    if (had && Number(had[1]) === m) return;
    document.cookie =
      "necropsy_tz=" + m + "; path=/; max-age=31536000; SameSite=Lax" +
      (location.protocol === "https:" ? "; Secure" : "");
    if (sessionStorage.getItem("necropsy-tz-reload") !== String(m)) {
      sessionStorage.setItem("necropsy-tz-reload", String(m));
      if (m !== 0 || had) location.reload();
    }
  } catch {}
})();
