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

use crate::{checker, lexer, parser};
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

/// Lee un `Json::Num` como `usize` (las posiciones LSP son enteros).
fn as_usize(j: &Json) -> Option<usize> {
    match j {
        Json::Num(n) => Some(*n as usize),
        _ => None,
    }
}

// ── Análisis: el puente con el compilador ────────────────────────────────────────────

/// Un diagnóstico del front-end: posición **1-basada** (como reportan las fases) y el
/// mensaje, que es el `Display` del error (la misma cabecera que muestra el terminal).
pub struct Diag {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

/// Corre el front-end (lexer → parser → checker) **sin ejecutar** y devuelve el primer
/// error, si lo hay. Es todo el acoplamiento con el compilador: la API pública, nada más.
///
/// Nuestro compilador es *fail-fast* (devuelve el primer error), así que se publica **un**
/// diagnóstico por documento; reportar *todos* exigiría recolección de errores por fase.
pub fn analizar(src: &str) -> Option<Diag> {
    let tokens = match lexer::lex(src) {
        Ok(t) => t,
        Err(e) => return Some(Diag { line: e.line, col: e.col, message: e.to_string() }),
    };
    let mut program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => return Some(Diag { line: e.line, col: e.col, message: e.to_string() }),
    };
    if let Err(e) = checker::check(&mut program) {
        return Some(Diag { line: e.line, col: e.col, message: e.to_string() });
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
    let diags = match analizar(src) {
        Some(d) => vec![diagnostico_json(src, &d)],
        None => vec![], // sin errores: una lista vacía borra los diagnósticos previos
    };
    publish(uri, diags)
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
    // Subrayamos desde la columna del error hasta el final de la línea (subrayado visible).
    // El `character` de LSP cuenta unidades UTF-16; para código ASCII coincide con el número
    // de caracteres, que es con lo que medimos la línea.
    let line_len = src
        .lines()
        .nth(line0)
        .map(|l| l.chars().count())
        .unwrap_or(start_char + 1);
    let end_char = if start_char < line_len { line_len } else { start_char + 1 };
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
    fn analiza_programa_valido_sin_errores() {
        assert!(analizar("fn main() -> int { 1 + 2 }").is_none());
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
        let body = r#"{"jsonrpc":"2.0","id":9,"method":"textDocument/completion","params":{}}"#;
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
