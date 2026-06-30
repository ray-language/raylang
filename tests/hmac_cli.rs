//! Pruebas de HMAC-SHA256 + base64url + hex (`examples/web/{hmac,base64,hex}.ray`, M20.2).
//! Cómputo puro determinista → se corre el driver `examples/web/hmac_demo.ray` por ambos motores
//! (intérprete y VM) y se compara con los vectores de referencia (RFC 4231 para HMAC, `openssl`/
//! `base64.urlsafe_b64encode` para el resto).

use std::process::Command;

/// Las líneas que `hmac_demo.ray` debe imprimir, en orden.
const ESPERADO: &[&str] = &[
    "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843", // HMAC "Jefe"/"what do ya..."
    "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8", // HMAC "key"/"The quick..."
    "7d5c78edf5aae49e5d1bf1106e250485811585bcad0d78ebbea235d9f320a0dd", // HMAC clave > bloque
    "TWFu",                 // base64url("Man")
    "-_-_",                 // base64url([251,255,191]) — ejerce '-' y '_'
    "eyJhbGciOiJIUzI1NiJ9", // base64url cabecera JWT
    "true",                 // round-trip base64url decode == encode
    "deadbeef",             // round-trip hex
];

fn correr(flags: &[&str]) -> Vec<String> {
    let demo = format!("{}/examples/web/hmac_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta hmac_demo.ray");
    assert!(
        out.status.success(),
        "hmac_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn vectores_hmac_base64url_hex_interprete() {
    assert_eq!(correr(&[]), ESPERADO);
}

#[test]
fn vectores_hmac_base64url_hex_vm() {
    assert_eq!(correr(&["--vm"]), ESPERADO);
}
