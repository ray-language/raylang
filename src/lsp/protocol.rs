//! Protocolo JSON-RPC/LSP: framing, extracción de parámetros, construcción de mensajes
//! salientes y el puente de diagnósticos con el compilador (movimiento puro; usar
//! `git log --follow`).
//!
//! `read_message`/`send` son el *framing* (`Content-Length` + JSON); `open_params`/
//! `change_params`/`pos_params` extraen lo que necesita cada método del mensaje entrante;
//! `Diag`/`analyze`/`analyze_all`/`diagnostics` son el puente lex→parse→check hacia
//! `publishDiagnostics`; el resto construye las respuestas JSON-RPC salientes.

use super::*;

// ── Framing LSP ──────────────────────────────────────────────────────────────────────
//
// Cada mensaje es: cabeceras (líneas `Clave: valor\r\n`), una línea en blanco, y el cuerpo
// JSON de exactamente `Content-Length` bytes. Es lo único que LSP añade sobre JSON-RPC.

/// Lee un mensaje completo: las cabeceras hasta la línea en blanco y luego el cuerpo de
/// `Content-Length` bytes. `None` si la entrada se cerró (EOF) o el marco es inválido.
pub(super) fn read_message<R: BufRead>(reader: &mut R) -> Option<String> {
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
pub(super) fn send<W: Write>(out: &mut W, payload: &Json) {
    let body = payload.serialize();
    let _ = write!(out, "Content-Length: {}\r\n\r\n{}", body.len(), body);
    let _ = out.flush();
}

// ── Extracción de parámetros de los mensajes entrantes ───────────────────────────────

/// `(uri, texto)` de un `didOpen`: `params.textDocument.{uri,text}`.
pub(super) fn open_params(msg: &Json) -> Option<(String, String)> {
    let td = msg.get("params")?.get("textDocument")?;
    let uri = td.get("uri")?.as_str()?.to_string();
    let text = td.get("text")?.as_str()?.to_string();
    Some((uri, text))
}

/// `(uri, texto)` de un `didChange`. Con *Full sync*, el último `contentChange` trae el
/// documento completo en su campo `text`.
pub(super) fn change_params(msg: &Json) -> Option<(String, String)> {
    let params = msg.get("params")?;
    let uri = params.get("textDocument")?.get("uri")?.as_str()?.to_string();
    let changes = params.get("contentChanges")?.as_array()?;
    let text = changes.last()?.get("text")?.as_str()?.to_string();
    Some((uri, text))
}

/// La `uri` de un `didClose`: `params.textDocument.uri`.
pub(super) fn close_uri(msg: &Json) -> Option<String> {
    Some(msg.get("params")?.get("textDocument")?.get("uri")?.as_str()?.to_string())
}

/// `(uri, línea, carácter)` (0-basados) de una petición posicional (`hover`/`definition`):
/// `params.textDocument.uri` + `params.position.{line,character}`.
pub(super) fn pos_params(msg: &Json) -> Option<(String, usize, usize)> {
    let params = msg.get("params")?;
    let uri = params.get("textDocument")?.get("uri")?.as_str()?.to_string();
    let pos = params.get("position")?;
    let line = as_usize(pos.get("line")?)?;
    let character = as_usize(pos.get("character")?)?;
    Some((uri, line, character))
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
pub(super) fn initialize_response(id: Json) -> Json {
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
        // campos/métodos/builtins/UFCS del tipo del receptor); `>` lo dispara tras un `|>`
        // (pipeline) para ofrecer las funciones aplicables sin teclear la primera letra —el
        // segundo carácter de `|>` es la señal—. El espacio ` ` también dispara, pero **solo**
        // ofrece algo en contexto de pipeline (`|> `): fuera de él devuelve vacío, para no inundar
        // tras cada espacio del archivo. Tras `->`/`>` sueltos cae a completion de archivo.
        ("completionProvider", obj(vec![
            ("triggerCharacters", Json::Arr(vec![text("."), text(">"), text(" ")])),
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
pub(super) fn result_message(id: Json, result: Json) -> Json {
    obj(vec![("jsonrpc", text("2.0")), ("id", id), ("result", result)])
}

/// Una respuesta JSON-RPC de error "método no encontrado" (-32601).
pub(super) fn method_error(id: Json, method: &str) -> Json {
    let error = obj(vec![
        ("code", num(-32601)),
        ("message", Json::Str(format!("unsupported method: {method}"))),
    ]);
    obj(vec![("jsonrpc", text("2.0")), ("id", id), ("error", error)])
}

/// Analiza la fuente y construye la notificación `publishDiagnostics` para ese documento.
pub(super) fn diagnostics(uri: &str, src: &str) -> Json {
    // M55: un buffer `.ray.html` es un TEMPLATE — se diagnostica con su propio pipeline (generar +
    // analizar el módulo generado + traducir las líneas de vuelta al template con el line map).
    if is_template_uri(uri) {
        return template_diagnostics(uri, src);
    }
    // Soporte de módulos: si el documento es un archivo, se analiza **con el loader** (resolviendo
    // sus imports desde disco) para no marcar errores espurios en proyectos multi-archivo. Si no es
    // un archivo o el buffer ni siquiera parsea, se cae al análisis de un solo archivo (multi-error).
    let diags = analyze_modular(uri, src).unwrap_or_else(|| analyze_all(src));
    let json = diags.iter().map(|d| diagnostic_json(src, d)).collect();
    publish(uri, json)
}

/// ¿El documento es un template compilable (`.ray.html`)? Sus diagnósticos van por el compilador de templates;
/// el resto de features del LSP (hover/definición/…) no aplican (devuelven null).
pub(super) fn is_template_uri(uri: &str) -> bool {
    uri.ends_with(".ray.html")
}

/// Diagnósticos de un template `.ray.html` (M55): (1) los errores del PROPIO template (etiqueta sin
/// cerrar, params mal formados…) salen con su línea; (2) si el template genera, se analiza el
/// módulo raylang GENERADO (con el loader, contra la ruta del `.ray` hermano → `std/template` y las
/// path-deps resuelven) y cada error se TRADUCE de vuelta a su línea del template con el line map —
/// el typo en `{{ titluo }}` se subraya en el HTML.
pub(super) fn template_diagnostics(uri: &str, src: &str) -> Json {
    let path = uri_to_path(uri);
    let name = path
        .as_deref()
        .and_then(|p| crate::templ::fn_suffix_of(p).ok())
        .unwrap_or_else(|| "vista".to_string());
    let dir = path.as_deref().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let (code, map) = match crate::templ::generate_with_map_at(src, &name, dir.as_deref()) {
        Err(e) => {
            let d = Diag { line: e.line, col: 1, len: usize::MAX, message: format!("template: {}", e.msg) };
            return publish(uri, vec![diagnostic_json(src, &d)]);
        }
        Ok(x) => x,
    };
    // Analizar el generado: con loader si conocemos la ruta (resuelve `from std/template import …`);
    // si el buffer no es un archivo, al menos lex+parse del generado (el check daría falsos
    // positivos sin resolver el import del escape).
    let gen_diags: Vec<Diag> = match &path {
        Some(p) => {
            let gen_path = PathBuf::from(p.to_string_lossy().trim_end_matches(".html").to_string());
            analyze_modular(&path_to_uri(&gen_path), &code).unwrap_or_else(|| analyze_all(&code))
        }
        None => match crate::lexer::lex(&code) {
            Err(e) => vec![Diag { line: e.line, col: 1, len: usize::MAX, message: e.to_string() }],
            Ok(tokens) => match crate::parser::parse(tokens) {
                Err(e) => vec![Diag { line: e.line, col: 1, len: usize::MAX, message: e.to_string() }],
                Ok(_) => vec![],
            },
        },
    };
    let json = gen_diags
        .iter()
        .map(|d| {
            let tpl_line = map.get(d.line.saturating_sub(1)).copied().unwrap_or(1).max(1);
            let td = Diag { line: tpl_line, col: 1, len: usize::MAX, message: format!("template: {}", d.message) };
            diagnostic_json(src, &td)
        })
        .collect();
    publish(uri, json)
}

/// Diagnósticos **con módulos**: corre el loader sobre el buffer de entrada (imports leídos de
/// disco) y devuelve los errores que caen en ESTE archivo, con su línea local. Devuelve `None`
/// cuando conviene caer al análisis de un solo archivo: si el URI no es un `file:` (buffer sin
/// guardar) o si el buffer de entrada ni siquiera parsea (así el fallback da errores de sintaxis
/// precisos y multi-error sobre la entrada, en vez de un único error del loader).
pub(super) fn analyze_modular(uri: &str, src: &str) -> Option<Vec<Diag>> {
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
pub(super) fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file:///Users/…` → host vacío; la ruta arranca en el tercer '/'. Un raro `localhost` se ignora.
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    let decoded = percent_decode(path);
    // M176 (Windows): VS Code manda `file:///c%3A/Users/…` — decodificado, `/c:/Users/…`. La ruta
    // del sistema es `c:/Users/…` (sin la barra inicial); `file://C:/…` (sin la tercera barra) también
    // se acepta. Solo cuando lo que sigue es una letra de unidad: `/tmp/x` sigue siendo `/tmp/x`.
    let decoded = match decoded.as_bytes() {
        [b'/', d, b':', ..] if d.is_ascii_alphabetic() => decoded[1..].to_string(),
        _ => decoded,
    };
    Some(PathBuf::from(decoded))
}

/// La conversión inversa: una ruta del sistema → URI `file://…` como la esperan los clientes. En
/// unix, `file:///Users/…`; en Windows (M176), `file:///C:/Users/…` — la tercera barra y las barras
/// hacia delante (VS Code no reconoce `file://C:\Users\…` como el mismo documento, y `\` dentro de
/// un JSON exige escape). Se codifican `%`, espacio y `#`/`?` (los que romperían el URI).
pub(super) fn path_to_uri(path: &std::path::Path) -> String {
    let s = path.display().to_string();
    let s = if cfg!(windows) { s.replace('\\', "/") } else { s };
    let mut out = String::with_capacity(s.len() + 8);
    out.push_str("file://");
    // Solo ante una letra de unidad: `C:/…` → `file:///C:/…`. Una ruta relativa se deja tal cual
    // (los módulos embebidos de la std tienen rutas sintéticas como `std/math`, y `uri_to_path`
    // debe devolver exactamente esa ruta para reencontrarlos en el programa cargado).
    if matches!(s.as_bytes(), [d, b':', ..] if d.is_ascii_alphabetic()) {
        out.push('/');
    }
    for c in s.chars() {
        match c {
            '%' => out.push_str("%25"),
            ' ' => out.push_str("%20"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3F"),
            other => out.push(other),
        }
    }
    out
}

/// Decodifica los `%XX` de un URI (p. ej. `%20` → espacio). Sin dependencias.
pub(super) fn percent_decode(s: &str) -> String {
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
pub(super) fn dep_roots_for(entry: &Path) -> Vec<PathBuf> {
    // Compartida con el CLI (`deps::dependency_roots_for`): caché `.ray-deps/` + el PADRE de cada
    // dependencia por ruta (`nombre = "path:<dir>"`). Sin esto, un proyecto con path-deps (como
    // `examples/db`) compilaba con `ray run` pero el editor marcaba "no se encuentra el módulo".
    // No descarga nada (el LSP no toca la red al diagnosticar).
    let dir = entry.parent().unwrap_or(Path::new("."));
    crate::deps::dependency_roots_for(dir)
}

/// La raíz del proyecto que contiene `entry`: el **ancestro más cercano** (subiendo desde su carpeta)
/// que contiene un `main.ray` —la entrada convencional, y por definición la raíz desde la que son
/// absolutos los `import` (DESIGN §20.3: "la ruta es absoluta desde la raíz del proyecto, el directorio
/// del archivo de entrada")—. `None` si no hay ninguno (archivo suelto sin proyecto).
pub(super) fn project_root_for(entry: &Path) -> Option<PathBuf> {
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
pub(super) fn load(entry: &Path, src: &str) -> Result<loader::Loaded, loader::LoadError> {
    let deps = dep_roots_for(entry);
    match project_root_for(entry) {
        Some(root) => loader::load_source_module(entry, src, &root, &deps),
        None => loader::load_source(entry, src, &deps),
    }
}

/// Envuelve una lista de diagnósticos en la notificación `textDocument/publishDiagnostics`.
pub(super) fn publish(uri: &str, diags: Vec<Json>) -> Json {
    let params = obj(vec![("uri", text(uri)), ("diagnostics", Json::Arr(diags))]);
    obj(vec![
        ("jsonrpc", text("2.0")),
        ("method", text("textDocument/publishDiagnostics")),
        ("params", params),
    ])
}

/// Traduce un `Diag` (1-basado) a un diagnóstico LSP (rango 0-basado, severidad Error).
pub(super) fn diagnostic_json(src: &str, d: &Diag) -> Json {
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
pub(super) fn obj(pairs: Vec<(&str, Json)>) -> Json {
    Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}
/// Una cadena JSON.
pub(super) fn text(s: &str) -> Json {
    Json::Str(s.to_string())
}
/// Un número JSON (entero).
pub(super) fn num(x: i64) -> Json {
    Json::Num(x as f64)
}
