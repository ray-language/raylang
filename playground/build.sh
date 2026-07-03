#!/usr/bin/env sh
# M44a-3: construye el .wasm del playground (release) y lo deja junto a index.html.
# Si `wasm-opt` (binaryen) está instalado, reduce aún más el tamaño (-Oz).
set -e
cd "$(dirname "$0")/.."

echo "→ compilando raylang a wasm32 (release)…"
cargo build --target wasm32-unknown-unknown --lib --release

cp target/wasm32-unknown-unknown/release/raylang.wasm playground/raylang.wasm

if command -v wasm-opt >/dev/null 2>&1; then
  echo "→ wasm-opt -Oz…"
  wasm-opt -Oz playground/raylang.wasm -o playground/raylang.wasm
else
  echo "  (wasm-opt no encontrado; instala 'binaryen' para reducir el tamaño)"
fi

echo "✓ playground/raylang.wasm ($(du -h playground/raylang.wasm | cut -f1))"
echo "  sírvelo:  (cd playground && python3 -m http.server 8000)  →  http://localhost:8000"
