//! Prueba de Ed25519 (`examples/web/ed25519.ray`, M30.2) — firma sobre la curva de Edwards. Port de
//! TweetNaCl (aritmética de campo mod 2^255−19 en limbs i64, ley de grupo de Edwards, SHA-512).
//!
//! El demo, para tres semillas del RFC 8032 §7.1, deriva la clave pública, firma el mensaje y verifica.
//! Los valores esperados se **cross-checkean contra la implementación de referencia canónica del RFC
//! 8032** (apéndice; la salida de raylang es byte-idéntica a ella en clave pública y firma para las tres
//! semillas). Ambos motores (intérprete ↔ VM) deben coincidir.

use std::process::Command;

const ESPERADO: &[&str] = &[
    // Semilla 9d61b19d… (mensaje vacío).
    "ca3b229b946be2bb71cb50a9ccf1dc5991efc14f3baa3a20cabd77a56e620d3d",
    "871226d44624a3ac2c24a14251b7c033074c43174a814ff7446fdc8da35fc980\
3ee2a8a12e37166ba80a24decef1fcafe1626cc2abc75650d712042e6df20c01",
    "si", "no",
    // Semilla 4ccd089b… (mensaje 0x72) — coincide con el vector publicado del RFC 8032.
    "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
    "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da\
085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
    "si", "no",
    // Semilla c5aa8df4… (mensaje 0xaf82) — coincide con el vector publicado del RFC 8032.
    "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
    "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac\
18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
    "si", "no",
];

fn run(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/web/ed25519_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta ed25519_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    (lines, out.status.success())
}

/// El intérprete (tree-walking) es lento para Ed25519 (~110 s: millones de multiplicaciones de campo),
/// así que este test va marcado `#[ignore]` para no ralentizar cada `cargo test`. Corre a demanda con
/// `cargo test --test ed25519_cli -- --ignored`. La VM (más rápida) sí queda en la suite por defecto, y
/// ambos motores producen salida idéntica (verificado).
#[test]
#[ignore]
fn ed25519_interpreter() {
    let (lines, ok) = run(&[]);
    assert!(ok, "ed25519_demo falló en el intérprete");
    assert_eq!(lines, ESPERADO);
}

#[test]
fn ed25519_vm() {
    let (lines, ok) = run(&["--vm"]);
    assert!(ok, "ed25519_demo falló en la VM");
    assert_eq!(lines, ESPERADO);
}
