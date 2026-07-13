# rename.awk — reemplazo de identificadores español→inglés por PALABRA COMPLETA.
#
# Invocación: awk -v DICT=tools/es-en.dict -f tools/rename.awk <archivo>
#
# Reglas:
#   - Carga la tabla DICT (formato "es<TAB>en") en un array.
#   - Por cada línea: separa el código del comentario `//` y solo transforma el
#     código (el comentario en español queda intacto).
#   - Reemplaza solo TOKENS COMPLETOS ([A-Za-z0-9_]+): "par" NO toca "parser".
#   - Cubre TODAS las ocurrencias de la línea.
#
# Aviso: si un token corto aparece como palabra suelta dentro de un string en
# español (p.ej. "una" en un mensaje), también se transforma. Por eso el flujo
# es: correr -> `git diff` -> revisar. Los strings de mensajes se revisan a ojo.

BEGIN {
    n = 0
    while ((getline line < DICT) > 0) {
        if (line == "" || substr(line, 1, 1) == "#") continue
        # separar por tabulador
        t = index(line, "\t")
        if (t == 0) continue
        es = substr(line, 1, t - 1)
        en = substr(line, t + 1)
        map[es] = en
        n++
    }
    close(DICT)
}

# El `_` NO es parte del token: así un identificador snake_case como
# `funcion_inexistente` se parte en `funcion` + `_` + `inexistente` y cada
# subtoken se mapea por separado (-> `function_nonexistent`). Esto casa con el
# check `naming_policy`, que parte los identificadores por `_`.
function is_word_char(c) {
    return (c ~ /[A-Za-z0-9]/)
}

# Devuelve el índice (1-based) donde empieza el comentario `//` REAL, ignorando
# los `//` que aparecen dentro de un string ("...") o char ('...'). 0 si no hay.
# Respeta escapes `\"` / `\'`. Esto evita que un `://` de una URL en un string
# corte la línea y proteja código que en realidad debe transformarse.
function comment_start(s,    i, len, c, nx, in_str, in_chr, esc) {
    len = length(s)
    in_str = 0; in_chr = 0
    for (i = 1; i <= len; i++) {
        c = substr(s, i, 1)
        if (in_str) {
            if (c == "\\") { i++; continue }   # saltar el carácter escapado
            if (c == "\"") in_str = 0
        } else if (in_chr) {
            if (c == "\\") { i++; continue }
            if (c == "'") in_chr = 0
        } else {
            if (c == "\"") { in_str = 1 }
            else if (c == "'") { in_chr = 1 }
            else if (c == "/") {
                nx = substr(s, i + 1, 1)
                if (nx == "/") return i
            }
        }
    }
    return 0
}

# Transforma solo tokens completos de la cadena s (sin comentario).
function transform(s,    out, i, len, c, tok) {
    out = ""
    tok = ""
    len = length(s)
    for (i = 1; i <= len + 1; i++) {
        c = (i <= len) ? substr(s, i, 1) : ""
        if (c != "" && is_word_char(c)) {
            tok = tok c
        } else {
            # fin de token: resolver
            if (tok != "") {
                if (tok in map) {
                    out = out map[tok]
                } else {
                    out = out tok
                }
                tok = ""
            }
            out = out c
        }
    }
    return out
}

{
    # Separar código de comentario // (el primero REAL, fuera de strings).
    idx = comment_start($0)
    if (idx > 0) {
        code = substr($0, 1, idx - 1)
        comment = substr($0, idx)
    } else {
        code = $0
        comment = ""
    }
    print transform(code) comment
}
