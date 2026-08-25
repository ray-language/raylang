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


WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# Transforma el README de un ESPEJO para el consumidor público (M135c): (1) las líneas de
# dependencia por ruta (`x = "path:…"`) pasan a su URL git pinneada; (2) se antepone el bloque
# de instalación (índice + `ray add`) y el aviso de espejo de solo lectura. Idempotente.
# Uso: transform_readme <readme> <pkg> <version>
transform_readme() {
    R=$1; TPKG=$2; TVER=$3
    [ -f "$R" ] || return 0
    # Coloreado en GitHub: \`\`\`raylang aún no existe como lenguaje reconocido — por ahora los
    # bloques de código raylang se etiquetan \`rust\` (sintaxis de llaves cercana; linguist lo
    # colorea decente). Cuando la gramática de ray-language/raylang-grammar entre a linguist,
    # esta línea sobra.
    sed -i '' 's/^```raylang$/```rust/' "$R"
    for DN in $(sed -n 's/^\([a-z0-9_-]*\) = "path:.*/\1/p' "$R" | sort -u); do
        DV=$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO_ROOT/packages/$DN/ray.toml" 2>/dev/null | head -1)
        [ -n "$DV" ] || continue
        sed -i '' "s|^$DN = \"path:[^\"]*\"|$DN = \"git+$ORG_HTTPS/$DN@v$DV\"|" "$R"
    done
    if ! grep -q "Espejo de solo lectura" "$R"; then
        HDR=$WORK/.readme_header
        # Heredoc CITADO (sin expansión: los backticks de markdown son texto) + placeholders.
        cat > "$HDR" <<'EOF'
> **Espejo de solo lectura** — publicado desde
> [`raylang/packages/@PKG@`](https://github.com/ray-language/raylang/tree/main/packages/@PKG@);
> el desarrollo y los PRs van al monorepo.
>
> **Instalación** — en tu `ray.toml`:
>
> ```toml
> [registry]
> index = "git+https://github.com/ray-language/ray-index@main"
> ```
>
> y `ray add @PKG@` — o la dependencia directa:
> `@PKG@ = "git+https://github.com/ray-language/@PKG@@v@VER@"`.

EOF
        sed -i '' "s|@PKG@|$TPKG|g; s|@VER@|$TVER|g" "$HDR"
        # El bloque va DESPUÉS del título H1 (antes lo enterraba); sin H1, al principio.
        if head -1 "$R" | grep -q '^# '; then
            { head -1 "$R"; echo; cat "$HDR"; tail -n +2 "$R"; } > "$R.tmp"
        else
            cat "$HDR" "$R" > "$R.tmp"
        fi
        mv "$R.tmp" "$R"
    fi
}

# --refresh-readme: SOLO re-genera el README público de espejos ya publicados (rama main, sin
# tocar tags — el hash del índice verifica el contenido del TAG, así que es seguro). Para el
# accidente inverso (contenido nuevo) el camino es subir la versión y publicar normal.
if [ "$1" = "--refresh-readme" ]; then
    shift
    W2=$(mktemp -d); trap 'rm -rf "$W2"' EXIT
    WORK=$W2
    for PKG in "$@"; do
        VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO_ROOT/packages/$PKG/ray.toml" | head -1)
        git clone -q "$ORG_SSH/$PKG.git" "$W2/$PKG" || { echo "no mirror for $PKG"; exit 65; }
        cp "$REPO_ROOT/packages/$PKG/README.md" "$W2/$PKG/README.md" 2>/dev/null || { echo "$PKG: no README"; continue; }
        transform_readme "$W2/$PKG/README.md" "$PKG" "$VERSION"
        if git -C "$W2/$PKG" diff --quiet; then
            echo "$PKG: README already up to date"
        else
            git -C "$W2/$PKG" commit -qam "docs: public-mirror usage (index + ray add; no local paths)"
            git -C "$W2/$PKG" push -q
            echo "$PKG: README refreshed on main"
        fi
    done
    exit 0
fi

# El índice oficial: un clon fresco; las entradas nuevas se empujan al final.
git clone -q "$ORG_SSH/ray-index.git" "$WORK/index"

PUBLISHED=""
for PKG in "$@"; do
    SRC=$REPO_ROOT/packages/$PKG
    [ -d "$SRC" ] || { echo "no such package: packages/$PKG"; exit 64; }
    VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$SRC/ray.toml" | head -1)
    [ -n "$VERSION" ] || { echo "packages/$PKG/ray.toml has no version"; exit 65; }
    TAG=v$VERSION

    # Inmutabilidad: la fuente de verdad es el ÍNDICE. Si la versión ya tiene entrada, nada que
    # hacer; si el TAG existe remoto pero el índice no la tiene (un run anterior a medias), se
    # REANUDA: el contenido publicado es el del tag, solo falta su entrada.
    if grep -q "^\[$VERSION\]" "$WORK/index/$PKG.toml" 2>/dev/null; then
        echo "$PKG $TAG is already in the index (versions are immutable); bump the version"
        continue
    fi
    MIRROR=$WORK/$PKG
    if git ls-remote --tags "$ORG_SSH/$PKG.git" "refs/tags/$TAG" 2>/dev/null | grep -q .; then
        echo "$PKG $TAG exists in the mirror but not in the index; resuming its index entry"
        git clone -q "$ORG_SSH/$PKG.git" "$MIRROR"
        git -C "$MIRROR" checkout -q "$TAG"
        (cd "$MIRROR" && RAY_INDEX="$WORK/index" "$RAY" registry publish \
            --repo "git+$ORG_HTTPS/$PKG@$TAG")
        PUBLISHED="$PUBLISHED $PKG@$VERSION"
        continue
    fi

    # Espejo: clon del repo de la org (o repo nuevo si aún está vacío) + snapshot del paquete.
    if ! git clone -q "$ORG_SSH/$PKG.git" "$MIRROR" 2>/dev/null; then
        mkdir -p "$MIRROR"
        git -C "$MIRROR" init -q
        git -C "$MIRROR" remote add origin "$ORG_SSH/$PKG.git"
    fi
    find "$MIRROR" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
    (cd "$SRC" && tar cf - --exclude .ray-deps --exclude ray.lock .) | (cd "$MIRROR" && tar xf -)

    # M135b: las path-deps entre paquetes hermanos (`x = "path:../x"`) se REESCRIBEN en el
    # espejo a su URL git pinneada al tag de la versión ACTUAL del hermano en este monorepo —
    # el espejo debe ser autocontenido (un consumidor no tiene el monorepo al lado). La
    # resolución transitiva del consumidor (BFS de deps::ensure) sigue esa URL.
    for DEPPATH in $(sed -n 's/^\([a-z_][a-z0-9_-]*\) = "path:\.\.\/\([a-z0-9_-]*\)"$/\2/p' "$MIRROR/ray.toml"); do
        DEPV=$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO_ROOT/packages/$DEPPATH/ray.toml" | head -1)
        [ -n "$DEPV" ] || { echo "$PKG depends on packages/$DEPPATH which has no version"; exit 65; }
        sed -i '' "s|= \"path:\.\./$DEPPATH\"|= \"git+$ORG_HTTPS/$DEPPATH@v$DEPV\"|" "$MIRROR/ray.toml"
        echo "$PKG: rewrote dep $DEPPATH -> git+$ORG_HTTPS/$DEPPATH@v$DEPV"
    done

    transform_readme "$MIRROR/README.md" "$PKG" "$VERSION"

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
