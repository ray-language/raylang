#!/bin/sh
# Produce `ray-runtime-vendor.tar.gz` (M171, IDEAS §85 2a): las dependencias de crates.io de
# `crates/ray-runtime` con TODAS sus features, vendorizadas con `cargo vendor`, más el
# `Cargo.lock` con el que se resolvieron. `release.yml` lo sube como asset de la release y
# `ray toolchain install` lo deja en `~/.ray/toolchain/vendor/<versión>/`; el proyecto Cargo que
# genera `ray build --native` toma de ahí sus crates → el primer build tras instalar no
# necesita red.
#
# El proyecto sonda replica la forma del proyecto GENERADO (`ray-runtime` como dep `path` en un
# subdirectorio homónimo, `[workspace]` vacío) para que el lock sea el mismo que usará el build.
#
# Uso: sh tools/vendor-runtime.sh [salida.tar.gz]   (desde la raíz del repo)
set -eu

out="${1:-ray-runtime-vendor.tar.gz}"
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac
root="$(cd "$(dirname "$0")/.." && pwd)"
rt="$root/crates/ray-runtime"
[ -f "$rt/Cargo.toml" ] || { echo "vendor-runtime: $rt/Cargo.toml not found" >&2; exit 66; }

# Todas las features declaradas en el Cargo.toml de ray-runtime (menos `default`), leídas del
# propio manifiesto para no mantener dos listas.
features="$(awk '/^\[features\]/{f=1;next} /^\[/{f=0} f && /^[a-z0-9_-]+ *=/{sub(/ *=.*/,""); if($0!="default") print}' "$rt/Cargo.toml" | tr '\n' ',' | sed 's/,$//')"
[ -n "$features" ] || { echo "vendor-runtime: no features found in $rt/Cargo.toml" >&2; exit 65; }
feats_toml="$(printf '%s' "$features" | sed 's/,/", "/g; s/^/"/; s/$/"/')"

work="$(mktemp -d "${TMPDIR:-/tmp}/ray-vendor.XXXXXX")"
trap 'rm -rf "$work"' EXIT INT TERM
mkdir -p "$work/src" "$work/ray-runtime"
cp -R "$rt/Cargo.toml" "$rt/src" "$work/ray-runtime/"
cat > "$work/Cargo.toml" <<EOF
[package]
name = "ray-vendor-probe"
version = "0.0.0"
edition = "2024"

[workspace]

[dependencies]
ray-runtime = { path = "ray-runtime", default-features = false, features = [$feats_toml] }
EOF
printf 'fn main() {}\n' > "$work/src/main.rs"

echo "vendor-runtime: features = $features"
( cd "$work" && cargo generate-lockfile --quiet && cargo vendor --quiet vendor >/dev/null )
# El tarball lleva SOLO lo que consume `ray`: `vendor/` + `Cargo.lock` (installed_vendor exige ambos).
( cd "$work" && tar czf "$out" Cargo.lock vendor )
size="$(du -h "$out" | cut -f1)"
echo "vendor-runtime: wrote $out ($size, $(ls "$work/vendor" | wc -l | tr -d ' ') crates)"
