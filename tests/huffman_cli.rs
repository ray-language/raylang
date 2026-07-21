//! Prueba de HPACK-Huffman (`examples/web/huffman.ray`, M31.1). Cierra el diferido grande de M26 (el
//! códec HPACK emitía/aceptaba solo literales crudos). La tabla estática de 257 símbolos del RFC 7541
//! Apéndice B está validada (código prefijo completo, Kraft = 2^30). El demo codifica cadenas de los
//! ejemplos del RFC y verifica que el resultado coincide **byte a byte con los vectores oficiales**
//! (C.4.1/C.4.2/C.4.3, C.6.1) y que decodificar recupera el original; ambos motores deben coincidir.

use std::process::Command;

const EXPECTED: &[&str] = &[
    "f1e3c2e5f23a6ba0ab90f4ff", "roundtrip ok",           // C.4.1  www.example.com
    "a8eb10649cbf", "roundtrip ok",                        // C.4.2  no-cache
    "25a849e95ba97d7f", "roundtrip ok",                    // C.4.3  custom-key
    "25a849e95bb8e8b4bf", "roundtrip ok",                  // C.4.3  custom-value
    "aec3771a4b", "roundtrip ok",                          // C.6.1  private
    "d07abe941054d444a8200595040b8166e082a62d1bff", "roundtrip ok", // C.6.1  date
    "9d29ad171863c78f0b97c8e9ae82ae43d3", "roundtrip ok",           // C.6.1  location
];

fn run(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/web/huffman_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta huffman_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    (lines, out.status.success())
}

#[test]
fn huffman_interpreter() {
    let (lines, ok) = run(&[]);
    assert!(ok, "huffman_demo falló en el intérprete");
    assert_eq!(lines, EXPECTED);
}

#[test]
fn huffman_vm() {
    let (lines, ok) = run(&["--vm"]);
    assert!(ok, "huffman_demo falló en la VM");
    assert_eq!(lines, EXPECTED);
}
