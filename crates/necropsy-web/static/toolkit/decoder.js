// SPDX-License-Identifier: AGPL-3.0-or-later
// The decoder: CyberChef, served from this site, in a sandboxed frame (it has
// an opaque origin, so it cannot reach the site's cookies or API). Other tools
// and hints open it with an input and a recipe in the frame's URL.

let ws;
let els;
const MAX_INPUT = 1024 * 1024;

// Terminal equivalents of common recipe steps.
const CLI = [
  [/From_Base64/, "base64 -d"],
  [/To_Base64/, "base64"],
  [/From_Hex/, "xxd -r -p"],
  [/To_Hex/, "xxd -p"],
  [/URL_Decode/, `python3 -c 'import sys,urllib.parse; print(urllib.parse.unquote(sys.stdin.read()))'`],
  [/XOR/, `python3 -c 'import sys; k=0x41; sys.stdout.buffer.write(bytes(b ^ k for b in sys.stdin.buffer.read()))'`],
  [/ROT13/, `tr 'A-Za-z' 'N-ZA-Mn-za-m'`],
  [/Reverse/, "rev"],
  [/Gunzip|Zlib_Inflate/, `python3 -c 'import sys,zlib; sys.stdout.buffer.write(zlib.decompress(sys.stdin.buffer.read(), 47))'`],
  [/AES_Decrypt/, "openssl enc -d -aes-256-cbc -K <hex key> -iv <hex iv>"],
];

export function init(shared, pane) {
  ws = shared;
  els = {
    frame: pane.querySelector("#decoder-frame"),
    msg: pane.querySelector("#decoder-msg"),
    cli: pane.querySelector("#cli-decoder"),
  };
  if (!ws.tools.cyberchef) {
    els.msg.textContent = "The decoder is not installed on this server.";
    els.msg.classList.add("err-text");
    return;
  }
  open({});
}

export function receive(payload) {
  if (ws.tools.cyberchef) open(payload || {});
}

function b64(text) {
  const bytes = new TextEncoder().encode(text);
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

function open({ input, recipe }) {
  const hash = [];
  if (recipe) hash.push(`recipe=${encodeURIComponent(recipe)}`);
  if (input !== undefined) {
    if (input.length > MAX_INPUT) {
      els.msg.textContent = "That is too large to pass in. Paste it into the decoder's input.";
      els.msg.classList.add("err-text");
    } else {
      hash.push(`input=${encodeURIComponent(b64(input))}`);
      els.msg.textContent = "";
      els.msg.classList.remove("err-text");
    }
  }
  const iframe = document.createElement("iframe");
  iframe.title = "Decoder (CyberChef)";
  iframe.className = "decoder-iframe";
  iframe.setAttribute("sandbox", "allow-scripts allow-downloads allow-popups allow-modals allow-forms");
  iframe.referrerPolicy = "no-referrer";
  iframe.src = `${ws.tools.cyberchef}/index.html${hash.length ? `#${hash.join("&")}` : ""}`;
  els.frame.replaceChildren(iframe);
  const steps = CLI.filter(([re]) => recipe && re.test(recipe)).map(([, c]) => c);
  els.cli.textContent = steps.length
    ? `... | ${steps.join(" | ")}`
    : "base64 -d, xxd -r -p, or a few lines of python3";
}
