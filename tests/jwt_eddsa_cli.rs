//! Prueba de JWT EdDSA (`examples/web/jwt_eddsa.ray`, M30.3) — JSON Web Token con firma asimétrica
//! Ed25519 (RFC 8037). Apila Ed25519 (M30.2) + base64url. El demo firma unas claims con un seed del
//! RFC 8032 §7.1 y comprueba verificación con la clave correcta / equivocada / payload manipulado.
//!
//! El token esperado se **cross-checkea contra una computación independiente en Python** (Ed25519
//! canónico del RFC 8032 + base64url): la salida de raylang es byte-idéntica → firma interoperable.

use std::process::Command;

const ESPERADO: &[&str] = &[
    "eyJhbGciOiJFZERTQSIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJhZGEiLCJhZG1pbiI6dHJ1ZX0.\
2hgDGXGFX02ju6Jrdo7czKw-QH0ara8Xpfe1r2P_IUK5TYiz0Ma9owaJy-X8rZes4fXZNOSx0tTxK6rCFXbEDw",
    "ok: {\"sub\":\"ada\",\"admin\":true}",
    "clave equivocada rechazada",
    "manipulado rechazado",
    "alg:none rechazado",
];

fn correr(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/web/jwt_eddsa_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta jwt_eddsa_demo.ray");
    let lineas = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    (lineas, out.status.success())
}

/// El intérprete es lento para Ed25519 (~55 s); va `#[ignore]` (corre con `-- --ignored`). La VM queda
/// en la suite por defecto y ambos motores producen salida idéntica.
#[test]
#[ignore]
fn jwt_eddsa_interprete() {
    let (lineas, ok) = correr(&[]);
    assert!(ok, "jwt_eddsa_demo falló en el intérprete");
    assert_eq!(lineas, ESPERADO);
}

#[test]
fn jwt_eddsa_vm() {
    let (lineas, ok) = correr(&["--vm"]);
    assert!(ok, "jwt_eddsa_demo falló en la VM");
    assert_eq!(lineas, ESPERADO);
}
