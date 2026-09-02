#!/usr/bin/env bash
# Resincroniza compiler/inference/crates/ desde el repo de Skynet
# (GRAMMAR.md §3.233). Uso: scripts/sync-inference-engine.sh [ruta-skynet]
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
src="${1:-$HOME/Documents/skynet}/inference-engine"
[ -d "$src/crates" ] || { echo "no encuentro $src/crates" >&2; exit 1; }
for c in gguf tensor-core model-core llama gemma4 qwen2 qwen3 phi3 phimoe server; do
  rm -rf "$here/compiler/inference/crates/$c"
  cp -r "$src/crates/$c" "$here/compiler/inference/crates/$c"
  rm -rf "$here/compiler/inference/crates/$c/target"
done
commit="$(git -C "$src/.." rev-parse HEAD)"
sed -i "s/^- Commit de origen: .*/- Commit de origen: \`$commit\` ($(date +%F))/" "$here/compiler/inference/VENDORED.md"
echo "sincronizado desde $commit"
