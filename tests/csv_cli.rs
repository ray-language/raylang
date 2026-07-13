//! Prueba del parser/writer CSV (`examples/stdlib/csv.ray`, M32.2a — RFC 4180). El demo parsea un
//! documento con campos entrecomillados (coma interna, comillas escapadas `""`), imprime las filas y
//! comprueba el round-trip (write_csv → parse_csv). El test exige la salida esperada y que ambos motores
//! coincidan.

use std::process::Command;

const ESPERADO: &[&str] = &[
    "name|age|city",
    "Doe, John|42|New \"York\"",
    "Ada|36|London",
    "roundtrip ok",
];

fn run(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/stdlib/csv_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta csv_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    (lines, out.status.success())
}

#[test]
fn csv_interpreter() {
    let (lines, ok) = run(&[]);
    assert!(ok, "csv_demo falló en el intérprete");
    assert_eq!(lines, ESPERADO);
}

#[test]
fn csv_vm() {
    let (lines, ok) = run(&["--vm"]);
    assert!(ok, "csv_demo falló en la VM");
    assert_eq!(lines, ESPERADO);
}
