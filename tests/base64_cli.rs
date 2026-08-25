//! Pruebas de base64/base64url (`examples/web/base64.ray`), M59.3: decode ESTRICTO (RFC 4648
//! §3.5). Cómputo puro determinista → se corre `examples/web/base64_demo.ray` por ambos motores
//! y se compara con el golden (vectores RFC 4648 §10 + los rechazos del modo estricto).

use std::process::Command;

const EXPECTED: &[&str] = &[
    "Zg==",
    "Zg",
    "ok: 77 97 110",
    "ok: 102",
    "ok: 102 111",
    "ok:",
    "err: base64: data after '=' padding",
    "err: base64: invalid length (not a multiple of 4)",
    "err: base64: invalid length (not a multiple of 4)",
    "err: base64: non-zero trailing bits (non-canonical encoding)",
    "err: base64: invalid '=' padding",
    "err: invalid base64 character",
    "ok: 77 97 110",
    "ok: 102",
    "ok: 102 111",
    "err: base64url: non-zero trailing bits (non-canonical encoding)",
    "err: base64url: invalid length",
    "err: invalid base64url character",
];

fn run(flags: &[&str]) -> Vec<String> {
    let demo = format!("{}/examples/web/base64_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta base64_demo.ray");
    assert!(
        out.status.success(),
        "base64_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn base64_strict_interpreter() {
    assert_eq!(run(&[]), EXPECTED);
}

#[test]
fn base64_strict_vm() {
    assert_eq!(run(&["--vm"]), EXPECTED);
}
