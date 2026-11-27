// SPDX-License-Identifier: AGPL-3.0-or-later
// The request console's curl: a parser for the supported subset, the raw HTTP
// request it sends, and the block model of the beginner mode. No DOM here, so
// node can test it (crates/necropsy-web/tests-js/).
//
// Supported: one http(s) URL with path and query, -X/--request, -H/--header
// (repeatable), -d/--data/--data-raw (repeatable, joined with &), -i/--include
// and -v/--verbose. Anything else is an error that names the flag.

/** What curl itself sends unless a -H replaces it. */
export const CURL_UA = "curl/8.5.0";

const HELP = {
  "-A": "Set the header instead: -H 'User-Agent: ...'",
  "--user-agent": "Set the header instead: -H 'User-Agent: ...'",
  "-b": "Set the header instead: -H 'Cookie: name=value'",
  "--cookie": "Set the header instead: -H 'Cookie: name=value'",
  "-e": "Set the header instead: -H 'Referer: ...'",
  "--referer": "Set the header instead: -H 'Referer: ...'",
  "-u": "Set the header instead: -H 'Authorization: Basic <base64 of user:password>'",
  "--user": "Set the header instead: -H 'Authorization: Basic <base64 of user:password>'",
  "-G": "Put the parameters in the URL: 'https://host/path?name=value'",
  "--get": "Put the parameters in the URL: 'https://host/path?name=value'",
  "--data-binary": "Use --data or --data-raw.",
  "--data-urlencode": "Encode the value yourself and use --data.",
  "-F": "Only url-encoded and raw bodies are supported: use --data.",
  "--form": "Only url-encoded and raw bodies are supported: use --data.",
  "-L": "The console does not follow redirects: send a second request to the Location.",
  "--location": "The console does not follow redirects: send a second request to the Location.",
  "-k": "Not needed: the console does not check certificates.",
  "--insecure": "Not needed: the console does not check certificates.",
  "-s": "Not needed: the console shows no progress meter.",
  "--silent": "Not needed: the console shows no progress meter.",
  "-o": "The console shows the response; save it from there.",
  "--output": "The console shows the response; save it from there.",
};

export class CurlError extends Error {}

/**
 * Splits a command line the way a POSIX shell does for the quoting a curl
 * command uses: 'single quotes' are literal, "double quotes" allow \" \\ \$ \`,
 * a backslash outside quotes escapes the next character, and a backslash at
 * the end of a line continues it.
 */
export function tokenize(line) {
  const out = [];
  let cur = "";
  let has = false;
  let i = 0;
  const s = String(line);
  while (i < s.length) {
    const c = s[i];
    if (c === "'") {
      const j = s.indexOf("'", i + 1);
      if (j < 0) throw new CurlError("A single quote is not closed.");
      cur += s.slice(i + 1, j);
      has = true;
      i = j + 1;
    } else if (c === '"') {
      i++;
      let closed = false;
      while (i < s.length) {
        const d = s[i];
        if (d === '"') {
          closed = true;
          i++;
          break;
        }
        if (d === "\\" && i + 1 < s.length && '"\\$`\n'.includes(s[i + 1])) {
          if (s[i + 1] !== "\n") cur += s[i + 1];
          i += 2;
          continue;
        }
        cur += d;
        i++;
      }
      if (!closed) throw new CurlError("A double quote is not closed.");
      has = true;
    } else if (c === "\\") {
      if (s[i + 1] === "\n") {
        i += 2;
      } else if (s[i + 1] === "\r" && s[i + 2] === "\n") {
        i += 3;
      } else if (i + 1 < s.length) {
        cur += s[i + 1];
        has = true;
        i += 2;
      } else {
        i++;
      }
    } else if (/\s/.test(c)) {
      if (has) out.push(cur);
      cur = "";
      has = false;
      i++;
    } else {
      cur += c;
      has = true;
      i++;
    }
  }
  if (has) out.push(cur);
  return out;
}

/** Quotes one argument for a shell, only when it needs it. */
export function shellQuote(arg) {
  const s = String(arg);
  if (s !== "" && /^[A-Za-z0-9_\/:.,=@%+-]+$/.test(s)) return s;
  return `'${s.replace(/'/g, `'\\''`)}'`;
}

function takeValue(flag, args, i, attached) {
  if (attached !== null) return [attached, i];
  if (i + 1 >= args.length) throw new CurlError(`${flag} needs a value after it.`);
  return [args[i + 1], i + 1];
}

/**
 * Parses a curl command. Returns
 * { method, explicitMethod, url, scheme, host, path, query, headers: [[name, value]],
 *   removed: [name], data: [string], include, verbose }.
 */
export function parseCurl(line) {
  const args = tokenize(line);
  if (args.length === 0) throw new CurlError("Type a curl command.");
  if (args[0] !== "curl") throw new CurlError("The command must start with curl.");
  const r = { method: null, headers: [], removed: [], data: [], include: false, verbose: false, url: null };
  for (let i = 1; i < args.length; i++) {
    const a = args[i];
    if (a.startsWith("--")) {
      const eq = a.indexOf("=");
      const name = eq > 0 ? a.slice(0, eq) : a;
      const attached = eq > 0 ? a.slice(eq + 1) : null;
      let v;
      switch (name) {
        case "--request":
          [v, i] = takeValue(name, args, i, attached);
          r.method = v;
          break;
        case "--header":
          [v, i] = takeValue(name, args, i, attached);
          addHeader(r, v);
          break;
        case "--data":
        case "--data-raw":
        case "--data-ascii":
          [v, i] = takeValue(name, args, i, attached);
          addData(r, v, name !== "--data-raw");
          break;
        case "--include":
          r.include = true;
          break;
        case "--verbose":
          r.verbose = true;
          break;
        case "--url":
          [v, i] = takeValue(name, args, i, attached);
          setUrl(r, v);
          break;
        default:
          throw unsupported(name);
      }
    } else if (a.startsWith("-") && a.length > 1) {
      const flag = a.slice(0, 2);
      const rest = a.slice(2);
      let v;
      switch (flag) {
        case "-X":
          [v, i] = takeValue(flag, args, i, rest || null);
          r.method = v;
          break;
        case "-H":
          [v, i] = takeValue(flag, args, i, rest || null);
          addHeader(r, v);
          break;
        case "-d":
          [v, i] = takeValue(flag, args, i, rest || null);
          addData(r, v, true);
          break;
        case "-i":
        case "-v":
          // Display flags combine: -iv, -vi.
          for (const ch of a.slice(1)) {
            if (ch === "i") r.include = true;
            else if (ch === "v") r.verbose = true;
            else throw unsupported(`-${ch}`);
          }
          break;
        default:
          throw unsupported(flag);
      }
    } else {
      setUrl(r, a);
    }
  }
  if (!r.url) throw new CurlError("Give the URL to send to, like https://your-instance/path.");
  if (r.method !== null && !/^[A-Za-z]+$/.test(r.method)) {
    throw new CurlError(`-X ${r.method} is not a method name.`);
  }
  r.explicitMethod = r.method !== null;
  r.method = (r.method ?? (r.data.length ? "POST" : "GET")).toUpperCase();
  return r;
}

function unsupported(flag) {
  const help = HELP[flag];
  return new CurlError(
    `Unsupported flag: ${flag}.` +
      (help ? ` ${help}` : " The console supports -X, -H, -d/--data, -i and -v."),
  );
}

function addHeader(r, v) {
  const colon = v.indexOf(":");
  const semi = v.indexOf(";");
  // curl: "Name:" removes a header curl would send; "Name;" sends it empty.
  if (colon < 0 && semi > 0 && v.slice(semi + 1).trim() === "") {
    r.headers.push([v.slice(0, semi).trim(), ""]);
    return;
  }
  if (colon <= 0) throw new CurlError(`-H ${shellQuote(v)} is not a header: write it as 'Name: value'.`);
  const name = v.slice(0, colon).trim();
  if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(name)) throw new CurlError(`${shellQuote(name)} is not a header name.`);
  const value = v.slice(colon + 1).trim();
  if (/[\r\n]/.test(value)) throw new CurlError(`The value of ${name} has a line break.`);
  if (value === "") r.removed.push(name.toLowerCase());
  else r.headers.push([name, value]);
}

function addData(r, v, atMeansFile) {
  if (atMeansFile && v.startsWith("@")) {
    throw new CurlError("Reading the body from a file (@file) is not supported: paste the data, or use --data-raw.");
  }
  r.data.push(v);
}

function setUrl(r, v) {
  if (r.url !== null) throw new CurlError(`Only one URL per command: ${shellQuote(v)} is a second one.`);
  const m = /^(https?):\/\/([^/?#]+)([^#]*)/i.exec(v);
  if (!m) throw new CurlError(`${shellQuote(v)} is not an http:// or https:// URL.`);
  r.url = v;
  r.scheme = m[1].toLowerCase();
  r.host = m[2].toLowerCase();
  const rest = m[3] || "/";
  const q = rest.indexOf("?");
  r.path = (q < 0 ? rest : rest.slice(0, q)) || "/";
  if (!r.path.startsWith("/")) r.path = `/${r.path}`;
  r.query = q < 0 ? "" : rest.slice(q + 1);
}

function utf8(s) {
  return new TextEncoder().encode(s);
}

/**
 * The request curl would send, as text with CRLF line ends: request line, Host,
 * User-Agent, Accept, the -H headers, then Content-Length and Content-Type for a
 * body. Returns { text, body } where body is the joined --data.
 */
export function rawRequest(p) {
  const target = p.path + (p.query ? `?${p.query}` : "");
  const lines = [`${p.method} ${target} HTTP/1.1`];
  const given = new Set(p.headers.map(([n]) => n.toLowerCase()));
  const removed = new Set(p.removed);
  const add = (name, value) => {
    const k = name.toLowerCase();
    if (!given.has(k) && !removed.has(k)) lines.push(`${name}: ${value}`);
  };
  add("Host", p.host);
  add("User-Agent", CURL_UA);
  add("Accept", "*/*");
  for (const [n, v] of p.headers) lines.push(`${n}: ${v}`);
  const body = p.data.join("&");
  if (p.data.length) {
    add("Content-Length", String(utf8(body).length));
    add("Content-Type", "application/x-www-form-urlencoded");
  }
  return { text: `${lines.join("\r\n")}\r\n\r\n${body}`, body };
}

/** Splits a raw response into { status, reason, headers: [[n, v]], head, body (Uint8Array) }. */
export function splitResponse(bytes) {
  const b = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let end = -1;
  for (let i = 0; i + 3 < b.length; i++) {
    if (b[i] === 13 && b[i + 1] === 10 && b[i + 2] === 13 && b[i + 3] === 10) {
      end = i;
      break;
    }
  }
  const head = new TextDecoder().decode(end < 0 ? b : b.subarray(0, end));
  const body = end < 0 ? new Uint8Array() : b.subarray(end + 4);
  const [statusLine, ...rest] = head.split("\r\n");
  const m = /^HTTP\/\d(?:\.\d)? (\d{3}) ?(.*)$/.exec(statusLine || "");
  return {
    status: m ? Number(m[1]) : 0,
    reason: m ? m[2] : "",
    headers: rest.filter(Boolean).map((l) => {
      const i = l.indexOf(":");
      return [l.slice(0, i), l.slice(i + 1).trim()];
    }),
    head,
    body,
  };
}

/** What a terminal shows for this command: the body, headers too with -i, the exchange with -v. */
export function terminalOutput(p, requestText, response) {
  const bodyText = new TextDecoder().decode(response.body);
  if (p.verbose) {
    const reqHead = requestText.split("\r\n\r\n")[0];
    const out = [`*   Trying ${p.host}...`, `* Connected to ${p.host}`];
    for (const l of reqHead.split("\r\n")) out.push(`> ${l}`);
    out.push(">");
    for (const l of response.head.split("\r\n")) out.push(`< ${l}`);
    out.push("<");
    return `${out.join("\n")}\n${bodyText}`;
  }
  if (p.include) return `${response.head.replace(/\r\n/g, "\n")}\n\n${bodyText}`;
  return bodyText;
}

// ---- blocks ------------------------------------------------------------------

/**
 * Beginner mode. A command is the instance's base URL, a path, and a list of
 * blocks in the order they were added: { kind: "method" | "header" | "data" |
 * "query", value }. A query block is "name=value".
 */
export function blocksToCurl(base, path, blocks, flags = {}) {
  let p = path || "";
  if (!p.startsWith("/")) p = `/${p}`;
  const query = blocks.filter((b) => b.kind === "query" && b.value !== "").map((b) => b.value);
  const url = `${base.replace(/\/+$/, "")}${p}${query.length ? `?${query.join("&")}` : ""}`;
  const parts = ["curl"];
  if (flags.include) parts.push("-i");
  if (flags.verbose) parts.push("-v");
  parts.push(shellQuote(url));
  for (const b of blocks) {
    if (b.kind === "method") parts.push("-X", shellQuote(b.value));
    if (b.kind === "header") parts.push("-H", shellQuote(b.value));
    if (b.kind === "data") parts.push("--data", shellQuote(b.value));
  }
  return parts.join(" ");
}

/**
 * The blocks for a typed command, or a CurlError saying why it cannot be shown
 * as blocks (it does not parse, or its URL is not the instance).
 */
export function curlToBlocks(line, base) {
  const p = parseCurl(line);
  const baseHost = /^https?:\/\/([^/?#]+)/i.exec(base)?.[1]?.toLowerCase();
  if (baseHost && p.host !== baseHost) {
    throw new CurlError(`The URL is not your instance (${baseHost}), so it cannot be shown as blocks.`);
  }
  const blocks = [];
  if (p.explicitMethod) blocks.push({ kind: "method", value: p.method });
  for (const [n, v] of p.headers) blocks.push({ kind: "header", value: `${n}: ${v}` });
  for (const n of p.removed) blocks.push({ kind: "header", value: `${n}:` });
  for (const d of p.data) blocks.push({ kind: "data", value: d });
  if (p.query) for (const q of p.query.split("&")) blocks.push({ kind: "query", value: q });
  return { path: p.path, blocks, flags: { include: p.include, verbose: p.verbose } };
}

/** A block's text as it appears in the command. */
export function blockLabel(kind) {
  return { method: "-X", header: "-H", data: "--data", query: "?" }[kind];
}
