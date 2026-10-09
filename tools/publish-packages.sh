#!/bin/sh
# tools/publish-packages.sh — publica paquetes del monorepo como ESPEJOS de solo-lectura en la
# organización github.com/ray-language, y su entrada (versión + hash) en el índice oficial
# ray-language/ray-index (M135). Desde M368 es una envoltura fina de `ray registry mirror`
# (PUBLISH.md §8): el volcado, la reescritura de deps hermanas, el README público, el tag, el
# push y la entrada del índice viven en el subcomando.
#
# Modelo (DESIGN §132): el desarrollo vive en `raylang/packages/<pkg>` (tests + cambios en tándem
# con el lenguaje); el espejo es el ARTEFACTO DE RELEASE que consume el mundo (`ray add <pkg>`).
# Las versiones son inmutables: para publicar de nuevo, sube `version` en el ray.toml del paquete.
#
# Uso:      sh tools/publish-packages.sh [--refresh-readme] <paquete> [<paquete>...]
# Requiere: push por ssh a github.com/ray-language/<pkg> y ray-index (el repo del espejo debe
#           existir en la org), y el binario `ray` (release o debug).
set -e

ORG_SSH=git@github.com:ray-language
ORG_HTTPS=https://github.com/ray-language
REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)

# El binario `ray` MÁS FRESCO de los dos perfiles (un release rancio sin los fixes del publish
# es exactamente el accidente del estreno de M135b) — o el que fije $RAY.
if [ -z "$RAY" ]; then
    REL=$REPO_ROOT/target/release/ray
    DBG=$REPO_ROOT/target/debug/ray
    if [ -x "$REL" ] && [ -x "$DBG" ]; then
        if [ "$REL" -nt "$DBG" ]; then RAY=$REL; else RAY=$DBG; fi
    elif [ -x "$REL" ]; then RAY=$REL
    else RAY=$DBG
    fi
fi
[ -x "$RAY" ] || { echo "no 'ray' binary (build with: cargo build [--release])"; exit 66; }
[ $# -ge 1 ] || { echo "usage: sh tools/publish-packages.sh [--refresh-readme] <package> [<package>...]"; exit 64; }

MODE=publish
if [ "$1" = "--refresh-readme" ]; then
    MODE=readme
    shift
fi

for PKG in "$@"; do
    SRC=$REPO_ROOT/packages/$PKG
    [ -d "$SRC" ] || { echo "no such package: packages/$PKG"; exit 64; }
    if [ "$MODE" = "readme" ]; then
        (cd "$SRC" && "$RAY" registry mirror "$ORG_SSH/$PKG.git" --public "$ORG_HTTPS/$PKG" --readme-only)
    else
        (cd "$SRC" && "$RAY" registry mirror "$ORG_SSH/$PKG.git" --public "$ORG_HTTPS/$PKG" \
            --index "$ORG_SSH/ray-index.git")
    fi
done
