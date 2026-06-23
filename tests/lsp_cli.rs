//! Pruebas del Language Server (M10.2) sobre el binario: lanzan `raylang --lsp`, le envían
//! marcos LSP por stdin y comprueban las respuestas por stdout. Es la verificación de que el
//! servidor habla el protocolo de verdad, no solo en memoria (ver los tests unitarios en
//! `src/lsp.rs`).

use std::io::Write;
use std::process::{Command, Stdio};

/// Enmarca un cuerpo JSON con su cabecera `Content-Length`, como hace un cliente LSP.
fn frame(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

/// Lanza `raylang --lsp`, le envía `entrada` por stdin y devuelve todo su stdout.
fn lsp(entrada: &str) -> String {
    let mut hijo = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("lanza el servidor LSP");
    hijo.stdin
        .take()
        .unwrap()
        .write_all(entrada.as_bytes())
        .expect("escribe en stdin del servidor");
    let salida = hijo.wait_with_output().expect("espera al servidor");
    String::from_utf8_lossy(&salida.stdout).into_owned()
}

#[test]
fn responde_initialize_con_capacidades() {
    let entrada = frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
        + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entrada);
    assert!(out.contains("\"id\":1"), "eco del id\n{out}");
    assert!(out.contains("\"capabilities\""), "anuncia capacidades\n{out}");
    assert!(out.contains("\"textDocumentSync\":1"), "Full sync\n{out}");
}

#[test]
fn publica_diagnostico_ante_un_error() {
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.ray","text":"fn main() -> int { 1 + true }"}}}"#;
    let entrada = frame(open) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entrada);
    assert!(
        out.contains("textDocument/publishDiagnostics"),
        "publica diagnósticos\n{out}"
    );
    assert!(out.contains("\"severity\":1"), "severidad Error\n{out}");
    assert!(out.contains("\"source\":\"raylang\""), "la fuente es raylang\n{out}");
}

#[test]
fn programa_valido_publica_lista_vacia() {
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///ok.ray","text":"fn main() -> int { 42 }"}}}"#;
    let entrada = frame(open) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entrada);
    assert!(out.contains("\"diagnostics\":[]"), "sin errores, lista vacía\n{out}");
}
