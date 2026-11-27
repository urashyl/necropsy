#!/usr/bin/env bash
# Downloads the third-party parts of the in-browser toolkit (Wiregasm, Pyodide,
# CyberChef) listed in toolkit/sources.txt, checks each download against its
# SHA-256, and unpacks it into toolkit/dist/<name>-<version>/. Every file over
# 1 KiB also gets a .gz copy, which the server sends to browsers that accept
# gzip. See docs/toolkit.md.
#
#   toolkit/fetch.sh [dest]      dest defaults to toolkit/dist
#
# Safe to run again: a package whose directory already exists is skipped.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
dest="${1:-$here/dist}"
mkdir -p "$dest"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# Keeps only the files the toolkit serves.
install_wiregasm() { # src out
  mkdir -p "$2" "$work/wg"
  tar -xzf "$1" -C "$work/wg"
  cp "$work/wg/package/dist/wiregasm.js" "$work/wg/package/dist/wiregasm.wasm" \
     "$work/wg/package/dist/wiregasm.data" "$work/wg/package/LICENSE" "$2/"
}

install_pyodide() {
  mkdir -p "$2" "$work/py"
  tar -xzf "$1" -C "$work/py"
  local p="$work/py/package"
  cp "$p/pyodide.mjs" "$p/pyodide.asm.mjs" "$p/pyodide.asm.wasm" \
     "$p/python_stdlib.zip" "$p/pyodide-lock.json" "$2/"
  # The npm package has no licence file of its own; Pyodide is MPL-2.0.
  printf 'Pyodide is licensed under the Mozilla Public License 2.0.\nSource: https://github.com/pyodide/pyodide\n' > "$2/LICENSE"
}

install_cyberchef() {
  mkdir -p "$2"
  (cd "$2" && unzip -q -o "$1")
  # The release names its page after the version; serve it as index.html.
  mv "$2"/CyberChef_v*.html "$2/index.html"
  # The release ships its own precompressed copies; ours are made below.
  find "$2" \( -name '*.br' -o -name '*.gz' \) -delete
  printf 'CyberChef is licensed under the Apache License 2.0.\nSource: https://github.com/gchq/CyberChef\n' > "$2/LICENSE"
}

grep -v '^\s*#' "$here/sources.txt" | while read -r name version sum url; do
  [ -n "${name:-}" ] || continue
  out="$dest/$name-$version"
  if [ -d "$out" ]; then
    echo "$name $version: already in $out"
    continue
  fi
  file="$work/$name.download"
  echo "$name $version: downloading $url"
  curl -fsSL --retry 3 -o "$file" "$url"
  got="$(sha256 "$file")"
  if [ "$got" != "$sum" ]; then
    echo "$name $version: SHA-256 is $got, expected $sum. Not installing it." >&2
    exit 1
  fi
  "install_$name" "$file" "$out.tmp"
  find "$out.tmp" -type f -size +1k ! -name '*.gz' -exec gzip -9 -k -n {} \;
  mv "$out.tmp" "$out"
  echo "$name $version: installed in $out"
done
