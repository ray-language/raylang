//! Prueba de ChaCha20 (`examples/web/chacha20.ray`, M30.1a). Librería raylang pura (aritmética u32 de
//! M28.3, sin enmascarado a mano). El demo cifra el texto de ejemplo del RFC 8439 §2.4.2 con la
//! clave/nonce/contador del estándar; el test exige que el criptograma coincida **byte a byte con el
//! vector oficial del RFC** y que ambos motores (intérprete ↔ VM) den lo mismo.

use std::process::Command;

/// El criptograma oficial del RFC 8439 §2.4.2 (114 octetos en hex) + el veredicto del round-trip.
const EXPECTED: &[&str] = &[
    "6e2e359a2568f98041ba0728dd0d6981e97e7aec1d4360c20a27afccfd9fae0b\
f91b65c5524733ab8f593dabcd62b3571639d624e65152ab8f530c359f0861d8\
07ca0dbf500d6a6156a38e088a22b65e52bc514d16ccf806818ce91ab7793736\
5af90bbf74a35be6b40b8eedf2785e42874d",
    "roundtrip ok",
];

fn run(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/web/chacha20_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta chacha20_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    (lines, out.status.success())
}

#[test]
fn chacha20_interpreter() {
    let (lines, ok) = run(&[]);
    assert!(ok, "chacha20_demo falló en el intérprete");
    assert_eq!(lines, EXPECTED);
}

#[test]
fn chacha20_vm() {
    let (lines, ok) = run(&["--vm"]);
    assert!(ok, "chacha20_demo falló en la VM");
    assert_eq!(lines, EXPECTED);
}
