//! Prueba de SHA-512 (`examples/web/sha512.ray`, M30.2a — prerrequisito de Ed25519). Aritmética de 64
//! bits (`u64` de M28.3, sin enmascarado). El demo imprime los digests de "abc", el mensaje vacío y el
//! "quick brown fox"; el test exige que coincidan con los **vectores conocidos** (FIPS 180-4/NIST) y
//! que ambos motores (intérprete ↔ VM) den lo mismo.

use std::process::Command;

const ESPERADO: &[&str] = &[
    "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
    "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
    "07e547d9586f6a73f73fbac0435ed76951218fb7d0c8d788a309d785436bbb64\
2e93a252a954f23912547d1e8a3b5ed6e1bfd7097821233fa0538f3db854fee6",
];

fn run(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/web/sha512_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta sha512_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    (lines, out.status.success())
}

#[test]
fn sha512_interpreter() {
    let (lines, ok) = run(&[]);
    assert!(ok, "sha512_demo falló en el intérprete");
    assert_eq!(lines, ESPERADO);
}

#[test]
fn sha512_vm() {
    let (lines, ok) = run(&["--vm"]);
    assert!(ok, "sha512_demo falló en la VM");
    assert_eq!(lines, ESPERADO);
}
