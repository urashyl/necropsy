# Design and copy standards

Rules for anything a player reads or touches: pages, forms, emails, puzzle briefs and explainers. They exist to keep the site plain, fast to read and consistent. A change that breaks one is a bug.

## Copy

- **Say what the thing is or what to do, then stop.** If a label or the button already says it, add nothing. A heading that is followed by a sentence restating it should lose the sentence.
- **No filler.** Cut text that sells, reassures or narrates: "just for you", "your very own", "yours alone", "Please note", "feel free", "let's", "we're excited", "seamless", "powerful", "robust", "journey", "unlock", "dive into". If deleting a sentence loses no fact, delete it.
- **No explaining the product to itself.** Do not describe how the site works where a player is trying to use it. Facts a player needs (how long an instance runs, what a form field wants) are short and sit next to the control.
- **Plain, direct sentences.** Active voice, present tense, sentence case, no exclamation marks. Numbers as numbers. One idea per sentence.
- **Errors say what happened and what to do next**, in a sentence: "That code did not work." "Wait a few minutes and try again." Never blame the player, never apologise at length, never say "something went wrong" without a next step.
- **Emails are short.** Subject says what it is. The body has the link or code, its lifetime, and one line on what to do if the player did not ask for it.
- **Do not repeat a limit or rule in several places** on one page. State it once, where it applies.

### Puzzle text

- A brief has the situation in two or three sentences, what the player has, what to do, and what to submit. No scene-setting that carries no fact, no "Start it below" (the page has the button), no reassurance that the sandbox is isolated (the docs say so once).
- Say once, plainly, when a sample's server name does not resolve and the player should use their instance instead.
- An explainer teaches the technique and what a defender takes from it. It does not congratulate.
- Hints go from a nudge to nearly the answer. They never repeat the brief.

## Layout

- The site must work and look right from 320 px wide up. Nothing scrolls sideways at any width. Check the header with a long username, the Admin link and a streak showing.
- Long user-supplied text (usernames) is truncated with an ellipsis and its full text is the tooltip.
- Everything is reachable by keyboard, with visible focus, and readable in the light and dark themes. Do not rely on colour alone.
- Forms are plain HTML posts that work without JavaScript, except where the page is about JavaScript.

## Documentation

- **Every feature has documentation in `docs/` in the same change that adds it**: what it does, how to turn it on, its limits, and what an operator does when it goes wrong. A new setting is in `.env.example` and the relevant page. A new route is in `docs/web.md`. A new puzzle format feature is in `docs/pack-format.md` or `docs/live.md`.
- Docs follow the same copy rules: state the fact, skip the pitch.
- Behaviour that is easy to get wrong has a test, and the test says what the behaviour is for.

## Checks

`no_filler_in_copy` (in `crates/necropsy-web/src/tests.rs`) scans the site's pages, scripts and every brief and explainer for the filler phrases above. Add to its list when a new one shows up.
