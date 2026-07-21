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
mod json;
mod protocol;
mod features;
use json::Json;
use protocol::*;
use features::*;
pub use protocol::{analyze, analyze_all};

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

#[cfg(test)]
mod tests;
