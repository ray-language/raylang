//! Pruebas del logging estructurado en JSON (`examples/web/log.ray`, M21.1). Salida determinista (se
//! usa `render` con una marca de tiempo fija) → se corre `examples/web/log_demo.ray` por ambos motores
//! y se compara línea a línea. Además se valida que cada línea sea JSON parseable con Python.

use std::process::Command;

const ESPERADO: &[&str] = &[
    r#"{"ts":"2026-06-30T12:00:00Z","level":"INFO","service":"api","msg":"servidor iniciado"}"#,
    r#"{"ts":"2026-06-30T12:00:00Z","level":"INFO","service":"api","msg":"peticion","method":"GET","path":"/users/42","status":200}"#,
    r#"{"ts":"2026-06-30T12:00:00Z","level":"WARN","service":"api","msg":"latencia alta","ms":1500,"retry":true}"#,
    r#"{"ts":"2026-06-30T12:00:00Z","level":"ERROR","service":"api","msg":"fallo \"db\" en\nla conexion","code":"E_CONN"}"#,
    "(filtrado)",
    r#"{"ts":"2026-06-30T12:00:00Z","level":"ERROR","service":"worker","msg":"esto pasa"}"#,
];

fn correr(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/web/log_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta log_demo.ray");
    let lineas = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();
    (lineas, out.status.success())
}

#[test]
fn log_estructurado_interprete() {
    let (lineas, ok) = correr(&[]);
    assert!(ok, "log_demo falló");
    assert_eq!(lineas, ESPERADO);
}

#[test]
fn log_estructurado_vm() {
    let (lineas, ok) = correr(&["--vm"]);
    assert!(ok, "log_demo falló");
    assert_eq!(lineas, ESPERADO);
}

/// Cada línea JSON (salvo "(filtrado)") debe ser parseable por Python `json.loads` (escapado correcto).
#[test]
fn las_lineas_son_json_valido() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 no disponible: se omite la validación JSON");
        return;
    }
    let (lineas, ok) = correr(&[]);
    assert!(ok);
    for l in &lineas {
        if l == "(filtrado)" {
            continue;
        }
        let py = Command::new("python3")
            .arg("-c")
            .arg("import sys,json; json.loads(sys.argv[1])")
            .arg(l)
            .output()
            .expect("ejecuta python3");
        assert!(py.status.success(), "línea no es JSON válido: {l}\n{}", String::from_utf8_lossy(&py.stderr));
    }
}
