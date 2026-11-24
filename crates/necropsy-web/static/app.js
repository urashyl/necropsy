// necropsy client script. No accounts: everything the player accumulates
// (solves, hints, time, streak) lives in this browser's localStorage.
(() => {
  "use strict";

  const KEY = "necropsy:v1";
  const DAY_MS = 86400000;
  const utcDay = () => Math.floor(Date.now() / DAY_MS);

  // ---- storage -----------------------------------------------------------

  const empty = () => ({ solved: {}, hints: {}, time: {}, streak: 0, lastDay: null });

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

  /** Streak shown to the player: zero once a whole day has been missed. */
  function currentStreak(state) {
    if (state.lastDay == null) return 0;
    return utcDay() - state.lastDay <= 1 ? state.streak : 0;
  }

  function recordSolveDay(state) {
    const today = utcDay();
    if (state.lastDay === today) return;
    state.streak = state.lastDay === today - 1 ? state.streak + 1 : 1;
    state.lastDay = today;
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

  // ---- header streak -------------------------------------------------------

  const state = load();
  const chip = document.getElementById("streak-chip");
  function renderStreak() {
    if (!chip) return;
    const n = currentStreak(state);
    chip.classList.toggle("hidden", n < 1);
    chip.textContent = `${n} day streak`;
  }
  renderStreak();

  // ---- archive -------------------------------------------------------------

  document.querySelectorAll(".archive-link").forEach((a) => {
    if (state.solved[a.dataset.packId]) {
      a.querySelector(".solved-mark")?.classList.remove("hidden");
    }
  });

  // ---- puzzle page ---------------------------------------------------------

  const puzzle = document.getElementById("puzzle");
  if (!puzzle) return;

  const id = puzzle.dataset.id;
  const number = puzzle.dataset.number;
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
  let seconds = state.time[id] || 0;
  let hintsShown = state.hints[id] || 0;

  function renderHints() {
    hintItems.forEach((li, i) => li.classList.toggle("hidden", i >= hintsShown));
    if (hintsEl) hintsEl.textContent = String(hintsShown);
    if (hintBtn) {
      const left = hintTotal - hintsShown;
      hintBtn.classList.toggle("hidden", left <= 0 || solved);
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
  }

  async function check(answer) {
    const res = await fetch(`/api/p/${encodeURIComponent(id)}/answer`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ answer }),
    });
    if (!res.ok) throw new Error(String(res.status));
    return res.json();
  }

  renderHints();
  renderTime();

  // A returning player who already solved this gets the explainer back
  // without retyping: the answer they entered is kept locally.
  if (solved && typeof state.solved[id].answer === "string") {
    input.value = state.solved[id].answer;
    check(state.solved[id].answer)
      .then((r) => r.correct && showSolved(r.explainer_html))
      .catch(() => {});
  }

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
      const r = await check(answer);
      if (r.correct) {
        state.solved[id] = { answer, seconds, hints: hintsShown, day: utcDay() };
        state.time[id] = seconds;
        recordSolveDay(state);
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
      const s = state.solved[id] || {};
      const parts = [`necropsy #${number}`, `solved in ${formatTimeWords(s.seconds ?? seconds)}`];
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
