# spanglish.py — repara el spanglish que dejó el rename ES→EN en los MENSAJES
# (string literals) y lleva los diagnósticos a inglés completo.
#
# Por qué existe: el rename word-a-word (tools/rename-identifiers.sh, retirado)
# sustituía tokens sueltos DENTRO de los strings → frases corruptas ("el valor
# must ir between comillas"), largos rotos y keywords comidas. La lección: la
# unidad segura no es la palabra, es el LITERAL COMPLETO, y la traducción la
# escribe una persona/modelo frase a frase — el script solo extrae y aplica.
#
# Diseño (tres garantías):
#   1. Solo toca STRING LITERALS: un tokenizador mínimo de Rust/raylang separa
#      código, comentarios (`//`, `/* */`), chars y strings (normales y raw
#      r"…"/r#"…"#). Nada fuera de un literal es alcanzable; los comentarios
#      en español ni se catalogan.
#   2. Reemplazo EXACTO frase a frase: el catálogo (JSONL) empareja cada literal
#      original (comillas incluidas) con su traducción completa. Cero
#      diccionario palabra-a-palabra. `apply` verifica el nº de ocurrencias
#      esperado por archivo y es idempotente.
#   3. Espejos y tests en tándem: `check` avisa si un texto cambiado en
#      src/checker.rs sigue vivo en selfhost/checker.ray (mensajes
#      byte-idénticos) o en un test que lo asevera → esos van en el MISMO lote.
#
# Flujo:
#   python3 tools/spanglish.py scan  > tools/spanglish-catalog.jsonl
#     · extrae los literales sospechosos (contienen español) con archivo/línea.
#     · se rellena el campo "en" de cada entrada (a mano o con el modelo);
#       "en": null = saltar (aún sin decidir), "en" igual al texto = conservar.
#   python3 tools/spanglish.py apply tools/spanglish-catalog.jsonl [--write]
#     · sin --write: dry-run, imprime el diff por archivo.
#     · con --write: aplica archivo por archivo y reporta ocurrencias.
#   python3 tools/spanglish.py check tools/spanglish-catalog.jsonl
#     · busca textos viejos que sigan vivos (espejos selfhost / tests / goldens).
#   python3 tools/spanglish.py audit [ruta.ray | subdir]
#     · SOLO .ray: cruza el tokenizador Python contra el LEXER REAL auto-alojado
#       (`selfhost/lex_dump.ray`) y marca las líneas donde discrepan (casos límite
#       del tokenizador, p. ej. una cadena dentro de `${ f("x") }`). Verificador,
#       no motor: para Rust no hay lexer CLI; requiere `cargo build` previo.
#
# Tras cada lote: `make test-one T=<suite afectada>` (los tests que aseveran
# mensajes van en el mismo lote que el mensaje).

import json
import os
import re
import subprocess
import sys

RAIZ = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# Directorios bajo escaneo. examples/ y book/ quedan fuera del *scan* (código
# didáctico), pero `check` sí los mira para detectar textos viejos citados.
DIRS_SCAN = ["src", "tests", "selfhost", "packages", "benchmarks"]
DIRS_CHECK = DIRS_SCAN + ["examples"]

# ---------- detección de español ----------

# Palabras función del español que delatan una frase (no aparecen en inglés).
FUNCION_ES = {
    "el", "la", "los", "las", "un", "una", "unos", "unas", "de", "del", "al",
    "en", "que", "qué", "para", "por", "con", "sin", "según", "tras", "entre",
    "se", "su", "sus", "es", "son", "está", "están", "hay", "ya", "no", "ni",
    "como", "cómo", "donde", "dónde", "cuando", "cuándo", "este", "esta",
    "ese", "esa", "aquí", "más", "aún", "solo", "sólo", "también", "pero",
    "debe", "deben", "puede", "pueden", "tiene", "tienen", "falta", "faltan",
    "esperaba", "esperada", "esperado", "encontró", "admite", "requiere",
    "devuelve", "devolver", "toma", "lleva", "usa", "vacía", "vacío",
    "inválida", "inválido", "válida", "válido", "desconocida", "desconocido",
    "definida", "definido", "declarada", "declarado", "cerrar", "cerrada",
    "abierta", "abierto", "línea", "columna", "cadena", "carácter", "número",
    "tipo", "tipos", "nombre", "archivo", "módulo", "paquete", "función",
    "llamada", "parámetro", "parámetros", "argumento", "argumentos", "valor",
    "valores", "clave", "duplicada", "duplicado", "fuera", "dentro", "rango",
    "mismo", "misma", "propio", "propia", "ningún", "ninguna", "cada", "otra",
    "otro", "primero", "primera", "último", "última", "sobrante", "demasiado",
    "demasiados", "demasiadas",
}

# Cualquier carácter propio del español dentro de un literal = español seguro.
RE_ES_CHARS = re.compile(r"[áéíóúñü¿¡]", re.IGNORECASE)
RE_PALABRA = re.compile(r"[a-záéíóúñü]+", re.IGNORECASE)


def parece_espanol(texto: str) -> bool:
    """El literal contiene una frase en español (o spanglish con resto español)."""
    if RE_ES_CHARS.search(texto):
        return True
    palabras = [p.lower() for p in RE_PALABRA.findall(texto)]
    return any(p in FUNCION_ES for p in palabras)


# ---------- tokenizador de literales ----------

def literales_rust(src: str):
    """Genera (linea, texto_con_comillas) por cada string literal de un .rs.

    Entiende `//`, `/* */` (anidados), char literals ('a', '\\n'), strings
    normales con escapes y raw strings r"…" / r#"…"# (con N almohadillas).
    Suficiente para el código del repo; no pretende ser un lexer completo.
    """
    i, n, linea = 0, len(src), 1
    while i < n:
        c = src[i]
        if c == "\n":
            linea += 1
            i += 1
        elif c == "/" and i + 1 < n and src[i + 1] == "/":
            while i < n and src[i] != "\n":
                i += 1
        elif c == "/" and i + 1 < n and src[i + 1] == "*":
            prof, i = 1, i + 2
            while i < n and prof > 0:
                if src.startswith("/*", i):
                    prof += 1
                    i += 2
                elif src.startswith("*/", i):
                    prof -= 1
                    i += 2
                else:
                    if src[i] == "\n":
                        linea += 1
                    i += 1
        elif c == "'":
            # char literal o lifetime; consumir conservadoramente 'x' / '\x'.
            if i + 2 < n and src[i + 1] == "\\":
                j = i + 2
                while j < n and src[j] != "'":
                    j += 1
                i = j + 1
            elif i + 2 < n and src[i + 2] == "'":
                i += 3
            else:
                i += 1  # lifetime ('a en genéricos): no es literal
        elif c == "r" and i + 1 < n and src[i + 1] in "#\"":
            # raw string r"…" o r#"…"# (b-strings crudos: br…)
            j = i + 1
            hashes = 0
            while j < n and src[j] == "#":
                hashes += 1
                j += 1
            if j < n and src[j] == '"':
                cierre = '"' + "#" * hashes
                k = src.find(cierre, j + 1)
                if k == -1:
                    k = n - len(cierre)
                fin = k + len(cierre)
                yield linea, src[i:fin]
                linea += src.count("\n", i, fin)
                i = fin
            else:
                i += 1
        elif c == '"':
            j = i + 1
            while j < n:
                if src[j] == "\\":
                    j += 2
                elif src[j] == '"':
                    break
                else:
                    j += 1
            fin = min(j + 1, n)
            yield linea, src[i:fin]
            linea += src.count("\n", i, fin)
            i = fin
        else:
            i += 1


def literales_ray(src: str):
    """Como literales_rust pero para .ray: comentarios `//` y strings "…"."""
    i, n, linea = 0, len(src), 1
    while i < n:
        c = src[i]
        if c == "\n":
            linea += 1
            i += 1
        elif c == "/" and i + 1 < n and src[i + 1] == "/":
            while i < n and src[i] != "\n":
                i += 1
        elif c == "'":
            # char literal de raylang: 'a' o '\n'
            if i + 2 < n and src[i + 1] == "\\":
                i += 4 if i + 3 < n and src[i + 3] == "'" else 1
            elif i + 2 < n and src[i + 2] == "'":
                i += 3
            else:
                i += 1
        elif c == '"':
            j = i + 1
            while j < n:
                if src[j] == "\\":
                    j += 2
                elif src[j] == '"':
                    break
                else:
                    j += 1
            fin = min(j + 1, n)
            yield linea, src[i:fin]
            linea += src.count("\n", i, fin)
            i = fin
        else:
            i += 1


def archivos(dirs):
    for d in dirs:
        for raiz, _, nombres in os.walk(os.path.join(RAIZ, d)):
            for nm in sorted(nombres):
                if nm.endswith((".rs", ".ray")):
                    yield os.path.relpath(os.path.join(raiz, nm), RAIZ)


def literales_de(ruta: str):
    src = open(os.path.join(RAIZ, ruta), encoding="utf-8").read()
    gen = literales_rust if ruta.endswith(".rs") else literales_ray
    return src, list(gen(src))


# ---------- subcomandos ----------

def cmd_scan():
    """Emite el catálogo JSONL de literales con español, agrupado por texto."""
    entradas = {}  # texto → {texto, en, sitios: [file:line], total}
    for ruta in archivos(DIRS_SCAN):
        _, lits = literales_de(ruta)
        for linea, texto in lits:
            if len(texto) < 8:  # literales minúsculos ("", ",", "ok") no son frases
                continue
            if not parece_espanol(texto):
                continue
            e = entradas.setdefault(texto, {"text": texto, "en": None, "sites": [], "count": 0})
            e["sites"].append(f"{ruta}:{linea}")
            e["count"] += 1
    for e in entradas.values():
        print(json.dumps(e, ensure_ascii=False))
    print(f"[scan] {len(entradas)} literales distintos con español", file=sys.stderr)


def cargar_catalogo(ruta):
    with open(ruta, encoding="utf-8") as f:
        return [json.loads(l) for l in f if l.strip()]


def cmd_apply(ruta_catalogo, write):
    """Aplica los reemplazos frase a frase, archivo por archivo."""
    catalogo = [e for e in cargar_catalogo(ruta_catalogo)
                if e.get("en") and e["en"] != e["text"]]
    if not catalogo:
        print("[apply] nada que aplicar (ningún 'en' rellenado)")
        return 0
    # agrupar por archivo para el reporte y el modo archivo-por-archivo
    por_archivo = {}
    for e in catalogo:
        for sitio in e["sites"]:
            f = sitio.rsplit(":", 1)[0]
            por_archivo.setdefault(f, []).append(e)
    fallos = 0
    for ruta in sorted(por_archivo):
        src = open(os.path.join(RAIZ, ruta), encoding="utf-8").read()
        nuevo = src
        cambios = []
        for e in {id(x): x for x in por_archivo[ruta]}.values():
            esperadas = sum(1 for s in e["sites"] if s.rsplit(":", 1)[0] == ruta)
            reales = nuevo.count(e["text"])
            if reales == 0 and e["en"] in nuevo:
                continue  # ya aplicado (idempotencia)
            if reales != esperadas:
                print(f"[apply] OJO {ruta}: {reales} ocurrencias de {e['text'][:50]!r}, "
                      f"el catálogo esperaba {esperadas} — saltado", file=sys.stderr)
                fallos += 1
                continue
            nuevo = nuevo.replace(e["text"], e["en"])
            cambios.append((e["text"], e["en"], esperadas))
        if not cambios:
            continue
        if write:
            open(os.path.join(RAIZ, ruta), "w", encoding="utf-8").write(nuevo)
            print(f"[apply] {ruta}: {sum(c for _, _, c in cambios)} reemplazos")
        else:
            print(f"--- {ruta} (dry-run)")
            for a, b, cnt in cambios:
                print(f"  {cnt}× {a}")
                print(f"    → {b}")
    return 1 if fallos else 0


def cmd_check(ruta_catalogo):
    """Busca textos VIEJOS del catálogo que sigan vivos en el repo.

    Caza espejos sin actualizar (selfhost/checker.ray byte-idéntico), tests que
    aseveran el mensaje y goldens/citas en examples. El texto se busca SIN las
    comillas (un test puede citarlo con otro escapado).
    """
    catalogo = [e for e in cargar_catalogo(ruta_catalogo)
                if e.get("en") and e["en"] != e["text"]]
    vivos = 0
    for ruta in archivos(DIRS_CHECK):
        src = open(os.path.join(RAIZ, ruta), encoding="utf-8").read()
        for e in catalogo:
            desnudo = e["text"].strip('"').lstrip("r#").strip('"#')
            if len(desnudo) >= 12 and desnudo in src:
                print(f"[check] VIVO en {ruta}: {desnudo[:70]!r}")
                vivos += 1
    print(f"[check] {vivos} textos viejos aún vivos", file=sys.stderr)
    return 1 if vivos else 0


# ---------- audit: el tokenizador Python vs el LEXER REAL (solo .ray) ----------

def _ray_binario():
    """Ruta al binario ray/raylang ya compilado (debug o release), o None."""
    for nombre in ("raylang", "ray"):
        for perfil in ("debug", "release"):
            p = os.path.join(RAIZ, "target", perfil, nombre)
            if os.path.exists(p):
                return p
    return None


def _strs_del_lexer(binario, ruta_rel):
    """Corre `lex_dump.ray` (lexer auto-alojado) sobre un .ray y devuelve
    ({línea: [contenido, …]}, error). `error` != None si el archivo no lexea
    (o el driver falla): en ese caso los tokens no son fiables y se reporta."""
    res = subprocess.run(
        [binario, os.path.join(RAIZ, "selfhost/lex_dump.ray"), os.path.join(RAIZ, ruta_rel)],
        capture_output=True, text=True, cwd=RAIZ,
    )
    por_linea = {}
    for linea in res.stdout.splitlines():
        if linea.startswith("lex error at "):
            return {}, linea
        m = re.match(r'Str\("(.*)"\)@(\d+):\d+$', linea)
        if m:
            por_linea.setdefault(int(m.group(2)), []).append(m.group(1))
    if res.returncode != 0 and not por_linea:
        ultimo = (res.stderr.strip().splitlines() or ["(sin stderr)"])[-1]
        return {}, ultimo
    return por_linea, None


def cmd_audit(objetivo=None):
    """Verifica el tokenizador Python de .ray contra el LEXER REAL auto-alojado.

    Para cada .ray, corre `lex_dump.ray` y compara, línea a línea, cuántos
    tokens de cadena ve el lexer real vs cuántos literales extrae `literales_ray`.
    Un desacuerdo delata un caso límite del tokenizador Python (p. ej. una cadena
    dentro de una interpolación `${ f("x") }`). Solo .ray: no hay lexer CLI para
    Rust. `objetivo` opcional = archivo o subdirectorio a auditar (por velocidad).
    """
    binario = _ray_binario()
    if binario is None:
        print("[audit] no encuentro el binario ray/raylang; compila con 'cargo build'",
              file=sys.stderr)
        return 2
    total = discrepantes = no_lexean = 0
    for ruta in archivos(DIRS_SCAN):
        if not ruta.endswith(".ray"):
            continue
        if objetivo and not (ruta == objetivo or ruta.startswith(objetivo.rstrip("/") + "/")):
            continue
        total += 1
        py = {}
        for ln, txt in literales_de(ruta)[1]:
            py.setdefault(ln, []).append(txt)
        lex, error = _strs_del_lexer(binario, ruta)
        if error:
            no_lexean += 1
            print(f"[audit] {ruta}: NO LEXEA con el lexer real → {error}")
            continue
        difs = [ln for ln in sorted(set(py) | set(lex))
                if len(py.get(ln, [])) != len(lex.get(ln, []))]
        if difs:
            discrepantes += 1
            print(f"[audit] {ruta}: {len(difs)} línea(s) en desacuerdo")
            for ln in difs:
                p, l = py.get(ln, []), lex.get(ln, [])
                print(f"    L{ln}: python={len(p)} lexer={len(l)}")
                for s in p:
                    print(f"        py : {s}")
                for s in l:
                    print(f'        lex: "{s}"')
    print(f"[audit] {total} .ray auditados; {discrepantes} con desacuerdos; "
          f"{no_lexean} no lexean (límite del lexer auto-alojado, informativo)",
          file=sys.stderr)
    # Solo gatea la DISCREPANCIA real (bug del tokenizador Python). El 'no lexea'
    # es un límite del lexer auto-alojado (p. ej. sin bitwise) → informativo.
    return 1 if discrepantes else 0


def main():
    if len(sys.argv) < 2 or sys.argv[1] not in ("scan", "apply", "check", "audit"):
        print(__doc__ or "uso: spanglish.py scan|apply|check|audit", file=sys.stderr)
        print("uso: spanglish.py scan | apply <catalogo> [--write] | check <catalogo> "
              "| audit [ruta.ray | subdir]", file=sys.stderr)
        return 2
    if sys.argv[1] == "scan":
        cmd_scan()
        return 0
    if sys.argv[1] == "audit":
        objetivo = next((a for a in sys.argv[2:] if not a.startswith("--")), None)
        return cmd_audit(objetivo)
    if len(sys.argv) < 3:
        print("falta la ruta del catálogo", file=sys.stderr)
        return 2
    if sys.argv[1] == "apply":
        return cmd_apply(sys.argv[2], "--write" in sys.argv)
    return cmd_check(sys.argv[2])


if __name__ == "__main__":
    sys.exit(main())
