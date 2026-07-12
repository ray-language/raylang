#!/bin/sh
# PGO (A3, IDEAS §45): compila el binario de release optimizado por perfil.
# Tres pasos: (1) build instrumentado → (2) entrenar con cargas representativas
# (banco + strings/iter + parse del self-hosting + concurrencia) → (3) rebuild
# con el perfil. El resultado queda en ./target/release (el symlink ~/.local/bin/ray
# lo ve directo). Medido (best-of-15, M3 Pro): fib −5,2% · loop −5,4% ·
# arrays −8,7% · gcnested −6,0% — consistente, sobre el ruido (§27).
#
# Ojo: cambiar RUSTFLAGS invalida la caché de target/release → el siguiente
# `cargo build --release` a secas recompila entero (y viceversa). Este script
# es para cortar releases; el ciclo de desarrollo sigue con cargo a secas.
set -e
cd "$(dirname "$0")/.."
. "$HOME/.cargo/env" 2>/dev/null || true

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
    cargo build --release --quiet --target-dir target/pgo-gen

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
RUSTFLAGS="-Cprofile-use=$PROFDIR/merged.profdata" cargo build --release --quiet

echo "listo: ./target/release/{ray,raylang} optimizados por perfil"
