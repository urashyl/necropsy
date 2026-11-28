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
  // The viewer's calendar day, as days since 1970. The server uses the same rule: the
  // timezone is the browser's, limited to the real ones (UTC-12 to UTC+14).
  const tzMinutes = () => Math.min(14 * 60, Math.max(-12 * 60, -new Date().getTimezoneOffset()));
  const localDay = () => Math.floor((Date.now() + tzMinutes() * 60000) / DAY_MS);

  // ---- local storage -------------------------------------------------------

  const empty = () => ({ solved: {}, hints: {}, time: {}, started: {} });

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
          server.solved[id] = { id, day: s.day ?? localDay(), ontime: !!s.ontime, seconds: s.seconds || 0, hints: s.hints || 0 };
        } else {
          forgetSolve(id);
        }
      } catch {}
    }
  }

  const isSolved = (id) => !!(state.solved[id] || server.solved[id]);

  /** Moves a live daily's ranked-attempt notice from one state to the next. */
  function rankedNotice(from, to, text) {
    const el = document.querySelector(".ranked-notice");
    if (!el || el.dataset.state !== from) return;
    el.dataset.state = to;
    el.textContent = text;
    el.className = "muted ranked-notice";
  }

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
    let d = localDay();
    if (!days.has(d)) d -= 1;
    let n = 0;
    while (days.has(d)) {
      n += 1;
      d -= 1;
    }
    return n;
  }

  function formatTime(seconds) {
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = seconds % 60;
    const ss = String(s).padStart(2, "0");
    return h ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
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

  // ---- solve cards -----------------------------------------------------------

  /** Downloads a card's SVG as a PNG, drawn in the browser. */
  async function saveCardImage(svgUrl, name, msgEl) {
    try {
      // A data: URL, because the page's CSP allows images from itself and data: only.
      const text = await (await fetch(svgUrl, { credentials: "same-origin" })).text();
      const img = new Image();
      await new Promise((resolve, reject) => {
        img.onload = resolve;
        img.onerror = reject;
        img.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(text);
      });
      const canvas = document.createElement("canvas");
      canvas.width = 1200;
      canvas.height = 630;
      canvas.getContext("2d").drawImage(img, 0, 0, 1200, 630);
      const png = await new Promise((resolve) => canvas.toBlob(resolve, "image/png"));
      const a = document.createElement("a");
      a.href = URL.createObjectURL(png);
      a.download = `necropsy-${name}.png`;
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 5000);
    } catch {
      if (msgEl) msgEl.textContent = "Could not save the image.";
    }
  }

  const cardSave = document.getElementById("card-save");
  if (cardSave) {
    cardSave.addEventListener("click", () =>
      saveCardImage(cardSave.dataset.src, cardSave.dataset.name, document.getElementById("card-msg")),
    );
  }

  // ---- learn ---------------------------------------------------------------
  //
  // Checkpoints passed while signed out are kept here, with the answers, and sent
  // to the server once the player signs in (it checks them again), like solves.

  const LEARN_KEY = "necropsy:learn:v1";

  function loadLearn() {
    try {
      const s = JSON.parse(localStorage.getItem(LEARN_KEY));
      if (s && typeof s === "object" && s.passed && typeof s.passed === "object") return s;
    } catch {}
    return { passed: {} };
  }

  function saveLearn(l) {
    try {
      localStorage.setItem(LEARN_KEY, JSON.stringify(l));
    } catch {}
  }

  const learnModule = document.getElementById("learn-module");
  const learnIndex = document.getElementById("learn");
  if (learnModule || learnIndex) {
    const local = loadLearn();
    // A checkpoint just passed on this page: keep its answers.
    if (learnModule && learnModule.hasAttribute("data-passed")) {
      const inputs = [...learnModule.querySelectorAll(".checkpoint input, .checkpoint textarea")];
      if (inputs.length && inputs.every((i) => i.value.trim())) {
        local.passed[learnModule.dataset.module] = inputs.map((i) => i.value);
        saveLearn(local);
      }
    }
    if (learnModule && local.passed[learnModule.dataset.module]) {
      learnModule.querySelector(".passed-note")?.classList.remove("hidden");
    }
    const markIndex = (extra) => {
      if (!learnIndex) return;
      const passed = new Set([...(learnIndex.dataset.passed || "").split(",").filter(Boolean), ...extra]);
      for (const a of learnIndex.querySelectorAll("[data-module]")) {
        a.querySelector(".passed-mark")?.classList.toggle("hidden", !passed.has(a.dataset.module));
      }
      for (const t of learnIndex.querySelectorAll(".tier")) {
        const mods = [...t.querySelectorAll("[data-module]")].map((a) => a.dataset.module);
        t.querySelector(".tier-badge")?.classList.toggle("hidden", !mods.every((m) => passed.has(m)));
      }
    };
    markIndex(Object.keys(local.passed));
    (async () => {
      try {
        const r = await fetch("/api/me/learn", { credentials: "same-origin" });
        if (!r.ok) return;
        const me = await r.json();
        if (!me.user) return;
        const onServer = new Set(me.passed);
        for (const [mod, answers] of Object.entries(local.passed)) {
          if (onServer.has(mod) || !me.modules.includes(mod)) {
            delete local.passed[mod];
            continue;
          }
          const res = await fetch(`/api/learn/${encodeURIComponent(mod)}/checkpoint`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            credentials: "same-origin",
            body: JSON.stringify({ answers }),
          });
          if (res.ok) {
            const out = await res.json();
            // Passed is now on the server; a wrong answer means the checkpoint changed.
            delete local.passed[mod];
            if (out.passed) onServer.add(mod);
          }
        }
        saveLearn(local);
        markIndex(onServer);
      } catch {}
    })();
  }

  // ---- puzzle page ---------------------------------------------------------

  const puzzle = document.getElementById("puzzle");
  if (!puzzle) return;

  const id = puzzle.dataset.id;
  const label = puzzle.dataset.label;
  // The calendar day a daily is released, as days since 1970. Empty for intro stages.
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
  const hintList = document.getElementById("hint-list");
  const share = document.getElementById("share");
  const shareBtn = document.getElementById("share-btn");
  const shareMsg = document.getElementById("share-msg");

  let solved = !!state.solved[id];
  // The time on a puzzle is the wall-clock time since it was first opened, up to three
  // hours, so closing the tab or sleeping the laptop does not pause it. For a signed-in
  // player the server keeps the clock and this only displays it; signed out, it is kept
  // here from the first time the page was opened.
  // On a live puzzle the clock starts when the player starts an instance, so it stays at
  // zero until the server says it is running.
  const TIME_CAP = 3 * 3600;
  const isLive = !!document.getElementById("live");
  const clock = { base: null, at: 0, frozen: false };
  if (!isLive && !state.solved[id] && !state.started[id]) {
    state.started[id] = Date.now();
    save(state);
  }
  function elapsed() {
    const since = (ms) => Math.max(0, Math.floor((Date.now() - ms) / 1000));
    if (clock.base !== null) return clock.frozen ? clock.base : Math.min(clock.base + since(clock.at), TIME_CAP);
    if (isLive) return 0;
    return Math.min(since(state.started[id] || Date.now()), TIME_CAP);
  }
  let seconds = state.solved[id]?.seconds ?? elapsed();
  let hintsShown = state.solved[id]?.hints ?? state.hints[id] ?? 0;
  // The hints revealed so far. Their text is not in the page: the server sends each
  // one when asked, and counts it.
  let hintTexts = [];

  function renderHints() {
    if (hintList) {
      hintList.replaceChildren(
        ...hintTexts.map((t) => {
          const li = document.createElement("li");
          // A hint can end with {decoder: <CyberChef recipe>}: a link that opens the
          // workspace's decoder with that recipe (see docs/pack-format.md).
          const m = /^([\s\S]*?)\s*\{decoder:\s*([\s\S]+)\}\s*$/.exec(t);
          if (m) {
            li.append(m[1], " ");
            const a = document.createElement("a");
            a.href = `/w/${encodeURIComponent(id)}#decoder=${encodeURIComponent(m[2].trim())}`;
            a.textContent = "Open in the decoder";
            li.append(a);
          } else {
            li.textContent = t;
          }
          return li;
        }),
      );
    }
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
    document.getElementById("discuss-later")?.classList.remove("hidden");
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

  // ---- time and hints, kept by the server for a signed-in player -------------

  /**
   * Asks the server for this puzzle's state: a beat counts the time the page has been
   * open, a hint reveals the next one. A signed-in player's time and hint count come
   * back from the server and replace the browser's. A signed-out player's stay in
   * the browser, and the server only supplies the hint text.
   */
  async function sync(path) {
    const res = await fetch(`/api/p/${encodeURIComponent(id)}/${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify({ have: hintsShown, peek: solved && !server.solved[id] }),
    });
    if (!res.ok) throw new Error(String(res.status));
    const s = await res.json();
    if (s.user && typeof s.seconds === "number") {
      seconds = s.seconds;
      clock.base = s.seconds;
      clock.at = Date.now();
      clock.frozen = !!s.solved || !s.running;
    }
    hintsShown = s.hints;
    hintTexts = s.texts;
    if (!solved) state.hints[id] = hintsShown;
    renderHints();
    renderTime();
    return s;
  }

  // The first beat starts the server's clock and brings back any hints already shown. A
  // puzzle this browser solved while signed out only reads its hints, and starts no clock.
  // A beat a minute keeps the display in step with the server.
  ready.then(() => sync("beat").catch(() => {}));
  setInterval(() => {
    if (!solved) sync("beat").catch(() => {});
  }, 60000);
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && !solved) sync("beat").catch(() => {});
  });
  // Starting an instance starts the clock on a live puzzle.
  document.addEventListener("necropsy:instance-started", () => {
    if (!solved) sync("beat").catch(() => {});
  });

  if (hintBtn) {
    hintBtn.addEventListener("click", async () => {
      if (hintsShown >= hintTotal || solved) return;
      hintBtn.disabled = true;
      try {
        await sync("hint");
      } catch {
        msg.textContent = "Could not get a hint. Try again.";
        msg.classList.add("err-text");
      }
      hintBtn.disabled = false;
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
      if (r.attempt === "ranked") {
        rankedNotice("running", "solved", "Your ranked attempt is done. Further attempts are on your account page only.");
      }
      if (r.correct) {
        const ontime = releaseDay !== null && localDay() === releaseDay;
        // A signed-in player's time and hints are the ones the server recorded.
        if (typeof r.seconds === "number") seconds = r.seconds;
        if (typeof r.hints === "number") hintsShown = r.hints;
        state.solved[id] = { answer, seconds, hints: hintsShown, day: localDay(), ontime };
        state.time[id] = seconds;
        save(state);
        renderStreak();
        document.dispatchEvent(new Event("necropsy:solved"));
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

  setInterval(() => {
    if (solved || clock.frozen) return;
    seconds = elapsed();
    renderTime();
  }, 1000);

  // ---- live instance -------------------------------------------------------

  const liveBox = document.getElementById("live");
  const liveStart = document.getElementById("live-start");
  if (liveBox && liveStart) {
    const idle = document.getElementById("live-idle");
    const active = document.getElementById("live-active");
    const urlEl = document.getElementById("live-url");
    const leftEl = document.getElementById("live-left");
    const logEl = document.getElementById("live-log");
    const liveMsg = document.getElementById("live-msg");
    const api = `/api/p/${encodeURIComponent(id)}/instance`;
    // When the instance ends, as a clock time, so the countdown is right even after
    // the tab was in the background.
    let deadline = 0;

    // Everything below is written with textContent: log lines are made of
    // whatever the player sent.
    function renderInstance(v) {
      const on = !!v.active;
      idle.classList.toggle("hidden", on);
      active.classList.toggle("hidden", !on);
      if (!on) return;
      urlEl.textContent = v.url;
      urlEl.href = v.url;
      deadline = Date.now() + v.seconds_left * 1000;
      renderLeft();
      logEl.textContent = v.log
        .map((e) => `${new Date(e.t * 1000).toLocaleTimeString()}  ${e.status}  ${e.method} ${e.path}`)
        .join("\n") || "Nothing yet.";
    }

    function renderLeft() {
      leftEl.textContent = formatTime(Math.max(Math.ceil((deadline - Date.now()) / 1000), 0));
    }

    // Starting an instance needs a Cloudflare Turnstile token when the site
    // has a captcha. The widget is loaded the first time it is needed.
    const siteKey = liveBox.dataset.sitekey || "";
    const captchaBox = document.getElementById("live-captcha");
    let widget = null;
    let pendingToken = null;

    function loadTurnstile() {
      if (window.turnstile) return Promise.resolve();
      return new Promise((resolve, reject) => {
        const s = document.createElement("script");
        s.src = "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit";
        s.onload = resolve;
        s.onerror = reject;
        document.head.appendChild(s);
      });
    }

    async function startWithCaptcha() {
      if (!siteKey) return call("POST");
      liveStart.disabled = true;
      liveMsg.textContent = "Checking...";
      try {
        await loadTurnstile();
      } catch {
        liveMsg.textContent = "Could not load the captcha.";
        liveMsg.classList.add("err-text");
        liveStart.disabled = false;
        return;
      }
      const done = async (token) => {
        pendingToken = token;
        await call("POST");
        pendingToken = null;
        liveStart.disabled = false;
        if (widget !== null) window.turnstile.reset(widget);
      };
      if (widget === null) {
        widget = window.turnstile.render(captchaBox, { sitekey: siteKey, callback: done });
      } else {
        window.turnstile.reset(widget);
      }
    }

    async function call(method, path = "") {
      liveMsg.textContent = "";
      liveMsg.classList.remove("err-text");
      try {
        const headers = pendingToken ? { "X-Turnstile-Token": pendingToken } : {};
        const r = await fetch(api + path, { method, headers, credentials: "same-origin" });
        if (!r.ok) throw new Error(await r.text());
        renderInstance(await r.json());
        if (method === "POST" && path === "") {
          document.dispatchEvent(new Event("necropsy:instance-started"));
          rankedNotice("new", "running", "Your ranked attempt is running.");
        }
        if (method === "POST" && path === "/stop") {
          rankedNotice("running", "failed", "Your ranked attempt has ended. Further attempts are on your account page only.");
        }
      } catch (e) {
        liveMsg.textContent = e.message || "Something went wrong.";
        liveMsg.classList.add("err-text");
      }
    }

    liveStart.addEventListener("click", startWithCaptcha);
    // Solving stops the instance on the server. Show that.
    document.addEventListener("necropsy:solved", () => call("GET"));
    document.getElementById("live-stop").addEventListener("click", () => call("POST", "/stop"));
    document.getElementById("live-copy").addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(urlEl.textContent);
        liveMsg.textContent = "Copied.";
      } catch {
        liveMsg.textContent = "Could not copy.";
      }
    });

    call("GET");
    let polls = 0;
    setInterval(() => {
      if (active.classList.contains("hidden")) return;
      renderLeft();
      // The request log refreshes every 3 seconds while the tab is showing.
      if (!document.hidden && ++polls % 3 === 0) call("GET");
    }, 1000);
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden && !active.classList.contains("hidden")) call("GET");
    });
  }

  // ---- share ---------------------------------------------------------------
  //
  // One button. A solve the server timed is shared as its card: the server makes the
  // link and the text (cards.rs). Any other solve is shared as text with the puzzle's
  // link, made here the same way. Either way: the system share sheet where there is
  // one, otherwise the clipboard.

  const shareLabel = share.dataset.label || label;
  const preview = document.getElementById("share-preview");
  const saveBtn = document.getElementById("share-save");
  const stopBtn = document.getElementById("share-stop");
  let cardText = null;

  /** Squares, one per hint the puzzle has: yellow if used, green if not needed. */
  function hintGrid(used, total) {
    if (total <= 0) return "🟩";
    const u = Math.max(0, Math.min(used, total));
    return "🟨".repeat(u) + "🟩".repeat(total - u);
  }

  function localText() {
    const s = state.solved[id] || server.solved[id] || {};
    const used = s.hints ?? hintsShown;
    const secs = s.seconds ?? seconds;
    let line = `${hintGrid(used, hintTotal)} ${formatTime(secs)} · ${hintTotal ? `${used} of ${hintTotal} hints` : "no hints"}`;
    const streak = s.ontime ? currentStreak() : 0;
    if (streak > 1) line += ` · ${streak}-day streak`;
    const url = server.solved[id]?.share_url || shareUrl;
    return `necropsy ${shareLabel} · ${puzzle.dataset.title}\n${line}\n${url}`;
  }

  function renderShare() {
    const s = server.solved[id];
    const timed = !!(s && s.timed);
    saveBtn.classList.toggle("hidden", !(timed && s.share_url));
    stopBtn.classList.toggle("hidden", !(timed && s.share_url));
    if (preview) preview.textContent = cardText || localText();
  }

  if (solved) share.classList.remove("hidden");
  ready.then(renderShare);
  document.addEventListener("necropsy:solved", async () => {
    await loadProgress();
    renderShare();
  });

  shareBtn.addEventListener("click", async () => {
    shareMsg.textContent = "";
    let text;
    try {
      const s = server.solved[id];
      if (s && s.timed) {
        const r = await fetch(`/api/p/${encodeURIComponent(id)}/share`, { method: "POST", credentials: "same-origin" });
        if (!r.ok) throw new Error(await r.text());
        const out = await r.json();
        s.share_url = out.url;
        cardText = out.text;
        text = out.text;
      } else {
        text = localText();
      }
      renderShare();
    } catch (e) {
      shareMsg.textContent = e.message || "Could not make the card.";
      text = localText();
    }
    if (navigator.share) {
      try {
        await navigator.share({ text });
        return;
      } catch (e) {
        if (e && e.name === "AbortError") return;
      }
    }
    try {
      await navigator.clipboard.writeText(text);
      shareMsg.textContent = "Copied.";
    } catch {
      shareMsg.textContent = "Could not copy. Select the text above.";
    }
  });

  saveBtn.addEventListener("click", () => {
    const s = server.solved[id];
    if (s && s.share_url) saveCardImage(`${s.share_url}.svg`, id, shareMsg);
  });

  stopBtn.addEventListener("click", async () => {
    const s = server.solved[id];
    if (!s) return;
    try {
      const r = await fetch(`/api/p/${encodeURIComponent(id)}/share`, { method: "DELETE", credentials: "same-origin" });
      if (!r.ok) throw new Error();
      delete s.share_url;
      cardText = null;
      renderShare();
      shareMsg.textContent = "The card no longer works.";
    } catch {
      shareMsg.textContent = "Could not stop sharing.";
    }
  });
})();
