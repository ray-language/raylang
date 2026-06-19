#!/usr/bin/env bash
#
# Compara el rendimiento del intérprete (M1) contra la VM (M2) de raylang.
#
# Uso:   benchmarks/bench.sh [programa.ray] [args extra para hyperfine...]
# Requiere: hyperfine  (https://github.com/sharkdp/hyperfine)
set -euo pipefail

# Ir a la raíz del repo (este script vive en benchmarks/).
cd "$(dirname "$0")/.."

# Asegurar cargo en el PATH (instalación vía rustup).
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

if ! command -v hyperfine >/dev/null 2>&1; then
    echo "Falta hyperfine: https://github.com/sharkdp/hyperfine" >&2
    exit 1
fi

PROG="${1:-benchmarks/fib.ray}"
shift || true

echo "Compilando en modo release..."
cargo build --release --quiet
BIN=./target/release/raylang

echo "Benchmark de: $PROG"
hyperfine --warmup 1 "$@" \
    -n "interprete (M1)" "$BIN $PROG" \
    -n "vm (M2)"         "$BIN --vm $PROG"
