#!/bin/sh
# tools/publish-packages.sh — publica paquetes del monorepo como ESPEJOS de solo-lectura en la
# organización github.com/ray-language, y su entrada (versión + hash) en el índice oficial
# ray-language/ray-index (M135, piloto con `rpc`).
#
# Modelo (DESIGN §132): el desarrollo vive en `raylang/packages/<pkg>` (tests + cambios en tándem
# con el lenguaje); el espejo es el ARTEFACTO DE RELEASE que consume el mundo (`ray add <pkg>`).
# Las versiones son inmutables: para publicar de nuevo, sube `version` en el ray.toml del paquete.
#
# Uso:      sh tools/publish-packages.sh <paquete> [<paquete>...]
# Requiere: push por ssh a github.com/ray-language/<pkg> y ray-index (el repo del espejo debe
#           existir en la org), y el binario `ray` (release o debug).
set -e

ORG_SSH=git@github.com:ray-language
ORG_HTTPS=https://github.com/ray-language
REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)

RAY=${RAY:-$REPO_ROOT/target/release/ray}
[ -x "$RAY" ] || RAY=$REPO_ROOT/target/debug/ray
[ -x "$RAY" ] || { echo "no 'ray' binary (build with: cargo build [--release])"; exit 66; }
[ $# -ge 1 ] || { echo "usage: sh tools/publish-packages.sh <package> [<package>...]"; exit 64; }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# El índice oficial: un clon fresco; las entradas nuevas se empujan al final.
git clone -q "$ORG_SSH/ray-index.git" "$WORK/index"

PUBLISHED=""
for PKG in "$@"; do
    SRC=$REPO_ROOT/packages/$PKG
    [ -d "$SRC" ] || { echo "no such package: packages/$PKG"; exit 64; }
    VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$SRC/ray.toml" | head -1)
    [ -n "$VERSION" ] || { echo "packages/$PKG/ray.toml has no version"; exit 65; }
    TAG=v$VERSION

    # Inmutabilidad: si el tag ya existe en el espejo remoto, no se re-publica.
    if git ls-remote --tags "$ORG_SSH/$PKG.git" "refs/tags/$TAG" 2>/dev/null | grep -q .; then
        echo "$PKG $TAG is already published (versions are immutable); bump the version"
        continue
    fi

    # Espejo: clon del repo de la org (o repo nuevo si aún está vacío) + snapshot del paquete.
    MIRROR=$WORK/$PKG
    if ! git clone -q "$ORG_SSH/$PKG.git" "$MIRROR" 2>/dev/null; then
        mkdir -p "$MIRROR"
        git -C "$MIRROR" init -q
        git -C "$MIRROR" remote add origin "$ORG_SSH/$PKG.git"
    fi
    find "$MIRROR" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
    (cd "$SRC" && tar cf - --exclude .ray-deps --exclude ray.lock .) | (cd "$MIRROR" && tar xf -)

    SHA=$(git -C "$REPO_ROOT" rev-parse --short HEAD)
    git -C "$MIRROR" add -A
    if git -C "$MIRROR" diff --cached --quiet; then
        echo "$PKG: no content changes vs mirror; tagging current content as $TAG"
    else
        git -C "$MIRROR" commit -qm "$PKG $VERSION (from raylang@$SHA)"
    fi
    git -C "$MIRROR" branch -M main
    git -C "$MIRROR" tag "$TAG"
    git -C "$MIRROR" push -q -u origin main "$TAG"

    # La entrada del índice, con URL https (consumo ANÓNIMO; ver PUBLISH.md).
    (cd "$MIRROR" && RAY_INDEX="$WORK/index" "$RAY" registry publish \
        --repo "git+$ORG_HTTPS/$PKG@$TAG")
    PUBLISHED="$PUBLISHED $PKG@$VERSION"
done

if [ -n "$PUBLISHED" ]; then
    git -C "$WORK/index" add -A
    git -C "$WORK/index" commit -qm "publish:$PUBLISHED"
    git -C "$WORK/index" push -q
    echo "index updated:$PUBLISHED"
else
    echo "nothing to publish"
fi
