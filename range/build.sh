#!/bin/sh
# Builds every image under range/ as necropsy-range/<directory>:1, the names
# range/images.toml refers to. Run on the host that runs the spawner.
set -eu
cd "$(dirname "$0")"
for d in */; do
  name="${d%/}"
  [ -f "$name/Dockerfile" ] || continue
  echo "building necropsy-range/$name:1"
  docker build --pull -t "necropsy-range/$name:1" "$name"
done
