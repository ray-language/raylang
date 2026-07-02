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
                send(out, &respuesta_initialize(id));
            }
            // Notificación de cortesía tras initialize: no requiere respuesta.
            "initialized" => {}
            "shutdown" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &resultado(id, Json::Null));
            }
            "exit" => break,
            "textDocument/didOpen" => {
                if let Some((uri, text)) = open_params(&msg) {
                    send(out, &diagnosticos(&uri, &text));
                    docs.insert(uri, text);
                }
            }
            "textDocument/didChange" => {
                if let Some((uri, text)) = change_params(&msg) {
                    send(out, &diagnosticos(&uri, &text));
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
                send(out, &resultado(id, hover_result(&msg, &docs)));
            }
            // M10.2b: ir-a-definición — salta del uso a su declaración.
            "textDocument/definition" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &resultado(id, definition_result(&msg, &docs)));
            }
            // M10.2c-LSP (cluster 4): find-references — todos los usos (y la declaración).
            "textDocument/references" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &resultado(id, references_result(&msg, &docs)));
            }
            // Cluster 4: rename — renombra el símbolo en todas sus apariciones.
            "textDocument/rename" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &resultado(id, rename_result(&msg, &docs)));
            }
            // Cluster 4: completion — símbolos del documento + builtins + palabras clave.
            "textDocument/completion" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &resultado(id, completion_result(&msg, &docs)));
            }
            // M10.2f: signature help — la firma de la función cuya llamada se está escribiendo.
            "textDocument/signatureHelp" => {
                let id = msg.get("id").cloned().unwrap_or(Json::Null);
                send(out, &resultado(id, signature_help_result(&msg, &docs)));
            }
            // Petición desconocida (lleva `id`) → error JSON-RPC. Notificación → se ignora.
            _ => {
                if let Some(id) = msg.get("id") {
                    send(out, &error_metodo(id.clone(), method));
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

/// El `result` de un `textDocument/hover`: el tipo del identificador bajo el cursor, o `null`.
fn hover_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Null };
    let Some(src) = docs.get(&uri) else { return Json::Null };
    let Some((info, start, end)) = hover_at(src, line0, char0) else { return Json::Null };
    obj(vec![
        ("contents", obj(vec![("kind", text("plaintext")), ("value", Json::Str(info))])),
        ("range", rango(line0, start, end)),
    ])
}

/// Busca el identificador en `(line0, char0)` (0-basado) y devuelve `(texto, col_ini, col_fin)`
/// (columnas 0-basadas en esa línea). Corre el front-end recolectando el índice semántico.
fn hover_at(src: &str, line0: usize, char0: usize) -> Option<(String, usize, usize)> {
    let tokens = lexer::lex(src).ok()?;
    let mut program = parser::parse(tokens).ok()?;
    let idx = checker::semantic_index(&mut program);
    // El índice usa posiciones 1-basadas (como las fases); el cursor llega 0-basado.
    let (qline, qcol) = (line0 + 1, char0 + 1);
    let e = idx.hovers.iter().find(|h| h.line == qline && qcol >= h.col && qcol < h.col + h.len)?;
    Some((e.text.clone(), e.col - 1, e.col - 1 + e.len))
}

/// Un `range` LSP en una sola línea (0-basada), de la columna `start` a `end` (0-basadas).
fn rango(line0: usize, start: usize, end: usize) -> Json {
    let pos = |ch: usize| obj(vec![("line", num(line0 as i64)), ("character", num(ch as i64))]);
    obj(vec![("start", pos(start)), ("end", pos(end))])
}

// ── Ir-a-definición (M10.2b) ─────────────────────────────────────────────────────────

/// El `result` de un `textDocument/definition`: una `Location` (uri + rango de la
/// declaración) en el mismo documento, o `null` si el identificador no tiene una declaración
/// conocida (p. ej. un método, un builtin o un tipo).
fn definition_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Null };
    let Some(src) = docs.get(&uri) else { return Json::Null };
    let Some((def_line0, def_col0, len)) = definition_at(src, line0, char0) else { return Json::Null };
    obj(vec![
        ("uri", Json::Str(uri)),
        ("range", rango(def_line0, def_col0, def_col0 + len)),
    ])
}

/// Busca el identificador en `(line0, char0)` (0-basado) y devuelve la posición de su
/// declaración `(def_line0, def_col0, largo)` (0-basadas). Como hover, corre el front-end.
fn definition_at(src: &str, line0: usize, char0: usize) -> Option<(usize, usize, usize)> {
    let tokens = lexer::lex(src).ok()?;
    let mut program = parser::parse(tokens).ok()?;
    let idx = checker::semantic_index(&mut program);
    let (qline, qcol) = (line0 + 1, char0 + 1);
    let d = idx.defs.iter().find(|d| d.line == qline && qcol >= d.col && qcol < d.col + d.len)?;
    Some((d.def_line - 1, d.def_col - 1, d.len))
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

/// El símbolo bajo el cursor: su nombre, el rango de su **declaración** (si se localiza) y los
/// rangos de todos sus **usos**. `None` si no hay símbolo.
fn symbol_occurrences(src: &str, line0: usize, char0: usize) -> Option<(String, Option<Span>, Vec<Span>)> {
    let tokens = lexer::lex(src).ok()?;
    let mut program = parser::parse(tokens).ok()?;
    let idx = checker::semantic_index(&mut program);
    let lines: Vec<&str> = src.lines().collect();
    let (qline, qcol) = (line0 + 1, char0 + 1);

    // El texto (nombre) de un uso, leído de la fuente en su rango.
    let use_name = |d: &checker::DefEntry| -> Option<String> {
        let chars: Vec<char> = lines.get(d.line - 1)?.chars().collect();
        let start = d.col - 1;
        (start + d.len <= chars.len()).then(|| chars[start..start + d.len].iter().collect())
    };
    // Rango del nombre de una declaración: el primer `name` como palabra entera desde su posición
    // (un `let`/`fn` apunta al keyword → escanea hasta el nombre; un parámetro ya está en el nombre).
    let decl_range = |def_line: usize, def_col: usize, name: &str| -> Option<Span> {
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
    };

    // Clave de la declaración objetivo: (a) el cursor está sobre un uso, o (b) sobre el nombre de
    // una declaración (que tenga al menos un uso, del que se toma el nombre).
    let mut target: Option<(usize, usize, String)> = idx.defs.iter()
        .find(|d| d.line == qline && qcol >= d.col && qcol < d.col + d.len)
        .and_then(|d| use_name(d).map(|n| (d.def_line, d.def_col, n)));
    if target.is_none() {
        for d in &idx.defs {
            let Some(name) = use_name(d) else { continue };
            let sobre_decl = decl_range(d.def_line, d.def_col, &name)
                .is_some_and(|(dl, dc, len)| dl + 1 == qline && qcol > dc && qcol - 1 < dc + len);
            if sobre_decl {
                target = Some((d.def_line, d.def_col, name));
                break;
            }
        }
    }
    let (tdl, tdc, name) = target?;

    let decl = decl_range(tdl, tdc, &name);
    let mut usos: Vec<(usize, usize, usize)> = idx.defs.iter()
        .filter(|d| d.def_line == tdl && d.def_col == tdc)
        .map(|d| (d.line - 1, d.col - 1, d.len))
        .collect();
    usos.sort();
    usos.dedup();
    Some((name, decl, usos))
}

/// El `result` de `textDocument/references`: una lista de `Location` (uso, y la declaración si el
/// cliente pide `includeDeclaration`). Lista vacía si no hay símbolo.
fn references_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Arr(vec![]) };
    let Some(src) = docs.get(&uri) else { return Json::Arr(vec![]) };
    let incluir_decl = msg.get("params").and_then(|p| p.get("context"))
        .and_then(|c| c.get("includeDeclaration"))
        .map(|b| matches!(b, Json::Bool(true)))
        .unwrap_or(true);
    let Some((_, decl, usos)) = symbol_occurrences(src, line0, char0) else { return Json::Arr(vec![]) };
    let mut locs: Vec<Json> = Vec::new();
    if let Some((l, c, len)) = decl.filter(|_| incluir_decl) {
        locs.push(obj(vec![("uri", Json::Str(uri.clone())), ("range", rango(l, c, c + len))]));
    }
    for (l, c, len) in usos {
        locs.push(obj(vec![("uri", Json::Str(uri.clone())), ("range", rango(l, c, c + len))]));
    }
    Json::Arr(locs)
}

/// El `result` de `textDocument/rename`: un `WorkspaceEdit` que sustituye el símbolo (declaración
/// + usos) por el nuevo nombre. `null` si no hay símbolo o falta `newName`.
fn rename_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let Some((uri, line0, char0)) = pos_params(msg) else { return Json::Null };
    let Some(src) = docs.get(&uri) else { return Json::Null };
    let Some(new_name) = msg.get("params").and_then(|p| p.get("newName")).and_then(|n| n.as_str()) else {
        return Json::Null;
    };
    let Some((_, decl, usos)) = symbol_occurrences(src, line0, char0) else { return Json::Null };
    let mut rangos: Vec<(usize, usize, usize)> = decl.into_iter().chain(usos).collect();
    rangos.sort();
    rangos.dedup();
    let edits: Vec<Json> = rangos.into_iter().map(|(l, c, len)| obj(vec![
        ("range", rango(l, c, c + len)),
        ("newText", Json::Str(new_name.to_string())),
    ])).collect();
    let changes = obj(vec![(uri.as_str(), Json::Arr(edits))]);
    obj(vec![("changes", changes)])
}

/// El `result` de `textDocument/completion`: los símbolos ofrecibles en el documento (funciones y
/// tipos definidos —incluido el prelude—, builtins y palabras clave). No filtra por ámbito ni por
/// prefijo (el cliente filtra por lo ya escrito); es una completion "de archivo", el primer escalón.
fn completion_result(msg: &Json, docs: &HashMap<String, String>) -> Json {
    let uri = msg.get("params").and_then(|p| p.get("textDocument")).and_then(|t| t.get("uri")).and_then(|u| u.as_str());
    let Some(src) = uri.and_then(|u| docs.get(u)) else { return Json::Arr(vec![]) };
    let Ok(tokens) = lexer::lex(src) else { return Json::Arr(vec![]) };
    let Ok(mut program) = parser::parse(tokens) else { return Json::Arr(vec![]) };
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
    let lista: Vec<Json> = items.into_iter()
        .map(|(label, kind)| obj(vec![("label", Json::Str(label)), ("kind", num(kind))]))
        .collect();
    Json::Arr(lista)
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
    // 1. Hallar la llamada en curso: el nombre de función y cuántas comas la preceden (param activo).
    let Some((name, activo)) = enclosing_call(src, line0, char0) else { return Json::Null };
    // 2. Extraer la firma **textualmente** de la fuente. Es robusto ante el documento a medio
    //    escribir (mientras tecleas los argumentos, el parse del archivo falla); solo necesita que
    //    la *declaración* `fn nombre(...) -> ...` esté bien formada.
    let Some((params, ret)) = find_fn_signature(src, &name) else { return Json::Null };
    // 3. Construir el label `fn nombre(p: T, …) -> R` y la lista de parámetros (para resaltar).
    let label = format!("fn {}({}) -> {}", name, params.join(", "), ret);
    let parametros: Vec<Json> = params.iter().map(|p| obj(vec![("label", Json::Str(p.clone()))])).collect();
    let activo = activo.min(params.len().saturating_sub(1));
    let firma = obj(vec![
        ("label", Json::Str(label)),
        ("parameters", Json::Arr(parametros)),
    ]);
    obj(vec![
        ("signatures", Json::Arr(vec![firma])),
        ("activeSignature", num(0)),
        ("activeParameter", num(activo as i64)),
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
fn enclosing_call(src: &str, line0: usize, char0: usize) -> Option<(String, usize)> {
    // Prefijo: todo el texto desde el inicio hasta el cursor.
    let mut prefijo = String::new();
    for (i, linea) in src.lines().enumerate() {
        use std::cmp::Ordering::*;
        match i.cmp(&line0) {
            Less => { prefijo.push_str(linea); prefijo.push('\n'); }
            Equal => { prefijo.extend(linea.chars().take(char0)); break; }
            Greater => break,
        }
    }
    let cs: Vec<char> = prefijo.chars().collect();
    let (mut depth, mut comas, mut i) = (0i32, 0usize, cs.len());
    while i > 0 {
        i -= 1;
        match cs[i] {
            ')' => depth += 1,
            '(' if depth == 0 => return ident_before(&cs, i).map(|n| (n, comas)),
            '(' => depth -= 1,
            ',' if depth == 0 => comas += 1,
            _ => {}
        }
    }
    None
}

/// El identificador que termina justo antes del índice `i` (saltando espacios). `None` si no lo hay.
fn ident_before(cs: &[char], i: usize) -> Option<String> {
    let mut j = i;
    while j > 0 && cs[j - 1].is_whitespace() {
        j -= 1;
    }
    let fin = j;
    while j > 0 && is_ident_char(cs[j - 1]) {
        j -= 1;
    }
    if j == fin {
        return None;
    }
    let nombre: String = cs[j..fin].iter().collect();
    // Un identificador no empieza por dígito (descarta restos como `42`).
    if nombre.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(nombre)
}

/// Lee un `Json::Num` como `usize` (las posiciones LSP son enteros).
fn as_usize(j: &Json) -> Option<usize> {
    match j {
        Json::Num(n) => Some(*n as usize),
        _ => None,
    }
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
pub fn analizar_todos(src: &str) -> Vec<Diag> {
    let diag = |line: usize, col: usize, len: usize, message: String| Diag { line, col, len, message };
    let tokens = match lexer::lex(src) {
        Ok(t) => t,
        Err(e) => return vec![diag(e.line, e.col, e.len, e.to_string())],
    };
    let (mut program, perrs) = parser::parse_all(tokens);
    if !perrs.is_empty() {
        return perrs.into_iter().map(|e| diag(e.line, e.col, e.len, e.to_string())).collect();
    }
    checker::check_all(&mut program)
        .into_iter()
        .map(|e| diag(e.line, e.col, e.len, e.to_string()))
        .collect()
}

/// Corre el front-end (lexer → parser → checker) **sin ejecutar** y devuelve el primer
/// error, si lo hay. Es todo el acoplamiento con el compilador: la API pública, nada más.
pub fn analizar(src: &str) -> Option<Diag> {
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
fn respuesta_initialize(id: Json) -> Json {
    let capabilities = obj(vec![
        // 1 = Full sync: el cliente reenvía el documento entero en cada cambio.
        ("textDocumentSync", num(1)),
        // M10.2b: el servidor responde hover (el tipo bajo el cursor) e ir-a-definición.
        ("hoverProvider", Json::Bool(true)),
        ("definitionProvider", Json::Bool(true)),
        // Cluster 4: find-references y rename (sobre el índice semántico + la fuente).
        ("referencesProvider", Json::Bool(true)),
        ("renameProvider", Json::Bool(true)),
        // Cluster 4: completion (objeto vacío = sin resolveProvider ni triggerCharacters).
        ("completionProvider", obj(vec![])),
        // M10.2f: signature help — la firma de la función mientras se escriben los argumentos.
        ("signatureHelpProvider", obj(vec![
            ("triggerCharacters", Json::Arr(vec![text("("), text(",")])),
        ])),
    ]);
    let result = obj(vec![
        ("capabilities", capabilities),
        ("serverInfo", obj(vec![("name", text("raylang-lsp"))])),
    ]);
    resultado(id, result)
}

/// Una respuesta JSON-RPC exitosa: `{ jsonrpc, id, result }`.
fn resultado(id: Json, result: Json) -> Json {
    obj(vec![("jsonrpc", text("2.0")), ("id", id), ("result", result)])
}

/// Una respuesta JSON-RPC de error "método no encontrado" (-32601).
fn error_metodo(id: Json, method: &str) -> Json {
    let error = obj(vec![
        ("code", num(-32601)),
        ("message", Json::Str(format!("método no soportado: {method}"))),
    ]);
    obj(vec![("jsonrpc", text("2.0")), ("id", id), ("error", error)])
}

/// Analiza la fuente y construye la notificación `publishDiagnostics` para ese documento.
fn diagnosticos(uri: &str, src: &str) -> Json {
    // Soporte de módulos: si el documento es un archivo, se analiza **con el loader** (resolviendo
    // sus imports desde disco) para no marcar errores espurios en proyectos multi-archivo. Si no es
    // un archivo o el buffer ni siquiera parsea, se cae al análisis de un solo archivo (multi-error).
    let diags = analizar_modular(uri, src).unwrap_or_else(|| analizar_todos(src));
    let json = diags.iter().map(|d| diagnostico_json(src, d)).collect();
    publish(uri, json)
}

/// Diagnósticos **con módulos**: corre el loader sobre el buffer de entrada (imports leídos de
/// disco) y devuelve los errores que caen en ESTE archivo, con su línea local. Devuelve `None`
/// cuando conviene caer al análisis de un solo archivo: si el URI no es un `file:` (buffer sin
/// guardar) o si el buffer de entrada ni siquiera parsea (así el fallback da errores de sintaxis
/// precisos y multi-error sobre la entrada, en vez de un único error del loader).
fn analizar_modular(uri: &str, src: &str) -> Option<Vec<Diag>> {
    let path = uri_to_path(uri)?;
    let deps = dep_roots_for(&path);
    match loader::load_fuente(&path, src, &deps) {
        Ok(loaded) => {
            // El módulo de entrada es la banda que empieza más arriba (delta 0 → línea local =
            // global). Solo publicamos SUS errores: los de otros módulos pertenecen a sus URIs.
            let entry_start = loaded.modules.iter().map(|m| m.start_line).min().unwrap_or(1);
            let mut program = loaded.program;
            let diags = checker::check_all(&mut program)
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
            let entrada_parsea = lexer::lex(src).ok().and_then(|t| parser::parse(t).ok()).is_some();
            entrada_parsea.then(|| vec![Diag { line: 1, col: 1, len: 1, message: e.message }])
        }
    }
}

/// Convierte un URI `file://…` a una ruta del sistema (decodificando `%XX`). `None` si no es un
/// `file:` (p. ej. un buffer `untitled:` sin archivo → se analiza en modo de un solo archivo).
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file:///Users/…` → host vacío; la ruta arranca en el tercer '/'. Un raro `localhost` se ignora.
    let ruta = rest.strip_prefix("localhost").unwrap_or(rest);
    Some(PathBuf::from(percent_decode(ruta)))
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
    let raiz = crate::manifest::Manifest::find(dir)
        .and_then(|toml| toml.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| dir.to_path_buf());
    let cache = raiz.join(".ray-deps");
    if cache.is_dir() { vec![cache] } else { Vec::new() }
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
fn diagnostico_json(src: &str, d: &Diag) -> Json {
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
            let original = obj_de(vec![
                ("jsonrpc", Json::Str("2.0".into())),
                ("id", Json::Num(7.0)),
                ("ok", Json::Bool(true)),
                ("lista", Json::Arr(vec![Json::Num(1.0), Json::Null])),
            ]);
            let texto = original.serialize();
            assert_eq!(parse(&texto).unwrap(), original);
            // Los enteros no llevan parte decimal.
            assert!(texto.contains("\"id\":7"));
        }

        fn obj_de(pares: Vec<(&str, Json)>) -> Json {
            Json::Obj(pares.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analizar_todos_publica_varios_errores() {
        // M33c: dos errores de tipos → dos diagnósticos.
        let ds = analizar_todos("fn f() -> int { 1 + true }\nfn g() -> int { \"x\" * 2 }\nfn main() -> int { 0 }");
        assert_eq!(ds.len(), 2, "{:?}", ds.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert_eq!((ds[0].line, ds[1].line), (1, 2));
        // Dos errores de sintaxis → dos diagnósticos (recuperación del parser)…
        let ds = analizar_todos("fn f() -> int { let = 1; 0 }\nfn g() -> int { 2 + }\nfn main() -> int { 0 }");
        assert!(ds.len() >= 2, "{:?}", ds.iter().map(|d| &d.message).collect::<Vec<_>>());
        // …pero un parse sucio NO llega al checker (sería cascada sobre un AST parcial).
        assert!(ds.iter().all(|d| d.message.contains("sintaxis")), "{:?}",
            ds.iter().map(|d| &d.message).collect::<Vec<_>>());
        // Sin errores → lista vacía (borra los diagnósticos previos del editor).
        assert!(analizar_todos("fn main() -> int { 0 }").is_empty());
    }

        #[test]
    fn analiza_programa_valido_sin_errores() {
        assert!(analizar("fn main() -> int { 1 + 2 }").is_none());
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
        let ds = analizar_modular(&uri, src).expect("modo modular (es un archivo)");
        assert!(ds.is_empty(), "un import válido no debe dar errores: {:?}",
            ds.iter().map(|d| &d.message).collect::<Vec<_>>());

        // (b) Un error de tipos EN LA ENTRADA sí se reporta, con la línea local.
        let src = "from geo import duplicar;\nfn main() -> int { duplicar(true) }\n";
        let ds = analizar_modular(&uri, src).expect("modular");
        assert_eq!(ds.len(), 1, "{:?}", ds.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert_eq!(ds[0].line, 2, "línea local de la entrada, no la global del programa fusionado");

        // (c) Un import a un módulo inexistente se reporta (la entrada parsea → error del loader).
        let src = "from noexiste import cosa;\nfn main() -> int { 0 }\n";
        let ds = analizar_modular(&uri, src).expect("modular");
        assert_eq!(ds.len(), 1);
        assert!(ds[0].message.contains("noexiste"), "{}", ds[0].message);

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
        let (name, decl, usos) = symbol_occurrences(src, 2, 2).expect("hay símbolo");
        assert_eq!(name, "x");
        assert_eq!(decl, Some((1, 6, 1)), "la declaración apunta al NOMBRE x, no al 'let'");
        assert_eq!(usos.len(), 2, "x + x son dos usos");
        // Y desde el nombre de la declaración (línea 2 → 0-based 1, col 6) da lo mismo.
        let (n2, d2, u2) = symbol_occurrences(src, 1, 6).expect("símbolo desde la declaración");
        assert_eq!((n2, d2, u2.len()), ("x".to_string(), Some((1, 6, 1)), 2));
    }

    #[test]
    fn referencias_distinguen_ambitos() {
        // Dos `x` en funciones distintas no se mezclan (claves de declaración distintas).
        let src = "fn f(a: int) -> int {\n  let x = a;\n  x + x\n}\nfn main() -> int {\n  let x = 9;\n  x\n}\n";
        // El `x` de `f` (línea 3 → 0-based 2): 2 usos.
        let (_, _, uf) = symbol_occurrences(src, 2, 2).unwrap();
        assert_eq!(uf.len(), 2);
        // El `x` de `main` (línea 7 → 0-based 6): 1 uso.
        let (_, _, um) = symbol_occurrences(src, 6, 2).unwrap();
        assert_eq!(um.len(), 1);
    }

    #[test]
    fn referencias_de_funcion() {
        // Una función llamada dos veces: declaración + 2 usos.
        let src = "fn doble(n: int) -> int { n + n }\nfn main() -> int {\n  doble(1) + doble(2)\n}\n";
        // Cursor sobre la primera llamada `doble` (línea 3 → 0-based 2, col 2).
        let (name, decl, usos) = symbol_occurrences(src, 2, 2).expect("hay símbolo");
        assert_eq!(name, "doble");
        assert_eq!(decl, Some((0, 3, 5)), "la declaración apunta al nombre 'doble' tras 'fn '");
        assert_eq!(usos.len(), 2);
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

    #[test]
    fn analiza_error_de_tipos() {
        let d = analizar("fn main() -> int { 1 + true }").expect("debería haber error");
        assert_eq!(d.line, 1);
        assert!(d.col >= 1);
        assert!(!d.message.is_empty());
    }

    #[test]
    fn diagnostico_usa_coordenadas_0_basadas() {
        // Error en la línea 2: la fase reporta 1-basado; LSP debe verlo 0-basado.
        let src = "fn main() -> int {\n    1 + true\n}";
        let d = analizar(src).unwrap();
        assert_eq!(d.line, 2); // 1-basado
        let dj = diagnostico_json(src, &d);
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
        let mut entrada = String::new();
        entrada.push_str(&frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#));
        // didOpen de un programa con error de tipos.
        entrada.push_str(&frame(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.ray","text":"fn main() -> int { 1 + true }"}}}"#,
        ));
        entrada.push_str(&frame(r#"{"jsonrpc":"2.0","method":"exit"}"#));

        let mut reader = io::Cursor::new(entrada.into_bytes());
        let mut salida: Vec<u8> = Vec::new();
        serve(&mut reader, &mut salida);
        let out = String::from_utf8(salida).unwrap();

        assert!(out.contains("\"id\":1"));
        assert!(out.contains("\"capabilities\""));
        assert!(out.contains("textDocument/publishDiagnostics"));
        assert!(out.contains("\"severity\":1"));
    }

    #[test]
    fn serve_programa_valido_publica_lista_vacia() {
        let body = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///ok.ray","text":"fn main() -> int { 42 }"}}}"#;
        let mut entrada = frame(body);
        entrada.push_str(&frame(r#"{"jsonrpc":"2.0","method":"exit"}"#));

        let mut reader = io::Cursor::new(entrada.into_bytes());
        let mut salida: Vec<u8> = Vec::new();
        serve(&mut reader, &mut salida);
        let out = String::from_utf8(salida).unwrap();
        assert!(out.contains("\"diagnostics\":[]"));
    }

    #[test]
    fn serve_metodo_desconocido_con_id_da_error() {
        let body = r#"{"jsonrpc":"2.0","id":9,"method":"textDocument/formatting","params":{}}"#;
        let mut entrada = frame(body);
        entrada.push_str(&frame(r#"{"jsonrpc":"2.0","method":"exit"}"#));

        let mut reader = io::Cursor::new(entrada.into_bytes());
        let mut salida: Vec<u8> = Vec::new();
        serve(&mut reader, &mut salida);
        let out = String::from_utf8(salida).unwrap();
        assert!(out.contains("\"id\":9"));
        assert!(out.contains("-32601"));
    }
}
