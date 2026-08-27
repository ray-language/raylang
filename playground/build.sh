#!/usr/bin/env sh
# M44a-3: construye el .wasm del playground (release) y lo deja junto a index.html.
# Si `wasm-opt` (binaryen) está instalado, reduce aún más el tamaño (-Oz).
# IDEAS §74: también empaqueta el editor real (CodeMirror 6) con npm/esbuild.
set -e
cd "$(dirname "$0")/.."

echo "→ compilando raylang a wasm32 (release)…"
cargo build --target wasm32-unknown-unknown --lib --release

cp target/wasm32-unknown-unknown/release/raylang.wasm playground/raylang.wasm

if command -v wasm-opt >/dev/null 2>&1; then
  echo "→ wasm-opt -Oz…"
  # SOLO las features que emite rustc (--all-features dejó a un binaryen viejo escribir un
  # value type que el navegador rechaza); si tu wasm-opt falla, borra este paso — el wasm
  # sin optimizar funciona igual.
  wasm-opt -Oz --enable-bulk-memory --enable-sign-ext --enable-mutable-globals \
    --enable-nontrapping-float-to-int playground/raylang.wasm -o playground/raylang.wasm
else
  echo "  (wasm-opt no encontrado; instala 'binaryen' para reducir el tamaño)"
fi

if command -v npm >/dev/null 2>&1; then
  echo "→ editor.bundle.js (CodeMirror 6, esbuild)…"
  (cd playground/editor && npm install --no-audit --no-fund --silent && npm run --silent build)
else
  echo "  (npm no encontrado; el editor real no se puede construir — instala node/npm)"
fi

echo "✓ playground/raylang.wasm ($(du -h playground/raylang.wasm | cut -f1))"
echo "  sírvelo:  (cd playground && python3 -m http.server 8000)  →  http://localhost:8000"
