#!/usr/bin/env sh

# Compila las implementaciones del banco de carga web. node no compila (se ejecuta con
# `node main.js`), así que no aparece aquí.

set -eu

cd "$(dirname "$0")"

[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

for tool in ray go cargo; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "Error: no se encontró '$tool' en PATH." >&2
        exit 127
    fi
done

echo "Compilando plaintext/ray (nativo)..."
(cd plaintext/ray && ray build --native --release main.ray -o plaintext-ray)

echo "Compilando plaintext/go..."
(cd plaintext/go && go build -o plaintext-go main.go)

echo "Compilando plaintext/hyper (release)..."
(cd plaintext/hyper && cargo build --release --quiet)

echo "Listo."
