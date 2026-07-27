#!/usr/bin/env sh

# Compila todos los .ray (nativo), .go y .rs de los subdirectorios, en paralelo
# (un job por core): compilar no es medir, acá el multicore es gratis.

set -eu

if ! command -v ray >/dev/null 2>&1; then
    echo "Error: no se encontró el comando 'ray' en PATH." >&2
    exit 127
fi

if ! command -v go >/dev/null 2>&1; then
    echo "Error: no se encontró el comando 'go' en PATH." >&2
    exit 127
fi

if ! command -v rustc >/dev/null 2>&1; then
    echo "Error: no se encontró el comando 'rustc' en PATH." >&2
    exit 127
fi

NPROC=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)

compile_one() {
    file=$1
    dir=$(dirname "$file")
    name=$(basename "$file")
    name=${name%.*}
    echo "Compilando ${file#./}..."
    case $file in
        *.ray) ray build --native --release "$file" -o "$dir/$name" ;;
        *.go)  go build -o "$dir/${name}-go" "$file" ;;
        *.rs)  rustc -O --edition 2021 -o "$dir/${name}-rs" "$file" ;;
    esac
}

# xargs -P corre hasta NPROC compilaciones a la vez; sh -c re-entra a este
# mismo script con el modo interno "__one" para reusar compile_one.
if [ "${1:-}" = "__one" ]; then
    compile_one "$2"
    exit 0
fi

found=false
for pattern in './*/*.ray' './*/*.go' './*/*.rs'; do
    # shellcheck disable=SC2086
    set -- $pattern
    [ -f "$1" ] && found=true
done

if [ "$found" = false ]; then
    echo "No se encontraron archivos .ray/.go/.rs en los subdirectorios."
    exit 0
fi

ls ./*/*.ray ./*/*.go ./*/*.rs 2>/dev/null | xargs -n1 -P "$NPROC" "$0" __one

echo "Listo (${NPROC} jobs en paralelo)."
