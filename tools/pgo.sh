#!/bin/sh
# PGO (A3, IDEAS §45): compila el binario de release optimizado por perfil.
# Tres pasos: (1) build instrumentado → (2) entrenar con cargas representativas
# (banco + strings/iter + parse del self-hosting + concurrencia) → (3) rebuild
# con el perfil. El resultado queda en ./target/release (el symlink ~/.local/bin/ray
# lo ve directo). Guía completa de builds (features slim, flags extra): docs/build.md.
#
# Uso:
#   tools/pgo.sh                          # release default + PGO
#   tools/pgo.sh --slim                   # slim total (solo 'interp') + PGO
#   tools/pgo.sh --features "a,b,c"       # combo de features a medida + PGO
# El set de features se aplica a los DOS builds (instrumentado y final) para que
# el perfil case función a función. Las cargas de entrenamiento no usan
# TLS/SQLite/FFI → valen para cualquier combo.
#
# Qué esperar: el binario PGO da tiempos estables; el delta contra el plano
# depende del layout que le tocó al plano ese día (−5 a −10% medido en la ronda 2;
# ~0-4% tras el renombrado ES→EN). La métrica estable es el tiempo ABSOLUTO del
# binario PGO (benchmarks/measure.py, mejor de 15).
#
# Ojo: cambiar RUSTFLAGS invalida la caché de target/release → el siguiente
# `cargo build --release` a secas recompila entero (y viceversa). Este script
# es para cortar releases; el ciclo de desarrollo sigue con cargo a secas.
set -e
cd "$(dirname "$0")/.."
. "$HOME/.cargo/env" 2>/dev/null || true

# Features del build (sin comillas al expandir: el word-splitting es intencional).
FEATURES=""
case "${1:-}" in
    --slim)     FEATURES="--no-default-features --features interp" ;;
    --features) [ -n "${2:-}" ] || { echo "uso: tools/pgo.sh --features \"a,b,c\"" >&2; exit 1; }
                FEATURES="--no-default-features --features $2" ;;
    "")         ;;
    *)          echo "opción desconocida: $1 (uso: tools/pgo.sh [--slim | --features \"a,b,c\"])" >&2
                exit 1 ;;
esac

HOST=$(rustc -vV | sed -n 's/^host: //p')
PROFDATA="$(rustc --print sysroot)/lib/rustlib/$HOST/bin/llvm-profdata"
if [ ! -x "$PROFDATA" ]; then
    echo "falta llvm-profdata: rustup component add llvm-tools" >&2
    exit 1
fi

PROFDIR=$(mktemp -d)
trap 'rm -rf "$PROFDIR"' EXIT

echo "== 1/3 build instrumentado"
RUSTFLAGS="-Cprofile-generate=$PROFDIR" CARGO_PROFILE_RELEASE_STRIP=none \
    cargo build --release --quiet --target-dir target/pgo-gen $FEATURES

echo "== 2/3 entrenamiento"
B=./target/pgo-gen/release/raylang
for w in benchmarks/fib35.ray benchmarks/loop.ray benchmarks/arrays.ray \
         benchmarks/gcnested.ray benchmarks/strings.ray benchmarks/iter.ray; do
    "$B" --vm "$w" > /dev/null 2>&1 || true
done
# Parse pesado (el parser auto-alojado sobre sí mismo) y una carga concurrente.
"$B" --vm selfhost/parse_dump.ray selfhost/parser.ray > /dev/null 2>&1 || true
"$B" --vm examples/concurrency/concurrencia.ray > /dev/null 2>&1 || true

"$PROFDATA" merge -o "$PROFDIR/merged.profdata" "$PROFDIR"

echo "== 3/3 build final con el perfil"
RUSTFLAGS="-Cprofile-use=$PROFDIR/merged.profdata" cargo build --release --quiet $FEATURES

echo "listo: ./target/release/{ray,raylang} optimizados por perfil"
