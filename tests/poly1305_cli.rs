//! Prueba de Poly1305 (`examples/web/poly1305.ray`, M30.1b). Port de poly1305-donna: 130 bits en 5
//! limbs de 26 bits con productos u64 (M28.3). El demo calcula el tag del mensaje de ejemplo del RFC
//! 8439 §2.5.2; el test exige que coincida byte a byte con el **vector oficial** y que ambos motores
//! (intérprete ↔ VM) den lo mismo.

use std::process::Command;

/// El tag oficial del RFC 8439 §2.5.2 (16 octetos en hex).
const ESPERADO: &[&str] = &["a8061dc1305136c6c22b8baf0c0127a9"];

fn run(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/web/poly1305_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta poly1305_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    (lines, out.status.success())
}

#[test]
fn poly1305_interpreter() {
    let (lines, ok) = run(&[]);
    assert!(ok, "poly1305_demo falló en el intérprete");
    assert_eq!(lines, ESPERADO);
}

#[test]
fn poly1305_vm() {
    let (lines, ok) = run(&["--vm"]);
    assert!(ok, "poly1305_demo falló en la VM");
    assert_eq!(lines, ESPERADO);
}
