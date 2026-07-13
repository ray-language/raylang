//! Prueba del AEAD ChaCha20-Poly1305 (`examples/web/chacha20poly1305.ray`, M30.1c). Combina ChaCha20
//! (M30.1a) y Poly1305 (M30.1b). El demo sella el texto de ejemplo del RFC 8439 §2.8.2 con la clave/
//! nonce/AAD del estándar; el test exige que criptograma y tag coincidan byte a byte con el **vector
//! oficial**, que el round-trip recupere el texto y que manipular un octeto haga fallar la apertura —
//! todo idéntico en ambos motores.

use std::process::Command;

const ESPERADO: &[&str] = &[
    // Criptograma (114 octetos) — RFC 8439 §2.8.2.
    "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d6\
3dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b36\
92ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc\
3ff4def08e4b7a9de576d26586cec64b6116",
    // Tag (16 octetos).
    "1ae10b594f09e26a7e902ecbd0600691",
    "open ok",
    "tamper rechazado",
];

fn run(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/web/chacha20poly1305_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta chacha20poly1305_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    (lines, out.status.success())
}

#[test]
fn aead_interpreter() {
    let (lines, ok) = run(&[]);
    assert!(ok, "chacha20poly1305_demo falló en el intérprete");
    assert_eq!(lines, ESPERADO);
}

#[test]
fn aead_vm() {
    let (lines, ok) = run(&["--vm"]);
    assert!(ok, "chacha20poly1305_demo falló en la VM");
    assert_eq!(lines, ESPERADO);
}
