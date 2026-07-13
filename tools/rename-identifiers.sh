#!/bin/bash
#
# Renombra identificadores español→inglés en src/, tests/, selfhost/,
# packages/ y benchmarks/ usando la tabla tools/es-en.dict.
#
# Motor: tools/rename.awk (reemplazo por PALABRA COMPLETA, ignora comentarios //).
#
# Características:
#   - Reemplaza TODAS las ocurrencias de cada token del diccionario.
#   - Solo tokens completos: "par" no toca "parser", "ver" no toca "server".
#   - Ignora la parte tras `//` (los comentarios en español quedan intactos).
#   - Genera .bak de cada archivo modificado (reversible).
#
# CAVEAT: un token suelto dentro de un string en español (p.ej. "una" en un
# mensaje de error) también se transforma. Por eso: correr -> `git diff` ->
# revisar -> corregir a mano lo puntual. NO commitea nada automáticamente.
#
# Uso:
#   tools/rename-identifiers.sh [dry-run]   # Preview: nº de líneas por archivo (default)
#   tools/rename-identifiers.sh apply       # Aplica (genera .bak)

set -e

readonly REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
readonly DICT="$REPO_ROOT/tools/es-en.dict"
readonly AWK_SCRIPT="$REPO_ROOT/tools/rename.awk"
readonly DIRS=(src tests selfhost packages benchmarks)
readonly MODE="${1:-dry-run}"
readonly WORK_DIR=$(mktemp -d)
trap "rm -rf $WORK_DIR" EXIT

for f in "$DICT" "$AWK_SCRIPT"; do
    [ -f "$f" ] || { echo "Error: $f no encontrado"; exit 1; }
done

if [ "$MODE" != "dry-run" ] && [ "$MODE" != "apply" ]; then
    echo "Uso: $0 [dry-run|apply]"
    exit 1
fi

# Lista de archivos .rs y .ray bajo los directorios en política.
FILES=$(for dir in "${DIRS[@]}"; do
    find "$REPO_ROOT/$dir" \( -name "*.rs" -o -name "*.ray" \) 2>/dev/null
done | sort)

[ -n "$FILES" ] || { echo "No hay archivos en ${DIRS[*]}"; exit 1; }

echo "Archivos: $(echo "$FILES" | grep -c '^')  |  tokens: $(grep -cvE '^$|^#' "$DICT")"
echo ""

case "$MODE" in
    dry-run)
        echo "=== DRY RUN (ningún archivo se modifica) ==="
        echo ""
        total=0
        while IFS= read -r file; do
            [ -z "$file" ] && continue
            out="$WORK_DIR/out"
            LC_ALL=C awk -v DICT="$DICT" -f "$AWK_SCRIPT" < "$file" > "$out"
            # nº de líneas que difieren (solo las '-' del lado original)
            changes=$(diff "$file" "$out" 2>/dev/null | grep -c '^<' || true)
            if [ "$changes" -gt 0 ]; then
                echo "  ${file#$REPO_ROOT/}: $changes líneas"
                total=$((total + changes))
            fi
        done <<< "$FILES"
        echo ""
        echo "Total: $total líneas cambiarían."
        echo "Aplicar con: tools/rename-identifiers.sh apply"
        ;;
    apply)
        echo "=== APLICANDO ==="
        echo ""
        changed=0
        while IFS= read -r file; do
            [ -z "$file" ] && continue
            out="$WORK_DIR/out"
            LC_ALL=C awk -v DICT="$DICT" -f "$AWK_SCRIPT" < "$file" > "$out"
            if ! diff -q "$file" "$out" >/dev/null 2>&1; then
                cp "$file" "${file}.bak"
                cp "$out" "$file"
                echo "  ✓ ${file#$REPO_ROOT/}"
                changed=$((changed + 1))
            fi
        done <<< "$FILES"
        echo ""
        echo "Archivos modificados: $changed  (respaldos .bak junto a cada uno)"
        echo ""
        echo "Siguiente:"
        echo "  1. Revisar:        git diff"
        echo "  2. Compilar/test:  source \"\$HOME/.cargo/env\" && cargo build && cargo test"
        echo "  3. Verificar CI:   cargo test --test naming_policy"
        echo "  4. Si algo falla:  find $REPO_ROOT -name '*.bak' -exec sh -c 'mv \"\$1\" \"\${1%.bak}\"' _ {} \\;"
        echo "  5. Si todo ok:     find $REPO_ROOT -name '*.bak' -delete"
        ;;
esac
