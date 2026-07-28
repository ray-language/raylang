#!/usr/bin/env sh

# Compila las implementaciones de los DOS escalones del banco (ver README §Escalones).
# node no compila (se ejecuta con `node main.js`); express solo necesita `npm install`.

set -eu

cd "$(dirname "$0")"

[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

for tool in ray go cargo npm; do
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

echo "Compilando json/ray (framework, nativo)..."
(cd json/ray && ray build --native --release main.ray -o json-ray)

echo "Compilando json/chi..."
(cd json/chi && go build -o json-chi main.go)

echo "Compilando json/axum (release)..."
(cd json/axum && cargo build --release --quiet)

echo "Instalando json/express..."
(cd json/express && npm install --silent --no-audit --no-fund)

echo "Listo."
