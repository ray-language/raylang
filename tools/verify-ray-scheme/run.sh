#!/bin/sh
# Verificación del esquema ray:// en Linux (WebKitGTK) con una ventana real. Desde la raíz del repo:
#   sh tools/verify-ray-scheme/run.sh
# Requiere la sesión gráfica (DISPLAY/WAYLAND_DISPLAY), libgtk-3 y libwebkit2gtk-4.1 (o 4.0).
set -e
cd "$(dirname "$0")/../.."
. "$HOME/.cargo/env" 2>/dev/null || true
KIT="$(pwd)/tools/verify-ray-scheme"
[ -f "$KIT/big.bin" ] || head -c 8388608 /dev/urandom > "$KIT/big.bin"
cargo build --release --quiet
echo "== toolchain: $(./target/release/ray version)"
echo "== webkit: $(ldconfig -p 2>/dev/null | grep -o 'libwebkit2gtk-4\.[01]\.so[^ ]*' | head -1)"
./target/release/ray run "$KIT/probe.ray" "$KIT"
