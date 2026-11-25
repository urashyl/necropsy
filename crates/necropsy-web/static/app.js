// SPDX-License-Identifier: AGPL-3.0-or-later
// necropsy client script.
//
// Progress (solves, hints, active time) is kept in this browser's
// localStorage. When someone is signed in, solves are also saved on the
// server (the answer endpoint does that), and this script merges the two so
// progress follows the account to other devices.
(() => {
  "use strict";

  const KEY = "necropsy:v1";
  const DAY_MS = 86400000;
  const utcDay = () => Math.floor(Date.now() / DAY_MS);

  // ---- local storage -------------------------------------------------------

  const empty = () => ({ solved: {}, hints: {}, time: {} });

  function load() {
    try {
      const s = JSON.parse(localStorage.getItem(KEY));
      if (s && typeof s === "object") return { ...empty(), ...s };
    } catch {}
    return empty();
  }

  function save(state) {
    try {
      localStorage.setItem(KEY, JSON.stringify(state));
    } catch {}
  }

  const state = load();

  // ---- server progress (signed-in users) -----------------------------------

  const server = { user: null, solved: {}, playable: null };

  async function loadProgress() {
    try {
      const r = await fetch("/api/me/progress", { credentials: "same-origin" });
      if (!r.ok) return;
      const p = await r.json();
      server.user = p.user;
      server.solved = Object.fromEntries(p.solves.map((s) => [s.id, s]));
      if (Array.isArray(p.playable)) server.playable = new Set(p.playable);
    } catch {}
  }

  async function postAnswer(id, answer, seconds, hints) {
    const res = await fetch(`/api/p/${encodeURIComponent(id)}/answer`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ answer, seconds, hints }),
    });
    if (!res.ok) throw new Error(String(res.status));
    return res.json();
  }

  /**
   * A saved answer that no longer checks means the puzzle was changed after it
   * was solved here. Forget that solve so the new version can be played.
   */
  function forgetSolve(id) {
    delete state.solved[id];
    save(state);
  }

  /** On sign-in, upload solves that only exist in this browser. */
  async function importLocal() {
    if (!server.user || !server.playable) return;
    for (const [id, s] of Object.entries(state.solved)) {
      if (server.solved[id] || typeof s.answer !== "string") continue;
      // Removed puzzles and dailies not released yet would only 404.
      if (!server.playable.has(id)) continue;
      try {
        const r = await postAnswer(id, s.answer, s.seconds || 0, s.hints || 0);
        if (r.correct) {
          server.solved[id] = { id, day: s.day ?? utcDay(), ontime: !!s.ontime, seconds: s.seconds || 0, hints: s.hints || 0 };
        } else {
          forgetSolve(id);
        }
      } catch {}
    }
  }

  const isSolved = (id) => !!(state.solved[id] || server.solved[id]);

  // ---- streak ----------------------------------------------------------------

  /**
   * Consecutive UTC days, ending today or yesterday, on which the daily puzzle
   * was solved on its own day. Solving an old puzzle or an intro stage does not
   * count, so replaying cannot extend a streak.
   */
  function currentStreak() {
    const days = new Set();
    for (const s of Object.values(state.solved)) if (s.ontime && typeof s.day === "number") days.add(s.day);
    for (const s of Object.values(server.solved)) if (s.ontime) days.add(s.day);
    let d = utcDay();
    if (!days.has(d)) d -= 1;
    let n = 0;
    while (days.has(d)) {
      n += 1;
      d -= 1;
    }
    return n;
  }

  function formatTime(seconds) {
    const m = Math.floor(seconds / 60);
    const s = seconds % 60;
    return `${m}:${String(s).padStart(2, "0")}`;
  }

  function formatTimeWords(seconds) {
    const m = Math.floor(seconds / 60);
    const s = seconds % 60;
    return m ? `${m}m ${String(s).padStart(2, "0")}s` : `${s}s`;
  }

  // ---- theme ---------------------------------------------------------------

  const root = document.documentElement;
  const themeBtn = document.getElementById("theme-toggle");
  if (themeBtn) {
    themeBtn.addEventListener("click", () => {
      const dark =
        root.dataset.theme === "dark" ||
        (!root.dataset.theme && matchMedia("(prefers-color-scheme: dark)").matches);
      const next = dark ? "light" : "dark";
      root.dataset.theme = next;
      try {
        localStorage.setItem("necropsy-theme", next);
      } catch {}
    });
  }

  // ---- header and archive --------------------------------------------------

  const chip = document.getElementById("streak-chip");
  function renderStreak() {
    if (!chip) return;
    const n = currentStreak();
    chip.classList.toggle("hidden", n < 1);
    chip.textContent = `${n} day streak`;
  }

  function renderArchive() {
    document.querySelectorAll(".archive-link").forEach((a) => {
      a.querySelector(".solved-mark")?.classList.toggle("hidden", !isSolved(a.dataset.packId));
    });
  }

  // Daily pages suggest the intro stages until one has been solved.
  function renderBanner() {
    const banner = document.getElementById("intro-banner");
    if (!banner) return;
    const ids = (banner.dataset.intros || "").split(",").filter(Boolean);
    banner.classList.toggle("hidden", ids.length === 0 || ids.some(isSolved));
  }

  renderStreak();
  renderArchive();
  renderBanner();
  const ready = loadProgress().then(importLocal);
  ready.then(() => {
    renderStreak();
    renderArchive();
    renderBanner();
  });

  // ---- puzzle page ---------------------------------------------------------

  const puzzle = document.getElementById("puzzle");
  if (!puzzle) return;

  const id = puzzle.dataset.id;
  const label = puzzle.dataset.label;
  // The day a daily is released, as days since 1970 UTC. Empty for intro stages.
  const releaseDay = puzzle.dataset.day === "" ? null : Number(puzzle.dataset.day);
  const shareUrl = puzzle.dataset.url;
  const hintTotal = Number(puzzle.dataset.hints) || 0;

  const form = document.getElementById("answer-form");
  const input = document.getElementById("answer");
  const msg = document.getElementById("answer-msg");
  const explainer = document.getElementById("explainer");
  const explainerBody = document.getElementById("explainer-body");
  const timeEl = document.getElementById("stat-time");
  const hintsEl = document.getElementById("stat-hints");
  const hintBtn = document.getElementById("hint-btn");
  const hintItems = document.querySelectorAll("#hint-list li");
  const share = document.getElementById("share");
  const shareBtn = document.getElementById("share-btn");
  const shareMsg = document.getElementById("share-msg");

  let solved = !!state.solved[id];
  let seconds = state.solved[id]?.seconds ?? state.time[id] ?? 0;
  let hintsShown = state.solved[id]?.hints ?? state.hints[id] ?? 0;

  function renderHints() {
    hintItems.forEach((li, i) => li.classList.toggle("hidden", i >= hintsShown));
    if (hintsEl) hintsEl.textContent = String(hintsShown);
    if (hintBtn) {
      hintBtn.classList.toggle("hidden", hintTotal - hintsShown <= 0 || solved);
      hintBtn.textContent = hintsShown ? "Show another hint" : "Show a hint";
    }
  }

  function renderTime() {
    if (timeEl) timeEl.textContent = formatTime(seconds);
  }

  function showSolved(html) {
    solved = true;
    explainerBody.innerHTML = html;
    explainer.classList.remove("hidden");
    share.classList.remove("hidden");
    input.readOnly = true;
    form.querySelector("button[type=submit]").disabled = true;
    renderHints();
    renderTime();
  }

  renderHints();
  renderTime();

  // A returning player gets the explainer back without retyping. On this
  // device the answer they entered is kept locally; on another device a
  // signed-in user's server record is enough.
  const local = state.solved[id];
  if (local && typeof local.answer === "string") {
    input.value = local.answer;
    postAnswer(id, local.answer, local.seconds || 0, local.hints || 0)
      .then((r) => {
        if (r.correct) showSolved(r.explainer_html);
        else {
          forgetSolve(id);
          input.value = "";
        }
      })
      .catch(() => {});
  }
  ready.then(async () => {
    const s = server.solved[id];
    if (solved || !s) return;
    try {
      const res = await fetch(`/api/p/${encodeURIComponent(id)}/explainer`, { credentials: "same-origin" });
      if (!res.ok) return;
      seconds = s.seconds;
      hintsShown = s.hints;
      showSolved((await res.json()).explainer_html);
    } catch {}
  });

  if (hintBtn) {
    hintBtn.addEventListener("click", () => {
      if (hintsShown >= hintTotal) return;
      hintsShown += 1;
      state.hints[id] = hintsShown;
      save(state);
      renderHints();
    });
  }

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    if (solved) return;
    const answer = input.value;
    if (!answer.trim()) return;
    const submit = form.querySelector("button[type=submit]");
    submit.disabled = true;
    msg.textContent = "";
    msg.classList.remove("err-text");
    try {
      const r = await postAnswer(id, answer, seconds, hintsShown);
      if (r.correct) {
        const ontime = releaseDay !== null && utcDay() === releaseDay;
        state.solved[id] = { answer, seconds, hints: hintsShown, day: utcDay(), ontime };
        state.time[id] = seconds;
        save(state);
        renderStreak();
        msg.textContent = "Correct.";
        showSolved(r.explainer_html);
        explainer.scrollIntoView({ behavior: "smooth", block: "start" });
        return;
      }
      msg.textContent = "Not it.";
      msg.classList.add("err-text");
    } catch {
      msg.textContent = "Could not check the answer. Try again.";
      msg.classList.add("err-text");
    }
    submit.disabled = false;
  });

  // Time counts only while the tab is visible and the puzzle is unsolved.
  let ticks = 0;
  setInterval(() => {
    if (solved || document.hidden) return;
    seconds += 1;
    renderTime();
    if (++ticks % 5 === 0) persistTime();
  }, 1000);

  function persistTime() {
    if (solved) return;
    state.time[id] = seconds;
    save(state);
  }
  document.addEventListener("visibilitychange", () => document.hidden && persistTime());
  addEventListener("pagehide", persistTime);

  // ---- share ---------------------------------------------------------------

  if (solved) share.classList.remove("hidden");

  if (shareBtn) {
    shareBtn.addEventListener("click", async () => {
      const s = state.solved[id] || server.solved[id] || {};
      const parts = [`necropsy ${label}`, `solved in ${formatTimeWords(s.seconds ?? seconds)}`];
      if (s.hints) parts.push(`${s.hints} hint${s.hints === 1 ? "" : "s"}`);
      const text = `${parts.join(" · ")}\n${shareUrl}`;
      try {
        await navigator.clipboard.writeText(text);
        shareMsg.textContent = "Copied.";
      } catch {
        shareMsg.textContent = "Could not copy.";
      }
    });
  }
})();
