// SPDX-License-Identifier: AGPL-3.0-or-later
// node --test crates/necropsy-web/tests-js/
import test from "node:test";
import assert from "node:assert/strict";
import {
  tokenize, parseCurl, rawRequest, splitResponse, terminalOutput,
  blocksToCurl, curlToBlocks, shellQuote, CurlError,
} from "../static/toolkit/curl.js";

const HOST = "https://plank-orbit-cargo.live.example.test";

test("tokenizes like a shell", () => {
  assert.deepEqual(tokenize(`curl -H 'A: b c' "x \\"y\\"" z\\ w`), ["curl", "-H", "A: b c", 'x "y"', "z w"]);
  assert.deepEqual(tokenize("curl \\\n  -i \\\r\n -v"), ["curl", "-i", "-v"]);
  assert.deepEqual(tokenize(`curl ''`), ["curl", ""]);
  assert.throws(() => tokenize("curl 'open"), CurlError);
  assert.throws(() => tokenize('curl "open'), CurlError);
});

test("parses the supported subset", () => {
  const p = parseCurl(`curl -X post -H 'User-Agent: Pebble/2.4 (Win64; build 5521)' -H "X-Client-Id: 7f3a" --data 'd=abc' -d e=1 -iv '${HOST}/api/v2/sync?x=1&y=2'`);
  assert.equal(p.method, "POST");
  assert.equal(p.host, "plank-orbit-cargo.live.example.test");
  assert.equal(p.path, "/api/v2/sync");
  assert.equal(p.query, "x=1&y=2");
  assert.deepEqual(p.headers, [["User-Agent", "Pebble/2.4 (Win64; build 5521)"], ["X-Client-Id", "7f3a"]]);
  assert.deepEqual(p.data, ["d=abc", "e=1"]);
  assert.ok(p.include && p.verbose);
  assert.equal(parseCurl(`curl ${HOST}`).method, "GET");
  assert.equal(parseCurl(`curl -d a ${HOST}`).method, "POST");
  assert.equal(parseCurl(`curl -XPUT ${HOST}`).method, "PUT");
  assert.equal(parseCurl(`curl --request=DELETE ${HOST}`).method, "DELETE");
  assert.equal(parseCurl(`curl ${HOST}`).path, "/");
});

test("unsupported flags are named", () => {
  for (const [cmd, want] of [
    [`curl -A x ${HOST}`, "Unsupported flag: -A. Set the header instead"],
    [`curl --cookie a=b ${HOST}`, "Unsupported flag: --cookie."],
    [`curl --compressed ${HOST}`, "Unsupported flag: --compressed. The console supports"],
    [`curl -L ${HOST}`, "Unsupported flag: -L."],
    [`curl -ik ${HOST}`, "Unsupported flag: -k."],
  ]) {
    assert.throws(() => parseCurl(cmd), (e) => e instanceof CurlError && e.message.startsWith(want), cmd);
  }
});

test("other mistakes say what to fix", () => {
  for (const [cmd, want] of [
    ["", "Type a curl command"],
    ["wget x", "must start with curl"],
    ["curl -H", "-H needs a value"],
    ["curl -X 'P O' " + HOST, "not a method"],
    ["curl -H nocolon " + HOST, "is not a header"],
    ["curl ftp://x", "is not an http:// or https:// URL"],
    [`curl ${HOST} ${HOST}`, "Only one URL"],
    [`curl -d @secrets.txt ${HOST}`, "@file"],
    ["curl -i", "Give the URL"],
  ]) {
    assert.throws(() => parseCurl(cmd), (e) => e.message.includes(want), cmd);
  }
  assert.deepEqual(parseCurl(`curl --data-raw @x ${HOST}`).data, ["@x"]);
});

test("builds the raw request curl sends", () => {
  const p = parseCurl(`curl -H 'X-A: 1' -d 'a=1' -d 'b=2' '${HOST}/p?q=1'`);
  const { text } = rawRequest(p);
  assert.equal(
    text,
    "POST /p?q=1 HTTP/1.1\r\nHost: plank-orbit-cargo.live.example.test\r\nUser-Agent: curl/8.5.0\r\nAccept: */*\r\n" +
      "X-A: 1\r\nContent-Length: 7\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\na=1&b=2",
  );
  const ua = rawRequest(parseCurl(`curl -H 'user-agent: Pebble' -H 'Accept:' ${HOST}`)).text;
  assert.ok(ua.includes("user-agent: Pebble\r\n") && !ua.includes("curl/") && !ua.includes("Accept"));
  const ct = rawRequest(parseCurl(`curl -H 'Content-Type: application/json' -d '{}' ${HOST}`)).text;
  assert.ok(ct.includes("Content-Type: application/json") && !ct.includes("x-www-form-urlencoded"));
  // Content-Length counts bytes, not characters.
  assert.ok(rawRequest(parseCurl(`curl -d 'é' ${HOST}`)).text.includes("Content-Length: 2"));
});

test("splits a raw response and shows it as curl would", () => {
  const raw = new TextEncoder().encode("HTTP/1.1 404 Not Found\r\nx-a: b\r\ncontent-length: 4\r\n\r\nnope");
  const r = splitResponse(raw);
  assert.equal(r.status, 404);
  assert.deepEqual(r.headers, [["x-a", "b"], ["content-length", "4"]]);
  assert.equal(new TextDecoder().decode(r.body), "nope");
  const p = parseCurl(`curl ${HOST}/x`);
  const req = rawRequest(p).text;
  assert.equal(terminalOutput(p, req, r), "nope");
  assert.equal(terminalOutput({ ...p, include: true }, req, r), "HTTP/1.1 404 Not Found\nx-a: b\ncontent-length: 4\n\nnope");
  const v = terminalOutput({ ...p, verbose: true }, req, r);
  assert.ok(v.includes("> GET /x HTTP/1.1") && v.includes("< HTTP/1.1 404 Not Found") && v.endsWith("nope"));
});

test("blocks and text convert both ways", () => {
  const blocks = [
    { kind: "method", value: "POST" },
    { kind: "header", value: "User-Agent: Pebble/2.4 (Win64)" },
    { kind: "query", value: "id=7" },
    { kind: "data", value: "d=it's" },
  ];
  const cmd = blocksToCurl(HOST, "api/v2/sync", blocks);
  assert.equal(cmd, `curl '${HOST}/api/v2/sync?id=7' -X POST -H 'User-Agent: Pebble/2.4 (Win64)' --data 'd=it'\\''s'`);
  const back = curlToBlocks(cmd, HOST);
  assert.equal(back.path, "/api/v2/sync");
  assert.deepEqual(new Set(back.blocks.map((b) => `${b.kind}:${b.value}`)), new Set(blocks.map((b) => `${b.kind}:${b.value}`)));
  assert.equal(parseCurl(cmd).data[0], "d=it's");
  assert.equal(blocksToCurl(HOST, "/", [], { include: true }), `curl -i ${HOST}/`);
  assert.throws(() => curlToBlocks("curl https://elsewhere.test/", HOST), /not your instance/);
  assert.throws(() => curlToBlocks(`curl -A x ${HOST}`, HOST), /Unsupported flag: -A/);
});

test("quotes only what needs it", () => {
  assert.equal(shellQuote("abc/def:1"), "abc/def:1");
  assert.equal(shellQuote("a b"), "'a b'");
  assert.equal(shellQuote("it's"), "'it'\\''s'");
  assert.equal(shellQuote(""), "''");
});
