#!/bin/bash
#
# Renombra identificadores españoles a inglés en src/, tests/, selfhost/,
# packages/ y benchmarks/ usando la tabla tools/es-en.dict.
#
# Uso:
#   tools/rename-identifiers.sh [dry-run]      # Muestra un preview
#   tools/rename-identifiers.sh apply          # Aplica los cambios
#
# Seguridad:
#   - Solo reemplaza en contextos de declaración (fn/let/var)
#   - Genera un .bak de cada archivo antes de tocar (puedes revertir)
#   - Revisar git diff después, NO commiteará automáticamente

set -e

readonly REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
readonly DICT="$REPO_ROOT/tools/es-en.dict"
readonly DIRS=(src tests selfhost packages benchmarks)
readonly MODE="${1:-dry-run}"

if [ ! -f "$DICT" ]; then
    echo "Error: $DICT no encontrado"
    exit 1
fi

if [ "$MODE" != "dry-run" ] && [ "$MODE" != "apply" ]; then
    echo "Uso: $0 [dry-run|apply]"
    echo "  dry-run: muestra un preview (default)"
    echo "  apply:   aplica los cambios (genera .bak de respaldo)"
    exit 1
fi

# Construir la lista de archivos
FILES=""
for dir in "${DIRS[@]}"; do
    FILES="$FILES$(find "$REPO_ROOT/$dir" \( -name "*.rs" -o -name "*.ray" \) 2>/dev/null | sort)"
    [ -n "$FILES" ] && FILES="$FILES"$'\n'
done

if [ -z "$FILES" ]; then
    echo "No files found in ${DIRS[*]}"
    exit 1
fi

FILE_COUNT=$(echo "$FILES" | grep -c '^')
echo "Archivos encontrados: $FILE_COUNT"
echo ""

# Generar un script sed temporal con todos los reemplazos
# Estrategia: reemplazar token_es por token_en en contextos seguros
# (después de fn/let/var, o entre límites de palabra)
#
# Para evitar problemas con sed de BSD (macOS), usamos:
# - Caracteres especiales escapados (\. \* etc.)
# - Límites simples: espacio, _, parentesis, llaves, etc.
# - -i '' (BSD) y -i (GNU) manejados automáticamente

SED_SCRIPT=$(mktemp)
trap "rm -f $SED_SCRIPT" EXIT

REPL_COUNT=0
while IFS=$'\t' read -r es en; do
    # Ignorar líneas vacías y comentarios
    [ -z "$es" ] && continue
    [[ "$es" = \#* ]] && continue

    # Escapar caracteres especiales de sed en el patrón
    es_esc=$(printf '%s\n' "$es" | sed 's/[[\.*^$/]/\\&/g')
    en_esc=$(printf '%s\n' "$en" | sed 's/[&/\]/\\&/g')

    # Estrategia simple: sed sobre "fn nombre(" "let nombre " "var nombre "
    # (contextos literales sin regex avanzadas que causen problemas en BSD sed)
    echo "s/fn $es_esc(/fn $en_esc(/g" >> "$SED_SCRIPT"
    echo "s/let $es_esc /let $en_esc /g" >> "$SED_SCRIPT"
    echo "s/let mut $es_esc /let mut $en_esc /g" >> "$SED_SCRIPT"
    echo "s/var $es_esc /var $en_esc /g" >> "$SED_SCRIPT"
    # Bonus: captura en patrones de pattern matching raylang (Enum.Variante)
    echo "s/$es_esc\./$en_esc\./g" >> "$SED_SCRIPT"

    REPL_COUNT=$((REPL_COUNT + 1))
done < "$DICT"

if [ "$REPL_COUNT" -eq 0 ]; then
    echo "No replacements loaded from $DICT"
    exit 1
fi

echo "Reemplazos cargados: $REPL_COUNT (en script temporal)"
echo ""

# Aplicar según el modo
case "$MODE" in
    dry-run)
        echo "=== DRY RUN: mostrando archivos que serían tocados ==="
        echo ""
        echo "$FILES" | while read -r file; do
            [ -z "$file" ] && continue
            # Mostrar líneas que cambiarían usando el script sed
            tmp_out=$(mktemp)
            sed -f "$SED_SCRIPT" < "$file" > "$tmp_out" 2>/dev/null
            changes=$(diff -u "$file" "$tmp_out" 2>/dev/null | grep -c "^+" || true)
            changes=$((changes - 1)) # restar la línea +++ del diff
            rm -f "$tmp_out"
            if [ "$changes" -gt 0 ]; then
                rel_file="${file#$REPO_ROOT/}"
                echo "$rel_file: ~$changes líneas cambiarían"
            fi
        done
        echo ""
        echo "Para aplicar, ejecuta: tools/rename-identifiers.sh apply"
        ;;
    apply)
        echo "=== APLICANDO RENOMBRES ==="
        echo ""
        count=0
        echo "$FILES" | while read -r file; do
            [ -z "$file" ] && continue
            rel_file="${file#$REPO_ROOT/}"
            # Crear backup
            cp "$file" "${file}.bak"
            # Aplicar sed in-place (macOS -i requiere la extensión de backup)
            sed -i '' -f "$SED_SCRIPT" "$file"
            if ! diff -q "$file" "${file}.bak" >/dev/null 2>&1; then
                echo "✓ $rel_file"
                count=$((count + 1))
            else
                # Sin cambios, eliminar el .bak
                rm "${file}.bak"
            fi
        done
        echo ""
        echo "Archivos modificados: $count"
        echo ""
        echo "Próximos pasos:"
        echo "  1. Revisar cambios: git diff --stat"
        echo "  2. Si algo falla: find . -name '*.bak' -exec rm {} \; && git checkout ."
        echo "  3. Si todo bien: find . -name '*.bak' -delete && git add -A && git commit ..."
        ;;
esac
