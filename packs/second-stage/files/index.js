"use strict";

const codes = {
  red: [31, 39],
  green: [32, 39],
  yellow: [33, 39],
  blue: [34, 39],
  bold: [1, 22],
  dim: [2, 22],
};

const enabled = process.stdout.isTTY && process.env.NO_COLOR === undefined;

for (const [name, [open, close]] of Object.entries(codes)) {
  module.exports[name] = (s) =>
    enabled ? `\x1b[${open}m${s}\x1b[${close}m` : String(s);
}
