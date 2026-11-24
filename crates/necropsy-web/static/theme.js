// Runs before first paint so a saved theme never flashes the wrong colors.
(() => {
  try {
    const t = localStorage.getItem("necropsy-theme");
    if (t === "light" || t === "dark") document.documentElement.dataset.theme = t;
  } catch {}
})();
