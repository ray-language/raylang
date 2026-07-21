//! Pruebas de HPACK (RFC 7541) + framing HTTP/2 (RFC 7540) (M26). HPACK se valida contra los **vectores
//! oficiales del RFC 7541 §C.3** (los tres bloques de petición con tabla dinámica compartida — referencias
//! `be`/`bf` a la tabla dinámica incluidas), que son autoritativos. Más round-trip de decodificación y de
//! un HEADERS frame. Golden por ambos motores.

use std::process::Command;

const EXPECTED: &[&str] = &[
    // RFC 7541 §C.3.1 / C.3.2 / C.3.3 (sin Huffman): los tres bloques de petición.
    "828684410f7777772e6578616d706c652e636f6d",
    "828684be58086e6f2d6361636865",
    "828785bf400a637573746f6d2d6b65790c637573746f6d2d76616c7565",
    // Decodificación de C.3.1 → cabeceras originales.
    ":method: GET",
    ":scheme: http",
    ":path: /",
    ":authority: www.example.com",
    // Round-trip de un HEADERS frame (tipo 1, stream 1, flags END_HEADERS|END_STREAM = 5).
    "HEADERS type=1 stream=1 flags=5 blocklen=2",
    // La connection preface del cliente: "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".
    "505249202a20485454502f322e300d0a0d0a534d0d0a0d0a",
];

fn run(flags: &[&str]) -> Vec<String> {
    let demo = format!("{}/examples/web/http2_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta http2_demo.ray");
    assert!(
        out.status.success(),
        "http2_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn hpack_y_framing_interpreter() {
    assert_eq!(run(&[]), EXPECTED);
}

#[test]
fn hpack_y_framing_vm() {
    assert_eq!(run(&["--vm"]), EXPECTED);
}
