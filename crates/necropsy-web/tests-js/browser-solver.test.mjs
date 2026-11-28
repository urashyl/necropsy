// SPDX-License-Identifier: AGPL-3.0-or-later
// The browser solver's pure parts. The runs against a real site and browser
// are tools/browser-solver/selftest.mjs.
import test from "node:test";
import assert from "node:assert/strict";
import { releasedCopy } from "../../../tools/browser-solver/solve.mjs";

test("the copy is a daily released yesterday, whatever its date was", () => {
  const toml = 'id = "x"\ntrack = "daily"\ndate = "2027-03-01"\nnumber = 9\n';
  const out = releasedCopy(toml, new Date(Date.UTC(2026, 0, 1, 23, 59)));
  assert.equal(out, 'id = "x"\ntrack = "daily"\ndate = "2025-12-31"\nnumber = 9\n');
});

test("a fallback with no date gets one", () => {
  const out = releasedCopy('id = "x"\ntrack = "daily"\nnumber = 9\n', new Date(Date.UTC(2026, 2, 1)));
  assert.match(out, /^track = "daily"\ndate = "2026-02-28"$/m);
});
