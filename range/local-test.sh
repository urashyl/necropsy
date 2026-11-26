#!/usr/bin/env bash
# Runs the code-based labs (implant-lab, elf-lab) on this machine without Docker or a
# spawner: builds the ELF, starts each lab's server on a loopback port with a throwaway
# flag, and runs the lab's own walkthrough against it. Fast, and catches protocol and
# build breakage before the real-Docker range job does.
#
#   range/local-test.sh        needs python3 and gcc

set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d)"
pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; rm -rf "$tmp"; }
trap cleanup EXIT

port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'; }

run_lab() {
  local lab="$1" flag="$2" extra=("${@:3}")
  local sock="$tmp/$lab-sock" p
  mkdir -p "$sock"; printf '%s' "$flag" > "$sock/flag"
  p="$(port)"
  env LAB_SOCK_DIR="$sock" LAB_PORT="$p" "${extra[@]}" python3 "$root/range/$lab/server.py" &
  pids+=("$!")
  for _ in $(seq 1 50); do curl -fs "http://127.0.0.1:$p/" >/dev/null 2>&1 && break; sleep 0.2; done
  [ ! -e "$sock/flag" ] || { echo "local-test: $lab left the flag file in place" >&2; exit 1; }
  python3 "$root/range/$lab/solve.py" "http://127.0.0.1:$p" --flag "$flag" >/dev/null
}

run_lab implant-lab 'implant{local0000test0000}'
gcc -O0 -s -no-pie -fno-stack-protector -o "$tmp/implant.bin" "$root/range/elf-lab/implant.c"
run_lab elf-lab 'elf{local0000test0000}' LAB_BINARY="$tmp/implant.bin"
echo "local-test: ok"
