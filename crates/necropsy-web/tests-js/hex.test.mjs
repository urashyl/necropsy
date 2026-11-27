// SPDX-License-Identifier: AGPL-3.0-or-later
import test from "node:test";
import assert from "node:assert/strict";
import { hexdump, strings, parseOffset } from "../static/toolkit/hex.js";

test("hexdump matches xxd's layout", () => {
  const b = new TextEncoder().encode("necropsy.dev rocks\x00\x01");
  assert.equal(
    hexdump(b),
    "00000000: 6e65 6372 6f70 7379 2e64 6576 2072 6f63  necropsy.dev roc\n" +
      "00000010: 6b73 0001                                ks..",
  );
  assert.equal(hexdump(b, 17, 1).split("\n").length, 1);
});

test("strings finds printable runs with offsets", () => {
  const b = new Uint8Array([0, ...new TextEncoder().encode("short"), 0, ...new TextEncoder().encode("longer one"), 1]);
  assert.deepEqual(strings(b, 6), [{ offset: 7, text: "longer one" }]);
  assert.deepEqual(strings(b, 5).map((s) => s.text), ["short", "longer one"]);
});

test("offsets are decimal or 0x hex", () => {
  assert.equal(parseOffset("0x10"), 16);
  assert.equal(parseOffset("16"), 16);
  assert.equal(parseOffset(""), 0);
  assert.equal(parseOffset("zz"), null);
});
