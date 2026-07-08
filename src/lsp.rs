//! Language Server (LSP) de raylang — diagnósticos en vivo (M10.2).
//!
//! Un **Language Server** habla un protocolo (LSP) por stdin/stdout para que *cualquier*
//! editor (VSCode, Neovim, Helix…) muestre los errores del compilador mientras escribes.
//! Se escribe **una vez** y sirve a todos.
//!
//! Fiel al proyecto, esto es un **cliente externo**, como el REPL (M8.2) y el runner de
//! `@test` (M10.1): usa solo la API pública (`lex`/`parse`/`check`) y **no toca el núcleo**.
//! Y fiel a la invariante de *cero dependencias de Cargo*, el transporte es **JSON-RPC a
//! mano**: el *framing* (`Content-Length: N\r\n\r\n` + N bytes) y un mini-parser/serializador
//! JSON propios (`mod json`), todo en `std`. Más plomería, pero se *ve* el protocolo por
//! dentro —que es el punto pedagógico—.
//!
//! Alcance (M10.2): **solo diagnósticos**. `initialize` + `didOpen`/`didChange`/`didClose`
//! → `publishDiagnostics`. Sin hover ni go-to-definition (futuros; exigirían exponer una API
//! de tipos del checker y un índice de símbolos). DESIGN §19.2.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use crate::{checker, lexer, loader, parser};
use json::Json;

/// Arranca el servidor: lee mensajes de stdin y escribe respuestas a stdout hasta `exit`.
pub fn run() {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    serve(&mut reader, &mut out);
}

/// El bucle del servidor, parametrizado por los flujos para poder probarlo en memoria.
///
/// Lee un mensaje, lo despacha por su `method` y, cuando corresponde, analiza el documento
/// y publica diagnósticos. Termina al recibir `exit` o al cerrarse la entrada (EOF).
///
/// Guarda los documentos abiertos (M10.2b): una petición `hover`/`definition` trae solo la
/// `uri` y la posición, no el texto, así que el servidor debe recordarlo.
fn serve<R: BufRead, W: Write>(reader: &mut R, out: &mut W) {
    let mut docs: HashMap<String, String> = HashMap::new();
    while let Some(raw) = read_message(reader) {
        let Ok(msg) = json::parse(&raw) else {
            continue; // mensaje ilegible: lo ignoramos (un servidor robusto no se cae)
        };
        let method = msg.get("method").and_then(Json::as_str).unwrap_or("");
        match method {
            "initialize" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &initialize_response(id));
            }
            // Notificación de cortesía tras initialize: no requiere respuesta.
            "initialized" => {}
            "shutdown" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &result_message(id, Json::Null));
            }
            "exit" => break,
            "textDocument/didOpen" => {
                if let Some((uri, text)) = open_params(&msg) {
                    send(out, &diagnostics(&uri, &text));
                    docs.insert(uri, text);
                }
            }
            "textDocument/didChange" => {
                if let Some((uri, text)) = change_params(&msg) {
                    send(out, &diagnostics(&uri, &text));
                    docs.insert(uri, text);
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = close_uri(&msg) {
                    docs.remove(&uri);
                    // Limpiamos los diagnósticos del editor con una lista vacía.
                    send(out, &publish(&uri, vec![]));
                }
            }
            // M10.2b: hover — el tipo del identificador bajo el cursor.
            "textDocument/hover" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &result_message(id, hover_result(&msg, &docs)));
            }
            // M10.2b: ir-a-definición — salta del uso a su declaración.
            "textDocument/definition" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &result_message(id, definition_result(&msg, &docs)));
            }
            // M10.2c-LSP (cluster 4): find-references — todos los usos (y la declaración).
            "textDocument/references" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &result_message(id, references_result(&msg, &docs)));
            }
            // Cluster 4: rename — renombra el símbolo en todas sus apariciones.
            "textDocument/rename" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &result_message(id, rename_result(&msg, &docs)));
            }
            // Cluster 4: completion — símbolos del documento + builtins + palabras clave.
            "textDocument/completion" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &result_message(id, completion_result(&msg, &docs)));
            }
            // M10.2f: signature help — la firma de la función cuya llamada se está escribiendo.
            "textDocument/signatureHelp" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &result_message(id, signature_help_result(&msg, &docs)));
            }
            // Formateo del documento — reusa el formateador de `ray fmt` (`fmt::format_source`).
            "textDocument/formatting" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &result_message(id, formatting_result(&msg, &docs)));
            }
            // Outline / "ir a símbolo en el archivo": los ítems de nivel superior del documento.
            "textDocument/documentSymbol" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &result_message(id, document_symbol_result(&msg, &docs)));
            }
            // Resaltar todas las apariciones del símbolo bajo el cursor (reusa `symbol_occurrences`).
            "textDocument/documentHighlight" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &result_message(id, document_highlight_result(&msg, &docs)));
            }
            // Petición desconocida (lleva `id`) → error JSON-RPC. Notificación → se ignora.
            _ => {
                if let Some(id) = msg.get("id") {
                    send(out, &method_error(id.clone(), method));
                }
            }
        }
    }
}

// ── Framing LSP ──────────────────────────────────────────────────────────────────────
//
// Cada mensaje es: cabeceras (líneas `Clave: valor\r\n`), una línea en blanco, y el cuerpo
// JSON de exactamente `Content-Length` bytes. Es lo único que LSP añade sobre JSON-RPC.

/// Lee un mensaje completo: las cabeceras hasta la línea en blanco y luego el cuerpo de
/// `Content-Length` bytes. `None` si la entrada se cerró (EOF) o el marco es inválido.
fn read_message<R: BufRead>(reader: &mut R) -> Option<String> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None; // EOF
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // fin de cabeceras
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse().ok();
        }
        // Otras cabeceras (p. ej. Content-Type) se ignoran.
    }
    let len = content_length?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).ok()?;
    String::from_utf8(buf).ok()
}

/// Escribe un mensaje: la cabecera `Content-Length` (en bytes) y el cuerpo JSON.
fn send<W: Write>(out: &mut W, payload: &Json) {
    let body = payload.serialize();
    let _ = write!(out, "Content-Length: {}\r\n\r\n{}", body.len(), body);
    let _ = out.flush();
}

// ── Extracción de parámetros de los mensajes entrantes ───────────────────────────────

/// `(uri, texto)` de un `didOpen`: `params.textDocument.{uri,text}`.
fn open_params(msg: &Json) -> Option<(String, String)> {
    let td = msg.get("params")?.get("textDocument")?;
    let uri = td.get("uri")?.as_str()?.to_string();
    let text = td.get("text")?.as_str()?.to_string();
    Some((uri, text))
}

/// `(uri, texto)` de un `didChange`. Con *Full sync*, el último `contentChange` trae el
/// documento completo en su campo `text`.
fn change_params(msg: &Json) -> Option<(String, String)> {
    let params = msg.get("params")?;
    let uri = params.get("textDocument")?.get("uri")?.as_str()?.to_string();
    let changes = params.get("contentChanges")?.as_array()?;
    let text = changes.last()?.get("text")?.as_str()?.to_string();
    Some((uri, text))
}

/// La `uri` de un `didClose`: `params.textDocument.uri`.
fn close_uri(msg: &Json) -> Option<String> {
    Some(msg.get("params")?.get("textDocument")?.get("uri")?.as_str()?.to_string())
}

/// `(uri, línea, carácter)` (0-basados) de una petición posicional (`hover`/`definition`):
/// `params.textDocument.uri` + `params.position.{line,character}`.
fn pos_params(msg: &Json) -> Option<(String, usize, usize)> {
    let params = msg.get("params")?;
    let uri = params.get("textDocument")?.get("uri")?.as_str()?.to_string();
    let pos = params.get("position")?;
    let line = as_usize(pos.get("line")?)?;
    let character = as_usize(pos.get("character")?)?;
    Some((uri, line, character))
}

// ── Hover (M10.2b) ───────────────────────────────────────────────────────────────────

/// El `result` de un `textDocument/hover`: la firma/tipo del identificador bajo el cursor y, si el
/// símbolo tiene **comentarios de documentación** (`///` encima de su declaración), también la
/// documentación —renderizada como Markdown, con la firma en un bloque de código—. `null` si no hay
/// identificador bajo el cursor.
fn hover_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Null };
    let Some(src) = docs.get(&uri) else { return Json::Null };
    // Documentación: se localiza la DECLARACIÓN del símbolo (cruza archivos, como ir-a-definición) y
    // se escanean los `///` que la preceden en su propio archivo. Reusa `raydoc::doc_lineas_arriba`.
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
fn doc_builtin_type(name: &str) -> Option<&'static str> {
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
fn ident_range_under_cursor(src: &str, line0: usize, char0: usize) -> Option<(String, usize, usize)> {
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
fn doc_of_symbol(uri: &str, src: &str, line0: usize, char0: usize, docs: &HashMap<String, String>) -> Option<String> {
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
            if let Some(ls) = crate::raydoc::doc_lineas_arriba(&lines, def_line0 + 1) {
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
fn loaded_module_source(entry_uri: &str, entry_src: &str, target_uri: &str) -> Option<String> {
    let entry_path = uri_to_path(entry_uri)?;
    let target_path = uri_to_path(target_uri)?;
    let loaded = load(&entry_path, entry_src).ok()?;
    loaded.modules.into_iter().find(|m| m.path == target_path).map(|m| m.source)
}

/// El identificador bajo el cursor `(line0, char0)` (0-basados), expandiendo a izquierda y derecha
/// sobre caracteres de identificador. `None` si el cursor no está sobre uno.
fn ident_under_cursor(src: &str, line0: usize, char0: usize) -> Option<String> {
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
/// declaración no vive en el archivo abierto, así que no vale `doc_lineas_arriba` sobre el buffer.
fn doc_in_prelude(name: &str) -> Option<String> {
    let lines: Vec<&str> = crate::prelude::SOURCE.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        let l = l.trim_start();
        // Declaraciones de nivel superior y métodos de trait/impl: `fn nombre(`/`fn nombre<`,
        // `enum/struct/trait Nombre`.
        let is_decl = ["fn ", "enum ", "struct ", "trait "].iter().any(|kw| {
            l.strip_prefix(kw).is_some_and(|rest| {
                rest.starts_with(name)
                    && !rest[name.len()..].chars().next().is_some_and(is_ident_char)
            })
        });
        if is_decl && let Some(ls) = crate::raydoc::doc_lineas_arriba(&lines, i + 1) {
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
fn index_for(uri: Option<&str>, src: &str) -> Option<checker::SemanticIndex> {
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
fn hover_at(uri: Option<&str>, src: &str, line0: usize, char0: usize) -> Option<(String, usize, usize)> {
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
fn facade_name(serialized: &str, imports: &[(String, String)]) -> String {
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
fn imports_of(src: &str) -> Vec<(String, String)> {
    let Ok(tokens) = lexer::lex(src) else { return Vec::new() };
    let (program, _errs) = parser::parse_all(tokens);
    program.imports.iter()
        .map(|imp| (imp.leaf().to_string(), imp.module.replace('/', "::")))
        .collect()
}

/// Longitud del identificador que empieza en `(line0, col0)` (0-basados) en la fuente: cuántos
/// caracteres de identificador consecutivos hay. Sirve para no subrayar de más cuando el índice
/// trae un nombre namespacado más largo que el token escrito.
fn token_len(src: &str, line0: usize, col0: usize) -> usize {
    let Some(line) = src.lines().nth(line0) else { return 0 };
    line.chars().skip(col0).take_while(|&c| is_ident_char(c)).count()
}

/// Un `range` LSP en una sola línea (0-basada), de la columna `start` a `end` (0-basadas).
fn range(line0: usize, start: usize, end: usize) -> Json {
    let pos = |ch: usize| obj(vec![("line", num(line0 as i64)), ("character", num(ch as i64))]);
    obj(vec![("start", pos(start)), ("end", pos(end))])
}

// ── Ir-a-definición (M10.2b) ─────────────────────────────────────────────────────────

/// El `result` de un `textDocument/definition`: una `Location` (uri + rango de la declaración), o
/// `null` si el identificador no tiene declaración conocida (método, builtin, tipo). La declaración
/// puede estar en **otro archivo** (M10.2h): se resuelve el import y se devuelve el URI del módulo
/// donde vive, con su línea local.
fn definition_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Null };
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
fn definition_at(uri: &str, src: &str, line0: usize, char0: usize) -> Option<(String, usize, usize, usize)> {
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

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Un rango en una línea: `(line0, col0, len)`, todo 0-basado.
type Span = (usize, usize, usize);

/// El identificador escrito en `(line0, col0)` (0-basados) de `lines`, o `None` si ahí no empieza uno.
fn token_at(lines: &[&str], line0: usize, col0: usize) -> Option<String> {
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
fn use_name(lines: &[&str], d: &checker::DefEntry) -> Option<String> {
    token_at(lines, d.line - 1, d.col - 1)
}

/// Rango del **nombre** `name` como palabra entera desde `(def_line, def_col)` (1-basados) en
/// `lines`: un `let`/`fn` apunta al keyword → escanea hasta el nombre; un parámetro ya está en él.
fn decl_name_range(lines: &[&str], def_line: usize, def_col: usize, name: &str) -> Option<Span> {
    let chars: Vec<char> = lines.get(def_line - 1)?.chars().collect();
    let nm: Vec<char> = name.chars().collect();
    let mut i = def_col.saturating_sub(1);
    while i + nm.len() <= chars.len() {
        let antes = i == 0 || !is_ident_char(chars[i - 1]);
        let despues = i + nm.len() == chars.len() || !is_ident_char(chars[i + nm.len()]);
        if antes && despues && chars[i..i + nm.len()] == nm[..] {
            return Some((def_line - 1, i, nm.len()));
        }
        i += 1;
    }
    None
}

/// El símbolo bajo el cursor: su nombre, el rango de su **declaración** (si se localiza) y los
/// rangos de todos sus **usos**. `None` si no hay símbolo.
fn symbol_occurrences(
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
fn references_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Arr(vec![]) };
    let Some(src) = docs.get(&uri) else { return Json::Arr(vec![]) };
    let include_decl = msg.get("params").and_then(|p| p.get("context"))
        .and_then(|c| c.get("includeDeclaration"))
        .map(|b| matches!(b, Json::Bool(true)))
        .unwrap_or(true);
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
fn references_cross(uri: &str, src: &str, line0: usize, char0: usize, include_decl: bool) -> Option<Vec<(String, Span)>> {
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
fn rename_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Null };
    let Some(src) = docs.get(&uri) else { return Json::Null };
    let Some(new_name) = msg.get("params").and_then(|p| p.get("newName")).and_then(|n| n.as_str()) else {
        return Json::Null;
    };
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
fn rename_cross(uri: &str, src: &str, line0: usize, char0: usize) -> Option<Vec<(String, Span)>> {
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
fn push_rename(pos: &mut Vec<(String, Span)>, safe: &mut bool, name: &str, u: String, dl0: usize, dc0: usize, lines: &[&str]) {
    match token_at(lines, dl0, dc0) {
        Some(t) if t == name => pos.push((u, (dl0, dc0, name.chars().count()))),
        _ => *safe = false,
    }
}

/// El nombre global del ítem top-level (función/struct/enum/trait) cuya declaración está en
/// `(line, col)`, o `None` si ahí no hay uno (p. ej. es una variable local).
fn def_global_name(program: &crate::ast::Program, line: usize, col: usize) -> Option<String> {
    let en = |l: usize, c: usize| l == line && c == col;
    program.functions.iter().find(|f| en(f.line, f.col)).map(|f| f.name.clone())
        .or_else(|| program.structs.iter().find(|s| en(s.line, s.col)).map(|s| s.name.clone()))
        .or_else(|| program.enums.iter().find(|e| en(e.line, e.col)).map(|e| e.name.clone()))
        .or_else(|| program.traits.iter().find(|t| en(t.line, t.col)).map(|t| t.name.clone()))
}

/// El contexto de import en el que está el cursor (M45c), detectado textualmente sobre el prefijo
/// de la línea (el import a medio escribir no parsea).
enum ImportCtx {
    /// `from <ruta> import <cursor>` — completar los **símbolos `pub`** del módulo `<ruta>`.
    Symbols(String),
    /// `import <cursor>` / `from <cursor> import` — completar **rutas de módulo** del proyecto.
    ModulePath,
}

/// Detecta el contexto de import a partir del prefijo de la línea hasta el cursor (M45c).
/// Reconoce `import <ruta>`, `[pub] from <ruta> import <símbolos>` (y su fase de ruta). Devuelve
/// `None` si la línea no es un import.
fn import_context(prefix: &str) -> Option<ImportCtx> {
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
fn import_roots(entry: &Path) -> Vec<PathBuf> {
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
fn classify_source_symbol(source: &str, name: &str) -> Option<(i64, Option<(Vec<String>, String)>)> {
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
fn module_symbol_item(label: String, kind: i64, signature: Option<(Vec<String>, String)>, ctx: &SigCtx) -> Vec<Json> {
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
fn item_struct_literal(name: &str, fields: &[(String, String)]) -> Json {
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

fn module_pub_symbols(entry: &Path, path: &str) -> Option<Vec<(String, i64, Option<(Vec<String>, String)>)>> {
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
fn import_completion_items(uri: Option<&str>, src: &str, line0: usize, char0: usize) -> Option<Json> {
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
fn module_path_completion_items(entry: &Path, line0: usize, col: usize, chars: &[char]) -> Option<Json> {
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
fn struct_literal_completion_items(uri: Option<&str>, src: &str, line0: usize, char0: usize) -> Option<Json> {
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
    // Guarda: descartar posiciones que NO son un literal aunque lleven un nombre de tipo antes del `{`
    // — cuerpo de función (`-> T {`), impl (`for T {`), definición (`struct/enum/trait T {`).
    let mut k = start;
    while k > 0 && cs[k - 1].is_whitespace() { k -= 1; }
    if k >= 2 && cs[k - 1] == '>' && cs[k - 2] == '-' { return None; } // `-> T {`
    if let Some((prev, _)) = ident_before(&cs, start) {
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
fn fields_already_written(body: &[char]) -> std::collections::HashSet<String> {
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
fn module_member_completion_items(uri: Option<&str>, src: &str, line0: usize, char0: usize) -> Option<Json> {
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

fn member_completion_items(uri: Option<&str>, src: &str, line0: usize, char0: usize, docs: &HashMap<String, String>) -> Option<Json> {
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

    // Corre el front-end sobre la fuente reparada (con recuperación de errores) y enumera.
    let tokens = lexer::lex(&repaired).ok()?;
    let (mut program, _errs) = parser::parse_all(tokens);
    let members = checker::member_completion(&mut program);

    let orig_lines: Vec<&str> = src.split('\n').collect();
    let ctx = SigCtx::new(src, uri.and_then(uri_to_path).as_deref()); // M46a: firmas para el detalle
    let items: Vec<Json> = members
        .into_iter()
        .map(|m| {
            // Documentación: builtin → `///` sobre la declaración del método/UFCS (M45b) → prelude.
            let doc = crate::builtins::doc(&m.label).map(|s| s.to_string())
                .or_else(|| m.def.and_then(|(dl, _)| {
                    // La def vive en la fuente original (el reparado no cambia números de línea);
                    // si cae fuera (símbolo del prelude), se intenta el prelude más abajo.
                    if dl >= 1 && dl <= orig_lines.len() {
                        crate::raydoc::doc_lineas_arriba(&orig_lines, dl).map(|ls| ls.join("\n"))
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
                fields.push(("insertText", Json::Str(insert_call(&m.label, ps_ref, m.has_args))));
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
        .collect();
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
fn module_alias_symbols(uri: Option<&str>, src: &str, receiver: &str) -> Option<Json> {
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
fn completion_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let uri = msg.get("params").and_then(|p| p.get("textDocument")).and_then(|t| t.get("uri")).and_then(|u| u.as_str());
    let Some(src) = uri.and_then(|u| docs.get(u)) else { return Json::Arr(vec![]) };
    if let Some((line0, char0)) = pos_params(msg).map(|(_, l, c)| (l, c)) {
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
    let structs_ofrecibles: Vec<String> = items.iter()
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
    for label in structs_ofrecibles {
        if let Some(fields) = ctx.struct_fields(&label).filter(|c| !c.is_empty()) {
            list.push(item_struct_literal(&label, &fields));
        }
    }
    Json::Arr(list)
}

/// Los nombres en ámbito local (params + `let`/`var`) de la función que contiene `cursor_line`
/// (1-basado), declarados en o antes de esa línea (M10.2f). Sin spans, el alcance es la **función**
/// envolvente (la de mayor línea de inicio que no la supera), no el bloque exacto: degradación honesta.
fn scope_locals(program: &crate::ast::Program, cursor_line: usize) -> Vec<String> {
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
fn collect_lets(block: &crate::ast::Block, cursor_line: usize, out: &mut Vec<String>) {
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
fn signature_help_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Null };
    let Some(src) = docs.get(&uri) else { return Json::Null };
    // 1. Hallar la llamada en curso: el nombre, cuántas comas la preceden (param activo) y el receptor
    //    si es una llamada por punto (`recv.m(`).
    let Some((name, active, receiver)) = enclosing_call(src, line0, char0) else { return Json::Null };
    // 2. Resolver la firma con el resolutor unificado (M46b): buffer + módulos importados + prelude +
    //    builtins. Textual → robusto ante el documento a medio escribir (solo exige que la
    //    *declaración* `fn nombre(...) -> ...` esté bien formada). Así el signature help funciona
    //    también para funciones importadas (`u.cuadrado(`) y del prelude, no solo las del archivo.
    let ctx = SigCtx::new(src, uri_to_path(&uri).as_deref());
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
    // 3. Construir el label `fn nombre(p: T, …) -> R` y la lista de parámetros (para resaltar).
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
fn find_fn_signature(src: &str, name: &str) -> Option<(Vec<String>, String)> {
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
struct SigCtx {
    sources: Vec<String>,
}

/// Las rutas de módulo que `src` importa o reexporta (`import M;` / `[pub] from M import …`), para
/// el cierre transitivo de `SigCtx`.
fn imported_paths(src: &str) -> Vec<String> {
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
fn snippet_args(params: &[String]) -> String {
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
fn insert_call(label: &str, params: Option<&[String]>, has_args: bool) -> String {
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
fn assoc_param_names(sig: &str) -> Vec<String> {
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
fn push_signature_raw(fields: &mut Vec<(&'static str, Json)>, mut params: Vec<String>, ret: String, is_method: bool) {
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
fn split_top_commas(s: &str) -> Vec<String> {
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
fn enclosing_call(src: &str, line0: usize, char0: usize) -> Option<(String, usize, Option<String>)> {
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
fn ident_before(cs: &[char], i: usize) -> Option<(String, Option<String>)> {
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
fn is_imported_module(src: &str, name: &str) -> bool {
    lexer::lex(src)
        .ok()
        .map(|t| {
            let (program, _) = parser::parse_all(t);
            program.imports.iter().any(|d| d.leaf() == name)
        })
        .unwrap_or(false)
}

/// Lee un `Json::Num` como `usize` (las posiciones LSP son enteros).
fn as_usize(j: &Json) -> Option<usize> {
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
fn formatting_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
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
    let Ok(formatted) = crate::fmt::format_source_with_indent(src, &unit) else { return Json::Arr(vec![]) };
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
fn document_symbol_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let uri = msg.get("params").and_then(|p| p.get("textDocument")).and_then(|t| t.get("uri")).and_then(|u| u.as_str());
    let Some(src) = uri.and_then(|u| docs.get(u)) else { return Json::Arr(vec![]) };
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
fn doc_symbol(lines: &[&str], line: usize, col: usize, name: &str, kind: i64, children: Vec<Json>) -> Json {
    doc_symbol_named(lines, line, col, name, name, kind, children)
}

/// Un `DocumentSymbol` con `etiqueta` como nombre mostrado y `buscar` como el identificador a
/// localizar en la fuente (p. ej. un `impl` muestra "impl T for X" pero su rango apunta a `T`).
fn doc_symbol_named(lines: &[&str], line: usize, col: usize, search_name: &str, label: &str, kind: i64, children: Vec<Json>) -> Json {
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
fn document_highlight_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Arr(vec![]) };
    let Some(src) = docs.get(&uri) else { return Json::Arr(vec![]) };
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

// ── Análisis: el puente con el compilador ────────────────────────────────────────────

/// Un diagnóstico del front-end: posición **1-basada** (como reportan las fases), la
/// extensión del error en caracteres (M33a; `1` si la fase no la conoce) y el mensaje,
/// que es el `Display` del error (la misma cabecera que muestra el terminal).
pub struct Diag {
    pub line: usize,
    pub col: usize,
    pub len: usize,
    pub message: String,
}

/// Corre el front-end **sin ejecutar** y devuelve TODOS los errores (M33c, hasta el tope
/// de cada fase): un error léxico corta (sin tokens no hay nada que recuperar); los de
/// sintaxis se acumulan con recuperación (`parse_all`); solo si el parse quedó limpio se
/// pasa al checker (`check_all`) — los errores de tipos sobre un AST parcial serían
/// cascada basura. Es lo que publica `publishDiagnostics`.
pub fn analyze_all(src: &str) -> Vec<Diag> {
    let diag = |line: usize, col: usize, len: usize, message: String| Diag { line, col, len, message };
    let tokens = match lexer::lex(src) {
        Ok(t) => t,
        Err(e) => return vec![diag(e.line, e.col, e.len, e.to_string())],
    };
    let (mut program, perrs) = parser::parse_all(tokens);
    if !perrs.is_empty() {
        return perrs.into_iter().map(|e| diag(e.line, e.col, e.len, e.to_string())).collect();
    }
    // `check_all_modulo`: sin exigir `main`. Un buffer sin guardar puede ser un archivo de módulo
    // (sin entrada); que le falte `main` es asunto de proyecto (lo cazan `ray build`/`ray run`), no
    // un diagnóstico de este archivo. Así un módulo suelto muestra sus errores reales, no ese ruido.
    checker::check_all_modulo(&mut program)
        .into_iter()
        .map(|e| diag(e.line, e.col, e.len, e.to_string()))
        .collect()
}

/// Corre el front-end (lexer → parser → checker) **sin ejecutar** y devuelve el primer
/// error, si lo hay. Es todo el acoplamiento con el compilador: la API pública, nada más.
pub fn analyze(src: &str) -> Option<Diag> {
    let tokens = match lexer::lex(src) {
        Ok(t) => t,
        Err(e) => return Some(Diag { line: e.line, col: e.col, len: e.len, message: e.to_string() }),
    };
    let mut program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => return Some(Diag { line: e.line, col: e.col, len: e.len, message: e.to_string() }),
    };
    if let Err(e) = checker::check(&mut program) {
        return Some(Diag { line: e.line, col: e.col, len: e.len, message: e.to_string() });
    }
    None
}

// ── Construcción de mensajes salientes ───────────────────────────────────────────────

/// La respuesta a `initialize`: anuncia las capacidades del servidor.
fn initialize_response(id: Json) -> Json {
    let capabilities = obj(vec![
        // 1 = Full sync: el cliente reenvía el documento entero en cada cambio.
        ("textDocumentSync", num(1)),
        // M10.2b: el servidor responde hover (el tipo bajo el cursor) e ir-a-definición.
        ("hoverProvider", Json::Bool(true)),
        ("definitionProvider", Json::Bool(true)),
        // Cluster 4: find-references y rename (sobre el índice semántico + la fuente).
        ("referencesProvider", Json::Bool(true)),
        ("renameProvider", Json::Bool(true)),
        // Cluster 4 + M45: completion. `.` dispara el completion de **miembros** (`recv.` →
        // campos/métodos/builtins/UFCS del tipo del receptor).
        ("completionProvider", obj(vec![
            ("triggerCharacters", Json::Arr(vec![text(".")])),
        ])),
        // M10.2f: signature help — la firma de la función mientras se escriben los argumentos.
        ("signatureHelpProvider", obj(vec![
            ("triggerCharacters", Json::Arr(vec![text("("), text(",")])),
        ])),
        // Formateo del documento (reusa `ray fmt`), outline de símbolos y resaltado de ocurrencias.
        ("documentFormattingProvider", Json::Bool(true)),
        ("documentSymbolProvider", Json::Bool(true)),
        ("documentHighlightProvider", Json::Bool(true)),
    ]);
    let result = obj(vec![
        ("capabilities", capabilities),
        ("serverInfo", obj(vec![("name", text("raylang-lsp"))])),
    ]);
    result_message(id, result)
}

/// Una respuesta JSON-RPC exitosa: `{ jsonrpc, id, result }`.
fn result_message(id: Json, result: Json) -> Json {
    obj(vec![("jsonrpc", text("2.0")), ("id", id), ("result", result)])
}

/// Una respuesta JSON-RPC de error "método no encontrado" (-32601).
fn method_error(id: Json, method: &str) -> Json {
    let error = obj(vec![
        ("code", num(-32601)),
        ("message", Json::Str(format!("método no soportado: {method}"))),
    ]);
    obj(vec![("jsonrpc", text("2.0")), ("id", id), ("error", error)])
}

/// Analiza la fuente y construye la notificación `publishDiagnostics` para ese documento.
fn diagnostics(uri: &str, src: &str) -> Json {
    // Soporte de módulos: si el documento es un archivo, se analiza **con el loader** (resolviendo
    // sus imports desde disco) para no marcar errores espurios en proyectos multi-archivo. Si no es
    // un archivo o el buffer ni siquiera parsea, se cae al análisis de un solo archivo (multi-error).
    let diags = analyze_modular(uri, src).unwrap_or_else(|| analyze_all(src));
    let json = diags.iter().map(|d| diagnostic_json(src, d)).collect();
    publish(uri, json)
}

/// Diagnósticos **con módulos**: corre el loader sobre el buffer de entrada (imports leídos de
/// disco) y devuelve los errores que caen en ESTE archivo, con su línea local. Devuelve `None`
/// cuando conviene caer al análisis de un solo archivo: si el URI no es un `file:` (buffer sin
/// guardar) o si el buffer de entrada ni siquiera parsea (así el fallback da errores de sintaxis
/// precisos y multi-error sobre la entrada, en vez de un único error del loader).
fn analyze_modular(uri: &str, src: &str) -> Option<Vec<Diag>> {
    let path = uri_to_path(uri)?;
    match load(&path, src) {
        Ok(loaded) => {
            // El módulo de entrada es la banda que empieza más arriba (delta 0 → línea local =
            // global). Solo publicamos SUS errores: los de otros módulos pertenecen a sus URIs.
            let entry_start = loaded.modules.iter().map(|m| m.start_line).min().unwrap_or(1);
            let mut program = loaded.program;
            // `check_all_modulo`: el archivo abierto puede ser un **submódulo sin `main`** (legítimo);
            // no exigimos la entrada para no marcar "falta la función de entrada 'main'" en cada módulo.
            let diags = checker::check_all_modulo(&mut program)
                .into_iter()
                .filter_map(|e| {
                    let m = loaded.modules.iter().rev().find(|m| m.start_line <= e.line)?;
                    (m.start_line == entry_start).then(|| Diag {
                        line: e.line - m.start_line + 1,
                        col: e.col,
                        len: e.len,
                        message: e.to_string(),
                    })
                })
                .collect();
            Some(diags)
        }
        Err(e) => {
            // El loader no pudo cargar. Si el buffer de entrada parsea, el fallo es de un import
            // (módulo ausente, cápsula violada, dependencia que no parsea) → un diagnóstico al
            // inicio. Si no parsea, `None` para que el fallback dé los errores de sintaxis precisos.
            let entry_parses = lexer::lex(src).ok().and_then(|t| parser::parse(t).ok()).is_some();
            entry_parses.then(|| vec![Diag { line: 1, col: 1, len: 1, message: e.message }])
        }
    }
}

/// Convierte un URI `file://…` a una ruta del sistema (decodificando `%XX`). `None` si no es un
/// `file:` (p. ej. un buffer `untitled:` sin archivo → se analiza en modo de un solo archivo).
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file:///Users/…` → host vacío; la ruta arranca en el tercer '/'. Un raro `localhost` se ignora.
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    Some(PathBuf::from(percent_decode(path)))
}

/// Decodifica los `%XX` de un URI (p. ej. `%20` → espacio). Sin dependencias.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let hex = |c: u8| (c as char).to_digit(16);
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Raíces de dependencias para el loader (M39c): la caché `.ray-deps/` del proyecto que contiene
/// `entry` (subiendo hasta el `ray.toml`), si existe. Alinea el LSP con `ray run`.
fn dep_roots_for(entry: &Path) -> Vec<PathBuf> {
    let dir = entry.parent().unwrap_or(Path::new("."));
    let root = crate::manifest::Manifest::find(dir)
        .and_then(|toml| toml.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| dir.to_path_buf());
    let cache = root.join(".ray-deps");
    if cache.is_dir() { vec![cache] } else { Vec::new() }
}

/// La raíz del proyecto que contiene `entry`: el **ancestro más cercano** (subiendo desde su carpeta)
/// que contiene un `main.ray` —la entrada convencional, y por definición la raíz desde la que son
/// absolutos los `import` (DESIGN §20.3: "la ruta es absoluta desde la raíz del proyecto, el directorio
/// del archivo de entrada")—. `None` si no hay ninguno (archivo suelto sin proyecto).
fn project_root_for(entry: &Path) -> Option<PathBuf> {
    let mut dir = entry.parent()?;
    loop {
        if dir.join("main.ray").is_file() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// Carga el buffer `src` (del archivo `entry`) con el loader, resolviendo los `import` desde disco.
/// Si `entry` vive en un proyecto (hay un `main.ray` ancestro), usa esa **raíz** y la identidad de
/// módulo **real** del archivo (su ruta relativa): así los imports absolutos y el enforcement de
/// cápsula funcionan aunque el archivo abierto sea un submódulo profundo dentro de una cápsula. Si no
/// hay proyecto (archivo suelto), carga como entrada en su propia carpeta (comportamiento clásico).
fn load(entry: &Path, src: &str) -> Result<loader::Loaded, loader::LoadError> {
    let deps = dep_roots_for(entry);
    match project_root_for(entry) {
        Some(root) => loader::load_source_module(entry, src, &root, &deps),
        None => loader::load_source(entry, src, &deps),
    }
}

/// Envuelve una lista de diagnósticos en la notificación `textDocument/publishDiagnostics`.
fn publish(uri: &str, diags: Vec<Json>) -> Json {
    let params = obj(vec![("uri", text(uri)), ("diagnostics", Json::Arr(diags))]);
    obj(vec![
        ("jsonrpc", text("2.0")),
        ("method", text("textDocument/publishDiagnostics")),
        ("params", params),
    ])
}

/// Traduce un `Diag` (1-basado) a un diagnóstico LSP (rango 0-basado, severidad Error).
fn diagnostic_json(src: &str, d: &Diag) -> Json {
    // 1-basado (nuestras fases) → 0-basado (LSP).
    let line0 = d.line.saturating_sub(1);
    let start_char = d.col.saturating_sub(1);
    // Con extensión conocida (M33a: lexer/parser reportan el token ofensor) subrayamos el
    // lexema exacto; con `len == 1` (el checker, hasta M33a-2) conservamos el subrayado
    // hasta el final de la línea (más visible que un solo carácter). El `character` de LSP
    // cuenta unidades UTF-16; para código ASCII coincide con el número de caracteres, que
    // es con lo que medimos la línea.
    let line_len = src
        .lines()
        .nth(line0)
        .map(|l| l.chars().count())
        .unwrap_or(start_char + 1);
    let end_char = if d.len > 1 {
        start_char.saturating_add(d.len).min(line_len.max(start_char + 1))
    } else if start_char < line_len {
        line_len
    } else {
        start_char + 1
    };
    let pos = |ch: usize| obj(vec![("line", num(line0 as i64)), ("character", num(ch as i64))]);
    obj(vec![
        ("range", obj(vec![("start", pos(start_char)), ("end", pos(end_char))])),
        ("severity", num(1)), // 1 = Error
        ("source", text("raylang")),
        ("message", Json::Str(d.message.clone())),
    ])
}

// ── Constructores breves para JSON ───────────────────────────────────────────────────

/// Un objeto JSON a partir de pares `(clave, valor)`.
fn obj(pairs: Vec<(&str, Json)>) -> Json {
    Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}
/// Una cadena JSON.
fn text(s: &str) -> Json {
    Json::Str(s.to_string())
}
/// Un número JSON (entero).
fn num(x: i64) -> Json {
    Json::Num(x as f64)
}

// ── mod json: mini-JSON en std puro ──────────────────────────────────────────────────

mod json {
    //! Un JSON mínimo —parser y serializador— en `std` puro, sin dependencias.
    //!
    //! No es un JSON de producción: es justo lo que el LSP intercambia (leer los campos de
    //! los mensajes entrantes y construir los salientes), pero correcto para ese tráfico,
    //! incluido el *unescape* de cadenas con `\uXXXX` y parejas sustitutas UTF-16.

    /// Un valor JSON. Los objetos preservan el orden de inserción (`Vec` de pares).
    #[derive(Debug, Clone, PartialEq)]
    pub enum Json {
        Null,
        Bool(bool),
        Num(f64),
        Str(String),
        Arr(Vec<Json>),
        Obj(Vec<(String, Json)>),
    }

    impl Json {
        /// Busca una clave en un objeto (o `None` si no es objeto o no está).
        pub fn get(&self, key: &str) -> Option<&Json> {
            match self {
                Json::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
                _ => None,
            }
        }
        /// El contenido si es una cadena.
        pub fn as_str(&self) -> Option<&str> {
            match self {
                Json::Str(s) => Some(s),
                _ => None,
            }
        }
        /// Los elementos si es un arreglo.
        pub fn as_array(&self) -> Option<&[Json]> {
            match self {
                Json::Arr(a) => Some(a),
                _ => None,
            }
        }

        /// Serializa a texto JSON compacto.
        pub fn serialize(&self) -> String {
            let mut s = String::new();
            self.write(&mut s);
            s
        }

        fn write(&self, out: &mut String) {
            match self {
                Json::Null => out.push_str("null"),
                Json::Bool(true) => out.push_str("true"),
                Json::Bool(false) => out.push_str("false"),
                Json::Num(n) => {
                    // Los enteros se escriben sin parte decimal (id, line, severity…).
                    if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e15 {
                        out.push_str(&(*n as i64).to_string());
                    } else {
                        out.push_str(&n.to_string());
                    }
                }
                Json::Str(s) => write_string(s, out),
                Json::Arr(items) => {
                    out.push('[');
                    for (i, it) in items.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        it.write(out);
                    }
                    out.push(']');
                }
                Json::Obj(pairs) => {
                    out.push('{');
                    for (i, (k, v)) in pairs.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        write_string(k, out);
                        out.push(':');
                        v.write(out);
                    }
                    out.push('}');
                }
            }
        }
    }

    /// Escribe una cadena JSON, escapando comillas, barras y caracteres de control.
    fn write_string(s: &str, out: &mut String) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                '\r' => out.push_str("\\r"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
    }

    /// Parsea un texto JSON a un `Json`.
    pub fn parse(input: &str) -> Result<Json, String> {
        let chars: Vec<char> = input.chars().collect();
        let mut p = Parser { chars: &chars, i: 0 };
        p.skip_ws();
        let v = p.value()?;
        p.skip_ws();
        Ok(v)
    }

    /// Un descenso recursivo sobre el texto (vector de `char` + cursor).
    struct Parser<'a> {
        chars: &'a [char],
        i: usize,
    }

    impl Parser<'_> {
        fn peek(&self) -> Option<char> {
            self.chars.get(self.i).copied()
        }
        fn bump(&mut self) -> Option<char> {
            let c = self.peek();
            if c.is_some() {
                self.i += 1;
            }
            c
        }
        fn skip_ws(&mut self) {
            while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
                self.i += 1;
            }
        }

        fn value(&mut self) -> Result<Json, String> {
            self.skip_ws();
            match self.peek() {
                Some('{') => self.object(),
                Some('[') => self.array(),
                Some('"') => Ok(Json::Str(self.string()?)),
                Some('t' | 'f') => self.boolean(),
                Some('n') => self.null(),
                Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
                other => Err(format!("token JSON inesperado: {other:?}")),
            }
        }

        fn object(&mut self) -> Result<Json, String> {
            self.bump(); // '{'
            let mut pairs = Vec::new();
            self.skip_ws();
            if self.peek() == Some('}') {
                self.bump();
                return Ok(Json::Obj(pairs));
            }
            loop {
                self.skip_ws();
                if self.peek() != Some('"') {
                    return Err("se esperaba una clave de objeto".into());
                }
                let key = self.string()?;
                self.skip_ws();
                if self.bump() != Some(':') {
                    return Err("se esperaba ':' en un objeto".into());
                }
                let val = self.value()?;
                pairs.push((key, val));
                self.skip_ws();
                match self.bump() {
                    Some(',') => continue,
                    Some('}') => break,
                    other => return Err(format!("se esperaba ',' o '}}', vino {other:?}")),
                }
            }
            Ok(Json::Obj(pairs))
        }

        fn array(&mut self) -> Result<Json, String> {
            self.bump(); // '['
            let mut items = Vec::new();
            self.skip_ws();
            if self.peek() == Some(']') {
                self.bump();
                return Ok(Json::Arr(items));
            }
            loop {
                items.push(self.value()?);
                self.skip_ws();
                match self.bump() {
                    Some(',') => continue,
                    Some(']') => break,
                    other => return Err(format!("se esperaba ',' o ']', vino {other:?}")),
                }
            }
            Ok(Json::Arr(items))
        }

        fn string(&mut self) -> Result<String, String> {
            self.bump(); // '"'
            let mut s = String::new();
            loop {
                match self.bump() {
                    None => return Err("cadena sin terminar".into()),
                    Some('"') => break,
                    Some('\\') => self.escape(&mut s)?,
                    Some(c) => s.push(c),
                }
            }
            Ok(s)
        }

        /// Procesa un escape tras `\` y empuja el carácter resultante.
        fn escape(&mut self, s: &mut String) -> Result<(), String> {
            match self.bump() {
                Some('"') => s.push('"'),
                Some('\\') => s.push('\\'),
                Some('/') => s.push('/'),
                Some('n') => s.push('\n'),
                Some('t') => s.push('\t'),
                Some('r') => s.push('\r'),
                Some('b') => s.push('\u{0008}'),
                Some('f') => s.push('\u{000C}'),
                Some('u') => {
                    let cp = self.hex4()?;
                    // Una pareja sustituta UTF-16 (alta + baja) codifica un carácter > BMP.
                    if (0xD800..=0xDBFF).contains(&cp) {
                        if self.bump() != Some('\\') || self.bump() != Some('u') {
                            return Err("pareja sustituta UTF-16 incompleta".into());
                        }
                        let lo = self.hex4()?;
                        let c = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                        if let Some(ch) = char::from_u32(c) {
                            s.push(ch);
                        }
                    } else if let Some(ch) = char::from_u32(cp) {
                        s.push(ch);
                    }
                }
                other => return Err(format!("escape inválido: \\{other:?}")),
            }
            Ok(())
        }

        /// Cuatro dígitos hexadecimales (`\uXXXX`).
        fn hex4(&mut self) -> Result<u32, String> {
            let mut v = 0u32;
            for _ in 0..4 {
                let c = self.bump().ok_or("escape \\u incompleto")?;
                let d = c.to_digit(16).ok_or("dígito hexadecimal inválido")?;
                v = v * 16 + d;
            }
            Ok(v)
        }

        fn number(&mut self) -> Result<Json, String> {
            let start = self.i;
            if self.peek() == Some('-') {
                self.bump();
            }
            while matches!(
                self.peek(),
                Some(c) if c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-')
            ) {
                self.bump();
            }
            let s: String = self.chars[start..self.i].iter().collect();
            s.parse::<f64>()
                .map(Json::Num)
                .map_err(|_| format!("número inválido: {s}"))
        }

        fn boolean(&mut self) -> Result<Json, String> {
            if self.lit("true") {
                Ok(Json::Bool(true))
            } else if self.lit("false") {
                Ok(Json::Bool(false))
            } else {
                Err("literal booleano inválido".into())
            }
        }

        fn null(&mut self) -> Result<Json, String> {
            if self.lit("null") {
                Ok(Json::Null)
            } else {
                Err("literal null inválido".into())
            }
        }

        /// Consume `word` si aparece literalmente en el cursor.
        fn lit(&mut self, word: &str) -> bool {
            let end = self.i + word.len();
            if end <= self.chars.len() && self.chars[self.i..end].iter().collect::<String>() == word
            {
                self.i = end;
                true
            } else {
                false
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parsea_objeto_anidado() {
            let v = parse(r#"{"a":1,"b":[true,null,"x"],"c":{"d":-2.5}}"#).unwrap();
            assert_eq!(v.get("a"), Some(&Json::Num(1.0)));
            assert_eq!(v.get("b").unwrap().as_array().unwrap().len(), 3);
            assert_eq!(v.get("c").unwrap().get("d"), Some(&Json::Num(-2.5)));
        }

        #[test]
        fn desescapa_cadenas() {
            let v = parse(r#""línea\n\t\"fin\"""#).unwrap();
            assert_eq!(v.as_str(), Some("línea\n\t\"fin\""));
            // \uXXXX (BMP) y pareja sustituta (emoji).
            assert_eq!(parse(r#""é""#).unwrap().as_str(), Some("é"));
            assert_eq!(parse(r#""😀""#).unwrap().as_str(), Some("😀"));
        }

        #[test]
        fn serializa_y_reparsea_igual() {
            let original = obj_from(vec![
                ("jsonrpc", Json::Str("2.0".into())),
                ("id", Json::Num(7.0)),
                ("ok", Json::Bool(true)),
                ("lista", Json::Arr(vec![Json::Num(1.0), Json::Null])),
            ]);
            let serialized = original.serialize();
            assert_eq!(parse(&serialized).unwrap(), original);
            // Los enteros no llevan parte decimal.
            assert!(serialized.contains("\"id\":7"));
        }

        fn obj_from(pairs: Vec<(&str, Json)>) -> Json {
            Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analizar_todos_publica_varios_errores() {
        // M33c: dos errores de tipos → dos diagnósticos.
        let ds = analyze_all("fn f() -> int { 1 + true }\nfn g() -> int { \"x\" * 2 }\nfn main() -> int { 0 }");
        assert_eq!(ds.len(), 2, "{:?}", ds.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert_eq!((ds[0].line, ds[1].line), (1, 2));
        // Dos errores de sintaxis → dos diagnósticos (recuperación del parser)…
        let ds = analyze_all("fn f() -> int { let = 1; 0 }\nfn g() -> int { 2 + }\nfn main() -> int { 0 }");
        assert!(ds.len() >= 2, "{:?}", ds.iter().map(|d| &d.message).collect::<Vec<_>>());
        // …pero un parse sucio NO llega al checker (sería cascada sobre un AST parcial).
        assert!(ds.iter().all(|d| d.message.contains("sintaxis")), "{:?}",
            ds.iter().map(|d| &d.message).collect::<Vec<_>>());
        // Sin errores → lista vacía (borra los diagnósticos previos del editor).
        assert!(analyze_all("fn main() -> int { 0 }").is_empty());
    }

        #[test]
    fn analiza_programa_valido_sin_errores() {
        assert!(analyze("fn main() -> int { 1 + 2 }").is_none());
    }

    #[test]
    fn diagnosticos_con_modulos() {
        // Un proyecto de dos archivos: `geo.ray` (en disco) y la entrada `main.ray` (en el buffer).
        let dir = std::env::temp_dir().join("ray_lsp_mod");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("geo.ray"), "pub fn duplicar(x: int) -> int { x * 2 }\n").unwrap();
        let entry = dir.join("main.ray");
        let uri = format!("file://{}", entry.display());

        // (a) Un import válido NO produce diagnósticos (antes: "función 'duplicar' no declarada").
        let src = "from geo import duplicar;\nfn main() -> int { duplicar(21) }\n";
        let ds = analyze_modular(&uri, src).expect("modo modular (es un archivo)");
        assert!(ds.is_empty(), "un import válido no debe dar errores: {:?}",
            ds.iter().map(|d| &d.message).collect::<Vec<_>>());

        // (b) Un error de tipos EN LA ENTRADA sí se reporta, con la línea local.
        let src = "from geo import duplicar;\nfn main() -> int { duplicar(true) }\n";
        let ds = analyze_modular(&uri, src).expect("modular");
        assert_eq!(ds.len(), 1, "{:?}", ds.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert_eq!(ds[0].line, 2, "línea local de la entrada, no la global del programa fusionado");

        // (c) Un import a un módulo inexistente se reporta (la entrada parsea → error del loader).
        let src = "from noexiste import cosa;\nfn main() -> int { 0 }\n";
        let ds = analyze_modular(&uri, src).expect("modular");
        assert_eq!(ds.len(), 1);
        assert!(ds[0].message.contains("noexiste"), "{}", ds[0].message);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn diagnosticos_submodulo_sin_main_y_import_por_ruta() {
        // Reproduce dos problemas al abrir un ARCHIVO DE MÓDULO (no la entrada) en el editor:
        //   1. Un submódulo `pub` sin `main` marcaba "falta la función de entrada 'main'".
        //   2. Un `import geo/util;` (ruta absoluta desde la raíz) no resolvía, porque el loader
        //      tomaba como raíz la carpeta del propio submódulo, no la raíz del proyecto.
        // Estructura (igual a examples/proyecto):
        //   root/main.ray                 (entrada, con main)
        //   root/geo/util.ray             (pub fn, sin main)
        //   root/geo/formas/circulo.ray   (import geo/util; pub struct; pub fn area, sin main)
        let dir = std::env::temp_dir().join("ray_lsp_submod");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("geo/formas")).unwrap();
        std::fs::write(dir.join("main.ray"),
            "import geo/formas/circulo;\nfn main() -> int { circulo.area(circulo.Circulo { radio: 4 }) }\n").unwrap();
        let util_src = "pub fn cuadrado(n: int) -> int { n * n }\n";
        std::fs::write(dir.join("geo/util.ray"), util_src).unwrap();
        let circulo_src = "import geo/util;\npub struct Circulo { radio: int }\npub fn area(c: Circulo) -> int { 3 * util.cuadrado(c.radio) }\n";
        std::fs::write(dir.join("geo/formas/circulo.ray"), circulo_src).unwrap();

        // La raíz del proyecto se detecta como el ancestro con `main.ray`, desde un submódulo profundo.
        let root = project_root_for(&dir.join("geo/formas/circulo.ray")).expect("raíz");
        assert_eq!(root, dir, "la raíz es el directorio con main.ray, no la del submódulo");

        // (1) util.ray: submódulo sin `main` → SIN diagnósticos (antes: "falta … 'main'").
        let uri_util = format!("file://{}", dir.join("geo/util.ray").display());
        let ds = analyze_modular(&uri_util, util_src).expect("modular");
        assert!(ds.is_empty(), "un submódulo sin main no debe dar errores: {:?}",
            ds.iter().map(|d| &d.message).collect::<Vec<_>>());

        // (2) circulo.ray: `import geo/util;` resuelve desde la raíz + sin `main` → SIN diagnósticos.
        let uri_circ = format!("file://{}", dir.join("geo/formas/circulo.ray").display());
        let ds = analyze_modular(&uri_circ, circulo_src).expect("modular");
        assert!(ds.is_empty(), "el import por ruta absoluta debe resolver: {:?}",
            ds.iter().map(|d| &d.message).collect::<Vec<_>>());

        // (3) Un error de tipos REAL en el submódulo sí se reporta (no se traga por el modo módulo).
        let circulo_malo = "import geo/util;\npub struct Circulo { radio: int }\npub fn area(c: Circulo) -> int { 3 * util.cuadrado(c) }\n";
        let ds = analyze_modular(&uri_circ, circulo_malo).expect("modular");
        assert_eq!(ds.len(), 1, "el error real del cuerpo debe verse: {:?}",
            ds.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert_eq!(ds[0].line, 3, "línea local del submódulo");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn diagnosticos_submodulo_interno_de_capsula() {
        // Un submódulo DENTRO de una cápsula que importa a un vecino interno de la MISMA cápsula.
        // `geo/mod.ray` hace de `geo` una cápsula; `geo/util` es interno; `geo/formas/circulo.ray`
        // (también dentro de `geo/`) hace `import geo/util;` → legítimo (el importador vive bajo la
        // cápsula). Antes, al abrir el submódulo, el loader lo identificaba por su stem ("circulo")
        // y el enforcement lo trataba como externo: "el módulo 'geo/util' es interno a la cápsula 'geo'".
        let dir = std::env::temp_dir().join("ray_lsp_capsula");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("geo/formas")).unwrap();
        std::fs::write(dir.join("main.ray"),
            "import geo;\nfn main() -> int { geo.area(geo.Circulo { radio: 4 }) }\n").unwrap();
        std::fs::write(dir.join("geo/mod.ray"),
            "pub from geo/formas/circulo import Circulo, area;\n").unwrap();
        std::fs::write(dir.join("geo/util.ray"), "pub fn cuadrado(n: int) -> int { n * n }\n").unwrap();
        let circulo_src = "import geo/util;\npub struct Circulo { radio: int }\npub fn area(c: Circulo) -> int { 3 * util.cuadrado(c.radio) }\n";
        std::fs::write(dir.join("geo/formas/circulo.ray"), circulo_src).unwrap();

        // La identidad real del submódulo es "geo/formas/circulo" → está bajo la cápsula "geo".
        let root = project_root_for(&dir.join("geo/formas/circulo.ray")).expect("raíz");
        assert_eq!(root, dir);

        // Abrir el submódulo interno: SIN diagnósticos (import a un vecino de la cápsula, y sin main).
        let uri = format!("file://{}", dir.join("geo/formas/circulo.ray").display());
        let ds = analyze_modular(&uri, circulo_src).expect("modular");
        assert!(ds.is_empty(), "importar a un vecino interno de la propia cápsula es legítimo: {:?}",
            ds.iter().map(|d| &d.message).collect::<Vec<_>>());

        // Hover DENTRO del submódulo (sin `main`): antes no daba nada porque el chequeo de main
        // cortaba antes de recorrer los cuerpos. `circulo_src` línea 3 (0-based 2):
        //   `pub fn area(c: Circulo) -> int { 3 * util.cuadrado(c.radio) }`  — uso de `c` en col 51.
        let (t, _, _) = hover_at(Some(&uri), circulo_src, 2, 51).expect("hover sobre uso de 'c'");
        assert_eq!(t, "c: Circulo", "hover de un uso dentro de un submódulo sin main");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn formatting_reemplaza_el_documento() {
        let mut docs = HashMap::new();
        let uri = "file:///t.ray".to_string();
        // Código mal formateado (espaciado irregular) → un edit que cubre todo el buffer.
        docs.insert(uri.clone(), "fn  main( )->int{1+2}\n".to_string());
        let msg = obj(vec![("params", obj(vec![("textDocument", obj(vec![("uri", text(&uri))]))]))]);
        let r = formatting_result(&msg, &docs);
        let edits = r.as_array().expect("array de edits");
        assert_eq!(edits.len(), 1, "un único edit de documento completo");
        let new = edits[0].get("newText").and_then(Json::as_str).unwrap();
        // El resultado es el mismo que `ray fmt` (el formateador compartido).
        assert_eq!(new, crate::fmt::format_source("fn  main( )->int{1+2}\n").unwrap());
        assert!(new.contains("fn main()"), "se normalizó el espaciado: {new:?}");
        // El rango arranca en (0,0).
        let start = edits[0].get("range").unwrap().get("start").unwrap();
        assert_eq!(start.get("line"), Some(&Json::Num(0.0)));
        assert_eq!(start.get("character"), Some(&Json::Num(0.0)));

        // Ya formateado → lista vacía (nada que cambiar).
        let already = crate::fmt::format_source("fn  main( )->int{1+2}\n").unwrap();
        docs.insert(uri.clone(), already);
        assert!(formatting_result(&msg, &docs).as_array().unwrap().is_empty());

        // Código que no parsea → lista vacía (no se formatea código inválido).
        docs.insert(uri.clone(), "fn main( { ".to_string());
        assert!(formatting_result(&msg, &docs).as_array().unwrap().is_empty());
    }

    #[test]
    fn document_symbol_lista_el_outline() {
        let mut docs = HashMap::new();
        let uri = "file:///t.ray".to_string();
        let src = "const K: int = 3;\nstruct Punto { x: int, y: int }\nenum Color { Rojo, Verde }\ntrait Show { fn mostrar(self) -> string; }\nimpl Show for Punto { fn mostrar(self) -> string { \"p\" } }\nfn main() -> int { 0 }\n";
        docs.insert(uri.clone(), src.to_string());
        let msg = obj(vec![("params", obj(vec![("textDocument", obj(vec![("uri", text(&uri))]))]))]);
        let syms = document_symbol_result(&msg, &docs);
        let arr = syms.as_array().expect("array");
        let names: Vec<&str> = arr.iter().filter_map(|s| s.get("name").and_then(Json::as_str)).collect();
        // Todos los ítems de nivel superior, en orden de archivo.
        assert_eq!(names, vec!["K", "Punto", "Color", "Show", "impl Show for Punto", "main"], "{names:?}");
        // El enum lleva sus variantes como hijos.
        let color = arr.iter().find(|s| s.get("name").and_then(Json::as_str) == Some("Color")).unwrap();
        let vars: Vec<&str> = color.get("children").unwrap().as_array().unwrap().iter()
            .filter_map(|c| c.get("name").and_then(Json::as_str)).collect();
        assert_eq!(vars, vec!["Rojo", "Verde"]);
        // El trait lleva su método como hijo; el kind del struct es 23 (Struct).
        let show = arr.iter().find(|s| s.get("name").and_then(Json::as_str) == Some("Show")).unwrap();
        assert_eq!(show.get("children").unwrap().as_array().unwrap().len(), 1);
        let punto = arr.iter().find(|s| s.get("name").and_then(Json::as_str) == Some("Punto")).unwrap();
        assert_eq!(punto.get("kind"), Some(&Json::Num(23.0)));
        // El selectionRange del struct apunta a su NOMBRE (col 7 0-based en "struct Punto").
        let sel = punto.get("selectionRange").unwrap().get("start").unwrap();
        assert_eq!(sel.get("line"), Some(&Json::Num(1.0)));
        assert_eq!(sel.get("character"), Some(&Json::Num(7.0)));
    }

    #[test]
    fn hover_y_signature_de_builtins() {
        // M10.2i: los builtins (print/char_code/…) no viven en la fuente; aun así el hover muestra su
        // firma (con los tipos de la llamada) y el signature help su firma fija. (M49: sqrt/pow/abs/… se
        // movieron a `std/math`, ya no son builtins → se prueba con `char_code`, que sí lo sigue siendo.)
        let src = "fn main() -> int {\n  let x = char_code('a');\n  print(x);\n  0\n}\n";
        // `char_code('a')` → int.
        let cc = src.lines().nth(1).unwrap().find("char_code").unwrap();
        let (ta, _, _) = hover_at(None, src, 1, cc).expect("hover de char_code");
        assert_eq!(ta, "char_code: fn(char) -> int");
        // `print(x)` → unit.
        let cp = src.lines().nth(2).unwrap().find("print").unwrap();
        let (tp, _, _) = hover_at(None, src, 2, cp).expect("hover de print");
        assert_eq!(tp, "print: fn(int) -> unit");
        // Signature help por firma fija de la tabla (`signature`): sigue sirviendo a `math.pow(` (el
        // envoltorio de `std/math`) aunque `pow` ya no sea builtin.
        let (ps, ret) = crate::builtins::signature("pow").expect("firma de pow");
        assert_eq!((ps, ret), (vec!["base: float", "exp: float"], "float"));
        // M46a: los builtins-método también llevan firma (para el detalle del popup).
        assert_eq!(crate::builtins::signature("len"), Some((vec!["c"], "int")));
        assert!(crate::builtins::signature("print").is_none(), "print es variádico ad-hoc, sin firma fija");
    }

    #[test]
    fn hover_muestra_documentacion() {
        // Una función con `///` encima: el hover trae la firma + la doc, como Markdown.
        let src = "/// Duplica un número.\n/// Segunda línea.\nfn duplicar(x: int) -> int { x * 2 }\nfn main() -> int {\n  duplicar(21)\n}\n";
        // Uso de `duplicar` en la línea 5 (0-based 4), col 2.
        let (info, _, _) = hover_at(None, src, 4, 2).expect("hover");
        assert!(info.starts_with("duplicar: fn(int) -> int"), "{info}");
        // La doc se localiza escaneando los `///` encima de la declaración.
        let mut docs = HashMap::new();
        let uri = format!("file://{}", std::env::temp_dir().join("ray_hover_doc.ray").display());
        docs.insert(uri.clone(), src.to_string());
        let d = doc_of_symbol(&uri, src, 4, 2, &docs).expect("documentación");
        assert_eq!(d, "Duplica un número.\nSegunda línea.");
        // El result de hover la mete en un bloque Markdown con la firma.
        let msg = obj(vec![("params", obj(vec![
            ("textDocument", obj(vec![("uri", text(&uri))])),
            ("position", obj(vec![("line", num(4)), ("character", num(2))])),
        ]))]);
        let r = hover_result(&msg, &docs);
        let val = r.get("contents").unwrap().get("value").and_then(Json::as_str).unwrap();
        assert!(val.contains("```raylang"), "firma en bloque de código: {val}");
        assert!(val.contains("Duplica un número."), "incluye la doc: {val}");
        assert_eq!(r.get("contents").unwrap().get("kind"), Some(&Json::Str("markdown".into())));

        // Un MÉTODO documentado con `///` también muestra su doc (M10.2h: los métodos se indexan).
        let src_m = "trait Show { fn mostrar(self) -> string; }\nstruct P { v: int }\nimpl Show for P {\n  /// Muestra el valor.\n  fn mostrar(self) -> string { \"p\" }\n}\nfn main() -> int {\n  let p = P { v: 1 };\n  print(p.mostrar());\n  0\n}\n";
        docs.insert(uri.clone(), src_m.to_string());
        // `p.mostrar()` en la línea 9 (0-based 8); `mostrar` tras el punto.
        let col_m = src_m.lines().nth(8).unwrap().find("mostrar").unwrap();
        let d = doc_of_symbol(&uri, src_m, 8, col_m, &docs).expect("doc del método");
        assert_eq!(d, "Muestra el valor.", "hover-doc de método");

        // Un símbolo SIN doc → hover en texto plano, sin bloque Markdown.
        let src2 = "fn triple(x: int) -> int { x * 3 }\nfn main() -> int { triple(1) }\n";
        docs.insert(uri.clone(), src2.to_string());
        let msg2 = obj(vec![("params", obj(vec![
            ("textDocument", obj(vec![("uri", text(&uri))])),
            ("position", obj(vec![("line", num(1)), ("character", num(19))])),
        ]))]);
        let r2 = hover_result(&msg2, &docs);
        assert_eq!(r2.get("contents").unwrap().get("kind"), Some(&Json::Str("plaintext".into())));
    }

    #[test]
    fn hover_doc_de_builtins_y_prelude() {
        let mut docs = HashMap::new();
        let uri = format!("file://{}", std::env::temp_dir().join("ray_doc_builtin.ray").display());
        // Un builtin (`pow`) no tiene declaración en el archivo: la doc sale de la tabla
        // (`builtins::doc`, en inglés).
        let src = "fn main() -> int {\n  print(pow(2.0, 10.0));\n  0\n}\n";
        docs.insert(uri.clone(), src.to_string());
        let col = src.lines().nth(1).unwrap().find("pow").unwrap();
        let d = doc_of_symbol(&uri, src, 1, col, &docs).expect("doc de pow");
        assert!(d.contains("Raises `base` to the power"), "{d}");
        // Un símbolo del PRELUDE (`sort`): su declaración vive en la fuente inyectada, no en el
        // buffer → la doc se busca por nombre en `prelude::SOURCE`.
        let src2 = "fn main() -> int {\n  let xs = sort([3, 1, 2]);\n  xs[0]\n}\n";
        docs.insert(uri.clone(), src2.to_string());
        let col2 = src2.lines().nth(1).unwrap().find("sort").unwrap();
        let d2 = doc_of_symbol(&uri, src2, 1, col2, &docs).expect("doc de sort");
        assert!(!d2.is_empty(), "doc del prelude para sort: {d2}");
        // Una variable local que TAPA un nombre de builtin no hereda su doc: `min` local.
        let src3 = "fn main() -> int {\n  let min = 5;\n  min\n}\n";
        docs.insert(uri.clone(), src3.to_string());
        let col3 = src3.lines().nth(2).unwrap().find("min").unwrap();
        assert_eq!(doc_of_symbol(&uri, src3, 2, col3, &docs), None, "local sin doc, aunque exista el builtin min");
        // El completion adjunta la doc del builtin como Markdown.
        let msg = obj(vec![("params", obj(vec![
            ("textDocument", obj(vec![("uri", text(&uri))])),
            ("position", obj(vec![("line", num(1)), ("character", num(2))])),
        ]))]);
        docs.insert(uri.clone(), src.to_string());
        let r = completion_result(&msg, &docs);
        let Json::Arr(items) = &r else { panic!("completion no es lista") };
        // M49: `pow`/`sqrt`/`abs`/… se movieron a `std/math` (ya no en el completion global de builtins);
        // se prueba con `char_code`, que sigue siendo builtin.
        let cc = items.iter().find(|i| i.get("label") == Some(&Json::Str("char_code".into()))).expect("char_code en completion");
        let doc_val = cc.get("documentation").and_then(|d| d.get("value")).and_then(Json::as_str).expect("documentation de char_code");
        assert!(doc_val.contains("Unicode"), "{doc_val}");
    }

    #[test]
    fn document_highlight_resalta_ocurrencias() {
        let mut docs = HashMap::new();
        let uri = "file:///t.ray".to_string();
        // `x` se declara y se usa dos veces.
        let src = "fn main() -> int {\n  let x = 3;\n  x + x\n}\n";
        docs.insert(uri.clone(), src.to_string());
        // Cursor sobre el primer uso de `x` (línea 3 0-based 2, col 2).
        let msg = obj(vec![
            ("params", obj(vec![
                ("textDocument", obj(vec![("uri", text(&uri))])),
                ("position", obj(vec![("line", num(2)), ("character", num(2))])),
            ])),
        ]);
        let hl = document_highlight_result(&msg, &docs);
        let arr = hl.as_array().expect("array");
        // Declaración (let x) + dos usos = 3 rangos, sin duplicados.
        assert_eq!(arr.len(), 3, "decl + 2 usos: {arr:?}");
        // Exactamente uno es Write (kind 3, la declaración); el resto Text (kind 1).
        let writes = arr.iter().filter(|h| h.get("kind") == Some(&Json::Num(3.0))).count();
        assert_eq!(writes, 1, "la declaración es el único Write");
    }

    #[test]
    fn hover_con_modulos() {
        // Antes: un archivo con `import` no daba NINGÚN hover (el checker fallaba por el import).
        let dir = std::env::temp_dir().join("ray_lsp_hover_mod");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("geo.ray"), "pub fn duplicar(x: int) -> int { x * 2 }\n").unwrap();
        let uri = format!("file://{}", dir.join("main.ray").display());
        let src = "from geo import duplicar;\nfn main() -> int {\n  let y = 5;\n  duplicar(y)\n}\n";

        // Variable LOCAL: hover funciona pese al import (índice sobre el programa fusionado).
        let (t, _, _) = hover_at(Some(&uri), src, 3, 11).expect("hover sobre 'y'");
        assert_eq!(t, "y: int");
        // Función IMPORTADA de otro módulo: muestra su tipo en forma de fachada (`geo.duplicar`,
        // no el `geo::duplicar` interno).
        let (t, _, _) = hover_at(Some(&uri), src, 3, 2).expect("hover sobre 'duplicar'");
        assert_eq!(t, "geo.duplicar: fn(int) -> int", "forma de fachada, sin '::'");

        // Rename de una variable local en un archivo multi-módulo: seguro (vive entera aquí).
        let (_, _, _, is_local) = symbol_occurrences(Some(&uri), src, 3, 11).expect("símbolo");
        assert!(is_local, "'y' es local → renombrable");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hover_de_campos_y_metodos() {
        // Campo de struct: `p.x` → el tipo del campo, en la posición del nombre tras el `.`.
        let src = "struct Punto { x: int, y: int }\nfn main() -> int {\n  let p = Punto { x: 3, y: 4 };\n  p.x + p.y\n}\n";
        let (t, _, _) = hover_at(None, src, 3, 4).expect("hover del campo x");
        assert_eq!(t, "x: int");

        // Método de trait: `n.doblar()` → la firma del método (incluye el receptor).
        let src = "trait D { fn doblar(self) -> int; }\nstruct N { v: int }\nimpl D for N { fn doblar(self) -> int { self.v * 2 } }\nfn main() -> int {\n  let n = N { v: 21 };\n  n.doblar()\n}\n";
        let (t, _, _) = hover_at(None, src, 5, 4).expect("hover del método doblar");
        assert_eq!(t, "doblar: fn(N) -> int");

        // Método por UFCS a una función del prelude: `xs.map(f)`.
        let src = "fn main() -> int {\n  let xs = [1, 2, 3];\n  xs.map(fn(a: int) -> int { a + 1 });\n  0\n}\n";
        let (t, _, _) = hover_at(None, src, 2, 5).expect("hover del método map");
        assert!(t.starts_with("map: fn("), "{t}");
    }

    #[test]
    fn hover_dentro_de_interpolacion() {
        // Las expresiones de `${…}` se re-lexan con posiciones reales → hover funciona dentro.
        let src = "fn main() -> int {\n  let x = 7;\n  print(\"n=${x} y ${x + 1}\");\n  0\n}\n";
        // línea 3 (0-based 2): `  print("n=${x} y ${x + 1}");`
        //   `x` de `${x}` en col 13; `x` de `${x + 1}` en col 20.
        let (t, _, _) = hover_at(None, src, 2, 13).expect("hover 'x' en ${x}");
        assert_eq!(t, "x: int");
        let (t, _, _) = hover_at(None, src, 2, 20).expect("hover 'x' en ${x + 1}");
        assert_eq!(t, "x: int");
    }

    #[test]
    fn nombre_fachada_colapsa_namespaces() {
        // Sin imports conocidos → fallback `primer.último` (respeta la cápsula, sin `::`).
        assert_eq!(facade_name("geo::formas::circulo::Circulo", &[]), "geo.Circulo");
        assert_eq!(facade_name("geo::area: fn(geo::formas::circulo::Circulo) -> int", &[]),
            "geo.area: fn(geo.Circulo) -> int");
        // Nombres sin namespacing (locales, primitivos) intactos.
        assert_eq!(facade_name("c: Punto", &[]), "c: Punto");
        assert_eq!(facade_name("n: int", &[]), "n: int");
        assert_eq!(facade_name("f: fn(int, bool) -> string", &[]), "f: fn(int, bool) -> string");
        // M49: con el import `std/math` (leaf `math`, ns_prefix `std::math`), la fachada usa el LEAF
        // con el que el usuario accede: `math.sqrt`, no `std.sqrt`. Una cápsula `geo` (ns_prefix `geo`)
        // sigue mostrando su raíz (`geo.Circulo`) porque su ns_prefix también es prefijo.
        let imp = vec![("math".to_string(), "std::math".to_string()), ("geo".to_string(), "geo".to_string())];
        assert_eq!(facade_name("std::math::sqrt: fn(float) -> float", &imp), "math.sqrt: fn(float) -> float");
        assert_eq!(facade_name("std::math::PI: float", &imp), "math.PI: float");
        assert_eq!(facade_name("geo::formas::circulo::Circulo", &imp), "geo.Circulo");
    }

    #[test]
    fn definicion_cruza_modulos() {
        let dir = std::env::temp_dir().join("ray_lsp_def_mod");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("geo.ray"), "pub fn duplicar(x: int) -> int { x * 2 }\n").unwrap();
        let uri = format!("file://{}", dir.join("main.ray").display());
        let src = "from geo import duplicar;\nfn main() -> int {\n  let r = duplicar(21);\n  r\n}\n";

        // Ir-a-definición de la función IMPORTADA → salta al otro archivo (geo.ray, línea 0).
        let (turi, line, _, _) = definition_at(&uri, src, 2, 10).expect("def de duplicar");
        assert!(turi.ends_with("geo.ray"), "el destino es el archivo del módulo: {turi}");
        assert_eq!(line, 0);

        // Ir-a-definición de una variable LOCAL → se queda en este archivo.
        let (turi, line, _, _) = definition_at(&uri, src, 3, 2).expect("def de r");
        assert!(turi.ends_with("main.ray"), "{turi}");
        assert_eq!(line, 2, "la declaración de 'r' está en la línea 3 (0-based 2)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn references_cruzan_modulos() {
        let dir = std::env::temp_dir().join("ray_lsp_refs_mod");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // geo.ray: define `duplicar` y lo usa una vez internamente.
        std::fs::write(dir.join("geo.ray"),
            "pub fn duplicar(x: int) -> int { x * 2 }\npub fn cuad(x: int) -> int { duplicar(x) }\n").unwrap();
        let uri = format!("file://{}", dir.join("main.ray").display());
        let src = "from geo import duplicar;\nfn main() -> int {\n  duplicar(21)\n}\n";

        // Find-references desde el uso en main.ray → apariciones en AMBOS archivos.
        let locs = references_cross(&uri, src, 2, 2, true).expect("references");
        let files: std::collections::HashSet<&str> = locs.iter()
            .map(|(u, _)| u.rsplit('/').next().unwrap())
            .collect();
        assert!(files.contains("geo.ray"), "incluye la declaración y el uso interno: {locs:?}");
        assert!(files.contains("main.ray"), "incluye el uso del archivo abierto: {locs:?}");
        // 3 apariciones: declaración + uso interno en geo.ray, uso en main.ray.
        assert_eq!(locs.len(), 3, "{locs:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_cruza_modulos() {
        let dir = std::env::temp_dir().join("ray_lsp_ren_mod");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("geo.ray"),
            "pub fn duplicar(x: int) -> int { x * 2 }\npub fn cuad(x: int) -> int { duplicar(x) }\n").unwrap();
        let uri = format!("file://{}", dir.join("main.ray").display());

        // Rename de una función importada → toca AMBOS archivos, INCLUIDA la línea del import.
        let src = "from geo import duplicar;\nfn main() -> int {\n  duplicar(21)\n}\n";
        let pos = rename_cross(&uri, src, 2, 2).expect("rename cross-módulo");
        let files: std::collections::HashSet<&str> =
            pos.iter().map(|(u, _)| u.rsplit('/').next().unwrap()).collect();
        assert!(files.contains("geo.ray") && files.contains("main.ray"), "{pos:?}");
        // 4 posiciones: decl + uso interno (geo.ray), especificador de import + uso (main.ray).
        assert_eq!(pos.len(), 4, "{pos:?}");
        // El especificador del import (main.ray línea 0) está entre las posiciones.
        assert!(pos.iter().any(|(u, (l, _, _))| u.ends_with("main.ray") && *l == 0), "falta el import: {pos:?}");

        // Con `as alias`, el rename se NIEGA (los usos van por el alias → incompleto e inseguro).
        let src = "from geo import duplicar as d;\nfn main() -> int { d(21) }\n";
        assert!(rename_cross(&uri, src, 1, 19).is_none(), "rename con alias debe rechazarse");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uri_a_ruta_decodifica() {
        assert_eq!(uri_to_path("file:///a/b/c.ray"), Some(PathBuf::from("/a/b/c.ray")));
        assert_eq!(uri_to_path("file:///a/mi%20carpeta/x.ray"), Some(PathBuf::from("/a/mi carpeta/x.ray")));
        assert_eq!(uri_to_path("untitled:Untitled-1"), None); // buffer sin archivo → single-file
    }

    #[test]
    fn referencias_de_variable_local() {
        // `let x = 1; x + x` → declaración + 2 usos.
        let src = "fn main() -> int {\n  let x = 1;\n  x + x\n}\n";
        // Cursor sobre el primer uso de `x` (línea 3 → 0-based 2, col 2).
        let (name, decl, uses, is_local) = symbol_occurrences(None, src, 2, 2).expect("hay símbolo");
        assert_eq!(name, "x");
        assert_eq!(decl, Some((1, 6, 1)), "la declaración apunta al NOMBRE x, no al 'let'");
        assert_eq!(uses.len(), 2, "x + x son dos usos");
        assert!(is_local, "una variable local vive entera en este archivo");
        // Y desde el nombre de la declaración (línea 2 → 0-based 1, col 6) da lo mismo.
        let (n2, d2, u2, _) = symbol_occurrences(None, src, 1, 6).expect("símbolo desde la declaración");
        assert_eq!((n2, d2, u2.len()), ("x".to_string(), Some((1, 6, 1)), 2));
    }

    #[test]
    fn referencias_distinguen_ambitos() {
        // Dos `x` en funciones distintas no se mezclan (claves de declaración distintas).
        let src = "fn f(a: int) -> int {\n  let x = a;\n  x + x\n}\nfn main() -> int {\n  let x = 9;\n  x\n}\n";
        // El `x` de `f` (línea 3 → 0-based 2): 2 usos.
        let (_, _, uf, _) = symbol_occurrences(None, src, 2, 2).unwrap();
        assert_eq!(uf.len(), 2);
        // El `x` de `main` (línea 7 → 0-based 6): 1 uso.
        let (_, _, um, _) = symbol_occurrences(None, src, 6, 2).unwrap();
        assert_eq!(um.len(), 1);
    }

    #[test]
    fn referencias_de_funcion() {
        // Una función llamada dos veces: declaración + 2 usos.
        let src = "fn doble(n: int) -> int { n + n }\nfn main() -> int {\n  doble(1) + doble(2)\n}\n";
        // Cursor sobre la primera llamada `doble` (línea 3 → 0-based 2, col 2).
        let (name, decl, uses, _) = symbol_occurrences(None, src, 2, 2).expect("hay símbolo");
        assert_eq!(name, "doble");
        assert_eq!(decl, Some((0, 3, 5)), "la declaración apunta al nombre 'doble' tras 'fn '");
        assert_eq!(uses.len(), 2);
    }

    #[test]
    fn rename_produce_workspace_edit() {
        let src = "fn main() -> int {\n  let x = 1;\n  x + x\n}\n";
        let msg = json::parse(
            r#"{"params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":2,"character":2},"newName":"y"}}"#
        ).unwrap();
        let mut docs = HashMap::new();
        docs.insert("file:///t.ray".to_string(), src.to_string());
        let res = rename_result(&msg, &docs);
        let edits = res.get("changes").unwrap().get("file:///t.ray").unwrap().as_array().unwrap();
        assert_eq!(edits.len(), 3, "declaración + 2 usos");
        assert_eq!(edits[0].get("newText"), Some(&Json::Str("y".to_string())));
    }

    #[test]
    fn completion_ofrece_simbolos_builtins_y_keywords() {
        let src = "struct Punto { x: int }\nfn doble(n: int) -> int { n + n }\nfn main() -> int { 0 }\n";
        let msg = json::parse(
            r#"{"params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":2,"character":0}}}"#
        ).unwrap();
        let mut docs = HashMap::new();
        docs.insert("file:///t.ray".to_string(), src.to_string());
        let res = completion_result(&msg, &docs);
        let items = res.as_array().unwrap();
        let labels: Vec<&str> = items.iter().filter_map(|i| i.get("label").and_then(|l| l.as_str())).collect();
        assert!(labels.contains(&"doble"), "función propia\n{labels:?}");
        assert!(labels.contains(&"Punto"), "tipo propio");
        assert!(labels.contains(&"print"), "builtin");
        assert!(labels.contains(&"map"), "función del prelude");
        assert!(labels.contains(&"while"), "palabra clave");
        // No expone nombres sintéticos (manglados, internos).
        assert!(!labels.iter().any(|l| l.contains('#') || l.starts_with("__")), "sin nombres sintéticos");
    }

    /// Helper: labels de la completion en `(line, character)` (0-basados) sobre `src`.
    fn completion_labels(src: &str, line: usize, character: usize) -> Vec<String> {
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{character}}}}}}}"#
        )).unwrap();
        let mut docs = HashMap::new();
        docs.insert("file:///t.ray".to_string(), src.to_string());
        completion_result(&msg, &docs).as_array().unwrap().iter()
            .filter_map(|i| i.get("label").and_then(|l| l.as_str()).map(|s| s.to_string()))
            .collect()
    }

    #[test]
    fn completion_de_miembros_de_struct() {
        // M45: `p.` sobre un struct ofrece sus campos y sus métodos de trait, no los símbolos de archivo.
        let src = "struct P { x: int, y: int }\ntrait Ver { fn ver(self) -> int; }\nimpl Ver for P { fn ver(self) -> int { self.x } }\nfn suma(p: P) -> int { p.x + p.y }\nfn main() -> int {\n    let p = P { x: 1, y: 2 };\n    p.\n    0\n}\n";
        let labels = completion_labels(src, 6, 6); // línea "    p." (0-basada), tras el punto
        assert!(labels.contains(&"x".to_string()) && labels.contains(&"y".to_string()), "campos: {labels:?}");
        assert!(labels.contains(&"ver".to_string()), "método de trait: {labels:?}");
        assert!(labels.contains(&"suma".to_string()), "UFCS del usuario: {labels:?}");
        // NO ofrece los símbolos de archivo (no es una completion de archivo tras el punto).
        assert!(!labels.contains(&"print".to_string()), "sin builtins globales: {labels:?}");
        assert!(!labels.contains(&"while".to_string()), "sin palabras clave: {labels:?}");
    }

    #[test]
    fn completion_de_miembros_de_string_y_array() {
        // string: builtins de string + métodos de trait; NADA de funciones de E/S que toman una ruta.
        let s = "fn main() -> int {\n    let s = \"h\";\n    s.\n    0\n}\n";
        let ls = completion_labels(s, 2, 6);
        assert!(ls.contains(&"trim".to_string()) && ls.contains(&"split".to_string()) && ls.contains(&"len".to_string()), "string builtins: {ls:?}");
        assert!(!ls.contains(&"read_file".to_string()) && !ls.contains(&"env".to_string()), "sin E/S sobre string: {ls:?}");
        // array: builtins + orden superior del prelude por UFCS.
        let a = "fn main() -> int {\n    let xs = [1, 2, 3];\n    xs.\n    0\n}\n";
        let la = completion_labels(a, 2, 7);
        for m in ["len", "push", "reverse", "map", "filter", "fold", "sort"] {
            assert!(la.contains(&m.to_string()), "array debe ofrecer '{m}': {la:?}");
        }
    }

    #[test]
    fn completion_de_miembros_en_contexto_de_expresion() {
        // M45b: `x.` como argumento de una llamada NO debe romper el parseo (bug del `;` dentro
        // del paréntesis). `sum(x.)` ofrece los miembros del array.
        let src = "fn sum(xs: [int]) -> int { 0 }\nfn main() -> int {\n    let x = [1, 2];\n    let y = sum(x.);\n    0\n}\n";
        let labels = completion_labels(src, 3, 18); // tras "sum(x." (el punto está en col 17, cursor 18)
        assert!(labels.contains(&"len".to_string()) && labels.contains(&"map".to_string()), "en expresión: {labels:?}");
    }

    #[test]
    fn completion_de_miembros_snippet_y_doc() {
        // M45b/M46c: un método con args → snippet con placeholders por parámetro, sin el receptor
        // (`doblar(${1:k})`); un campo no inserta `()`; los métodos del usuario traen su doc `///`.
        let src = "struct P { x: int }\n/// Duplica x.\nfn doblar(p: P, k: int) -> int { p.x * k }\nfn main() -> int {\n    let p = P { x: 1 };\n    p.\n    0\n}\n";
        let msg = json::parse(
            r#"{"params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":5,"character":6}}}"#
        ).unwrap();
        let mut docs = HashMap::new();
        docs.insert("file:///t.ray".to_string(), src.to_string());
        let items = completion_result(&msg, &docs);
        let arr = items.as_array().unwrap();
        let doblar = arr.iter().find(|i| i.get("label").and_then(|l| l.as_str()) == Some("doblar")).expect("doblar");
        assert_eq!(doblar.get("insertText").and_then(|t| t.as_str()), Some("doblar(${1:k})"), "snippet con placeholder, sin receptor");
        assert!(doblar.get("command").is_some(), "dispara signature help");
        let doc = doblar.get("documentation").and_then(|d| d.get("value")).and_then(|v| v.as_str()).unwrap_or("");
        assert!(doc.contains("Duplica x"), "doc /// del método: {doc}");
        // el campo x no es invocable → sin insertText de llamada.
        let x = arr.iter().find(|i| i.get("label").and_then(|l| l.as_str()) == Some("x")).expect("x");
        assert!(x.get("insertText").is_none(), "un campo no inserta ()");
    }

    #[test]
    fn completion_de_miembros_en_interpolacion() {
        // M45b: dentro de `${x.}` el LSP ofrece los miembros (el reparado no rompe la cadena).
        let src = "fn main() -> int {\n    let x = [1, 2];\n    let y = \"n ${x.}\";\n    0\n}\n";
        let line = "    let y = \"n ${x.}\";";
        let col = line.find("x.").unwrap() + 2; // tras el punto
        let labels = completion_labels(src, 2, col);
        assert!(labels.contains(&"len".to_string()) && labels.contains(&"push".to_string()), "en interpolación: {labels:?}");
    }

    #[test]
    fn completion_de_import_simbolos_y_rutas() {
        // M45c: crea un proyecto temporal en disco y comprueba el completion de import.
        let base = std::env::temp_dir().join("ray_lsp_import_test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("geo/formas")).unwrap();
        std::fs::create_dir_all(base.join("util")).unwrap();
        let w = |rel: &str, txt: &str| std::fs::write(base.join(rel), txt).unwrap();
        w("main.ray", "fn main() -> int { 0 }\n");
        w("geo.ray", "pub struct Circulo { r: int }\npub fn area(c: Circulo) -> int { c.r }\nfn interno() -> int { 0 }\n");
        w("geo/formas/circulo.ray", "pub fn dibujar() -> int { 0 }\n");
        w("util/mod.ray", "pub fn publico() -> int { 0 }\n"); // cápsula
        w("util/interno.ray", "pub fn oculto() -> int { 0 }\n"); // interno a la cápsula

        let uri = format!("file://{}/main.ray", base.display());
        let labels = |line: &str, ch: usize| -> Vec<String> {
            let src = format!("{line}\nfn main() -> int {{ 0 }}\n");
            let mut docs = HashMap::new();
            docs.insert(uri.clone(), src);
            let msg = json::parse(&format!(
                r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":0,"character":{ch}}}}}}}"#
            )).unwrap();
            completion_result(&msg, &docs).as_array().unwrap().iter()
                .filter_map(|i| i.get("label").and_then(|l| l.as_str()).map(|s| s.to_string())).collect()
        };

        // from geo import <cursor> → símbolos `pub` (no `interno`).
        let syms = labels("from geo import ", 16);
        assert!(syms.contains(&"Circulo".to_string()) && syms.contains(&"area".to_string()), "pub de geo: {syms:?}");
        assert!(!syms.contains(&"interno".to_string()), "no expone lo privado: {syms:?}");
        assert!(!syms.contains(&"print".to_string()), "no cae al completion de archivo: {syms:?}");

        // import <cursor> → rutas de módulo; la cápsula `util` sí, su interno NO.
        let paths = labels("import ", 7);
        assert!(paths.contains(&"geo".to_string()), "módulo geo: {paths:?}");
        assert!(paths.contains(&"geo/formas/circulo".to_string()), "ruta de directorios: {paths:?}");
        assert!(paths.contains(&"util".to_string()), "la cápsula util: {paths:?}");
        assert!(!paths.contains(&"util/interno".to_string()), "el interno de la cápsula queda oculto: {paths:?}");

        // M45c-3: acceso calificado `u.` (alias) / `circulo.` (leaf) → símbolos `pub` del módulo.
        let qualified = |body: &str, line: usize, ch: usize| -> Vec<String> {
            let src = format!("import geo/formas/circulo;\nimport geo as u;\nfn main() -> int {{\n{body}\n0\n}}\n");
            let mut docs = HashMap::new();
            docs.insert(uri.clone(), src);
            let msg = json::parse(&format!(
                r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
            )).unwrap();
            completion_result(&msg, &docs).as_array().unwrap().iter()
                .filter_map(|i| i.get("label").and_then(|l| l.as_str()).map(|s| s.to_string())).collect()
        };
        let by_alias = qualified("    u.", 3, 6); // `import geo as u` → símbolos pub de geo
        assert!(by_alias.contains(&"Circulo".to_string()) && by_alias.contains(&"area".to_string()), "alias u.: {by_alias:?}");
        assert!(!by_alias.contains(&"interno".to_string()), "no expone lo privado del módulo: {by_alias:?}");
        let by_leaf = qualified("    circulo.", 3, 12); // leaf `circulo` (geo/formas/circulo.ray)
        assert!(by_leaf.contains(&"dibujar".to_string()), "leaf circulo.: {by_leaf:?}");

        // M46c: en el acceso calificado, las funciones traen firma + snippet con placeholders, y las
        // firmas de un RE-EXPORT de cápsula se resuelven en el módulo origen (no en `mod.ray`).
        std::fs::write(base.join("util/mod.ray"),
            "pub from util/interno import saludar;\npub fn publico() -> int { 0 }\n").unwrap();
        std::fs::write(base.join("util/interno.ray"),
            "pub fn saludar(nombre: string, veces: int) -> string { nombre }\n").unwrap();
        let items_at = |body: &str, line: usize, ch: usize| -> Json {
            let src = format!("import util;\nfn main() -> int {{\n{body}\n0\n}}\n");
            let mut docs = HashMap::new();
            docs.insert(uri.clone(), src);
            let msg = json::parse(&format!(
                r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
            )).unwrap();
            completion_result(&msg, &docs)
        };
        let it = |arr: &Json, label: &str| arr.as_array().unwrap().iter()
            .find(|i| i.get("label").and_then(|l| l.as_str()) == Some(label)).cloned();
        let items = items_at("    util.", 2, 9);
        let saludar = it(&items, "saludar").expect("re-export saludar");
        assert_eq!(saludar.get("insertText").and_then(Json::as_str), Some("saludar(${1:nombre}, ${2:veces})"),
                   "firma del re-export resuelta en el módulo origen + placeholders");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn completion_simbolos_importados_y_variantes_de_enum() {
        // Proyecto: un módulo `figuras` con una función `pub`, un struct y un enum; el archivo de
        // entrada los importa.
        let base = std::env::temp_dir().join("ray_lsp_import_syms_test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("main.ray"), "fn main() -> int { 0 }\n").unwrap();
        std::fs::write(base.join("figuras.ray"),
            "pub fn area(a: int, b: int) -> int { a * b }\npub struct Rect { ancho: int }\npub enum Orientacion { Horizontal, Vertical }\n").unwrap();
        let uri = format!("file://{}/main.ray", base.display());
        let items = |body: &str, line: usize, ch: usize| -> Vec<(String, i64)> {
            let src = format!("import figuras;\nfrom figuras import Orientacion, area;\nfn main() -> int {{\n{body}\n0\n}}\n");
            let mut docs = HashMap::new();
            docs.insert(uri.clone(), src);
            let msg = json::parse(&format!(
                r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
            )).unwrap();
            completion_result(&msg, &docs).as_array().unwrap().iter()
                .filter_map(|i| Some((i.get("label")?.as_str()?.to_string(), as_usize(i.get("kind")?)? as i64)))
                .collect()
        };
        // Completion de archivo: el nombre de módulo `figuras` (kind 9) y los from-imports.
        let file_items = items("    x", 3, 5);
        assert!(file_items.iter().any(|(l, k)| l == "figuras" && *k == 9), "módulo figuras (kind Module): {file_items:?}");
        assert!(file_items.iter().any(|(l, _)| l == "Orientacion"), "from-import Orientacion: {file_items:?}");
        assert!(file_items.iter().any(|(l, _)| l == "area"), "from-import area: {file_items:?}");
        // `Orientacion.` → sus variantes (kind 20 = EnumMember).
        let vars = items("    Orientacion.", 3, 16);
        assert!(vars.iter().any(|(l, k)| l == "Horizontal" && *k == 20), "variante Horizontal: {vars:?}");
        assert!(vars.iter().any(|(l, _)| l == "Vertical"), "variante Vertical: {vars:?}");
        assert!(!vars.iter().any(|(l, _)| l == "figuras"), "tras el punto NO sale la completion de archivo: {vars:?}");

        // Una variante con payload muestra los tipos en el popup (`labelDetails.detail`).
        let src = "enum Figura { Circulo(float), Rect(float, float), Punto }\nfn main() -> int {\n    Figura.\n0\n}\n";
        let mut docs = HashMap::new();
        docs.insert(uri.clone(), src.to_string());
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":2,"character":11}}}}}}"#
        )).unwrap();
        let vs = completion_result(&msg, &docs);
        let rect = vs.as_array().unwrap().iter()
            .find(|i| i.get("label").and_then(Json::as_str) == Some("Rect")).unwrap();
        let det = rect.get("labelDetails").and_then(|d| d.get("detail")).and_then(Json::as_str);
        assert_eq!(det, Some("(float, float)"), "tipos del payload en el popup: {rect:?}");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn completion_y_signature_de_funciones_asociadas() {
        // M48.1: `Channel.` completa `new`/`bounded` (kind 3); el sig help de `Channel.bounded(` sale
        // del registro de asociadas.
        let uri = "file:///t.ray";
        let src = "fn main() -> int {\n    let c: Channel<int> = Channel.\n    0\n}\n";
        let mut docs = HashMap::new();
        docs.insert(uri.to_string(), src.to_string());
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":1,"character":34}}}}}}"#
        )).unwrap();
        let vs = completion_result(&msg, &docs);
        let labels: Vec<&str> = vs.as_array().unwrap().iter()
            .filter_map(|i| i.get("label").and_then(Json::as_str)).collect();
        assert!(labels.contains(&"new") && labels.contains(&"bounded"), "asociadas de Channel: {labels:?}");
        // Signature help dentro de `Channel.bounded(`.
        let src2 = "fn main() -> int {\n    let c: Channel<int> = Channel.bounded(\n    0\n}\n";
        docs.insert(uri.to_string(), src2.to_string());
        let msg2 = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":1,"character":42}}}}}}"#
        )).unwrap();
        let r = signature_help_result(&msg2, &docs);
        let label = r.get("signatures").and_then(|s| s.as_array()).and_then(|a| a.first())
            .and_then(|s| s.get("label")).and_then(Json::as_str).unwrap_or("");
        assert_eq!(label, "Channel.bounded(n: int) -> Channel<T>", "sig de Channel.bounded: {r:?}");
    }

    #[test]
    fn signature_help_metodos_y_prelude() {
        // M46b: el signature help resuelve funciones del prelude y recorta el receptor en un método,
        // pero NO en una llamada calificada de módulo (esa parte cross-módulo se cubre en el CLI).
        let sig = |src: &str, line: usize, ch: usize| -> Option<(String, usize)> {
            let mut docs = HashMap::new();
            docs.insert("file:///t.ray".to_string(), src.to_string());
            let msg = json::parse(&format!(
                r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
            )).unwrap();
            let r = signature_help_result(&msg, &docs);
            if r == Json::Null { return None; }
            let label = r.get("signatures").and_then(|s| s.as_array()).and_then(|a| a.first())
                .and_then(|s| s.get("label")).and_then(Json::as_str)?.to_string();
            let active = r.get("activeParameter").and_then(as_usize).unwrap_or(0);
            Some((label, active))
        };
        // Prelude: sort(.
        assert_eq!(sig("fn main() -> int {\n    let xs = [3,1];\n    sort(\n}\n", 2, 9),
                   Some(("fn sort(a: [T]) -> [T]".into(), 0)));
        // Método: p.doblar( → receptor recortado, `(k: int)`.
        let m = "struct P { x: int }\nfn doblar(p: P, k: int) -> int { p.x }\nfn main() -> int {\n    let p = P { x: 1 };\n    p.doblar(\n}\n";
        assert_eq!(sig(m, 4, 13), Some(("fn doblar(k: int) -> int".into(), 0)));
        // Función libre con una coma: doblar(1, → firma completa, param activo 1.
        let free_call = "fn doblar(p: int, k: int) -> int { p }\nfn main() -> int {\n    doblar(1, \n}\n";
        assert_eq!(sig(free_call, 2, 13), Some(("fn doblar(p: int, k: int) -> int".into(), 1)));
        // Builtin: pow(.
        assert_eq!(sig("fn main() -> int {\n    let x = pow(\n    0\n}\n", 1, 16),
                   Some(("fn pow(base: float, exp: float) -> float".into(), 0)));
        // Construcción de variante de enum: `Figura.Rect(1.0, ` → firma con los tipos del payload,
        // param activo 1. No es una `fn`, pero el receptor es un enum con esa variante.
        let e = "enum Figura { Circulo(float), Rect(float, float) }\nfn main() -> int {\n    let r: Figura = Figura.Rect(1.0, \n    0\n}\n";
        assert_eq!(sig(e, 2, 36), Some(("Figura.Rect(float, float)".into(), 1)));
    }

    #[test]
    fn completion_snippet_con_placeholders_por_parametro() {
        // M46c: el insertText usa un placeholder por parámetro (nombre), navegable con Tab.
        let insert = |src: &str, line: usize, ch: usize, label: &str| -> Option<String> {
            let mut docs = HashMap::new();
            docs.insert("file:///t.ray".to_string(), src.to_string());
            let msg = json::parse(&format!(
                r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
            )).unwrap();
            completion_result(&msg, &docs).as_array().unwrap().iter()
                .find(|i| i.get("label").and_then(|l| l.as_str()) == Some(label))
                .and_then(|i| i.get("insertText")).and_then(Json::as_str).map(|s| s.to_string())
        };
        // Función de archivo con dos params.
        let f = "fn doblar(p: int, k: int) -> int { p }\nfn main() -> int {\n    dob\n    0\n}\n";
        assert_eq!(insert(f, 2, 7, "doblar").as_deref(), Some("doblar(${1:p}, ${2:k})"));
        // Método: receptor recortado → solo el resto.
        let m = "struct P { x: int }\nfn tri(p: P, k: int) -> int { p.x }\nfn main() -> int {\n    let p = P { x: 1 };\n    p.\n    0\n}\n";
        assert_eq!(insert(m, 4, 6, "tri").as_deref(), Some("tri(${1:k})"));
        // Builtin sin argumentos → `()`.
        let a = "fn main() -> int {\n    let xs = [1];\n    xs.\n    0\n}\n";
        assert_eq!(insert(a, 2, 7, "len").as_deref(), Some("len()"));
    }

    #[test]
    fn completion_de_campos_de_literal_de_struct() {
        // M47a: dentro de `Nombre { … }` (posición de nombre de campo), los campos del struct.
        let labels = |body: &str, line: usize, ch: usize| -> Vec<(String, i64, Option<String>)> {
            let src = format!("struct Punto {{ x: int, y: int }}\nfn dobla(n: int) -> int {{ n }}\nfn main() -> int {{\n{body}\n0\n}}\n");
            let mut docs = HashMap::new();
            docs.insert("file:///t.ray".to_string(), src);
            let msg = json::parse(&format!(
                r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
            )).unwrap();
            completion_result(&msg, &docs).as_array().unwrap().iter()
                .map(|i| (i.get("label").unwrap().as_str().unwrap().to_string(),
                          as_usize(i.get("kind").unwrap()).unwrap() as i64,
                          i.get("insertText").and_then(Json::as_str).map(|s| s.to_string())))
                .collect()
        };
        // `Punto { |` → ambos campos, kind Field (5), insertText `campo: `.
        let empty = labels("    let p = Punto { ", 3, 20);
        assert!(empty.iter().any(|(l, k, ins)| l == "x" && *k == 5 && ins.as_deref() == Some("x: ")), "campo x: {empty:?}");
        assert!(empty.iter().any(|(l, _, _)| l == "y"), "campo y: {empty:?}");
        assert!(!empty.iter().any(|(l, _, _)| l == "print"), "no cae a la completion de archivo: {empty:?}");
        // `Punto { x: 1, |` → solo el campo que falta.
        let one = labels("    let p = Punto { x: 1, ", 3, 26);
        assert!(one.iter().any(|(l, _, _)| l == "y") && !one.iter().any(|(l, _, _)| l == "x"),
                "excluye el campo ya escrito: {one:?}");
        // `Punto { x: dob|` (posición de VALOR) → cae a la completion de archivo (dobla), no campos.
        let value_labels = labels("    let p = Punto { x: dob", 3, 26);
        assert!(value_labels.iter().any(|(l, _, _)| l == "dobla"), "en posición de valor, completion de archivo: {value_labels:?}");

        // M47b: al teclear el TIPO, un ítem extra `Punto {…}` que inserta el literal con placeholders,
        // aparte del tipo pelado `Punto`.
        let type_labels = labels("    let p = Pun", 3, 15);
        assert!(type_labels.iter().any(|(l, k, _)| l == "Punto" && *k == 22), "el tipo pelado sigue: {type_labels:?}");
        assert!(type_labels.iter().any(|(l, k, ins)| l == "Punto {…}" && *k == 15
            && ins.as_deref() == Some("Punto { x: ${1:int}, y: ${2:int} }")), "el literal-snippet: {type_labels:?}");
    }

    #[test]
    fn hover_de_funcion_asociada() {
        // M48.1: hover sobre el nombre asociado (`Channel.new`) → su firma del registro de asociadas.
        let src = "fn main() -> int {\n    let ch: Channel<int> = Channel.new();\n    0\n}\n";
        let mut docs = HashMap::new();
        docs.insert("file:///t.ray".to_string(), src.to_string());
        // Posición sobre `new` (tras `Channel.`).
        let cn = src.lines().nth(1).unwrap().find("Channel.new").unwrap() + "Channel.".len() + 1;
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":1,"character":{cn}}}}}}}"#
        )).unwrap();
        let r = hover_result(&msg, &docs);
        let v = r.get("contents").and_then(|c| c.get("value")).and_then(Json::as_str).unwrap_or("");
        assert_eq!(v, "Channel.new() -> Channel<T>", "hover de Channel.new: {v}");
    }

    #[test]
    fn hover_de_const_y_tipo_incorporado() {
        let hover_of = |src: &str, line: usize, ch: usize| -> String {
            let mut docs = HashMap::new();
            docs.insert("file:///t.ray".to_string(), src.to_string());
            let msg = json::parse(&format!(
                r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
            )).unwrap();
            let r = hover_result(&msg, &docs);
            r.get("contents").and_then(|c| c.get("value")).and_then(Json::as_str).unwrap_or("").to_string()
        };
        // Uso de una constante → su tipo (como una variable).
        let c = hover_of("const MAXIMO: int = 100;\nfn main() -> int {\n    let x = MAXIMO;\n    x\n}\n", 2, 13);
        assert!(c.contains("MAXIMO: int"), "hover de const en uso: {c}");
        // Tipo incorporado (Channel) → descripción breve.
        let t = hover_of("fn main() -> int {\n    let ch: Channel<int> = channel();\n    0\n}\n", 1, 13);
        assert!(t.contains("Channel<T>"), "hover de tipo Channel: {t}");
    }

    #[test]
    fn completion_ofrece_consts_y_tipos_incorporados() {
        let comp = |src: &str, line: usize, ch: usize| -> Vec<String> {
            let mut docs = HashMap::new();
            docs.insert("file:///t.ray".to_string(), src.to_string());
            let msg = json::parse(&format!(
                r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
            )).unwrap();
            completion_result(&msg, &docs).as_array().unwrap().iter()
                .filter_map(|i| i.get("label").and_then(Json::as_str).map(|s| s.to_string())).collect()
        };
        // Constante de nivel superior.
        let c = comp("const MAXIMO: int = 100;\nfn main() -> int {\n    x\n    0\n}\n", 2, 5);
        assert!(c.contains(&"MAXIMO".to_string()), "const MAXIMO: {c:?}");
        // Tipos genéricos incorporados / del prelude.
        for t in ["Channel", "Task", "Map", "Option", "Result"] {
            assert!(c.contains(&t.to_string()), "tipo {t}: {c:?}");
        }
    }

    #[test]
    fn completion_muestra_la_firma_en_el_detalle() {
        // M46a: los ítems invocables llevan `labelDetails` con params y retorno.
        let detail_of = |src: &str, line: usize, ch: usize, label: &str| -> Option<(String, String)> {
            let mut docs = HashMap::new();
            docs.insert("file:///t.ray".to_string(), src.to_string());
            let msg = json::parse(&format!(
                r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
            )).unwrap();
            completion_result(&msg, &docs).as_array().unwrap().iter()
                .find(|i| i.get("label").and_then(|l| l.as_str()) == Some(label))
                .and_then(|i| i.get("labelDetails"))
                .map(|ld| (
                    ld.get("detail").and_then(Json::as_str).unwrap_or("").to_string(),
                    ld.get("description").and_then(Json::as_str).unwrap_or("").to_string(),
                ))
        };
        // Función de archivo (incompleta: `parse_all` la recupera) → firma completa.
        let src = "fn doblar(p: int, k: int) -> int { p * k }\nfn main() -> int {\n    dob\n    0\n}\n";
        assert_eq!(detail_of(src, 2, 7, "doblar"), Some(("(p: int, k: int)".into(), "int".into())));
        // Método → sin el receptor.
        let m = "struct P { x: int }\nfn tri(p: P, k: int) -> int { p.x }\nfn main() -> int {\n    let p = P { x: 1 };\n    p.\n    0\n}\n";
        assert_eq!(detail_of(m, 4, 6, "tri"), Some(("(k: int)".into(), "int".into())));
        // Builtin-método → firma sin receptor.
        let a = "fn main() -> int {\n    let xs = [1, 2];\n    xs.\n    0\n}\n";
        assert_eq!(detail_of(a, 2, 7, "push"), Some(("(value: T)".into(), "unit".into())));
    }

    #[test]
    fn completion_sin_punto_sigue_siendo_de_archivo() {
        // Sin `.` delante, la completion es la de archivo (regresión de M10.2e).
        let src = "fn doble(n: int) -> int { n + n }\nfn main() -> int { 0 }\n";
        let labels = completion_labels(src, 1, 19);
        assert!(labels.contains(&"doble".to_string()) && labels.contains(&"print".to_string()), "{labels:?}");
    }

    #[test]
    fn analiza_error_de_tipos() {
        let d = analyze("fn main() -> int { 1 + true }").expect("debería haber error");
        assert_eq!(d.line, 1);
        assert!(d.col >= 1);
        assert!(!d.message.is_empty());
    }

    #[test]
    fn diagnostico_usa_coordenadas_0_basadas() {
        // Error en la línea 2: la fase reporta 1-basado; LSP debe verlo 0-basado.
        let src = "fn main() -> int {\n    1 + true\n}";
        let d = analyze(src).unwrap();
        assert_eq!(d.line, 2); // 1-basado
        let dj = diagnostic_json(src, &d);
        let start = dj.get("range").unwrap().get("start").unwrap();
        assert_eq!(start.get("line"), Some(&Json::Num(1.0))); // 0-basado
        assert_eq!(dj.get("severity"), Some(&Json::Num(1.0))); // Error
    }

    /// Enmarca un cuerpo JSON con su cabecera `Content-Length`, como un cliente real.
    fn frame(body: &str) -> String {
        format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
    }

    #[test]
    fn serve_responde_initialize_y_publica_diagnosticos() {
        let mut input = String::new();
        input.push_str(&frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#));
        // didOpen de un programa con error de tipos.
        input.push_str(&frame(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.ray","text":"fn main() -> int { 1 + true }"}}}"#,
        ));
        input.push_str(&frame(r#"{"jsonrpc":"2.0","method":"exit"}"#));

        let mut reader = io::Cursor::new(input.into_bytes());
        let mut output: Vec<u8> = Vec::new();
        serve(&mut reader, &mut output);
        let out = String::from_utf8(output).unwrap();

        assert!(out.contains("\"id\":1"));
        assert!(out.contains("\"capabilities\""));
        // Las capacidades nuevas se anuncian (el cliente las descubre aquí).
        assert!(out.contains("documentFormattingProvider"));
        assert!(out.contains("documentSymbolProvider"));
        assert!(out.contains("documentHighlightProvider"));
        assert!(out.contains("textDocument/publishDiagnostics"));
        assert!(out.contains("\"severity\":1"));
    }

    #[test]
    fn serve_programa_valido_publica_lista_vacia() {
        let body = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///ok.ray","text":"fn main() -> int { 42 }"}}}"#;
        let mut input = frame(body);
        input.push_str(&frame(r#"{"jsonrpc":"2.0","method":"exit"}"#));

        let mut reader = io::Cursor::new(input.into_bytes());
        let mut output: Vec<u8> = Vec::new();
        serve(&mut reader, &mut output);
        let out = String::from_utf8(output).unwrap();
        assert!(out.contains("\"diagnostics\":[]"));
    }

    #[test]
    fn serve_metodo_desconocido_con_id_da_error() {
        let body = r#"{"jsonrpc":"2.0","id":9,"method":"textDocument/inexistente","params":{}}"#;
        let mut input = frame(body);
        input.push_str(&frame(r#"{"jsonrpc":"2.0","method":"exit"}"#));

        let mut reader = io::Cursor::new(input.into_bytes());
        let mut output: Vec<u8> = Vec::new();
        serve(&mut reader, &mut output);
        let out = String::from_utf8(output).unwrap();
        assert!(out.contains("\"id\":9"));
        assert!(out.contains("-32601"));
    }
}
