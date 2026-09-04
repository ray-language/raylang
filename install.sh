#!/bin/sh
# Instalador de raylang (M44c). Descarga el binario de la plataforma desde la GitHub Release y lo instala.
#
#   curl -sSfL https://raylang.dev/install.sh | sh
#
# Variables de entorno (opcionales):
#   RAYLANG_VERSION   tag a instalar (p. ej. v1.0.0). Por defecto: la última release.
#   RAYLANG_BIN_DIR   directorio de instalación. Por defecto: $HOME/.local/bin
#   RAYLANG_REPO      owner/repo. Por defecto: ray-language/raylang
#   RAYLANG_DRY_RUN   si está definido, imprime el plan y NO descarga (para probar la detección).
set -eu

REPO="${RAYLANG_REPO:-ray-language/raylang}"
BIN_DIR="${RAYLANG_BIN_DIR:-$HOME/.local/bin}"

info() { printf '\033[1;34m→\033[0m %s\n' "$1"; }
err()  { printf '\033[1;31merror:\033[0m %s\n' "$1" >&2; exit 1; }

# --- Detectar plataforma → target triple de Rust ---
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)  suffix="unknown-linux-gnu"; ext="tar.gz" ;;
  Darwin) suffix="apple-darwin";      ext="tar.gz" ;;
  MINGW*|MSYS*|CYGWIN*)
    # Windows (Git Bash / MSYS): el instalador real es install.ps1 (M165). Si hay PowerShell a mano
    # se delega en él (hereda las variables RAYLANG_*); si no, se indica el comando.
    ps_url="https://raw.githubusercontent.com/$REPO/main/install.ps1"
    for ps in pwsh powershell powershell.exe; do
      if command -v "$ps" >/dev/null 2>&1; then
        info "Windows detectado: delegando en install.ps1 vía $ps"
        exec "$ps" -NoProfile -ExecutionPolicy Bypass -Command "irm $ps_url | iex"
      fi
    done
    err "en Windows, instala desde PowerShell:
       irm $ps_url | iex" ;;
  *) err "sistema operativo no soportado: $os" ;;
esac
case "$arch" in
  x86_64|amd64)   cpu="x86_64" ;;
  arm64|aarch64)  cpu="aarch64" ;;
  *) err "arquitectura no soportada: $arch" ;;
esac
target="${cpu}-${suffix}"
asset="raylang-${target}.${ext}"

# --- Resolver la URL de descarga ---
if [ -n "${RAYLANG_VERSION:-}" ]; then
  url="https://github.com/$REPO/releases/download/$RAYLANG_VERSION/$asset"
  version="$RAYLANG_VERSION"
else
  url="https://github.com/$REPO/releases/latest/download/$asset"
  version="latest"
fi

info "raylang · $target · $version"
info "asset:   $asset"
info "destino: $BIN_DIR"

if [ -n "${RAYLANG_DRY_RUN:-}" ]; then
  info "DRY RUN — url: $url"
  exit 0
fi

# --- Descargar ---
if command -v curl >/dev/null 2>&1; then
  dl() { curl -sSfL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  dl() { wget -qO "$2" "$1"; }
else
  err "hace falta 'curl' o 'wget'"
fi
command -v tar >/dev/null 2>&1 || err "hace falta 'tar'"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

info "descargando…"
dl "$url" "$tmp/$asset" || err "no se pudo descargar $url
       ¿existe una Release con ese asset? Mira https://github.com/$REPO/releases"

info "extrayendo…"
tar -xzf "$tmp/$asset" -C "$tmp"

# --- Instalar ---
mkdir -p "$BIN_DIR"
for bin in ray raylang; do
  [ -f "$tmp/$bin" ] || err "el paquete no contiene '$bin'"
  install -m 0755 "$tmp/$bin" "$BIN_DIR/$bin" 2>/dev/null || {
    cp "$tmp/$bin" "$BIN_DIR/$bin"; chmod 0755 "$BIN_DIR/$bin";
  }
done

info "instalado: $BIN_DIR/ray  (+ raylang)"
"$BIN_DIR/ray" version 2>/dev/null || true

# --- Guía de PATH ---
case ":$PATH:" in
  *":$BIN_DIR:"*) : ;;
  *) printf '\n\033[1;33mnota:\033[0m %s no está en tu PATH. Añádelo a tu shell:\n  export PATH="%s:$PATH"\n' "$BIN_DIR" "$BIN_DIR" ;;
esac
