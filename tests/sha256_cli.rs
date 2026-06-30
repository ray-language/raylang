//! Pruebas de SHA-256 (`examples/web/sha256.ray`, M20.1), el cimiento criptográfico moderno.
//! Es **cómputo puro determinista** → se corre el driver `examples/web/sha256_demo.ray` por ambos
//! motores (intérprete y VM) y se compara su salida con los **vectores de referencia** de FIPS 180-4.

use std::process::Command;

/// Las líneas que `sha256_demo.ray` debe imprimir, en orden. Cada una es un vector estándar.
const ESPERADO: &[&str] = &[
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", // ""
    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", // "abc"
    "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1", // 56 octetos (multi-bloque)
    "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592", // "The quick brown fox..."
    "ef537f25c895bfa782526529a9b63d97aa631564d5d789c2b765448c8635fb6c", // "...lazy dog." (avalancha)
];

/// Corre `examples/web/sha256_demo.ray` con los flags dados y devuelve sus líneas de stdout.
fn correr(flags: &[&str]) -> Vec<String> {
    let demo = format!("{}/examples/web/sha256_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta sha256_demo.ray");
    assert!(
        out.status.success(),
        "sha256_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn vectores_sha256_interprete() {
    assert_eq!(correr(&[]), ESPERADO);
}

#[test]
fn vectores_sha256_vm() {
    assert_eq!(correr(&["--vm"]), ESPERADO);
}
