//! Pruebas de las librerías criptográficas de M19.3b (`examples/sha1.ray`, `examples/base64.ray`),
//! la base del handshake de WebSocket. Son **cómputo puro determinista** → no hace falta red ni
//! oráculo cruzado a mano: se corre el driver `examples/crypto_demo.ray` por ambos motores
//! (intérprete y VM) y se compara su salida con los **vectores de referencia** conocidos (RFC 3174
//! para SHA-1, RFC 4648 para base64, RFC 6455 §1.3 para el accept de WebSocket).

use std::process::Command;

/// Las líneas que `crypto_demo.ray` debe imprimir, en orden. Cada una es un vector estándar.
const ESPERADO: &[&str] = &[
    // SHA-1 (hex)
    "da39a3ee5e6b4b0d3255bfef95601890afd80709", // ""
    "a9993e364706816aba3e25717850c26c9cd0d89d", // "abc"
    "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12", // "The quick brown fox jumps over the lazy dog"
    "c2db330f6083854c99d4b5bfb6e8f29f201be699", // 56 × 'a' (mensaje multi-bloque)
    // base64
    "TWFu",                        // "Man"
    "TWE=",                        // "Ma"
    "TQ==",                        // "M"
    "YW55IGNhcm5hbCBwbGVhc3VyZS4=", // "any carnal pleasure."
    // Handshake de WebSocket: base64(SHA-1(key + GUID)) — el ejemplo canónico del RFC 6455.
    "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=",
];

/// Corre `examples/crypto_demo.ray` con los flags dados y devuelve sus líneas de stdout.
fn correr(flags: &[&str]) -> Vec<String> {
    let demo = format!("{}/examples/crypto_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta crypto_demo.ray");
    assert!(
        out.status.success(),
        "crypto_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn vectores_sha1_base64_handshake_interprete() {
    assert_eq!(correr(&[]), ESPERADO);
}

#[test]
fn vectores_sha1_base64_handshake_vm() {
    assert_eq!(correr(&["--vm"]), ESPERADO);
}
