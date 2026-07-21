//! Las features del LSP: hover, ir-a-definición, referencias+rename, completion,
//! inteligencia semántica en templates, formateo, outline y resaltado de ocurrencias
//! (movimiento puro; usar `git log --follow`).

use super::*;

// ── Hover (M10.2b) ───────────────────────────────────────────────────────────────────

/// El `result` de un `textDocument/hover`: la firma/tipo del identificador bajo el cursor y, si el
/// símbolo tiene **comentarios de documentación** (`///` encima de su declaración), también la
/// documentación —renderizada como Markdown, con la firma en un bloque de código—. `null` si no hay
/// identificador bajo el cursor.
pub(super) fn hover_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Null };
    if is_template_uri(&uri) {
        // M55: hover en un template — primero el semántico (vía el módulo generado: tipos REALES,
        // p. ej. `fila.precio: float`); si el template no genera o la posición no mapea, el textual
        // (params + vars de for → `nombre: tipo`).
        if let Some(src) = docs.get(&uri)
            && let Some((info, start, end)) = template_semantic_hover(&uri, src, line0, char0)
                .or_else(|| template_hover_at(src, line0, char0))
        {
            return obj(vec![
                ("contents", obj(vec![("kind", text("plaintext")), ("value", Json::Str(info))])),
                ("range", range(line0, start, end)),
            ]);
        }
        return Json::Null;
    }
    let Some(src) = docs.get(&uri) else { return Json::Null };
    // Documentación: se localiza la DECLARACIÓN del símbolo (cruza archivos, como ir-a-definición) y
    // se escanean los `///` que la preceden en su propio archivo. Reusa `raydoc::doc_lines_above`.
    let doc = doc_of_symbol(&uri, src, line0, char0, docs);
    if let Some((info, start, end)) = hover_at(Some(&uri), src, line0, char0) {
        let contents = match doc {
            Some(d) => obj(vec![
                ("kind", text("markdown")),
                ("value", Json::Str(format!("```raylang\n{info}\n```\n\n{d}"))),
            ]),
            None => obj(vec![("kind", text("plaintext")), ("value", Json::Str(info))]),
        };
        return obj(vec![("contents", contents), ("range", range(line0, start, end))]);
    }
    // Fallback: un **builtin** sin entrada en el índice semántico (tipo indeterminado por contexto:
    // `channel`, `map_new`, `send`/`recv`, `spawn`, …). Se muestra su firma (si tiene) + doc.
    if let Some((name, start, end)) = ident_range_under_cursor(src, line0, char0) {
        if crate::builtins::is_builtin(&name) {
            let signature = crate::builtins::signature(&name)
                .map(|(ps, r)| format!("{}({}) -> {}", name, ps.join(", "), r));
            let doc_bi = crate::builtins::doc(&name);
            let value = match (signature, doc_bi) {
                (Some(f), Some(d)) => format!("```raylang\n{f}\n```\n\n{d}"),
                (Some(f), None) => format!("```raylang\n{f}\n```"),
                (None, Some(d)) => d.to_string(),
                (None, None) => return Json::Null,
            };
            return obj(vec![
                ("contents", obj(vec![("kind", text("markdown")), ("value", Json::Str(value))])),
                ("range", range(line0, start, end)),
            ]);
        }
        // Tipos incorporados / del prelude (Channel, Task, Map, Option, Result): descripción breve.
        if let Some(d) = doc_builtin_type(&name) {
            return obj(vec![
                ("contents", obj(vec![("kind", text("markdown")), ("value", Json::Str(d.to_string()))])),
                ("range", range(line0, start, end)),
            ]);
        }
    }
    Json::Null
}

/// Descripción breve (Markdown) de un tipo genérico incorporado o del prelude, para el hover.
pub(super) fn doc_builtin_type(name: &str) -> Option<&'static str> {
    Some(match name {
        "Channel" => "`Channel<T>` — a typed channel for communicating between fibers (CSP). Created with `channel()` / `channel(n)`; used with `send`, `recv`, `close`, `select`.",
        "Task" => "`Task<T>` — the in-progress result of a `spawn(f)`. `join(t)` blocks until the task finishes and returns its value.",
        "Map" => "`Map<K, V>` — a dictionary from hashable keys to values. `Map.new()`, `insert`, `get`, `remove`, `keys`, `values`, `contains_key`, `len`.",
        "Option" => "`Option<T>` — an optional value: `Some(T)` or `None`. raylang has no `null`.",
        "Result" => "`Result<T, E>` — the result of a fallible operation: `Ok(T)` or `Err(E)`. Propagated with `?`.",
        _ => return None,
    })
}

/// El identificador bajo el cursor y su rango de columnas `[ini, fin)` (0-basadas). Como
/// `ident_under_cursor` pero devolviendo también el rango, para el hover de builtins.
pub(super) fn ident_range_under_cursor(src: &str, line0: usize, char0: usize) -> Option<(String, usize, usize)> {
    let line: Vec<char> = src.lines().nth(line0)?.chars().collect();
    if char0 >= line.len() || !is_ident_char(line[char0]) {
        return None;
    }
    let mut start = char0;
    while start > 0 && is_ident_char(line[start - 1]) { start -= 1; }
    let mut end = char0;
    while end < line.len() && is_ident_char(line[end]) { end += 1; }
    Some((line[start..end].iter().collect(), start, end))
}

/// Los comentarios de documentación (`///`) del símbolo bajo el cursor, si los tiene. Localiza su
/// declaración con `definition_at` (cruza archivos), abre la fuente de ESE archivo (el buffer si es
/// el mismo, o el disco si es otro módulo) y reúne los `///` contiguos encima de la línea de la
/// declaración. `None` si el símbolo no tiene declaración conocida (método, builtin) o no lleva docs.
pub(super) fn doc_of_symbol(uri: &str, src: &str, line0: usize, char0: usize, docs: &HashMap<String, String>) -> Option<String> {
    match definition_at(uri, src, line0, char0) {
        Some((target_uri, def_line0, _, _)) => {
            // Fuente del archivo donde vive la declaración: el buffer si es el mismo doc, el disco, o
            // —para un módulo EMBEBIDO de la std (`std/*`, sin archivo real)— su `source` del programa
            // cargado. Así el hover de `math.sqrt`/`math.PI` muestra también sus `///` (M49.1).
            let source = if target_uri == uri {
                src.to_string()
            } else {
                docs.get(&target_uri).cloned()
                    .or_else(|| uri_to_path(&target_uri).and_then(|p| std::fs::read_to_string(p).ok()))
                    .or_else(|| loaded_module_source(uri, src, &target_uri))?
            };
            let lines: Vec<&str> = source.lines().collect();
            if let Some(ls) = crate::raydoc::doc_lines_above(&lines, def_line0 + 1) {
                return Some(ls.join("\n"));
            }
            // La declaración resolvió FUERA del archivo (línea inexistente): es un símbolo del
            // prelude inyectado, cuya posición vive en su propia fuente → sus `///` se buscan ahí.
            if def_line0 >= lines.len() {
                let name = ident_under_cursor(src, line0, char0)?;
                return doc_in_prelude(&name);
            }
            None
        }
        // Sin declaración conocida: un builtin (sus docs son metadatos en la tabla Rust) o un
        // símbolo del prelude sin entrada en `defs`.
        None => {
            let name = ident_under_cursor(src, line0, char0)?;
            crate::builtins::doc(&name).map(|s| s.to_string())
                .or_else(|| doc_in_prelude(&name))
        }
    }
}

/// La `source` de un módulo por su URI de destino, tomada del programa **cargado**. Sirve para los
/// módulos **embebidos** de la std (`std/*`): su declaración vive en una fuente sin archivo en disco
/// (`LoadedModule.source`), así que el hover/def no puede leerla del sistema de archivos. `None` si no
/// carga o no hay un módulo con ese path.
pub(super) fn loaded_module_source(entry_uri: &str, entry_src: &str, target_uri: &str) -> Option<String> {
    let entry_path = uri_to_path(entry_uri)?;
    let target_path = uri_to_path(target_uri)?;
    let loaded = load(&entry_path, entry_src).ok()?;
    loaded.modules.into_iter().find(|m| m.path == target_path).map(|m| m.source)
}

/// El identificador bajo el cursor `(line0, char0)` (0-basados), expandiendo a izquierda y derecha
/// sobre caracteres de identificador. `None` si el cursor no está sobre uno.
pub(super) fn ident_under_cursor(src: &str, line0: usize, char0: usize) -> Option<String> {
    let line: Vec<char> = src.lines().nth(line0)?.chars().collect();
    if char0 >= line.len() || !is_ident_char(line[char0]) {
        return None;
    }
    let mut start = char0;
    while start > 0 && is_ident_char(line[start - 1]) {
        start -= 1;
    }
    let mut end = char0;
    while end < line.len() && is_ident_char(line[end]) {
        end += 1;
    }
    Some(line[start..end].iter().collect())
}

/// Los `///` de un símbolo del **prelude** (funciones, tipos y traits inyectados: `map`, `sort`,
/// `Option`…), buscados por nombre en su propia fuente (`prelude::SOURCE`): la posición de su
/// declaración no vive en el archivo abierto, así que no vale `doc_lines_above` sobre el buffer.
pub(super) fn doc_in_prelude(name: &str) -> Option<String> {
    let lines: Vec<&str> = crate::prelude::SOURCE.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        let l = l.trim_start();
        // Declaraciones de nivel superior y métodos de trait/impl: `fn name(`/`fn name<`,
        // `enum/struct/trait Nombre`.
        let is_decl = ["fn ", "enum ", "struct ", "trait "].iter().any(|kw| {
            l.strip_prefix(kw).is_some_and(|rest| {
                rest.starts_with(name)
                    && !rest[name.len()..].chars().next().is_some_and(is_ident_char)
            })
        });
        if is_decl && let Some(ls) = crate::raydoc::doc_lines_above(&lines, i + 1) {
            return Some(ls.join("\n"));
        }
    }
    None
}

/// El índice semántico para las consultas (hover/def/refs/rename). Si el documento es un archivo,
/// se construye sobre el **programa fusionado** del loader (imports resueltos desde disco): así en
/// un proyecto multi-archivo los símbolos —locales y de otros módulos— se resuelven, en vez de que
/// el checker falle por el `import` y no se recoja nada. El archivo de entrada queda en delta 0, así
/// que sus posiciones coinciden con las del buffer. Si no es un archivo o el loader falla, se
/// construye sobre el buffer aislado (comportamiento previo). Es la misma idea que en los diagnósticos.
pub(super) fn index_for(uri: Option<&str>, src: &str) -> Option<checker::SemanticIndex> {
    if let Some(path) = uri.and_then(uri_to_path)
        && let Ok(loaded) = load(&path, src)
    {
        let mut program = loaded.program;
        return Some(checker::semantic_index(&mut program));
    }
    let tokens = lexer::lex(src).ok()?;
    let mut program = parser::parse(tokens).ok()?;
    Some(checker::semantic_index(&mut program))
}

/// Busca el identificador en `(line0, char0)` (0-basado) y devuelve `(texto, col_ini, col_fin)`
/// (columnas 0-basadas en esa línea). El índice es módulo-aware (ver `index_for`).
pub(super) fn hover_at(uri: Option<&str>, src: &str, line0: usize, char0: usize) -> Option<(String, usize, usize)> {
    let idx = index_for(uri, src)?;
    // El índice usa posiciones 1-basadas (como las fases); el cursor llega 0-basado. La consulta
    // cae siempre en la banda de la entrada (delta 0), así que coincide con las posiciones locales.
    let (qline, qcol) = (line0 + 1, char0 + 1);
    // Entre los que solapan la posición, el **más específico** (menor rango): un nombre namespacado
    // (`geo::duplicar`) registra un `len` mayor que el token de la fuente y solaparía el siguiente.
    let e = idx.hovers.iter()
        .filter(|h| h.line == qline && qcol >= h.col && qcol < h.col + h.len)
        .min_by_key(|h| h.len)?;
    let start = e.col - 1;
    // Recorta el fin al identificador real de la fuente (el `len` namespacado puede excederlo).
    let end = start + e.len.min(token_len(src, line0, start));
    Some((facade_name(&e.text, &imports_of(src)), start, end))
}

/// Presenta los nombres globales para el usuario: convierte cada ruta namespacada del loader
/// (`std::math::sqrt`, `geo::formas::circulo::Circulo`) a la **forma que el usuario escribe**
/// (`math.sqrt`, `geo.Circulo`) — el `leaf` con el que importó el módulo + el nombre —, usando el
/// separador `.` del lenguaje en vez del interno `::`. `imports` son los `(leaf, ns_prefix)` del
/// archivo (`import std/math;` → `("math", "std::math")`): se elige el import cuyo `ns_prefix` es
/// prefijo de la ruta (el más largo), de modo que un módulo directo muestra su leaf (`math`) y una
/// **cápsula** su raíz (`geo`, cuyo `ns_prefix` también es prefijo) — respetando la encapsulación.
/// Sin `imports` (o sin match) cae a `primer.último` (comportamiento previo). Un nombre sin `::` se
/// deja igual.
pub(super) fn facade_name(serialized: &str, imports: &[(String, String)]) -> String {
    let chars: Vec<char> = serialized.chars().collect();
    let seg = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if !seg(chars[i]) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // Lee una ruta `seg (:: seg)*` completa.
        let mut segs: Vec<String> = Vec::new();
        loop {
            let s = i;
            while i < chars.len() && seg(chars[i]) {
                i += 1;
            }
            segs.push(chars[s..i].iter().collect());
            if i + 1 < chars.len() && chars[i] == ':' && chars[i + 1] == ':' {
                i += 2;
            } else {
                break;
            }
        }
        if segs.len() == 1 {
            out.push_str(&segs[0]);
            continue;
        }
        let full = segs.join("::");
        let Some(name) = segs.last() else { continue }; // segs tiene ≥2 aquí; evita el unwrap a pelo
        // Import cuyo ns_prefix sea prefijo de la ruta (el más largo gana): su leaf es cómo se accede.
        let leaf = imports.iter()
            .filter(|(_, ns)| full == *ns || full.starts_with(&format!("{ns}::")))
            .max_by_key(|(_, ns)| ns.len())
            .map(|(leaf, _)| leaf.as_str())
            .unwrap_or(&segs[0]); // fallback: el primer segmento (cápsula/desconocido)
        out.push_str(leaf);
        out.push('.');
        out.push_str(name);
    }
    out
}

/// Los `(leaf, ns_prefix)` de los `import a/b/c [as x];` del archivo. Reusa lex + `parse_all` (con
/// recuperación de errores): los `import` van al principio, así que se recogen aunque el resto del
/// documento no parsee a medio escribir (`math.`). Si ni lexa, devuelve vacío.
pub(super) fn imports_of(src: &str) -> Vec<(String, String)> {
    let Ok(tokens) = lexer::lex(src) else { return Vec::new() };
    let (program, _errs) = parser::parse_all(tokens);
    program.imports.iter()
        .map(|imp| (imp.leaf().to_string(), imp.module.replace('/', "::")))
        .collect()
}

/// Longitud del identificador que empieza en `(line0, col0)` (0-basados) en la fuente: cuántos
/// caracteres de identificador consecutivos hay. Sirve para no subrayar de más cuando el índice
/// trae un nombre namespacado más largo que el token escrito.
pub(super) fn token_len(src: &str, line0: usize, col0: usize) -> usize {
    let Some(line) = src.lines().nth(line0) else { return 0 };
    line.chars().skip(col0).take_while(|&c| is_ident_char(c)).count()
}

/// Un `range` LSP en una sola línea (0-basada), de la columna `start` a `end` (0-basadas).
pub(super) fn range(line0: usize, start: usize, end: usize) -> Json {
    let pos = |ch: usize| obj(vec![("line", num(line0 as i64)), ("character", num(ch as i64))]);
    obj(vec![("start", pos(start)), ("end", pos(end))])
}

/// Un `range` LSP que cruza líneas (0-basadas): de `(l1, c1)` a `(l2, c2)`.
pub(super) fn range_multiline(l1: usize, c1: usize, l2: usize, c2: usize) -> Json {
    let pos = |l: usize, ch: usize| obj(vec![("line", num(l as i64)), ("character", num(ch as i64))]);
    obj(vec![("start", pos(l1, c1)), ("end", pos(l2, c2))])
}

// ── Ir-a-definición (M10.2b) ─────────────────────────────────────────────────────────

/// El `result` de un `textDocument/definition`: una `Location` (uri + rango de la declaración), o
/// `null` si el identificador no tiene declaración conocida (método, builtin, tipo). La declaración
/// puede estar en **otro archivo** (M10.2h): se resuelve el import y se devuelve el URI del módulo
/// donde vive, con su línea local.
pub(super) fn definition_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Null };
    if is_template_uri(&uri) {
        // M55: la posición se traduce al módulo generado y se resuelve ahí; una declaración en el
        // propio generado (un param) vuelve traducida al template (la línea del `{% params %}`).
        return docs.get(&uri)
            .and_then(|src| template_definition(&uri, src, line0, char0))
            .unwrap_or(Json::Null);
    }
    let Some(src) = docs.get(&uri) else { return Json::Null };
    let Some((target_uri, def_line0, def_col0, len)) = definition_at(&uri, src, line0, char0) else {
        return Json::Null;
    };
    obj(vec![
        ("uri", Json::Str(target_uri)),
        ("range", range(def_line0, def_col0, def_col0 + len)),
    ])
}

/// Busca el identificador en `(line0, char0)` (0-basado) y devuelve `(uri_destino, línea0, col0,
/// largo)` de su declaración. Módulo-aware: si el documento es un archivo, se carga con el loader y
/// la declaración se mapea a su **módulo** (archivo + línea local) —así se navega cruzando archivos—;
/// si no, se resuelve solo dentro del buffer.
pub(super) fn definition_at(uri: &str, src: &str, line0: usize, char0: usize) -> Option<(String, usize, usize, usize)> {
    let (qline, qcol) = (line0 + 1, char0 + 1);
    // Camino módulo-aware: el loader da el programa fusionado + las bandas de cada módulo (con su
    // ruta). La declaración se localiza por su banda → archivo y línea local correctos.
    if let Some(path) = uri_to_path(uri)
        && let Ok(loaded) = load(&path, src)
    {
        let mut program = loaded.program;
        let idx = checker::semantic_index(&mut program);
        let d = idx.defs.iter()
            .filter(|d| d.line == qline && qcol >= d.col && qcol < d.col + d.len)
            .min_by_key(|d| d.len)?;
        // ¿En qué módulo (banda) cae la declaración? El de mayor `start_line` que no la supere.
        let m = loaded.modules.iter().rev().find(|m| m.start_line <= d.def_line)?;
        let local = d.def_line - m.start_line; // 0-basada
        let col0 = d.def_col - 1;
        // Recorta el largo al token real del ARCHIVO DESTINO (el `len` puede venir namespacado).
        let len = d.len.min(token_len(&m.source, local, col0)).max(1);
        let target_uri = format!("file://{}", m.path.display());
        return Some((target_uri, local, col0, len));
    }
    // Fallback un solo archivo (buffer sin guardar): la declaración vive en el propio documento.
    let idx = index_for(Some(uri), src)?;
    let d = idx.defs.iter()
        .filter(|d| d.line == qline && qcol >= d.col && qcol < d.col + d.len)
        .min_by_key(|d| d.len)?;
    let (dl0, dc0) = (d.def_line - 1, d.def_col - 1);
    let len = d.len.min(token_len(src, dl0, dc0)).max(1);
    Some((uri.to_string(), dl0, dc0, len))
}

// ── Find-references y rename (cluster 4) ─────────────────────────────────────────────
//
// Ambos se construyen sobre el índice semántico (`defs`: uso → posición de su declaración) más
// la **fuente**. Una declaración se identifica por su *clave* `(def_line, def_col)`; todos los
// usos con la misma clave son el mismo símbolo (los ámbitos ya están resueltos: dos `x` distintos
// tienen claves distintas). El rango del *nombre* de la declaración se localiza escaneando la
// línea de la declaración (la del `let`/`fn`, o ya el nombre en un parámetro). Es un cliente
// externo: cero cambios en el núcleo (igual que el resto del LSP).

pub(super) fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Un rango en una línea: `(line0, col0, len)`, todo 0-basado.
type Span = (usize, usize, usize);

/// El identificador escrito en `(line0, col0)` (0-basados) de `lines`, o `None` si ahí no empieza uno.
pub(super) fn token_at(lines: &[&str], line0: usize, col0: usize) -> Option<String> {
    let chars: Vec<char> = lines.get(line0)?.chars().collect();
    if col0 >= chars.len() {
        return None;
    }
    let end = col0 + chars[col0..].iter().take_while(|&&c| is_ident_char(c)).count();
    (end > col0).then(|| chars[col0..end].iter().collect())
}

/// El texto (nombre) de un uso `d`, leído de `lines` en la posición de `d` (1-basado en `d.line`).
/// Se lee el **identificador** que empieza ahí (no `d.len`, que en un uso namespacado —`geo::f`—
/// excede el token escrito en la fuente).
pub(super) fn use_name(lines: &[&str], d: &checker::DefEntry) -> Option<String> {
    token_at(lines, d.line - 1, d.col - 1)
}

/// Rango del **nombre** `name` como palabra entera desde `(def_line, def_col)` (1-basados) en
/// `lines`: un `let`/`fn` apunta al keyword → escanea hasta el nombre; un parámetro ya está en él.
pub(super) fn decl_name_range(lines: &[&str], def_line: usize, def_col: usize, name: &str) -> Option<Span> {
    let chars: Vec<char> = lines.get(def_line - 1)?.chars().collect();
    let nm: Vec<char> = name.chars().collect();
    let mut i = def_col.saturating_sub(1);
    while i + nm.len() <= chars.len() {
        let before = i == 0 || !is_ident_char(chars[i - 1]);
        let after = i + nm.len() == chars.len() || !is_ident_char(chars[i + nm.len()]);
        if before && after && chars[i..i + nm.len()] == nm[..] {
            return Some((def_line - 1, i, nm.len()));
        }
        i += 1;
    }
    None
}

/// El símbolo bajo el cursor: su nombre, el rango de su **declaración** (si se localiza) y los
/// rangos de todos sus **usos**. `None` si no hay símbolo.
pub(super) fn symbol_occurrences(
    uri: Option<&str>,
    src: &str,
    line0: usize,
    char0: usize,
) -> Option<(String, Option<Span>, Vec<Span>, bool)> {
    let idx = index_for(uri, src)?;
    let entry_lines = src.lines().count();
    let lines: Vec<&str> = src.lines().collect();
    let (qline, qcol) = (line0 + 1, char0 + 1);

    // Clave de la declaración objetivo: (a) el cursor está sobre un uso, o (b) sobre el nombre de
    // una declaración (que tenga al menos un uso, del que se toma el nombre).
    let mut target: Option<(usize, usize, String)> = idx.defs.iter()
        .filter(|d| d.line == qline && qcol >= d.col && qcol < d.col + d.len)
        .min_by_key(|d| d.len)
        .and_then(|d| use_name(&lines, d).map(|n| (d.def_line, d.def_col, n)));
    if target.is_none() {
        for d in &idx.defs {
            let Some(name) = use_name(&lines, d) else { continue };
            let on_decl = decl_name_range(&lines, d.def_line, d.def_col, &name)
                .is_some_and(|(dl, dc, len)| dl + 1 == qline && qcol > dc && qcol - 1 < dc + len);
            if on_decl {
                target = Some((d.def_line, d.def_col, name));
                break;
            }
        }
    }
    let (tdl, tdc, name) = target?;

    let decl = decl_name_range(&lines, tdl, tdc, &name);
    // Todos los usos con la clave de esta declaración. Se filtran a la **banda de la entrada** (este
    // archivo): los usos en otros módulos tienen líneas fuera del buffer y no deben devolverse aquí.
    let all_uses: Vec<&checker::DefEntry> =
        idx.defs.iter().filter(|d| d.def_line == tdl && d.def_col == tdc).collect();
    let mut uses: Vec<Span> = all_uses
        .iter()
        .filter(|d| d.line <= entry_lines)
        .map(|d| (d.line - 1, d.col - 1, d.len))
        .collect();
    uses.sort();
    uses.dedup();
    // ¿El símbolo vive ENTERO en este archivo? (declaración local + ningún uso en otro módulo). Solo
    // entonces es seguro renombrarlo desde aquí; si cruza módulos, un rename local lo dejaría a medias.
    let is_local = tdl <= entry_lines && all_uses.iter().all(|d| d.line <= entry_lines);
    Some((name, decl, uses, is_local))
}

/// El `result` de `textDocument/references`: una lista de `Location` (uso, y la declaración si el
/// cliente pide `includeDeclaration`). **Cruza archivos** (M10.2h): un símbolo usado en varios
/// módulos devuelve sus usos en cada archivo. Lista vacía si no hay símbolo.
pub(super) fn references_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Arr(vec![]) };
    let Some(src) = docs.get(&uri) else { return Json::Arr(vec![]) };
    let include_decl = msg.get("params").and_then(|p| p.get("context"))
        .and_then(|c| c.get("includeDeclaration"))
        .map(|b| matches!(b, Json::Bool(true)))
        .unwrap_or(true);
    if is_template_uri(&uri) {
        // M55: las apariciones del símbolo (param / var de for) DENTRO del template.
        let occs = template_occurrences(src);
        let Some(cur) = template_occurrence_at(&occs, line0, char0).cloned() else {
            return Json::Arr(vec![]);
        };
        let list: Vec<Json> = occs.iter()
            .filter(|(_, _, _, k, is_decl)| *k == cur.3 && (include_decl || !*is_decl))
            .map(|(l, c, len, _, _)| obj(vec![
                ("uri", Json::Str(uri.clone())),
                ("range", range(*l, *c, *c + *len)),
            ]))
            .collect();
        return Json::Arr(list);
    }
    // Camino cross-archivo (si es un archivo); si no (buffer sin guardar), un solo archivo.
    let locs: Vec<(String, Span)> = references_cross(&uri, src, line0, char0, include_decl)
        .unwrap_or_else(|| {
            let mut v = Vec::new();
            if let Some((_, decl, uses, _)) = symbol_occurrences(Some(&uri), src, line0, char0) {
                if let Some(s) = decl.filter(|_| include_decl) {
                    v.push((uri.clone(), s));
                }
                v.extend(uses.into_iter().map(|s| (uri.clone(), s)));
            }
            v
        });
    let json = locs.into_iter()
        .map(|(u, (l, c, len))| obj(vec![("uri", Json::Str(u)), ("range", range(l, c, c + len))]))
        .collect();
    Json::Arr(json)
}

/// Todas las apariciones de un símbolo (declaración + usos) **cruzando módulos** (M10.2h): corre el
/// loader, halla la clave de la declaración objetivo (cursor sobre un uso, o sobre el nombre de una
/// declaración de este archivo) y mapea cada aparición a su módulo → `(uri, rango 0-basado)`. El
/// largo se recorta al token real del archivo destino (los usos namespacados registran más). `None`
/// si el uri no es un archivo o el loader falla (→ el llamador cae a un solo archivo).
pub(super) fn references_cross(uri: &str, src: &str, line0: usize, char0: usize, include_decl: bool) -> Option<Vec<(String, Span)>> {
    let path = uri_to_path(uri)?;
    let loaded = load(&path, src).ok()?;
    let mut program = loaded.program;
    let idx = checker::semantic_index(&mut program);
    let entry_lines: Vec<&str> = src.lines().collect();
    let (qline, qcol) = (line0 + 1, char0 + 1);

    // Objetivo: clave `(tdl, tdc)` + nombre. Cursor sobre un uso, o sobre el nombre de una
    // declaración de la ENTRADA (el cursor siempre está en el archivo abierto).
    let mut target: Option<(usize, usize, String)> = idx.defs.iter()
        .filter(|d| d.line == qline && qcol >= d.col && qcol < d.col + d.len)
        .min_by_key(|d| d.len)
        .and_then(|d| use_name(&entry_lines, d).map(|n| (d.def_line, d.def_col, n)));
    if target.is_none() {
        for d in &idx.defs {
            if d.def_line > entry_lines.len() {
                continue; // la declaración no está en este archivo → el cursor no puede estar en ella
            }
            let Some(name) = use_name(&entry_lines, d) else { continue };
            let on_decl = decl_name_range(&entry_lines, d.def_line, d.def_col, &name)
                .is_some_and(|(dl, dc, len)| dl + 1 == qline && qcol > dc && qcol - 1 < dc + len);
            if on_decl {
                target = Some((d.def_line, d.def_col, name));
                break;
            }
        }
    }
    let (tdl, tdc, name) = target?;

    // Localiza el módulo (banda) de una línea global y devuelve su URI + línea local.
    let module_of = |gl: usize| loaded.modules.iter().rev().find(|m| m.start_line <= gl);

    let mut locs: Vec<(String, Span)> = Vec::new();
    // La declaración: apunta al **nombre** en su archivo (escanea la fuente del módulo destino).
    if include_decl
        && let Some(m) = module_of(tdl)
    {
        let m_lines: Vec<&str> = m.source.lines().collect();
        let local_decl = tdl - m.start_line + 1; // 1-basada en el módulo
        if let Some(span) = decl_name_range(&m_lines, local_decl, tdc, &name) {
            locs.push((format!("file://{}", m.path.display()), span));
        }
    }
    // Los usos: cada uno mapeado a su módulo, con el largo recortado al token real.
    for d in idx.defs.iter().filter(|d| d.def_line == tdl && d.def_col == tdc) {
        if let Some(m) = module_of(d.line) {
            let local0 = d.line - m.start_line;
            let col0 = d.col - 1;
            let len = d.len.min(token_len(&m.source, local0, col0)).max(1);
            locs.push((format!("file://{}", m.path.display()), (local0, col0, len)));
        }
    }
    locs.sort();
    locs.dedup();
    Some(locs)
}

/// El `result` de `textDocument/rename`: un `WorkspaceEdit` que sustituye el símbolo (declaración
/// + usos) por el nuevo nombre. `null` si no hay símbolo o falta `newName`.
pub(super) fn rename_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Null };
    let Some(src) = docs.get(&uri) else { return Json::Null };
    let Some(new_name) = msg.get("params").and_then(|p| p.get("newName")).and_then(|n| n.as_str()) else {
        return Json::Null;
    };
    if is_template_uri(&uri) {
        // M55: renombrar un param o una var de for DENTRO del template (declaración + usos). Es
        // seguro hacia afuera: los llamadores del `render_<x>` generado pasan args POSICIONALES.
        let valid = new_name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && new_name.chars().all(is_ident_char);
        if !valid {
            return Json::Null;
        }
        let occs = template_occurrences(src);
        let Some(cur) = template_occurrence_at(&occs, line0, char0).cloned() else {
            return Json::Null;
        };
        let edits: Vec<Json> = occs.iter()
            .filter(|(_, _, _, k, _)| *k == cur.3)
            .map(|(l, c, len, _, _)| obj(vec![
                ("range", range(*l, *c, *c + *len)),
                ("newText", Json::Str(new_name.to_string())),
            ]))
            .collect();
        return obj(vec![("changes", obj(vec![(uri.as_str(), Json::Arr(edits))]))]);
    }
    // Camino cross-archivo (si es un archivo). Agrupa las ediciones por URI. Un rename que no puede
    // hacerse de forma **completa y segura** (alias, refs calificadas, símbolo no resoluble)
    // devuelve `None` → aquí `null` (el editor avisa de que no se puede renombrar).
    let positions = if uri_to_path(&uri).is_some() {
        match rename_cross(&uri, src, line0, char0) {
            Some(p) => p,
            None => return Json::Null,
        }
    } else {
        // Buffer sin guardar: solo dentro de este archivo (con el gate `es_local`).
        let Some((_, decl, uses, is_local)) = symbol_occurrences(Some(&uri), src, line0, char0) else {
            return Json::Null;
        };
        if !is_local {
            return Json::Null;
        }
        decl.into_iter().chain(uses).map(|s| (uri.clone(), s)).collect()
    };
    if positions.is_empty() {
        return Json::Null;
    }
    // Agrupar las ediciones por archivo (`WorkspaceEdit.changes`).
    let mut by_uri: HashMap<String, Vec<Json>> = HashMap::new();
    for (u, (l, c, len)) in positions {
        by_uri.entry(u).or_default().push(obj(vec![
            ("range", range(l, c, c + len)),
            ("newText", Json::Str(new_name.to_string())),
        ]));
    }
    let changes = obj(by_uri.into_iter().map(|(u, edits)| (u, Json::Arr(edits))).collect::<Vec<_>>()
        .iter().map(|(u, e)| (u.as_str(), e.clone())).collect());
    obj(vec![("changes", changes)])
}

/// Todas las posiciones a sustituir para renombrar el símbolo bajo el cursor, **cruzando archivos**
/// (M10.2h): declaración + usos (de todos los módulos) + los especificadores de `from`-import que lo
/// traen. Devuelve `None` (→ rename rechazado) si no hay símbolo o si el rename no sería **completo
/// y seguro**: se exige que TODA posición contenga exactamente el nombre (un uso por **alias** o una
/// referencia **calificada** tienen otro texto → se rechaza, para no corromper el código).
pub(super) fn rename_cross(uri: &str, src: &str, line0: usize, char0: usize) -> Option<Vec<(String, Span)>> {
    let path = uri_to_path(uri)?;
    let loaded = load(&path, src).ok()?;
    let mut program = loaded.program.clone();
    let idx = checker::semantic_index(&mut program);
    let entry_lines: Vec<&str> = src.lines().collect();
    let (qline, qcol) = (line0 + 1, char0 + 1);

    // Objetivo: (tdl, tdc, nombre) — cursor sobre un uso o sobre el nombre de una decl de la entrada.
    let mut target: Option<(usize, usize, String)> = idx.defs.iter()
        .filter(|d| d.line == qline && qcol >= d.col && qcol < d.col + d.len)
        .min_by_key(|d| d.len)
        .and_then(|d| use_name(&entry_lines, d).map(|n| (d.def_line, d.def_col, n)));
    if target.is_none() {
        for d in &idx.defs {
            if d.def_line > entry_lines.len() {
                continue;
            }
            let Some(name) = use_name(&entry_lines, d) else { continue };
            let on_decl = decl_name_range(&entry_lines, d.def_line, d.def_col, &name)
                .is_some_and(|(dl, dc, len)| dl + 1 == qline && qcol > dc && qcol - 1 < dc + len);
            if on_decl {
                target = Some((d.def_line, d.def_col, name));
                break;
            }
        }
    }
    let (tdl, tdc, name) = target?;

    // El global al que apunta la declaración (para casar los `from`-import): el nombre del ítem cuya
    // definición está en (tdl, tdc). Si no es un ítem top-level (variable local), el propio nombre.
    let global = def_global_name(&loaded.program, tdl, tdc).unwrap_or_else(|| name.clone());

    let module_of = |gl: usize| loaded.modules.iter().rev().find(|m| m.start_line <= gl);
    let uri_of = |m: &loader::LoadedModule| format!("file://{}", m.path.display());

    // Recoge las posiciones (uri, span), verificando que el TEXTO de cada una sea exactamente
    // `name`; si alguna no lo es (alias, ref calificada), `seguro` pasa a false y el rename se rechaza.
    let mut positions: Vec<(String, Span)> = Vec::new();
    let mut safe = true;

    // (1) La declaración (su nombre, en el archivo donde vive).
    if let Some(m) = module_of(tdl) {
        let m_lines: Vec<&str> = m.source.lines().collect();
        let local = tdl - m.start_line + 1;
        if let Some((dl0, dc0, _)) = decl_name_range(&m_lines, local, tdc, &name) {
            push_rename(&mut positions, &mut safe, &name, uri_of(m), dl0, dc0, &m_lines);
        }
    }
    // (2) Los usos (de todos los módulos).
    for d in idx.defs.iter().filter(|d| d.def_line == tdl && d.def_col == tdc) {
        if let Some(m) = module_of(d.line) {
            let m_lines: Vec<&str> = m.source.lines().collect();
            push_rename(&mut positions, &mut safe, &name, uri_of(m), d.line - m.start_line, d.col - 1, &m_lines);
        }
    }
    // (3) Los especificadores de `from M import name` que traen este global. Un import con `as` va
    // por alias (los usos son el alias, ya rechazados) → inseguro. Se lee de la fuente del módulo ya
    // cargado (el archivo de entrada usa el buffer, no el disco).
    for s in loaded.from_import_sites.iter().filter(|s| s.global == global) {
        if s.aliased {
            safe = false;
            continue;
        }
        let Some(m) = loaded.modules.iter().find(|m| m.path == s.path) else {
            safe = false;
            continue;
        };
        let m_lines: Vec<&str> = m.source.lines().collect();
        push_rename(&mut positions, &mut safe, &name, uri_of(m), s.line - 1, s.col - 1, &m_lines);
    }

    if !safe {
        return None; // rename incompleto/ambiguo → mejor no tocar nada
    }
    positions.sort();
    positions.dedup();
    Some(positions)
}

/// Añade una posición a renombrar si el texto en `(dl0, dc0)` de `lines` es exactamente `name`;
/// si no (alias, ref calificada, posición inesperada), marca el rename como **inseguro**.
pub(super) fn push_rename(pos: &mut Vec<(String, Span)>, safe: &mut bool, name: &str, u: String, dl0: usize, dc0: usize, lines: &[&str]) {
    match token_at(lines, dl0, dc0) {
        Some(t) if t == name => pos.push((u, (dl0, dc0, name.chars().count()))),
        _ => *safe = false,
    }
}

/// El nombre global del ítem top-level (función/struct/enum/trait) cuya declaración está en
/// `(line, col)`, o `None` si ahí no hay uno (p. ej. es una variable local).
pub(super) fn def_global_name(program: &crate::ast::Program, line: usize, col: usize) -> Option<String> {
    let en = |l: usize, c: usize| l == line && c == col;
    program.functions.iter().find(|f| en(f.line, f.col)).map(|f| f.name.clone())
        .or_else(|| program.structs.iter().find(|s| en(s.line, s.col)).map(|s| s.name.clone()))
        .or_else(|| program.enums.iter().find(|e| en(e.line, e.col)).map(|e| e.name.clone()))
        .or_else(|| program.traits.iter().find(|t| en(t.line, t.col)).map(|t| t.name.clone()))
}

/// El contexto de import en el que está el cursor (M45c), detectado textualmente sobre el prefijo
/// de la línea (el import a medio escribir no parsea).
pub(super) enum ImportCtx {
    /// `from <ruta> import <cursor>` — completar los **símbolos `pub`** del módulo `<ruta>`.
    Symbols(String),
    /// `import <cursor>` / `from <cursor> import` — completar **rutas de módulo** del proyecto.
    ModulePath,
}

/// Detecta el contexto de import a partir del prefijo de la línea hasta el cursor (M45c).
/// Reconoce `import <ruta>`, `[pub] from <ruta> import <símbolos>` (y su fase de ruta). Devuelve
/// `None` si la línea no es un import.
pub(super) fn import_context(prefix: &str) -> Option<ImportCtx> {
    let t = prefix.trim_start();
    // `from <ruta> import <símbolos>` (con o sin `pub`). Si ya apareció `import`, estamos en los símbolos.
    let from_part = t.strip_prefix("pub ").unwrap_or(t);
    if let Some(rest) = from_part.strip_prefix("from ") {
        // ¿hay ya un `import ` (palabra) antes del cursor? Entonces completamos símbolos.
        if let Some(pos) = rest.find(" import ").or_else(|| rest.strip_suffix(" import").map(|_| rest.len() - 7)) {
            let path = rest[..pos].trim();
            if !path.is_empty() {
                return Some(ImportCtx::Symbols(path.to_string()));
            }
        }
        // Aún en la ruta del módulo (`from ge|`).
        return Some(ImportCtx::ModulePath);
    }
    // `import <ruta>` (sin `pub`).
    if t.strip_prefix("import ").is_some() {
        return Some(ImportCtx::ModulePath);
    }
    None
}

/// Las raíces de resolución de módulos para el archivo `entry` (M45c): la raíz del proyecto (si hay
/// `main.ray` ancestro) seguida de las de dependencias (`.ray-deps`). Reusa la lógica de los
/// diagnósticos modulares.
pub(super) fn import_roots(entry: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(root) = project_root_for(entry) {
        roots.push(root);
    }
    roots.extend(dep_roots_for(entry));
    roots
}

/// Los nombres **`pub`** exportados por el módulo en `ruta` (M45c-1): funciones, tipos (struct/enum/
/// trait), constantes y re-exports (`pub from … import …`). Cada uno con su `CompletionItemKind`.
/// `None` si el módulo no se resuelve o no parsea.
/// Clasifica el símbolo `name` en la fuente `fuente` (M46c): su `CompletionItemKind` (3=Function,
/// 22=Struct, 13=Enum, 8=Trait) y su firma si es función. `None` si no está definido ahí. Se usa para
/// los **re-exports** de una cápsula (`pub from … import …`), cuya declaración vive en el submódulo
/// interno, no en `mod.ray`.
pub(super) fn classify_source_symbol(source: &str, name: &str) -> Option<(i64, Option<(Vec<String>, String)>)> {
    let tokens = lexer::lex(source).ok()?;
    let (program, _) = parser::parse_all(tokens);
    if program.functions.iter().any(|f| f.name == name) {
        return Some((3, find_fn_signature(source, name)));
    }
    if program.structs.iter().any(|s| s.name == name) {
        return Some((22, None));
    }
    if program.enums.iter().any(|e| e.name == name) {
        return Some((13, None));
    }
    if program.traits.iter().any(|t| t.name == name) {
        return Some((8, None));
    }
    if program.consts.iter().any(|c| c.name == name) {
        return Some((21, None)); // 21 = Constant
    }
    None
}

/// Construye el `CompletionItem` de un símbolo `pub` de módulo (M46a/M46c): label + kind + —si es
/// función con firma— el detalle y el snippet con placeholders. El calificador de módulo NO es un
/// receptor (`figuras.area_cuadrado(4)` pasa 4), así que la firma va **completa**.
pub(super) fn module_symbol_item(label: String, kind: i64, signature: Option<(Vec<String>, String)>, ctx: &SigCtx) -> Vec<Json> {
    let mut fields = vec![("label", Json::Str(label.clone())), ("kind", num(kind))];
    if let Some((params, ret)) = signature {
        push_signature_raw(&mut fields, params.clone(), ret, false);
        fields.push(("insertText", Json::Str(insert_call(&label, Some(&params), !params.is_empty()))));
        fields.push(("insertTextFormat", num(2))); // 2 = Snippet
        if !params.is_empty() {
            fields.push(("command", obj(vec![
                ("title", Json::Str("signature".into())),
                ("command", Json::Str("editor.action.triggerParameterHints".into())),
            ])));
        }
    }
    let mut out = vec![obj(fields)];
    // M47b: para un struct calificado (`geo.Circulo`), el ítem-extra del literal `Circulo {…}`. Los
    // campos se resuelven en el cierre de imports (que incluye los internos de la cápsula).
    if kind == 22 {
        if let Some(cs) = ctx.struct_fields(&label).filter(|c| !c.is_empty()) {
            out.push(item_struct_literal(&label, &cs));
        }
    }
    out
}

/// El ítem-extra `Nombre {…}` (kind Snippet) que inserta el literal completo con un placeholder por
/// campo (M47b): `Nombre { c1: ${1:T1}, … }`. Compartido por la completion de archivo y la calificada.
pub(super) fn item_struct_literal(name: &str, fields: &[(String, String)]) -> Json {
    let body = fields.iter().enumerate()
        .map(|(i, (f, t))| format!("{}: ${{{}:{}}}", f, i + 1, t))
        .collect::<Vec<_>>().join(", ");
    obj(vec![
        ("label", Json::Str(format!("{} {{…}}", name))),
        ("kind", num(15)), // 15 = Snippet
        ("detail", Json::Str("literal de struct".into())),
        ("filterText", Json::Str(name.to_string())),
        ("insertText", Json::Str(format!("{} {{ {} }}", name, body))),
        ("insertTextFormat", num(2)), // 2 = Snippet
    ])
}

/// El ítem-extra de completion de un builtin que toma una **función anónima** (`spawn`/`scope`):
/// inserta `name(fn() {\n\t$0\n});` —la forma con cuerpo, cursor dentro— además del builtin pelado,
/// al estilo del literal de struct (M47b). El `\t` lo reindenta el editor según su config.
pub(super) fn item_closure_snippet(name: &str) -> Json {
    obj(vec![
        ("label", Json::Str(format!("{}(fn() {{…}})", name))),
        ("kind", num(15)), // 15 = Snippet
        ("detail", Json::Str("con función anónima".into())),
        ("filterText", Json::Str(name.to_string())),
        ("insertText", Json::Str(format!("{}(fn() {{\n\t$0\n}});", name))),
        ("insertTextFormat", num(2)), // 2 = Snippet
    ])
}

pub(super) fn module_pub_symbols(entry: &Path, path: &str) -> Option<Vec<(String, i64, Option<(Vec<String>, String)>)>> {
    let roots = import_roots(entry);
    let path = loader::resolve_module_path(&roots, path).ok()??;
    let source = std::fs::read_to_string(&path).ok()?;
    let tokens = lexer::lex(&source).ok()?;
    let (program, _errs) = parser::parse_all(tokens);
    let mut items: Vec<(String, i64, Option<(Vec<String>, String)>)> = Vec::new();
    // Kinds LSP: 3=Function, 22=Struct, 13=Enum, 8=Interface(trait), 21=Constant. Las funciones
    // llevan su firma (M46a), extraída de la fuente del módulo.
    for f in &program.functions {
        if f.is_pub { items.push((f.name.clone(), 3, find_fn_signature(&source, &f.name))); }
    }
    for s in &program.structs {
        if s.is_pub { items.push((s.name.clone(), 22, None)); }
    }
    for e in &program.enums {
        if e.is_pub { items.push((e.name.clone(), 13, None)); }
    }
    for tr in &program.traits {
        if tr.is_pub { items.push((tr.name.clone(), 8, None)); }
    }
    for c in &program.consts {
        if c.is_pub { items.push((c.name.clone(), 21, None)); }
    }
    // Re-exports: `pub from M import a [as b]` expone el nombre local (alias u original). La
    // declaración (kind + firma) vive en el módulo origen `M` (M46c: la firma se resuelve allí, no
    // en `mod.ray`); si no se resuelve, se cae a función sin firma.
    for fi in &program.from_imports {
        if fi.is_pub {
            let origin = loader::resolve_module_path(&roots, &fi.module).ok().flatten()
                .and_then(|p| std::fs::read_to_string(p).ok());
            for n in &fi.names {
                let (kind, signature) = origin.as_deref()
                    .and_then(|s| classify_source_symbol(s, &n.name))
                    .unwrap_or((3, None));
                items.push((n.local().to_string(), kind, signature));
            }
        }
    }
    items.sort();
    items.dedup();
    Some(items)
}

/// Completion en un **import** (M45c): símbolos `pub` de `from M import …`. `None` si el cursor no
/// está en un contexto de import completable (entonces se sigue con miembro/archivo). Las rutas de
/// módulo (`import …`) se resuelven en `module_path_completion_items`.
pub(super) fn import_completion_items(uri: Option<&str>, src: &str, line0: usize, char0: usize) -> Option<Json> {
    let line = src.split('\n').nth(line0)?;
    let chars: Vec<char> = line.chars().collect();
    let col = char0.min(chars.len());
    let prefix: String = chars[..col].iter().collect();
    let ctx = import_context(&prefix)?;
    let entry = uri.and_then(uri_to_path)?;
    match ctx {
        ImportCtx::Symbols(path) => {
            let items = module_pub_symbols(&entry, &path).unwrap_or_default();
            let ctx = SigCtx::new(src, Some(&entry));
            let list: Vec<Json> = items.into_iter()
                .flat_map(|(label, kind, signature)| module_symbol_item(label, kind, signature, &ctx))
                .collect();
            Some(Json::Arr(list))
        }
        ImportCtx::ModulePath => {
            let chars: Vec<char> = line.chars().collect();
            module_path_completion_items(&entry, line0, col, &chars)
        }
    }
}

/// Completion de **rutas de módulo** (M45c-2): `import <cursor>` / `from <cursor> import`. Ofrece las
/// rutas importables del proyecto (`loader::available_modules`, con la encapsulación aplicada).
///
/// Las rutas llevan `/`, que VSCode no cuenta como carácter de palabra → filtrar `geo/for` contra
/// `geo/formas/circulo` fallaría. Se resuelve con un **`textEdit`** cuyo rango cubre toda la ruta
/// parcial (desde su primer carácter hasta el cursor): así el editor usa el texto completo `geo/for`
/// para el *fuzzy match* contra `filterText`, y al aceptar reemplaza la ruta entera.
pub(super) fn module_path_completion_items(entry: &Path, line0: usize, col: usize, chars: &[char]) -> Option<Json> {
    let roots = import_roots(entry);
    if roots.is_empty() {
        return Some(Json::Arr(vec![]));
    }
    // Inicio de la ruta parcial que se está escribiendo: el último tramo sin espacios antes del cursor.
    let mut start = col;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let range = obj(vec![
        ("start", obj(vec![("line", num(line0 as i64)), ("character", num(start as i64))])),
        ("end", obj(vec![("line", num(line0 as i64)), ("character", num(col as i64))])),
    ]);
    let items: Vec<Json> = loader::available_modules(&roots, entry)
        .into_iter()
        .map(|path| obj(vec![
            ("label", Json::Str(path.clone())),
            ("kind", num(9)), // 9 = Module
            ("filterText", Json::Str(path.clone())),
            ("textEdit", obj(vec![("range", range.clone()), ("newText", Json::Str(path))])),
        ]))
        .collect();
    Some(Json::Arr(items))
}

/// Completion de **campos de un literal de struct** (M47a): dentro de `Nombre { … | … }` ofrece los
/// campos del struct que faltan por poner, en vez de la completion de archivo. `None` si el cursor no
/// está en la **posición de nombre de campo** de un literal de struct conocido (entonces se sigue con
/// miembro/archivo).
pub(super) fn struct_literal_completion_items(uri: Option<&str>, src: &str, line0: usize, char0: usize) -> Option<Json> {
    // Prefijo (todo el texto hasta el cursor) como vector de chars.
    let mut prefix = String::new();
    for (i, line) in src.split('\n').enumerate() {
        use std::cmp::Ordering::*;
        match i.cmp(&line0) {
            Less => { prefix.push_str(line); prefix.push('\n'); }
            Equal => { prefix.extend(line.chars().take(char0)); break; }
            Greater => break,
        }
    }
    let cs: Vec<char> = prefix.chars().collect();
    // El `{` sin cerrar más cercano (el literal en curso).
    let (mut depth, mut i, mut brace) = (0i32, cs.len(), None);
    while i > 0 {
        i -= 1;
        match cs[i] {
            '}' => depth += 1,
            '{' if depth == 0 => { brace = Some(i); break; }
            '{' => depth -= 1,
            _ => {}
        }
    }
    let brace = brace?;
    // El identificador (último segmento) justo antes del `{`: el nombre del struct.
    let mut end = brace;
    while end > 0 && cs[end - 1].is_whitespace() { end -= 1; }
    let mut start = end;
    while start > 0 && is_ident_char(cs[start - 1]) { start -= 1; }
    if start == end { return None; } // no hay identificador antes del `{` → es un bloque, no un literal
    let name: String = cs[start..end].iter().collect();
    // El nombre puede venir CALIFICADO (`webserver.Response {`, M11.3c-3): retrocede sobre los
    // segmentos `ident.` para que las guardas de abajo vean lo que precede al nombre COMPLETO
    // (con `qstart = start`, un `-> M.Tipo {` vería el `.` y no el `->` → cuerpo de función
    // confundido con literal, campos del tipo de RETORNO sugeridos dentro del cuerpo).
    let mut qstart = start;
    while qstart > 1 && cs[qstart - 1] == '.' && is_ident_char(cs[qstart - 2]) {
        qstart -= 1; // el `.`
        while qstart > 0 && is_ident_char(cs[qstart - 1]) { qstart -= 1; }
    }
    // Guarda: descartar posiciones que NO son un literal aunque lleven un nombre de tipo antes del `{`
    // — cuerpo de función (`-> T {`), impl (`for T {`), definición (`struct/enum/trait T {`).
    let mut k = qstart;
    while k > 0 && cs[k - 1].is_whitespace() { k -= 1; }
    if k >= 2 && cs[k - 1] == '>' && cs[k - 2] == '-' { return None; } // `-> T {`
    if let Some((prev, _)) = ident_before(&cs, qstart) {
        if matches!(prev.as_str(), "for" | "struct" | "enum" | "trait" | "impl") { return None; }
    }
    // ¿El struct existe (en el archivo o el cierre de imports)?
    let ctx = SigCtx::new(src, uri.and_then(uri_to_path).as_deref());
    let fields = ctx.struct_fields(&name)?;

    // El texto del literal desde el `{` hasta el cursor: separa la ENTRADA de campo actual (tras la
    // última coma de nivel superior) y detecta si estamos escribiendo un VALOR (`campo: …`) — en ese
    // caso NO es posición de nombre de campo (se sigue con miembro/archivo).
    let body = &cs[brace + 1..];
    let (mut d, mut entry_start) = (0i32, 0usize);
    for (idx, &c) in body.iter().enumerate() {
        match c {
            '{' | '(' | '[' => d += 1,
            '}' | ')' | ']' => d -= 1,
            ',' if d == 0 => entry_start = idx + 1,
            _ => {}
        }
    }
    let mut d2 = 0i32;
    let in_value = body[entry_start..].iter().any(|&c| match c {
        '{' | '(' | '[' => { d2 += 1; false }
        '}' | ')' | ']' => { d2 -= 1; false }
        ':' if d2 == 0 => true,
        _ => false,
    });
    if in_value { return None; }

    // Campos ya escritos en el literal (todos los `ident:` de nivel superior) para no repetirlos.
    let already = fields_already_written(body);
    let items: Vec<Json> = fields.into_iter()
        .filter(|(f, _)| !already.contains(f))
        .map(|(f, ty)| obj(vec![
            ("label", Json::Str(f.clone())),
            ("kind", num(5)), // 5 = Field
            ("detail", Json::Str(ty)),
            ("insertText", Json::Str(format!("{}: ", f))),
        ]))
        .collect();
    Some(Json::Arr(items))
}

/// Los nombres de campo ya escritos en el cuerpo de un literal de struct (`ident:` de nivel superior).
pub(super) fn fields_already_written(body: &[char]) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < body.len() {
        match body[i] {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            c if depth == 0 && is_ident_char(c) => {
                let start = i;
                while i < body.len() && is_ident_char(body[i]) { i += 1; }
                let name: String = body[start..i].iter().collect();
                let mut j = i;
                while j < body.len() && body[j].is_whitespace() { j += 1; }
                if body.get(j) == Some(&':') { out.insert(name); }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Completion de **miembros** (M45): si el cursor `(line0, char0)` (0-basados) está tras un `.`,
/// devuelve `Some(items)` con los campos/métodos/builtins/UFCS del tipo del receptor; `None` si no
/// es un contexto de miembro (entonces el llamador hace la completion de archivo). La lista puede
/// venir vacía (receptor sin tipo conocido): sigue siendo `Some` (no hay que ofrecer todo el archivo
/// tras un punto).
///
/// Estrategia: **reparar** la fuente insertando el centinela `__raycomplete__` en lugar de la
/// palabra-miembro que se está escribiendo (`recv.par|` → `recv.__raycomplete__`), que es sintaxis
/// válida y sobrevive a la recuperación de errores del parser (M33c). El checker enumera los
/// miembros al tipar ese acceso.
/// Completion de miembros de un **módulo** importado (M49.1): si el cursor está tras `<leaf>.` y
/// `<leaf>` es el nombre de un módulo importado (`import std/math;` → `math`), ofrece los ítems `pub`
/// de ese módulo —funciones, `const`, structs/enums/traits— por su nombre corto. Devuelve `None` si el
/// receptor no es un módulo importado (para que el llamador siga con la completion de miembros por tipo).
pub(super) fn module_member_completion_items(uri: Option<&str>, src: &str, line0: usize, char0: usize) -> Option<Json> {
    let line = src.split('\n').nth(line0)?;
    let chars: Vec<char> = line.chars().collect();
    let col = char0.min(chars.len());
    // La palabra-miembro parcial a la izquierda del cursor, y el `.` que la precede.
    let mut start = col;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }
    if start == 0 || chars[start - 1] != '.' {
        return None;
    }
    // El receptor: el identificador simple justo antes del `.`.
    let dot = start - 1;
    let mut r_start = dot;
    while r_start > 0 && is_ident_char(chars[r_start - 1]) {
        r_start -= 1;
    }
    let receiver: String = chars[r_start..dot].iter().collect();
    if receiver.is_empty() {
        return None;
    }
    // ¿El receptor es un leaf de import? → su `ns_prefix` (`math` → `std::math`).
    let ns = imports_of(src).into_iter().find(|(leaf, _)| *leaf == receiver).map(|(_, n)| n)?;
    // El buffer del usuario puede NO parsear a medio escribir (`math.`), así que se carga el módulo con
    // un programa **sintético válido** (`import <ruta>; fn main…`), no el buffer. Se filtran luego los
    // ítems `pub` de ese módulo (nombre `ns::corto`, sin `#` ni sub-namespaces). Kinds LSP: 3=Function,
    // 21=Constant, 22=Struct, 13=Enum, 8=Interface(trait).
    let path = uri_to_path(uri?)?;
    let synthetic = format!("import {};\nfn main() -> int {{ 0 }}\n", ns.replace("::", "/"));
    let loaded = load(&path, &synthetic).ok()?;
    let prog = loaded.program;
    let prefix = format!("{ns}::");
    let short = |name: &str| -> Option<String> {
        let n = name.strip_prefix(&prefix)?;
        if n.contains("::") || n.contains('#') { None } else { Some(n.to_string()) }
    };
    let mut items: Vec<Json> = Vec::new();
    let mut push = |label: String, kind: i64| {
        items.push(obj(vec![("label", text(&label)), ("kind", num(kind))]));
    };
    for f in &prog.functions {
        if f.is_pub && let Some(n) = short(&f.name) { push(n, 3); }
    }
    for c in &prog.consts {
        if c.is_pub && let Some(n) = short(&c.name) { push(n, 21); }
    }
    for s in &prog.structs {
        if s.is_pub && let Some(n) = short(&s.name) { push(n, 22); }
    }
    for e in &prog.enums {
        if e.is_pub && let Some(n) = short(&e.name) { push(n, 13); }
    }
    for t in &prog.traits {
        if t.is_pub && let Some(n) = short(&t.name) { push(n, 8); }
    }
    Some(Json::Arr(items))
}

/// Convierte los miembros enumerados por el checker (para `recv.` o `x |>`) en ítems de completion:
/// etiqueta + kind + documentación (`///` del método/UFCS o del prelude) + —para invocables— la firma
/// sin el receptor, un snippet con placeholders por parámetro y el disparo del signature help (M45b/M46).
/// `insert_prefix` se antepone al texto insertado (para `x |>` pegado al operador se usa `" "` → `|> f`,
/// no `|>f`); vacío para `recv.`.
pub(super) fn members_to_completion_items(members: Vec<crate::checker::MemberItem>, src: &str, uri: Option<&str>, insert_prefix: &str) -> Vec<Json> {
    let orig_lines: Vec<&str> = src.split('\n').collect();
    let ctx = SigCtx::new(src, uri.and_then(uri_to_path).as_deref()); // M46a: firmas para el detalle
    members
        .into_iter()
        .map(|m| {
            // Documentación: builtin → `///` sobre la declaración del método/UFCS (M45b) → prelude.
            let doc = crate::builtins::doc(&m.label).map(|s| s.to_string())
                .or_else(|| m.def.and_then(|(dl, _)| {
                    // La def vive en la fuente original (el reparado no cambia números de línea);
                    // si cae fuera (símbolo del prelude), se intenta el prelude más abajo.
                    if dl >= 1 && dl <= orig_lines.len() {
                        crate::raydoc::doc_lines_above(&orig_lines, dl).map(|ls| ls.join("\n"))
                    } else {
                        None
                    }
                }))
                .or_else(|| doc_in_prelude(&m.label));
            let mut fields = vec![
                ("label", Json::Str(m.label.clone())),
                ("kind", num(m.kind as i64)),
            ];
            if let Some(d) = m.detail {
                fields.push(("detail", Json::Str(d)));
            }
            // Invocables (método/función): detalle de firma (M46a, sin el receptor) + snippet con
            // placeholders por parámetro (M46c) + disparo del signature help.
            if m.kind == 2 || m.kind == 3 {
                // Params en contexto de método: la firma sin el receptor.
                let method_params = ctx.signature(&m.label).map(|(mut ps, ret)| {
                    if !ps.is_empty() { ps.remove(0); } // el receptor
                    (ps, ret)
                });
                if let Some((ps, ret)) = &method_params {
                    push_signature_raw(&mut fields, ps.clone(), ret.clone(), false);
                }
                let ps_ref = method_params.as_ref().map(|(ps, _)| ps.as_slice());
                fields.push(("insertText", Json::Str(format!("{}{}", insert_prefix, insert_call(&m.label, ps_ref, m.has_args)))));
                fields.push(("insertTextFormat", num(2))); // 2 = Snippet
                if m.has_args {
                    fields.push(("command", obj(vec![
                        ("title", Json::Str("signature".into())),
                        ("command", Json::Str("editor.action.triggerParameterHints".into())),
                    ])));
                }
            }
            if let Some(d) = doc {
                fields.push(("documentation", obj(vec![
                    ("kind", Json::Str("markdown".into())),
                    ("value", Json::Str(d)),
                ])));
            }
            obj(fields)
        })
        .collect()
}

/// Completion tras un `|>` (pipeline, M7.2). Como `x |> f(a)` ≡ `f(x, a)` ≡ `x.f(a)`, el conjunto de
/// funciones "pipeables" es el mismo que enumera el acceso a miembro: se **repara** `x |> parc` como
/// `x |> __raycomplete__`, que el parser desazucara a `__raycomplete__(x)`; el checker (en modo
/// `completing`) enumera los miembros del tipo del operando izquierdo —incluidas las funciones libres
/// aplicables por UFCS—. Se ofrecen también métodos/campos del tipo (ligera sobre-inclusión inocua: el
/// editor filtra por el prefijo tecleado). `None` si el cursor no está tras un `|>`.
pub(super) fn pipeline_completion_items(uri: Option<&str>, src: &str, line0: usize, char0: usize) -> Option<Json> {
    let lines: Vec<&str> = src.split('\n').collect();
    let line = lines.get(line0)?;
    let chars: Vec<char> = line.chars().collect();
    let col = char0.min(chars.len());
    // Retrocede sobre la palabra parcial (nombre de función) que se teclea tras el `|>`.
    let mut start = col;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }
    // Antes de la palabra (saltando espacios) debe venir el operador `|>`.
    let mut p = start;
    while p > 0 && chars[p - 1].is_whitespace() {
        p -= 1;
    }
    if p < 2 || chars[p - 1] != '>' || chars[p - 2] != '|' {
        return None;
    }
    // ¿La palabra está **pegada** al `|>` (sin espacio entre medias)? Entonces el texto insertado
    // lleva un espacio inicial para que quede `|> f`, no `|>f`. Si ya hay un espacio, no se duplica.
    let glued = p == start;
    let insert_prefix = if glued { " " } else { "" };
    // Avanza sobre el resto de la palabra parcial (a la derecha del cursor) para reemplazarla entera.
    let mut end = col;
    while end < chars.len() && is_ident_char(chars[end]) {
        end += 1;
    }
    // Reconstruye `LEFT |> __raycomplete__`. En posición de sentencia hace falta `;` (o el bloque no
    // parsea); en posición de expresión NO —el delimitador que sigue ya la cierra— (como en `recv.`).
    let next = chars[end..].iter().find(|c| !c.is_whitespace()).copied();
    let in_expression = matches!(next, Some(')') | Some(']') | Some('}') | Some(',') | Some('('));
    let mut new_line: String = chars[..start].iter().collect();
    new_line.push_str(crate::checker::COMPLETION_SENTINEL);
    if !in_expression {
        new_line.push(';');
    }
    new_line.extend(chars[end..].iter());
    let mut repaired_lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    repaired_lines[line0] = new_line;
    let repaired = repaired_lines.join("\n");

    let members = member_completion_of(uri, &repaired);
    Some(Json::Arr(members_to_completion_items(members, src, uri, insert_prefix)))
}

/// Corre `checker::member_completion` sobre la fuente reparada (con el centinela), **módulo-aware**
/// si el documento es un archivo: el loader fusiona sus imports desde disco —el módulo de entrada
/// queda en delta 0, así el centinela no se mueve— y `recv.` resuelve también los tipos de otros
/// módulos (`webserver.Request`, un `from M import Tipo`). Si no es un archivo o el loader falla
/// (buffer suelto, otro error a medio escribir, import roto), cae al buffer aislado con
/// recuperación de errores (el comportamiento previo). Misma idea que `index_for` (hover/def).
pub(super) fn member_completion_of(uri: Option<&str>, repaired: &str) -> Vec<checker::MemberItem> {
    if let Some(path) = uri.and_then(uri_to_path)
        && let Ok(loaded) = load(&path, repaired)
    {
        let mut program = loaded.program;
        return checker::member_completion(&mut program);
    }
    let Ok(tokens) = lexer::lex(repaired) else { return Vec::new() };
    let (mut program, _errs) = parser::parse_all(tokens);
    checker::member_completion(&mut program)
}

pub(super) fn member_completion_items(uri: Option<&str>, src: &str, line0: usize, char0: usize, docs: &HashMap<String, String>) -> Option<Json> {
    let lines: Vec<&str> = src.split('\n').collect();
    let line = lines.get(line0)?;
    let chars: Vec<char> = line.chars().collect();
    let col = char0.min(chars.len());
    // Retrocede sobre la palabra-miembro parcial (identificador) que se está escribiendo.
    let mut start = col;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }
    // El carácter inmediatamente anterior a la palabra debe ser un `.` para ser acceso a miembro.
    if start == 0 || chars[start - 1] != '.' {
        return None;
    }
    // El **receptor**: el identificador simple justo antes del `.` (para el acceso calificado a un
    // módulo, `u.` / `circulo.`, M45c-3). Vacío si el receptor es una expresión compleja (`).`).
    let dot = start - 1;
    let mut r_start = dot;
    while r_start > 0 && is_ident_char(chars[r_start - 1]) {
        r_start -= 1;
    }
    let receiver: String = chars[r_start..dot].iter().collect();
    // Avanza sobre el resto de la palabra parcial (a la derecha del cursor) para reemplazarla entera.
    let mut end = col;
    while end < chars.len() && is_ident_char(chars[end]) {
        end += 1;
    }
    // Reconstruye la fuente con la palabra-miembro sustituida por el centinela. En **posición de
    // sentencia** (`x.` al final de una línea) hay que terminar con `;`, o el bloque no parsea y
    // `parse_all` descartaría la función al resincronizar. En **posición de expresión** (dentro de
    // `sum(x.)`, `[x.]`, `{ x. }`) NO se añade `;` —rompería la llamada/lista—: el delimitador que
    // sigue ya cierra la expresión. Se decide por el siguiente carácter no-espacio de la línea.
    let next = chars[end..].iter().find(|c| !c.is_whitespace()).copied();
    let in_expression = matches!(next, Some(')') | Some(']') | Some('}') | Some(',') | Some('('));
    let mut new_line: String = chars[..start].iter().collect();
    new_line.push_str(crate::checker::COMPLETION_SENTINEL);
    if !in_expression {
        new_line.push(';');
    }
    new_line.extend(chars[end..].iter());
    let mut repaired_lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    repaired_lines[line0] = new_line;
    let repaired = repaired_lines.join("\n");

    // Corre el front-end sobre la fuente reparada (módulo-aware si es un archivo) y enumera.
    let members = member_completion_of(uri, &repaired);

    let items = members_to_completion_items(members, src, uri, "");
    let _ = docs;
    // Sin miembros de valor, el receptor puede ser un TIPO o un MÓDULO (van tras el intento de valor,
    // así un local que los tape gana, como en el resolutor):
    if items.is_empty() {
        // (a0) FUNCIONES ASOCIADAS de un tipo incorporado (`Map.`/`Channel.` → `new`/`bounded`, M48.1;
        //      kind 3 = Function). Snippet con placeholders + firma en el popup + disparo del sig help.
        if crate::builtins::assoc_for_type(&receiver).next().is_some() {
            let visible_items: Vec<Json> = crate::builtins::assoc_for_type(&receiver).map(|a| {
                let params = assoc_param_names(a.sig);
                let snippet = params.iter().enumerate()
                    .map(|(i, p)| format!("${{{}:{}}}", i + 1, p.split(':').next().unwrap_or(p).trim()))
                    .collect::<Vec<_>>().join(", ");
                let inline = format!("({})", params.join(", "));
                let mut fields = vec![
                    ("label", Json::Str(a.fn_name.to_string())),
                    ("kind", num(3)),
                    ("insertText", Json::Str(format!("{}({})", a.fn_name, snippet))),
                    ("insertTextFormat", num(2)),
                    ("labelDetails", obj(vec![("detail", Json::Str(inline))])),
                    ("detail", Json::Str(a.sig.to_string())),
                    ("documentation", obj(vec![
                        ("kind", Json::Str("markdown".into())),
                        ("value", Json::Str(a.doc.to_string())),
                    ])),
                ];
                if a.arity > 0 {
                    fields.push(("command", obj(vec![
                        ("title", Json::Str("signature".into())),
                        ("command", Json::Str("editor.action.triggerParameterHints".into())),
                    ])));
                }
                obj(fields)
            }).collect();
            return Some(Json::Arr(visible_items));
        }
        // (a) un ENUM (`Orientacion.` → sus variantes; kind 20 = EnumMember). Las variantes con
        //     payload insertan placeholders por el tipo de cada campo.
        let ctx = SigCtx::new(src, uri.and_then(uri_to_path).as_deref());
        if let Some(variants) = ctx.enum_variants(&receiver) {
            let visible_items: Vec<Json> = variants.iter().map(|v| {
                let mut fields = vec![("label", Json::Str(v.name.clone())), ("kind", num(20))];
                if !v.payload.is_empty() {
                    let types: Vec<String> = v.payload.iter().map(|t| format!("{}", t)).collect();
                    let args = types.iter().enumerate()
                        .map(|(i, t)| format!("${{{}:{}}}", i + 1, t))
                        .collect::<Vec<_>>().join(", ");
                    fields.push(("insertText", Json::Str(format!("{}({})", v.name, args))));
                    fields.push(("insertTextFormat", num(2)));
                    // Muestra los tipos del payload en el popup, como la firma de una función (M46a).
                    let inline = format!("({})", types.join(", "));
                    fields.push(("labelDetails", obj(vec![("detail", Json::Str(inline.clone()))])));
                    fields.push(("detail", Json::Str(inline)));
                    // Al aceptar, dispara el signature help (que muestra los tipos del payload, M46c).
                    fields.push(("command", obj(vec![
                        ("title", Json::Str("signature".into())),
                        ("command", Json::Str("editor.action.triggerParameterHints".into())),
                    ])));
                }
                obj(fields)
            }).collect();
            return Some(Json::Arr(visible_items));
        }
        // (b) el **leaf de un `import`** (`import geo/util as u;` → `u.` accede calificado a sus `pub`).
        if let Some(mods) = module_alias_symbols(uri, src, &receiver) {
            return Some(mods);
        }
    }
    Some(Json::Arr(items))
}

/// Los símbolos `pub` del módulo cuyo **leaf** de import es `receptor` (M45c-3): `import a/b/c [as x];`
/// liga el leaf `x` (o `c`), y `x.` / `c.` accede calificado a sus `pub`. `None` si `receptor` no es
/// el leaf de ningún `import` del archivo (o no hay URI para resolver desde disco).
pub(super) fn module_alias_symbols(uri: Option<&str>, src: &str, receiver: &str) -> Option<Json> {
    if receiver.is_empty() {
        return None;
    }
    let entry = uri.and_then(uri_to_path)?;
    let tokens = lexer::lex(src).ok()?;
    let (program, _errs) = parser::parse_all(tokens);
    let modpath = program.imports.iter()
        .find(|d| d.leaf() == receiver)
        .map(|d| d.module.clone())?;
    let items = module_pub_symbols(&entry, &modpath).unwrap_or_default();
    let ctx = SigCtx::new(src, Some(&entry));
    let list: Vec<Json> = items.into_iter()
        .flat_map(|(label, kind, signature)| module_symbol_item(label, kind, signature, &ctx))
        .collect();
    Some(Json::Arr(list))
}

/// El `result` de `textDocument/completion`: los símbolos ofrecibles en el documento (funciones y
/// tipos definidos —incluido el prelude—, builtins y palabras clave). No filtra por ámbito ni por
/// prefijo (el cliente filtra por lo ya escrito); es una completion "de archivo", el primer escalón.
pub(super) fn completion_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let uri = msg.get("params").and_then(|p| p.get("textDocument")).and_then(|t| t.get("uri")).and_then(|u| u.as_str());
    if uri.is_some_and(is_template_uri) {
        // M55: en un template — tras un `.`, los MIEMBROS del receptor (vía el módulo generado:
        // campos del struct, métodos, builtins aplicables); si no, sus propios símbolos (params
        // tipados + vars de for + keywords).
        let (Some(u), Some(src)) = (uri, uri.and_then(|x| docs.get(x))) else { return Json::Arr(vec![]) };
        let Some((_, line0, char0)) = pos_params(msg) else { return Json::Arr(vec![]) };
        if let Some((code, map, gen_uri)) = template_generated(u, src)
            && let Some((gl, gc)) = template_pos_to_generated(src, &code, &map, line0, char0)
        {
            // `member_completion_items` es de un solo buffer (sin loader): el `from std/template
            // import escape_html;` del generado quedaría sin resolver y el checker abortaría antes
            // del centinela. Se sustituye por un stub local (misma cantidad de líneas).
            let code_sb = code.replacen(
                "from std/template import escape_html;",
                "fn escape_html(s: string) -> string { s }",
                1,
            );
            if let Some(items) = member_completion_items(Some(&gen_uri), &code_sb, gl, gc, docs) {
                return items;
            }
        }
        return template_completion_items(src, line0, char0);
    }
    let Some(src) = uri.and_then(|u| docs.get(u)) else { return Json::Arr(vec![]) };
    if let Some((line0, char0)) = pos_params(msg).map(|(_, l, c)| (l, c)) {
        // El espacio es un trigger char, pero SOLO ofrece algo en contexto de pipeline (`|> `): así,
        // tras teclear un espacio después de `|>`, la lista de funciones vuelve a aparecer, sin
        // inundar con la completion de archivo tras cada espacio del documento. Una invocación manual
        // o por letra no trae `triggerCharacter` → sigue el flujo normal de abajo.
        let space_triggered = msg.get("params").and_then(|p| p.get("context"))
            .and_then(|c| c.get("triggerCharacter")).and_then(|t| t.as_str()) == Some(" ");
        if space_triggered {
            return pipeline_completion_items(uri, src, line0, char0).unwrap_or_else(|| Json::Arr(vec![]));
        }
        // M45c: en una línea de `import`/`from … import`, ofrecemos rutas de módulo o símbolos `pub`,
        // no los símbolos de archivo.
        if let Some(items) = import_completion_items(uri, src, line0, char0) {
            return items;
        }
        // M47a: dentro de `Nombre { … }` (posición de nombre de campo), los campos del struct.
        if let Some(items) = struct_literal_completion_items(uri, src, line0, char0) {
            return items;
        }
        // M45: si el cursor viene tras un `.` (acceso a miembro), ofrecemos los miembros del tipo del
        // receptor, no los símbolos de archivo. Un contexto de miembro con lista vacía (receptor sin
        // tipo conocido) devuelve `[]` —mejor que inundar con todo el archivo tras un punto—.
        if let Some(items) = member_completion_items(uri, src, line0, char0, docs) {
            // M49.1: si no dio miembros, el receptor puede ser un MÓDULO importado (`math.`) —que no tiene
            // "tipo"—: se ofrecen sus ítems `pub` (funciones/consts/tipos) como fallback.
            let empty = items.as_array().map(|a| a.is_empty()).unwrap_or(true);
            if empty && let Some(m) = module_member_completion_items(uri, src, line0, char0) {
                return m;
            }
            return items;
        }
        // Tras un `|>` (pipeline): funciones aplicables al tipo del operando izquierdo (type-aware,
        // como el acceso a miembro). Va antes de la completion de archivo genérica.
        if let Some(items) = pipeline_completion_items(uri, src, line0, char0) {
            return items;
        }
    }
    let Ok(tokens) = lexer::lex(src) else { return Json::Arr(vec![]) };
    // `parse_all` (con recuperación de errores) en vez de `parse` fail-fast: mientras escribes, el
    // archivo casi nunca parsea entero; así la completion de archivo sigue ofreciendo los símbolos de
    // las funciones bien formadas en vez de quedarse vacía (M46a).
    let (mut program, _errs) = parser::parse_all(tokens);
    // Corre el front-end (inyecta el prelude y los métodos manglados) para ofrecer también sus
    // funciones; tolera errores (info parcial). Se filtran los nombres sintéticos (`#`, `::`, `__`).
    let _ = checker::semantic_index(&mut program);

    // Kinds de LSP CompletionItemKind: 3=Function, 22=Struct, 13=Enum, 8=Interface, 14=Keyword.
    let visible = |n: &str| !n.contains('#') && !n.contains("::") && !n.starts_with("__");
    let mut items: Vec<(String, i64)> = Vec::new();
    for f in &program.functions {
        if visible(&f.name) { items.push((f.name.clone(), 3)); }
    }
    for s in &program.structs {
        if visible(&s.name) { items.push((s.name.clone(), 22)); }
    }
    for e in &program.enums {
        if visible(&e.name) { items.push((e.name.clone(), 13)); }
    }
    for t in &program.traits {
        if visible(&t.name) { items.push((t.name.clone(), 8)); }
    }
    for c in &program.consts {
        if visible(&c.name) { items.push((c.name.clone(), 21)); } // 21 = Constant
    }
    for b in crate::builtins::names().filter(|n| visible(n)) {
        items.push((b.to_string(), 3));
    }
    for kw in [
        "let", "var", "const", "fn", "if", "else", "while", "for", "in", "match",
        "struct", "enum", "trait", "impl", "return", "true", "false", "dyn", "pub",
        "import", "from", "as",
        "int", "float", "bool", "string", "char", "bytes", "u8", "u32", "u64",
    ] {
        items.push((kw.to_string(), 14));
    }
    // Tipos genéricos incorporados y del prelude, que no son palabras clave pero se escriben como tipo.
    for (t, k) in [("Option", 13), ("Result", 13), ("Map", 7), ("Channel", 7), ("Task", 7)] {
        items.push((t.to_string(), k)); // 13 = Enum, 7 = Class
    }
    // M46: símbolos que NO viven en este archivo pero están en ámbito — el **nombre de cada módulo
    // importado** (`import figuras;` → `figuras`, kind 9=Module) y los **`from`-imports** (`cuad`/
    // `area`/`Rect`/`Orientacion`), clasificados en su módulo origen (fn/struct/enum/trait).
    let entry_path = uri.and_then(uri_to_path);
    for d in &program.imports {
        items.push((d.leaf().to_string(), 9));
    }
    if let Some(entry) = &entry_path {
        let roots = import_roots(entry);
        let mut cache: HashMap<String, Option<String>> = HashMap::new();
        for fi in &program.from_imports {
            let origin = cache.entry(fi.module.clone()).or_insert_with(|| {
                loader::resolve_module_path(&roots, &fi.module).ok().flatten()
                    .and_then(|p| std::fs::read_to_string(p).ok())
            }).clone();
            for n in &fi.names {
                let kind = origin.as_deref()
                    .and_then(|s| classify_source_symbol(s, &n.name))
                    .map(|(k, _)| k)
                    .unwrap_or(3);
                items.push((n.local().to_string(), kind));
            }
        }
    }
    // M10.2f: completion **por ámbito** — los parámetros y locales (let/var) de la función que
    // contiene el cursor, declarados en o antes de su línea. Kind 6 = Variable. Sin spans no se
    // distinguen bloques anidados; basta el alcance de la función (degradación honesta).
    if let Some((_, line0, _)) = pos_params(msg) {
        for local in scope_locals(&program, line0 + 1) {
            if visible(&local) { items.push((local, 6)); }
        }
    }
    items.sort();
    items.dedup();
    let ctx = SigCtx::new(src, entry_path.as_deref()); // M46a: firmas para el detalle
    // M47b: los structs ofrecibles (kind 22) para el ítem-extra del literal, más abajo.
    let offerable_structs: Vec<String> = items.iter()
        .filter(|(_, k)| *k == 22).map(|(l, _)| l.clone()).collect();
    let mut list: Vec<Json> = items.into_iter()
        .map(|(label, kind)| {
            // Documentación del ítem: metadatos del builtin, o los `///` del prelude.
            let doc = crate::builtins::doc(&label).map(|s| s.to_string())
                .or_else(|| doc_in_prelude(&label));
            let mut fields = vec![("label", Json::Str(label.clone())), ("kind", num(kind))];
            // M46a/M46c: las funciones/builtins (kind 3) muestran su firma en el detalle e insertan
            // un snippet `nombre(${1:p}, …)` con placeholders navegables por Tab.
            if kind == 3 {
                let signature = ctx.signature(&label);
                if let Some((ps, ret)) = &signature {
                    push_signature_raw(&mut fields, ps.clone(), ret.clone(), false);
                }
                let ps_ref = signature.as_ref().map(|(ps, _)| ps.as_slice());
                let has_args = ps_ref.map(|p| !p.is_empty()).unwrap_or(false);
                fields.push(("insertText", Json::Str(insert_call(&label, ps_ref, has_args))));
                fields.push(("insertTextFormat", num(2))); // 2 = Snippet
                if has_args {
                    fields.push(("command", obj(vec![
                        ("title", Json::Str("signature".into())),
                        ("command", Json::Str("editor.action.triggerParameterHints".into())),
                    ])));
                }
            }
            if let Some(d) = doc {
                fields.push(("documentation", obj(vec![
                    ("kind", Json::Str("markdown".into())),
                    ("value", Json::Str(d)),
                ])));
            }
            obj(fields)
        })
        .collect();
    // M47b: por cada struct ofrecible, un ítem EXTRA `Nombre {…}` que inserta el **literal completo**
    // con un placeholder por campo (`Nombre { c1: ${1:T1}, … }`), al estilo rust-analyzer. Va aparte
    // del tipo pelado (que sigue para las posiciones de tipo, `let x: Nombre`); `filterText` = el
    // nombre, así aparece al teclear el tipo. Solo para structs con campos conocidos.
    for label in offerable_structs {
        if let Some(fields) = ctx.struct_fields(&label).filter(|c| !c.is_empty()) {
            list.push(item_struct_literal(&label, &fields));
        }
    }
    // Ítem-extra de closure para los builtins que toman una función anónima (`spawn(fn() { … })`,
    // `scope(fn() { … })`): inserta la forma con cuerpo, aparte del builtin pelado.
    for name in ["spawn", "scope"] {
        if crate::builtins::names().any(|n| n == name) {
            list.push(item_closure_snippet(name));
        }
    }
    Json::Arr(list)
}

/// Completion dentro de un template `.ray.html` (M55): SOLO ofrece algo con el cursor **dentro de
/// los delimitadores** `{{ … }}` / `{% … %}` (el HTML no es nuestro; fuera se devuelve `[]` y el
/// editor sigue con su completion de HTML). Ofrece los **params tipados** de la cabecera
/// `{% params %}` (kind Variable, el tipo como detalle), las **variables de los `{% for %}` que
/// encierran el cursor** (tipo inferido: iterar un param `[T]` → `T`; un rango `a..b` → `int`) y,
/// en contexto de etiqueta `{%`, las palabras clave del template. Todo por escaneo textual: el
/// template está a medio escribir mientras se pide completion, no se puede tokenizar entero.
pub(super) fn template_completion_items(src: &str, line0: usize, char0: usize) -> Json {
    let Some((is_tag, vars)) = template_scope(src, line0, char0) else {
        // Fuera de los delimitadores (HTML): NO se compite con la completion de HTML del editor,
        // pero sí se ofrecen los BLOQUES del template como snippets (teclear `for` o `if` inserta
        // el bloque entero con placeholders navegables por Tab).
        return template_completion_list(template_block_snippets(true, false, None));
    };

    // Kinds LSP: 6 = Variable, 14 = Keyword. Los sortText ponen las variables antes que las keywords.
    let mut list: Vec<Json> = Vec::new();
    for (name, ty) in &vars {
        let mut fields = vec![
            ("label", Json::Str(name.clone())),
            ("kind", num(6)),
            ("sortText", Json::Str(format!("0{name}"))),
        ];
        if !ty.is_empty() {
            fields.push(("detail", Json::Str(ty.clone())));
        }
        list.push(obj(fields));
    }
    let keywords: &[&str] = if is_tag {
        &["params", "elif", "else", "endif", "endfor", "in", "import", "extends", "endblock", "true", "false"]
    } else {
        &["true", "false"]
    };
    for kw in keywords {
        list.push(obj(vec![
            ("label", Json::Str(kw.to_string())),
            ("kind", num(14)),
            ("sortText", Json::Str(format!("1{kw}"))),
        ]));
    }
    if is_tag {
        // `for`/`if`/`let` completan el BLOQUE entero (sin el `{%` inicial, que ya está escrito),
        // con un **textEdit explícito** que reemplaza [inicio de la palabra parcial .. cierre
        // huérfano]: el editor auto-cierra `{` con `}` (al teclear `{%` queda `{% |}`) y, como el
        // snippet trae su propio cierre, esa `}` (o un `%}` previo) quedaría duplicada — el rango
        // del textEdit se la come. Explícito a propósito: sin él, cada cliente ADIVINA qué
        // reemplazar (Sublime recorta el prefijo tecleado y se comía el espacio: `forelem`).
        // (La lista va con `isIncomplete` → el editor re-consulta en cada tecla y estas
        // posiciones nunca se quedan desfasadas por el filtrado en cliente.)
        let chars: Vec<char> = src.lines().nth(line0).unwrap_or("").chars().collect();
        let mut end = char0.min(chars.len());
        while end < chars.len() && is_ident_char(chars[end]) {
            end += 1;
        }
        let stray = if chars.get(end) == Some(&'%') && chars.get(end + 1) == Some(&'}') {
            2
        } else if chars.get(end) == Some(&'}') && chars.get(end + 1) != Some(&'}') {
            1
        } else {
            0
        };
        // `{%for` (sin espacio tras el delimitador): el snippet antepone el espacio.
        let mut start = char0.min(chars.len());
        while start > 0 && is_ident_char(chars[start - 1]) {
            start -= 1;
        }
        let lead_space = start > 0 && chars[start - 1] == '%';
        list.extend(template_block_snippets(false, lead_space, Some((line0, start, end + stray))));
    }
    template_completion_list(list)
}

/// Envuelve los ítems de completion de un template en una `CompletionList` con
/// **`isIncomplete: true`**: el editor re-consulta al servidor en CADA tecla (en vez de cachear la
/// lista y filtrar en cliente), así los `additionalTextEdits` de los snippets se calculan siempre
/// contra el documento actual y nunca aplican posiciones desfasadas. El cálculo es textual →
/// re-consultar es barato.
pub(super) fn template_completion_list(items: Vec<Json>) -> Json {
    obj(vec![("isIncomplete", Json::Bool(true)), ("items", Json::Arr(items))])
}

/// Los snippets de bloque del template (`{% for %}…{% endfor %}`, `{% if %}…{% endif %}`,
/// `{% let %}`), con placeholders navegables por Tab. `with_opener` incluye el `{%` inicial (para
/// ofrecerlos en el HTML, donde aún no hay delimitador); sin él, completan un `{% ` ya escrito.
/// `lead_space` antepone un espacio (el cursor está pegado al `%` de un `{%`). `replace` =
/// `(línea0, col_ini, col_fin)`: el rango que el snippet REEMPLAZA como `textEdit` explícito —
/// la palabra parcial tecleada + el cierre huérfano del auto-close — para que ningún cliente
/// tenga que adivinar el reemplazo (cada uno lo hace distinto).
pub(super) fn template_block_snippets(with_opener: bool, lead_space: bool, replace: Option<(usize, usize, usize)>) -> Vec<Json> {
    // Multilínea, con el ESTILO DEL FORMATEADOR (etiqueta en su línea, cuerpo sangrado): el `\t`
    // del snippet lo traduce el editor a su indentación, y re-indenta las líneas al nivel del
    // punto de inserción (comportamiento estándar de los snippets LSP).
    let cases: &[(&str, &str)] = &[
        ("for", "for ${1:elem} in ${2:coleccion} %}\n\t$0\n{% endfor %}"),
        ("if", "if ${1:condicion} %}\n\t$0\n{% endif %}"),
        ("if/else", "if ${1:condicion} %}\n\t$2\n{% else %}\n\t$0\n{% endif %}"),
        ("let", "let ${1:name} = ${2:expr} %}$0"),
        ("include", "include ${1:path/al/template}($2) %}$0"),
        ("block", "block ${1:name} %}\n\t$0\n{% endblock %}"),
    ];
    cases.iter().map(|(label, body)| {
        let insert = if with_opener {
            format!("{{% {body}")
        } else if lead_space {
            format!(" {body}")
        } else {
            body.to_string()
        };
        let mut fields = vec![
            ("label", Json::Str(format!("{{% {label} %}}"))),
            // filterText = la keyword: teclear `for` en el HTML lo ofrece aunque el label lleve {%.
            ("filterText", Json::Str(label.split('/').next().unwrap_or(label).to_string())),
            ("kind", num(15)), // 15 = Snippet
            ("insertText", Json::Str(insert.clone())),
            ("insertTextFormat", num(2)), // 2 = Snippet (placeholders ${n:...})
            ("sortText", Json::Str(format!("1{label}"))),
        ];
        if let Some((l0, ini, fin)) = replace {
            fields.push(("textEdit", obj(vec![
                ("range", range(l0, ini, fin)),
                ("newText", Json::Str(insert)),
            ])));
        }
        obj(fields)
    }).collect()
}

/// El ámbito de un template en una posición: `None` si el cursor está FUERA de `{{ … }}`/`{% … %}`
/// (el HTML no es nuestro); si está dentro, `(es_etiqueta, variables)` — los params tipados de la
/// cabecera `{% params %}` más las variables de los `{% for %}` que encierran el cursor (con su
/// tipo inferido: iterar un param `[T]` → `T`, un rango `a..b` → `int`). Escaneo textual del
/// prefijo hasta el cursor: mientras escribes, el template no tokeniza entero.
pub(super) fn template_scope(src: &str, line0: usize, char0: usize) -> Option<(bool, Vec<(String, String)>)> {
    // El texto hasta el cursor (la línea del cursor cortada en char0, por caracteres).
    let mut prefix = String::new();
    for (i, l) in src.lines().enumerate() {
        if i < line0 {
            prefix.push_str(l);
            prefix.push('\n');
        } else if i == line0 {
            prefix.extend(l.chars().take(char0));
            break;
        }
    }
    // ¿Dentro de un delimitador? El último abridor (`{{` o `{%`) antes del cursor, sin su cerrador.
    let (open_at, is_tag) = match (prefix.rfind("{{"), prefix.rfind("{%")) {
        (Some(e), Some(t)) if t > e => (t, true),
        (Some(e), _) => (e, false),
        (None, Some(t)) => (t, true),
        (None, None) => return None,
    };
    if prefix[open_at..].contains(if is_tag { "%}" } else { "}}" }) {
        return None;
    }

    let params = crate::templ::header_params(src);
    // Variables EN ÁMBITO: grupos apilados — la base (los `{% let %}` de nivel superior) nunca se
    // cierra; cada `{% for %}` abre un grupo (su variable + los `let` de dentro) que muere en su
    // `endfor`. Solo cuentan las etiquetas completas (cerradas con `%}`) del prefijo.
    let mut groups: Vec<Vec<(String, String)>> = vec![Vec::new()];
    let mut rest = prefix.as_str();
    while let Some(i) = rest.find("{%") {
        rest = &rest[i + 2..];
        let Some(j) = rest.find("%}") else { break };
        let tag = rest[..j].trim();
        rest = &rest[j + 2..];
        if tag == "endfor" {
            if groups.len() > 1 {
                groups.pop();
            }
        } else if let Some(body) = tag.strip_prefix("for ")
            && let Some((v, expr)) = body.split_once(" in ")
        {
            let expr = expr.trim();
            let ty = if expr.contains("..") {
                "int".to_string()
            } else {
                params.iter().find(|(n, _)| n == expr)
                    .and_then(|(_, t)| t.strip_prefix('[').and_then(|t| t.strip_suffix(']')))
                    .map(|t| t.trim().to_string())
                    .unwrap_or_default()
            };
            groups.push(vec![(v.trim().to_string(), ty)]);
        } else if let Some(body) = tag.strip_prefix("let ")
            && let Some((v, _)) = body.split_once('=')
        {
            // Tipo desconocido textualmente (el hover semántico vía el generado sí lo da).
            groups
                .last_mut()
                .unwrap_or_else(|| crate::ice!("groups se inicializa con el grupo base"))
                .push((v.trim().to_string(), String::new()));
        }
    }
    let mut vars = params;
    vars.extend(groups.into_iter().flatten());
    Some((is_tag, vars))
}

/// Hover en un template `.ray.html` (M55): sobre un **param** de la cabecera o una **variable de
/// `{% for %}`** en ámbito, dentro de los delimitadores → `nombre: tipo` con el rango del
/// identificador. `None` en cualquier otro sitio (el HTML no es nuestro).
pub(super) fn template_hover_at(src: &str, line0: usize, char0: usize) -> Option<(String, usize, usize)> {
    let (name, start, end) = ident_range_under_cursor(src, line0, char0)?;
    // El ámbito se calcula en el INICIO del identificador (así el propio nombre no cuenta como
    // texto del prefijo y un cursor al final del ident no cambia el resultado).
    let (_, vars) = template_scope(src, line0, start)?;
    let (_, ty) = vars.iter().find(|(n, _)| *n == name)?;
    if ty.is_empty() {
        return None;
    }
    Some((format!("{name}: {ty}"), start, end))
}

// ── Templates: inteligencia DENTRO de las expresiones, vía el módulo generado ────────────────────
//
// La heurística textual de arriba (params + vars de for) cubre lo básico sin compilar nada. Para lo
// semántico —hover con el tipo REAL de una subexpresión, completar miembros tras `.`, ir a la
// definición de un tipo en otro archivo— el truco es el mismo que en los diagnósticos (M55): el
// template GENERA un módulo raylang (con line map), las expresiones se empalman VERBATIM en él, así
// que basta con **traducir la posición del cursor al generado**, correr la maquinaria existente
// (hover_at/definition_at/member_completion_items/signature_help_at) sobre el generado, y traducir
// el resultado de vuelta. Cero lógica semántica nueva.

/// El módulo generado de un template + su line map + el URI del `.ray` hermano (con el que el
/// loader resuelve `from std/template import …` y las path-deps). `None` si el template no genera
/// (con error de sintaxis del template no hay semántica; la heurística textual sigue funcionando).
pub(super) fn template_generated(uri: &str, src: &str) -> Option<(String, Vec<usize>, String)> {
    let path = uri_to_path(uri);
    let name = path
        .as_deref()
        .and_then(|p| crate::templ::fn_suffix_of(p).ok())
        .unwrap_or_else(|| "vista".to_string());
    let dir = path.as_deref().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let (code, map) = crate::templ::generate_with_map_at(src, &name, dir.as_deref()).ok()?;
    let gen_uri = uri.trim_end_matches(".html").to_string();
    Some((code, map, gen_uri))
}

/// Busca `needle` en `hay` con **fronteras de identificador** (si la aguja empieza/termina en un
/// carácter de identificador, el vecino no puede serlo): evita casar `n` dentro de `nombre`.
pub(super) fn find_frag(hay: &str, needle: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = hay[from..].find(needle) {
        let p = from + i;
        let pre_ok = !(needle.starts_with(is_ident_char)
            && hay[..p].chars().next_back().is_some_and(is_ident_char));
        let post_ok = !(needle.ends_with(is_ident_char)
            && hay[p + needle.len()..].chars().next().is_some_and(is_ident_char));
        if pre_ok && post_ok {
            return Some(p);
        }
        from = p + needle.len().max(1);
    }
    None
}

/// Mapea una posición del template (0-basada, con el cursor dentro de un `{{ … }}`/`{% … %}`
/// abierto y cerrado en su misma línea) a la posición equivalente `(línea0, col0-en-chars)` del
/// módulo generado. Como la expresión se empalma verbatim, se usa como aguja el contenido del
/// delimitador (en una etiqueta, sin la palabra clave: `{% if cond %}` → `cond`, que sí aparece en
/// el `if cond {` generado; en `{% params … %}` la lista aparece en la firma de la función) y se
/// localiza en la línea generada que el line map atribuye a esta línea del template.
pub(super) fn template_pos_to_generated(src: &str, code: &str, map: &[usize], line0: usize, char0: usize) -> Option<(usize, usize)> {
    let line = src.lines().nth(line0)?;
    let cursor_b = line.char_indices().nth(char0).map(|(b, _)| b).unwrap_or(line.len());
    // El abridor más cercano a la izquierda del cursor (en esta línea). En `{{&`, `{{` casa en la
    // misma posición: el max por (posición, largo) elige el delimitador completo.
    let (open_b, open_len) = ["{{", "{{&", "{%"].iter()
        .filter_map(|d| line[..cursor_b].rfind(d).map(|i| (i, d.len())))
        .max()?;
    let is_tag = &line[open_b..open_b + 2] == "{%";
    let close_b = open_b + line[open_b..].find(if is_tag { "%}" } else { "}}" })?;
    if cursor_b > close_b {
        return None; // el cursor está tras el cierre → HTML
    }
    let content = &line[open_b + open_len..close_b];
    let mut needle = if is_tag {
        // Sin la palabra clave: `if cond` → `cond`, `for v in e` → `v in e`, `params a: T` → `a: T`
        // (el `elif` se reescribe a `else if` en el generado; la condición sola casa en todos).
        let t = content.trim_start();
        t.find(char::is_whitespace).map(|i| t[i..].trim()).unwrap_or("")
    } else {
        content.trim()
    };
    // `{% include ruta(args) %}`: en el generado la ruta se vuelve `leaf.render_leaf(args)` — solo
    // los ARGS aparecen verbatim, así que la aguja son los args (se empalman tal cual, recortados).
    if is_tag && content.trim_start().starts_with("include")
        && let Some((_, args)) = crate::templ::template_ref(needle)
    {
        needle = args;
    }
    if needle.is_empty() {
        return None;
    }
    let needle_start_b = open_b + open_len + content.find(needle)?;
    let off = cursor_b.saturating_sub(needle_start_b).min(needle.len());
    // La línea generada que provenga de ESTA línea del template y contenga la aguja.
    let tpl_line1 = line0 + 1;
    for (i, gline) in code.lines().enumerate() {
        if map.get(i).copied() != Some(tpl_line1) {
            continue;
        }
        if let Some(p) = find_frag(gline, needle) {
            return Some((i, gline[..p + off].chars().count()));
        }
    }
    None
}

/// Una ocurrencia de identificador dentro de los delimitadores del template:
/// `(línea0, col0 en chars, largo en chars, clave del binding, es_declaración)`. La clave liga
/// todas las apariciones del MISMO símbolo: `p:<nombre>` para un param de la cabecera,
/// `f:<línea>:<col>` para una variable de `{% for %}` (su posición de declaración la identifica,
/// así dos `for` anidados con el mismo nombre no se confunden).
type TplOcc = (usize, usize, usize, String, bool);

/// Escanea TODO el template y resuelve cada identificador de los `{{ }}`/`{% %}` a su binding —
/// el motor común de find-references / rename / highlight / outline del template. Textual con
/// ámbitos: pila de bloques `for` (cada uno puede declarar varias variables: `for (k, v) in m`);
/// un ident precedido por `.` (miembro), una palabra clave o un nombre de tipo de la cabecera
/// no ligan. Solo cuenta delimitadores abiertos y cerrados en la misma línea.
pub(super) fn template_occurrences(src: &str) -> Vec<TplOcc> {
    let params: Vec<String> = crate::templ::header_params(src).into_iter().map(|(n, _)| n).collect();
    let keywords = ["params", "if", "elif", "else", "endif", "for", "endfor", "in", "include", "import", "let", "extends", "block", "endblock", "true", "false"];
    let mut out: Vec<TplOcc> = Vec::new();
    // Grupos de (var, clave) por bloque: la base (los `{% let %}` de nivel superior) nunca se
    // cierra; cada `for` apila un grupo que su `endfor` cierra (los `let` de dentro van con él).
    let mut for_stack: Vec<Vec<(String, String)>> = vec![Vec::new()];
    for (l0, line) in src.lines().enumerate() {
        let mut from = 0usize; // byte
        loop {
            // El siguiente abridor a partir de `from`.
            let rest = &line[from..];
            let (rel, olen, is_tag) = match (rest.find("{{"), rest.find("{%")) {
                (Some(e), Some(t)) if t < e => (t, 2, true),
                (Some(e), _) => (e, if rest[e..].starts_with("{{&") { 3 } else { 2 }, false),
                (None, Some(t)) => (t, 2, true),
                (None, None) => break,
            };
            let content_start = from + rel + olen;
            let Some(crel) = line[content_start..].find(if is_tag { "%}" } else { "}}" }) else { break };
            let close_b = content_start + crel;
            let content = &line[content_start..close_b];
            let first_word = content.split_whitespace().next().unwrap_or("");
            // Los identificadores del contenido, con su offset en bytes.
            let mut idents: Vec<(usize, &str)> = Vec::new();
            let mut it = content.char_indices().peekable();
            while let Some((i, c)) = it.next() {
                if c.is_alphabetic() || c == '_' {
                    let mut end = i + c.len_utf8();
                    while let Some(&(j, d)) = it.peek() {
                        if is_ident_char(d) { end = j + d.len_utf8(); it.next(); } else { break }
                    }
                    idents.push((i, &content[i..end]));
                } else if c.is_ascii_digit() {
                    // Un número no arranca un ident; consumir sus dígitos evita `0..2` raro.
                    while it.peek().is_some_and(|&(_, d)| is_ident_char(d)) { it.next(); }
                }
            }
            // En un `{% for a, (k, v) in expr %}`: los idents entre `for` y el `in` son DECLS.
            let in_idx = (is_tag && first_word == "for")
                .then(|| idents.iter().position(|(_, n)| *n == "in").unwrap_or(idents.len()))
                .unwrap_or(0);
            let mut group: Vec<(String, String)> = Vec::new();
            if is_tag && matches!(first_word, "import" | "extends" | "block") {
                from = close_b + 2;
                continue; // rutas de import/extends y nombres de bloque no son variables
            }
            // En `{% include ruta(args) %}` la RUTA no liga (solo los args son expresión).
            let idents_from = if is_tag && first_word == "include"
                && content.trim_start().strip_prefix("include")
                    .is_some_and(|r| crate::templ::template_ref(r.trim()).is_some())
            {
                content.find('(').map(|i| i + 1).unwrap_or(0)
            } else {
                0
            };
            for (k, (off, name)) in idents.iter().enumerate() {
                if *off < idents_from && !(k == 0 && *name == "include") {
                    continue; // la ruta del include: no liga
                }
                if content[..*off].trim_end().ends_with('.') {
                    continue; // miembro (`fila.trim`): no liga a variables
                }
                let col0 = line[..content_start + off].chars().count();
                let len = name.chars().count();
                if is_tag && first_word == "params" {
                    // Declaración de param: el ident seguido de `:` (los nombres de TIPO no ligan).
                    let is_decl = k > 0
                        && content[off + name.len()..].trim_start().starts_with(':')
                        && params.iter().any(|p| p == name);
                    if is_decl {
                        out.push((l0, col0, len, format!("p:{name}"), true));
                    }
                    continue;
                }
                if is_tag && first_word == "for" && k >= 1 && k < in_idx {
                    let key = format!("f:{l0}:{col0}");
                    out.push((l0, col0, len, key.clone(), true));
                    group.push((name.to_string(), key));
                    continue;
                }
                // `{% let x = expr %}`: el ident tras `let` (k==1) es la declaración; vive en el
                // grupo ABIERTO (nivel superior o el for envolvente), así muere con su endfor.
                if is_tag && first_word == "let" && k == 1 {
                    let key = format!("l:{l0}:{col0}");
                    out.push((l0, col0, len, key.clone(), true));
                    for_stack
                        .last_mut()
                        .unwrap_or_else(|| crate::ice!("for_stack se inicializa con el marco base"))
                        .push((name.to_string(), key));
                    continue;
                }
                if keywords.contains(name) {
                    continue;
                }
                // Uso: la variable de for más interna con ese nombre gana; si no, un param.
                if let Some((_, key)) = for_stack.iter().rev().flatten().find(|(v, _)| v == name) {
                    out.push((l0, col0, len, key.clone(), false));
                } else if params.iter().any(|p| p == name) {
                    out.push((l0, col0, len, format!("p:{name}"), false));
                }
            }
            if is_tag && first_word == "for" {
                for_stack.push(group);
            } else if is_tag && first_word == "endfor" && for_stack.len() > 1 {
                for_stack.pop();
            }
            from = close_b + 2;
        }
    }
    out
}

/// La ocurrencia del template que contiene la posición `(line0, char0)`, si la hay.
pub(super) fn template_occurrence_at(occs: &[TplOcc], line0: usize, char0: usize) -> Option<&TplOcc> {
    occs.iter().find(|(l, c, len, _, _)| *l == line0 && char0 >= *c && char0 < *c + *len)
}

/// Hover semántico en un template: la posición se traduce al módulo generado y el hover corre ahí
/// (tipos REALES del checker: `fila.precio` → `float`). El rango devuelto es el del identificador
/// en el TEMPLATE (las columnas del generado no significan nada para el editor).
pub(super) fn template_semantic_hover(uri: &str, src: &str, line0: usize, char0: usize) -> Option<(String, usize, usize)> {
    let (name, start, end) = ident_range_under_cursor(src, line0, char0)?;
    let (code, map, gen_uri) = template_generated(uri, src)?;
    let (gl, gc) = template_pos_to_generated(src, &code, &map, line0, char0)?;
    let (info, _, _) = hover_at(Some(&gen_uri), &code, gl, gc)?;
    // El hover debe ser DEL identificador bajo el cursor: un nombre sin entrada propia (un typo no
    // declarado) puede caer dentro del rango de un nodo envolvente con `len` namespacado (p. ej.
    // `std::template::escape_html` cubre sus argumentos) — eso no es un hover de este símbolo.
    let subject = info.split(':').next().unwrap_or("");
    if subject != name && !subject.ends_with(&format!(".{name}")) {
        return None;
    }
    Some((info, start, end))
}

/// Ir-a-definición desde un template: la posición se traduce al generado y se resuelve ahí. Una
/// declaración que cae en OTRO archivo (un struct importado, una función del proyecto) se devuelve
/// tal cual; una que cae en el propio generado (un param, una var de for) se traduce de vuelta al
/// template con el line map (p. ej. un param lleva a la línea del `{% params %}`).
pub(super) fn template_definition(uri: &str, src: &str, line0: usize, char0: usize) -> Option<Json> {
    let (code, map, gen_uri) = template_generated(uri, src)?;
    let (gl, gc) = template_pos_to_generated(src, &code, &map, line0, char0)?;
    let (target_uri, dl0, dc0, len) = definition_at(&gen_uri, &code, gl, gc)?;
    if target_uri != gen_uri {
        return Some(obj(vec![
            ("uri", Json::Str(target_uri)),
            ("range", range(dl0, dc0, dc0 + len)),
        ]));
    }
    // Dentro del generado → la línea del template. La columna se relocaliza buscando el nombre en
    // esa línea (las columnas del generado no se traducen); si no aparece, el inicio de la línea.
    let glines: Vec<&str> = code.split('\n').collect();
    let name = token_at(&glines, dl0, dc0).unwrap_or_default();
    let tl0 = map.get(dl0).copied().unwrap_or(1).max(1) - 1;
    let (start, end) = src.lines().nth(tl0)
        .and_then(|l| find_frag(l, &name).map(|b| {
            let s = l[..b].chars().count();
            (s, s + name.chars().count())
        }))
        .unwrap_or((0, 1));
    Some(obj(vec![
        ("uri", Json::Str(uri.to_string())),
        ("range", range(tl0, start, end)),
    ]))
}

/// Los nombres en ámbito local (params + `let`/`var`) de la función que contiene `cursor_line`
/// (1-basado), declarados en o antes de esa línea (M10.2f). Sin spans, el alcance es la **función**
/// envolvente (la de mayor línea de inicio que no la supera), no el bloque exacto: degradación honesta.
pub(super) fn scope_locals(program: &crate::ast::Program, cursor_line: usize) -> Vec<String> {
    let Some(f) = program.functions.iter()
        .filter(|f| f.line <= cursor_line && !f.name.contains('#') && !f.name.contains("::"))
        .max_by_key(|f| f.line)
    else {
        return vec![];
    };
    let mut out: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
    collect_lets(&f.body, cursor_line, &mut out);
    out
}

/// Recolecta los nombres de `let`/`var` de un bloque (recursivo en bloques anidados) cuya línea no
/// supere `cursor_line`.
pub(super) fn collect_lets(block: &crate::ast::Block, cursor_line: usize, out: &mut Vec<String>) {
    use crate::ast::{ExprKind, StmtKind};
    fn visit_expr(e: &crate::ast::Expr, cursor_line: usize, out: &mut Vec<String>) {
        match &e.kind {
            ExprKind::If { then_branch, else_branch, .. } => {
                collect_lets(then_branch, cursor_line, out);
                if let Some(eb) = else_branch { visit_expr(eb, cursor_line, out); }
            }
            ExprKind::While { body, .. } => collect_lets(body, cursor_line, out),
            ExprKind::Block(b) => collect_lets(b, cursor_line, out),
            ExprKind::Match { arms, .. } => {
                for arm in arms { visit_expr(&arm.body, cursor_line, out); }
            }
            _ => {}
        }
    }
    for stmt in &block.statements {
        if let StmtKind::Let { name, value, .. } = &stmt.kind {
            if stmt.line <= cursor_line { out.push(name.clone()); }
            visit_expr(value, cursor_line, out);
        } else if let StmtKind::Expr(e) = &stmt.kind {
            visit_expr(e, cursor_line, out);
        }
    }
    if let Some(t) = &block.tail {
        visit_expr(t, cursor_line, out);
    }
}

/// El `result` de `textDocument/signatureHelp` (M10.2f): la firma de la función cuya llamada se está
/// escribiendo bajo el cursor, con el parámetro activo resaltado. `null` si no hay una llamada en
/// curso o la función no se conoce.
pub(super) fn signature_help_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Null };
    let Some(src) = docs.get(&uri) else { return Json::Null };
    if is_template_uri(&uri) {
        // M55: la posición se traduce al módulo generado y el signature help corre ahí (la firma
        // es información, no posiciones → no hay nada que traducir de vuelta).
        if let Some((code, map, gen_uri)) = template_generated(&uri, src)
            && let Some((gl, gc)) = template_pos_to_generated(src, &code, &map, line0, char0)
        {
            return signature_help_at(&gen_uri, &code, gl, gc);
        }
        return Json::Null;
    }
    signature_help_at(&uri, src, line0, char0)
}

/// El signature help en una posición concreta de una fuente raylang (extraído de
/// `signature_help_result` para que los templates lo reusen sobre su módulo generado).
pub(super) fn signature_help_at(uri: &str, src: &str, line0: usize, char0: usize) -> Json {
    // 1. Hallar la llamada en curso: el nombre, cuántas comas la preceden (param activo) y el receptor
    //    si es una llamada por punto (`recv.m(`).
    let Some((name, active, receiver)) = enclosing_call(src, line0, char0) else { return Json::Null };
    // 2. Resolver la firma con el resolutor unificado (M46b): buffer + módulos importados + prelude +
    //    builtins. Textual → robusto ante el documento a medio escribir (solo exige que la
    //    *declaración* `fn name(...) -> ...` esté bien formada). Así el signature help funciona
    //    también para funciones importadas (`u.cuadrado(`) y del prelude, no solo las del archivo.
    let ctx = SigCtx::new(src, uri_to_path(uri).as_deref());
    // M48.1: función asociada de un tipo incorporado (`Map.new(`/`Channel.bounded(`): la firma sale del
    // registro (`assoc.sig`), no de una `fn`.
    if let Some(recv) = receiver.as_deref()
        && let Some(a) = crate::builtins::assoc_lookup(recv, &name)
    {
        let params = assoc_param_names(a.sig);
        let parameters: Vec<Json> = params.iter().map(|p| obj(vec![("label", Json::Str(p.clone()))])).collect();
        let active = active.min(params.len().saturating_sub(1));
        let signature = obj(vec![("label", Json::Str(a.sig.to_string())), ("parameters", Json::Arr(parameters))]);
        return obj(vec![
            ("signatures", Json::Arr(vec![signature])),
            ("activeSignature", num(0)),
            ("activeParameter", num(active as i64)),
        ]);
    }
    // La construcción de una variante de enum (`Figura.Circulo(`) no es una `fn`: si el receptor es un
    // enum con esa variante, se arma la firma con los tipos del payload (`Figura.Circulo(float, …)`).
    if let Some(recv) = receiver.as_deref()
        && ctx.signature(&name).is_none()
        && let Some(variants) = ctx.enum_variants(recv)
        && let Some(v) = variants.iter().find(|v| v.name == name)
    {
        let params: Vec<String> = v.payload.iter().map(|t| format!("{}", t)).collect();
        let label = format!("{}.{}({})", recv, name, params.join(", "));
        let parameters: Vec<Json> = params.iter().map(|p| obj(vec![("label", Json::Str(p.clone()))])).collect();
        let active = active.min(params.len().saturating_sub(1));
        let signature = obj(vec![("label", Json::Str(label)), ("parameters", Json::Arr(parameters))]);
        return obj(vec![
            ("signatures", Json::Arr(vec![signature])),
            ("activeSignature", num(0)),
            ("activeParameter", num(active as i64)),
        ]);
    }
    let Some((mut params, ret)) = ctx.signature(&name) else { return Json::Null };
    // En un **método** (`recv.m(args)` con `recv` un valor) el receptor es implícito → se recorta el
    // primer parámetro para que el `activeParameter` (que cuenta los args visibles) case. Un receptor
    // que es un **módulo** importado (`u.cuadrado(`) NO es un método: se muestra la firma completa.
    let is_method = receiver.as_deref().is_some_and(|r| !is_imported_module(src, r));
    if is_method && !params.is_empty() {
        params.remove(0);
    }
    // 3. Construir el label `fn name(p: T, …) -> R` y la lista de parámetros (para resaltar).
    let label = format!("fn {}({}) -> {}", name, params.join(", "), ret);
    let parameters: Vec<Json> = params.iter().map(|p| obj(vec![("label", Json::Str(p.clone()))])).collect();
    let active = active.min(params.len().saturating_sub(1));
    let signature = obj(vec![
        ("label", Json::Str(label)),
        ("parameters", Json::Arr(parameters)),
    ]);
    obj(vec![
        ("signatures", Json::Arr(vec![signature])),
        ("activeSignature", num(0)),
        ("activeParameter", num(active as i64)),
    ])
}

/// Extrae la firma de `fn <name>` de la fuente, textualmente (M10.2f): devuelve `(params, retorno)`
/// donde `params` son las cadenas `nombre: Tipo` y `retorno` el tipo de retorno (`unit` si no hay
/// `->`). Textual a propósito: funciona aunque el resto del archivo no parsee (el caso normal al
/// escribir argumentos). Solo exige que la **declaración** esté bien formada.
pub(super) fn find_fn_signature(src: &str, name: &str) -> Option<(Vec<String>, String)> {
    let cs: Vec<char> = src.chars().collect();
    let needle: Vec<char> = format!("fn {}", name).chars().collect();
    let mut i = 0;
    while i + needle.len() <= cs.len() {
        if cs[i..i + needle.len()] != needle[..] {
            i += 1;
            continue;
        }
        let mut j = i + needle.len();
        // Frontera de palabra: el char tras el nombre no debe ser de identificador (evita `sumar`).
        if cs.get(j).is_some_and(|c| is_ident_char(*c)) {
            i += 1;
            continue;
        }
        // Saltar genéricos `<…>` y espacios hasta el `(` de los parámetros.
        while j < cs.len() && cs[j] != '(' && cs[j] != '{' {
            j += 1;
        }
        if cs.get(j) != Some(&'(') {
            i += 1;
            continue;
        }
        // Leer los parámetros entre paréntesis equilibrados.
        let pstart = j;
        let mut depth = 0;
        while j < cs.len() {
            match cs[j] {
                '(' => depth += 1,
                ')' => { depth -= 1; if depth == 0 { break; } }
                _ => {}
            }
            j += 1;
        }
        if cs.get(j) != Some(&')') {
            return None;
        }
        let params_text: String = cs[pstart + 1..j].iter().collect();
        // Retorno: desde tras `)` hasta `{` (o fin de línea).
        let mut k = j + 1;
        let rstart = k;
        while k < cs.len() && cs[k] != '{' && cs[k] != '\n' {
            k += 1;
        }
        let ret_text: String = cs[rstart..k].iter().collect();
        let ret = ret_text.trim().trim_start_matches("->").trim().to_string();
        let params = split_top_commas(&params_text);
        return Some((params, if ret.is_empty() { "unit".to_string() } else { ret }));
    }
    None
}

/// Contexto para resolver la firma legible de una función (M46a): las fuentes donde buscar su
/// declaración `fn` —el buffer, los módulos importados (leídos de disco) y el prelude—. Se construye
/// **una vez** por petición de completion y se consulta por nombre. Textual (tolera archivo a medio
/// escribir); reusa `find_fn_signature`/`builtins::signature`, las mismas que el signature help.
pub(super) struct SigCtx {
    sources: Vec<String>,
}

/// Las rutas de módulo que `src` importa o reexporta (`import M;` / `[pub] from M import …`), para
/// el cierre transitivo de `SigCtx`.
pub(super) fn imported_paths(src: &str) -> Vec<String> {
    let Ok(tokens) = lexer::lex(src) else { return Vec::new() };
    let (program, _) = parser::parse_all(tokens);
    let mut paths: Vec<String> = program.imports.iter().map(|d| d.module.clone()).collect();
    paths.extend(program.from_imports.iter().map(|f| f.module.clone()));
    paths
}

impl SigCtx {
    /// Construye el contexto para `entry` con buffer `src`: buffer + fuentes de los módulos que
    /// importa (`import`/`from`) + prelude.
    fn new(src: &str, entry: Option<&Path>) -> SigCtx {
        let mut sources = vec![src.to_string()];
        if let Some(entry) = entry {
            let roots = import_roots(entry);
            // Cierre TRANSITIVO de imports (BFS): además de los módulos que el archivo importa, se
            // siguen los que ELLOS importan/reexportan. Necesario para las cápsulas: `import geo;`
            // trae `geo/mod.ray`, cuya `pub from geo/formas/circulo import area` apunta a la
            // definición real de `area` en `circulo.ray` — que así también entra al contexto.
            let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut queue: Vec<String> = imported_paths(src);
            while let Some(r) = queue.pop() {
                if !visited.insert(r.clone()) {
                    continue;
                }
                if let Ok(Some(path)) = loader::resolve_module_path(&roots, &r) {
                    if let Ok(source) = std::fs::read_to_string(&path) {
                        queue.extend(imported_paths(&source));
                        sources.push(source);
                    }
                }
            }
        }
        sources.push(crate::prelude::SOURCE.to_string());
        SigCtx { sources }
    }

    /// Los campos `(nombre, tipo)` del struct `name` (completion de literal `Nombre { … }`, M47a),
    /// buscándolo en las fuentes del contexto (buffer + módulos importados/reexportados). `None` si
    /// no hay un struct con ese nombre.
    fn struct_fields(&self, name: &str) -> Option<Vec<(String, String)>> {
        for source in &self.sources {
            if let Ok(tokens) = lexer::lex(source) {
                let (program, _) = parser::parse_all(tokens);
                if let Some(s) = program.structs.iter().find(|s| s.name == name) {
                    return Some(s.fields.iter().map(|(n, t)| (n.clone(), format!("{}", t))).collect());
                }
            }
        }
        None
    }

    /// Las variantes del enum `name` (completion `Enum.`), buscándolo en las fuentes del contexto
    /// (buffer + módulos importados/reexportados). `None` si no hay un enum con ese nombre.
    fn enum_variants(&self, name: &str) -> Option<Vec<crate::ast::VariantDef>> {
        for source in &self.sources {
            if let Ok(tokens) = lexer::lex(source) {
                let (program, _) = parser::parse_all(tokens);
                if let Some(e) = program.enums.iter().find(|e| e.name == name) {
                    return Some(e.variants.clone());
                }
            }
        }
        None
    }

    /// La firma `(params_con_nombre, retorno)` de `name`: builtin, o la primera declaración `fn name`
    /// hallada en las fuentes. `None` si no se encuentra.
    fn signature(&self, name: &str) -> Option<(Vec<String>, String)> {
        if let Some((ps, r)) = crate::builtins::signature(name) {
            return Some((ps.iter().map(|p| p.to_string()).collect(), r.to_string()));
        }
        self.sources.iter().find_map(|f| find_fn_signature(f, name))
    }
}

/// El cuerpo del snippet de argumentos a partir de los params `["p: P", "k: int"]` (M46c):
/// `${1:p}, ${2:k}` —solo el **nombre** como placeholder, para teclear encima y recorrer con Tab—.
/// Vacío si no hay params (→ `nombre()`).
pub(super) fn snippet_args(params: &[String]) -> String {
    params.iter().enumerate()
        .map(|(i, p)| {
            let name = p.split(':').next().unwrap_or(p).trim();
            format!("${{{}:{}}}", i + 1, name)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// El `insertText` (snippet) de una llamada (M46c): con los params resueltos, `nombre(${1:p}, …)`
/// (placeholders navegables); sin firma pero con args, `nombre($0)` (cursor dentro); si no, `nombre()`.
pub(super) fn insert_call(label: &str, params: Option<&[String]>, has_args: bool) -> String {
    match params {
        Some(ps) if !ps.is_empty() => format!("{}({})", label, snippet_args(ps)),
        Some(_) => format!("{}()", label),               // firma conocida, sin argumentos
        None if has_args => format!("{}($0)", label),    // firma desconocida, con args → cursor dentro
        None => format!("{}()", label),
    }
}

/// M48.1: los parámetros de una función asociada, extraídos textualmente de su firma del registro
/// (`Channel.bounded(n: int) -> Channel<T>` → `["n: int"]`; `Map.new() -> Map<K, V>` → `[]`). Toma el
/// contenido entre el primer `(` y su `)` de cierre y lo parte por comas de nivel superior.
pub(super) fn assoc_param_names(sig: &str) -> Vec<String> {
    let cs: Vec<char> = sig.chars().collect();
    let Some(open) = cs.iter().position(|&c| c == '(') else { return Vec::new() };
    let (mut depth, mut end) = (0i32, open);
    for (i, &c) in cs.iter().enumerate().skip(open) {
        match c {
            '(' => depth += 1,
            ')' => { depth -= 1; if depth == 0 { end = i; break; } }
            _ => {}
        }
    }
    let inside: String = cs[open + 1..end].iter().collect();
    split_top_commas(&inside)
}

/// Empuja el detalle de una firma **ya resuelta** (M46a): `labelDetails.detail` (params) +
/// `labelDetails.description` (retorno) + `detail` (panel). Si `metodo`, recorta el receptor.
pub(super) fn push_signature_raw(fields: &mut Vec<(&'static str, Json)>, mut params: Vec<String>, ret: String, is_method: bool) {
    if is_method && !params.is_empty() {
        params.remove(0); // el receptor
    }
    let inline = format!("({})", params.join(", "));
    fields.push(("labelDetails", obj(vec![
        ("detail", Json::Str(inline.clone())),
        ("description", Json::Str(ret.clone())),
    ])));
    fields.push(("detail", Json::Str(format!("{} -> {}", inline, ret))));
}

/// Parte una lista de parámetros por comas de **nivel superior** (ignora las anidadas en `<…>` o
/// `(…)`, p. ej. `f: fn(int) -> int`). Recorta los espacios; descarta los vacíos.
pub(super) fn split_top_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    let cs: Vec<char> = s.chars().collect();
    for (i, c) in cs.iter().enumerate() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                let p: String = cs[start..i].iter().collect();
                if !p.trim().is_empty() { out.push(p.trim().to_string()); }
                start = i + 1;
            }
            _ => {}
        }
    }
    let p: String = cs[start..].iter().collect();
    if !p.trim().is_empty() { out.push(p.trim().to_string()); }
    out
}

/// Halla la llamada que se está escribiendo en `(line0, char0)` (0-basado): el nombre de la función
/// y el nº de comas de nivel superior antes del cursor (el índice del parámetro activo). Escanea el
/// texto hasta el cursor hacia atrás contando paréntesis: el primer `(` sin cerrar abre la llamada
/// actual, y el identificador inmediatamente anterior es su nombre.
pub(super) fn enclosing_call(src: &str, line0: usize, char0: usize) -> Option<(String, usize, Option<String>)> {
    // Prefijo: todo el texto desde el inicio hasta el cursor.
    let mut prefix = String::new();
    for (i, line) in src.lines().enumerate() {
        use std::cmp::Ordering::*;
        match i.cmp(&line0) {
            Less => { prefix.push_str(line); prefix.push('\n'); }
            Equal => { prefix.extend(line.chars().take(char0)); break; }
            Greater => break,
        }
    }
    let cs: Vec<char> = prefix.chars().collect();
    let (mut depth, mut commas, mut i) = (0i32, 0usize, cs.len());
    while i > 0 {
        i -= 1;
        match cs[i] {
            ')' => depth += 1,
            '(' if depth == 0 => return ident_before(&cs, i).map(|(n, r)| (n, commas, r)),
            '(' => depth -= 1,
            ',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    None
}

/// El identificador que termina justo antes del índice `i` (saltando espacios), y —si es una llamada
/// por **punto** (`recv.nombre`)— el **receptor** (`recv`). `None` (el receptor) si no hay `.`. Se usa
/// para el signature help: si el receptor es un **valor** se recorta el primer parámetro (es un
/// método); si es un **módulo** importado, no (llamada calificada, M46b).
pub(super) fn ident_before(cs: &[char], i: usize) -> Option<(String, Option<String>)> {
    let mut j = i;
    while j > 0 && cs[j - 1].is_whitespace() {
        j -= 1;
    }
    let end = j;
    while j > 0 && is_ident_char(cs[j - 1]) {
        j -= 1;
    }
    if j == end {
        return None;
    }
    let name: String = cs[j..end].iter().collect();
    // Un identificador no empieza por dígito (descarta restos como `42`).
    if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    // ¿Llamada por punto? El primer char no-espacio antes del nombre es un `.`; si lo hay, el
    // identificador anterior es el receptor.
    let mut k = j;
    while k > 0 && cs[k - 1].is_whitespace() {
        k -= 1;
    }
    let receiver = if k > 0 && cs[k - 1] == '.' {
        let mut b = k - 1; // el `.`
        while b > 0 && cs[b - 1].is_whitespace() {
            b -= 1;
        }
        let rfin = b;
        while b > 0 && is_ident_char(cs[b - 1]) {
            b -= 1;
        }
        (b < rfin).then(|| cs[b..rfin].iter().collect::<String>())
    } else {
        None
    };
    Some((name, receiver))
}

/// ¿`name` es el **leaf** de algún `import` del archivo (un módulo calificable, no un valor)?
/// (M46b: un `u.cuadrado(` no recorta el receptor, a diferencia de un método `p.doblar(`.)
pub(super) fn is_imported_module(src: &str, name: &str) -> bool {
    lexer::lex(src)
        .ok()
        .map(|t| {
            let (program, _) = parser::parse_all(t);
            program.imports.iter().any(|d| d.leaf() == name)
        })
        .unwrap_or(false)
}

/// Lee un `Json::Num` como `usize` (las posiciones LSP son enteros).
pub(super) fn as_usize(j: &Json) -> Option<usize> {
    match j {
        Json::Num(n) => Some(*n as usize),
        _ => None,
    }
}

// ── Formateo del documento ───────────────────────────────────────────────────────────

/// El `result` de `textDocument/formatting`: un único `TextEdit` que reemplaza el documento entero
/// por su versión formateada. Reusa `fmt::format_source` —**el mismo** formateador que `ray fmt`—,
/// así el LSP y la CLI dan idéntico resultado. Lista vacía si el documento **no parsea** (no se
/// formatea código inválido; el editor conserva lo escrito) o si ya está formateado (nada que tocar).
pub(super) fn formatting_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let params = msg.get("params");
    let uri = params.and_then(|p| p.get("textDocument")).and_then(|t| t.get("uri")).and_then(|u| u.as_str());
    let Some(src) = uri.and_then(|u| docs.get(u)) else { return Json::Arr(vec![]) };
    // Honramos la preferencia de indentación del EDITOR (LSP `options.tabSize`/`insertSpaces`), en vez
    // de imponer siempre 4 espacios: así "Format File" respeta 2 espacios/tabs si así está el editor
    // (Sublime los deriva de su config, incl. `.editorconfig` si tiene ese plugin). El `ray fmt` de
    // consola sigue canónico. Por defecto (sin opciones) → 4 espacios (idéntico al canónico).
    let opts = params.and_then(|p| p.get("options"));
    let insert_spaces = opts.and_then(|o| o.get("insertSpaces")).map(|b| matches!(b, Json::Bool(true))).unwrap_or(true);
    let tab_size = opts.and_then(|o| o.get("tabSize")).and_then(as_usize).filter(|&n| n > 0).unwrap_or(4);
    let unit = if insert_spaces { " ".repeat(tab_size) } else { "\t".to_string() };
    // M55: un template se formatea con SU formateador (etiquetas en su línea + indentación por
    // bloques del template); un buffer que no tokeniza no se toca.
    let formatted = if uri.is_some_and(is_template_uri) {
        match crate::templ::format_template(src, &unit) {
            Some(f) => f,
            None => return Json::Arr(vec![]),
        }
    } else {
        match crate::fmt::format_source_with_indent(src, &unit) {
            Ok(f) => f,
            Err(_) => return Json::Arr(vec![]),
        }
    };
    if formatted == *src {
        return Json::Arr(vec![]);
    }
    // Un edit que cubre TODO el buffer: de (0,0) al final. `split('\n')` incluye el último segmento
    // (vacío si el texto termina en '\n'), así el rango abarca exactamente el documento.
    let segs: Vec<&str> = src.split('\n').collect();
    let last_line = segs.len().saturating_sub(1);
    let last_col = segs.last().map(|l| l.chars().count()).unwrap_or(0);
    let pos = |l: usize, c: usize| obj(vec![("line", num(l as i64)), ("character", num(c as i64))]);
    let range = obj(vec![("start", pos(0, 0)), ("end", pos(last_line, last_col))]);
    let edit = obj(vec![("range", range), ("newText", Json::Str(formatted))]);
    Json::Arr(vec![edit])
}

// ── Outline (documentSymbol) ─────────────────────────────────────────────────────────

/// El `result` de `textDocument/documentSymbol`: el **outline** del archivo (funciones, structs con
/// sus variantes, enums, traits e impls con sus métodos, constantes), jerárquico (`DocumentSymbol[]`).
/// Se construye del AST del **propio buffer** (`parse_all`, tolerante a errores) —no del programa
/// fusionado— para listar solo lo que el usuario escribió, sin prelude ni otros módulos. Sin spans,
/// el rango de cada símbolo es el de su **nombre** (una línea); basta para el índice y para saltar.
pub(super) fn document_symbol_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let uri = msg.get("params").and_then(|p| p.get("textDocument")).and_then(|t| t.get("uri")).and_then(|u| u.as_str());
    let Some(src) = uri.and_then(|u| docs.get(u)) else { return Json::Arr(vec![]) };
    if uri.is_some_and(is_template_uri) {
        // M55: outline del template — la función `render_<stem>` como raíz (abarca el documento)
        // y, como hijos, cada param de la cabecera y cada variable de `{% for %}` (kind Variable).
        let name = uri.and_then(uri_to_path)
            .and_then(|p| crate::templ::fn_suffix_of(&p).ok())
            .map(|s| format!("render_{s}"))
            .unwrap_or_else(|| "render".to_string());
        let lines: Vec<&str> = src.split('\n').collect();
        let text_at = |l: usize, c: usize, len: usize| -> String {
            lines.get(l).map(|s| s.chars().skip(c).take(len).collect()).unwrap_or_default()
        };
        let children: Vec<Json> = template_occurrences(src).iter()
            .filter(|(_, _, _, _, is_decl)| *is_decl)
            .map(|(l, c, len, _, _)| {
                let r = range(*l, *c, *c + *len);
                obj(vec![
                    ("name", Json::Str(text_at(*l, *c, *len))),
                    ("kind", num(13)), // Variable
                    ("range", r.clone()),
                    ("selectionRange", r),
                ])
            })
            .collect();
        // El rango de la raíz debe CONTENER a los hijos → el documento entero.
        let last = lines.len().saturating_sub(1);
        let full = range_multiline(0, 0, last, lines.last().map(|s| s.chars().count()).unwrap_or(0));
        return Json::Arr(vec![obj(vec![
            ("name", Json::Str(name)),
            ("kind", num(12)), // Function
            ("range", full),
            ("selectionRange", range(0, 0, lines.first().map(|s| s.chars().count()).unwrap_or(1))),
            ("children", Json::Arr(children)),
        ])]);
    }
    let Ok(tokens) = lexer::lex(src) else { return Json::Arr(vec![]) };
    let (program, _) = parser::parse_all(tokens);
    let lines: Vec<&str> = src.lines().collect();
    // Oculta nombres sintéticos (métodos manglados, namespacados, internos). El buffer normalmente no
    // los tiene, pero es la misma salvaguarda que en completion.
    let visible = |n: &str| !n.contains('#') && !n.contains("::") && !n.starts_with("__");

    // Kinds de LSP SymbolKind: 6=Method, 10=Enum, 11=Interface, 12=Function, 14=Constant, 22=EnumMember,
    // 23=Struct, 5=Class (para el bloque `impl`).
    let mut syms: Vec<(usize, Json)> = Vec::new();
    for f in program.functions.iter().filter(|f| visible(&f.name)) {
        syms.push((f.line, doc_symbol(&lines, f.line, f.col, &f.name, 12, vec![])));
    }
    for s in program.structs.iter().filter(|s| visible(&s.name)) {
        syms.push((s.line, doc_symbol(&lines, s.line, s.col, &s.name, 23, vec![])));
    }
    for e in program.enums.iter().filter(|e| visible(&e.name)) {
        let children = e.variants.iter()
            .map(|v| doc_symbol(&lines, v.line, v.col, &v.name, 22, vec![]))
            .collect();
        syms.push((e.line, doc_symbol(&lines, e.line, e.col, &e.name, 10, children)));
    }
    for t in program.traits.iter().filter(|t| visible(&t.name)) {
        let children = t.methods.iter().filter(|m| visible(&m.name))
            .map(|m| doc_symbol(&lines, m.line, m.col, &m.name, 6, vec![]))
            .collect();
        syms.push((t.line, doc_symbol(&lines, t.line, t.col, &t.name, 11, children)));
    }
    for c in program.consts.iter().filter(|c| visible(&c.name)) {
        syms.push((c.line, doc_symbol(&lines, c.line, c.col, &c.name, 14, vec![])));
    }
    for im in &program.impls {
        let label = format!("impl {} for {}", im.trait_name, im.target);
        let children: Vec<Json> = im.methods.iter().filter(|m| visible(&m.name))
            .map(|m| doc_symbol(&lines, m.line, m.col, &m.name, 6, vec![]))
            .collect();
        // El `range`/`selectionRange` del impl apunta al nombre del trait tras `impl`.
        syms.push((im.line, doc_symbol_named(&lines, im.line, im.col, &im.trait_name, &label, 5, children)));
    }
    syms.sort_by_key(|(l, _)| *l);
    Json::Arr(syms.into_iter().map(|(_, s)| s).collect())
}

/// Un `DocumentSymbol` cuyo nombre visible **es** el identificador buscado en la fuente.
pub(super) fn doc_symbol(lines: &[&str], line: usize, col: usize, name: &str, kind: i64, children: Vec<Json>) -> Json {
    doc_symbol_named(lines, line, col, name, name, kind, children)
}

/// Un `DocumentSymbol` con `etiqueta` como nombre mostrado y `buscar` como el identificador a
/// localizar en la fuente (p. ej. un `impl` muestra "impl T for X" pero su rango apunta a `T`).
pub(super) fn doc_symbol_named(lines: &[&str], line: usize, col: usize, search_name: &str, label: &str, kind: i64, children: Vec<Json>) -> Json {
    // El rango del símbolo es el de su nombre (sin spans no tenemos la extensión completa del ítem;
    // el nombre basta para listarlo y para saltar al hacer clic). `decl_name_range` escanea desde la
    // posición de la declaración (que apunta al keyword `fn`/`struct`/… o ya al nombre).
    let (l0, c0, len) = decl_name_range(lines, line, col, search_name)
        .unwrap_or((line.saturating_sub(1), col.saturating_sub(1), search_name.chars().count().max(1)));
    let r = range(l0, c0, c0 + len);
    let mut pairs = vec![
        ("name", Json::Str(label.to_string())),
        ("kind", num(kind)),
        ("range", r.clone()),
        ("selectionRange", r),
    ];
    if !children.is_empty() {
        pairs.push(("children", Json::Arr(children)));
    }
    obj(pairs)
}

// ── Resaltado de ocurrencias (documentHighlight) ─────────────────────────────────────

/// El `result` de `textDocument/documentHighlight`: los rangos de **todas** las apariciones del
/// símbolo bajo el cursor en ESTE archivo (la declaración como *Write*, los usos como *Text*). Reusa
/// `symbol_occurrences` (el mismo motor de find-references), que ya resuelve los ámbitos y filtra a
/// la banda de la entrada. Lista vacía si no hay un símbolo bajo el cursor.
pub(super) fn document_highlight_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Arr(vec![]) };
    let Some(src) = docs.get(&uri) else { return Json::Arr(vec![]) };
    if is_template_uri(&uri) {
        // M55: resalta las apariciones del param/var de for bajo el cursor (decl = Write).
        let occs = template_occurrences(src);
        let Some(cur) = template_occurrence_at(&occs, line0, char0).cloned() else {
            return Json::Arr(vec![]);
        };
        let list: Vec<Json> = occs.iter()
            .filter(|(_, _, _, k, _)| *k == cur.3)
            .map(|(l, c, len, _, is_decl)| obj(vec![
                ("range", range(*l, *c, *c + *len)),
                ("kind", num(if *is_decl { 3 } else { 1 })),
            ]))
            .collect();
        return Json::Arr(list);
    }
    let Some((_, decl, uses, _)) = symbol_occurrences(Some(&uri), src, line0, char0) else {
        return Json::Arr(vec![]);
    };
    // DocumentHighlightKind: 1=Text, 2=Read, 3=Write. La declaración = Write; los usos = Text. Se
    // deduplica la declaración de entre los usos (si coincide) para no emitir el mismo rango dos veces.
    let mut highlights = Vec::new();
    if let Some((l, c, len)) = decl {
        highlights.push(obj(vec![("range", range(l, c, c + len)), ("kind", num(3))]));
    }
    for (l, c, len) in uses {
        if decl == Some((l, c, len)) {
            continue;
        }
        highlights.push(obj(vec![("range", range(l, c, c + len)), ("kind", num(1))]));
    }
    Json::Arr(highlights)
}
