#!/usr/bin/env bash
# El ritual de release mecanizado (GRAMMAR.md §3.215, PLAN.md §9.17 ítem 11).
#
# Este proyecto ya shipeó TRES veces una versión con CI en rojo por saltear a
# mano un paso del ritual (v1.130.0 y v1.165.0 por un snapshot sin regenerar
# antes del commit; v1.200.0 porque el propio ritual mecanizado no corría
# `cargo clippy` y CI sí lo exige con -D warnings, GRAMMAR.md §3.216). Cada
# paso de abajo existía ya como disciplina documentada (AGENTS.md, CHANGELOG);
# lo único nuevo es que ahora es un programa y no memoria.
#
# Uso, DESPUÉS de editar Cargo.toml (versión nueva) + CHANGELOG.md + GRAMMAR.md:
#
#   scripts/release.sh "mensaje del commit"
#   scripts/release.sh --skip-suite "mensaje"   # si la suite completa YA corrió verde sobre este árbol
#
# Qué hace, en orden, cortando al primer fallo:
#   1. Lee la versión de compiler/Cargo.toml y verifica que CHANGELOG.md la nombra.
#   2. Verifica que el tag vX.Y.Z no exista todavía.
#   3. cargo build --release
#   4. cargo clippy --all-targets -- -D warnings (la causa raíz del CI rojo de v1.200.0).
#   5. Regenera examples/users.link.snap (la causa raíz de los otros dos CI rojos).
#   6. Regenera examples/taskboard/frontend/src/gen (la otra mitad, §9.17 ítem 10).
#   7. Suite completa (--test-threads=4) salvo --skip-suite.
#   8. Commit + tag vX.Y.Z + push de rama y tag.
#
# Lo que NO hace a propósito: verificar CI. `gh run view --exit-status` sigue
# siendo el último paso manual -- un push verde local nunca prueba CI verde.

set -euo pipefail
cd "$(dirname "$0")/.."

SKIP_SUITE=0
if [ "${1:-}" = "--skip-suite" ]; then
  SKIP_SUITE=1
  shift
fi
MSG="${1:-}"
if [ -z "$MSG" ]; then
  echo "uso: scripts/release.sh [--skip-suite] \"mensaje del commit\"" >&2
  exit 2
fi

VERSION=$(grep -m1 '^version = ' compiler/Cargo.toml | sed 's/version = "\(.*\)"/\1/')
if [ -z "$VERSION" ]; then
  echo "error: no se pudo leer la versión de compiler/Cargo.toml" >&2
  exit 1
fi
echo "== release v$VERSION =="

if ! grep -q "^## \[$VERSION\]" CHANGELOG.md; then
  echo "error: CHANGELOG.md no tiene una entrada '## [$VERSION]' -- escribila antes de releasear" >&2
  exit 1
fi

if git rev-parse -q --verify "refs/tags/v$VERSION" >/dev/null; then
  echo "error: el tag v$VERSION ya existe -- ¿olvidaste subir la versión en Cargo.toml?" >&2
  exit 1
fi

echo "== cargo build --release =="
(cd compiler && cargo build --release)

echo "== cargo clippy --all-targets -- -D warnings (GRAMMAR.md §3.216) =="
(cd compiler && cargo clippy --release --all-targets -- -D warnings)

BIN=compiler/target/release/linkc
[ -x "$BIN" ] || BIN="$BIN.exe"

echo "== regenerar examples/users.link.snap =="
"$BIN" test examples/users.link examples/users.link.snap --update

echo "== regenerar examples/taskboard/frontend/src/gen =="
# El warning de main.wasm (tipos compuestos sin soporte wasm) es esperado y
# no es un fallo -- ver el paso equivalente en .github/workflows/ci.yml.
"$BIN" build examples/taskboard/backend/taskboard.link examples/taskboard/frontend/src/gen

if [ "$SKIP_SUITE" -eq 0 ]; then
  echo "== suite completa (cargo test --release -- --test-threads=4) =="
  (cd compiler && cargo test --release -- --test-threads=4)
else
  echo "== suite completa SALTEADA (--skip-suite) -- solo válido si ya corrió verde sobre este mismo árbol =="
fi

echo "== commit + tag v$VERSION + push =="
git add -A
git status --short
git commit -m "$MSG"
git tag "v$VERSION"
git push
git push origin "v$VERSION"

echo ""
echo "== v$VERSION pusheada. ÚLTIMO PASO (manual, no opcional): verificar CI real =="
echo "   gh run list --limit 3"
echo "   gh run view <id> --exit-status   # CI y Release Binaries, los dos"
