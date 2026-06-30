//! Pruebas de fechas/horas UTC (`examples/web/time.ray`, M20.5). Cómputo puro determinista
//! (timestamps fijos) → se corre `examples/web/time_demo.ray` por ambos motores y se compara con los
//! vectores de referencia (`datetime.utcfromtimestamp` de Python). `now_utc` se prueba por propiedad
//! (coherencia con `now()`), no por valor.

use std::process::Command;

const ESPERADO: &[&str] = &[
    "1970-01-01T00:00:00Z",
    "Thu, 01 Jan 1970 00:00:00 GMT",
    "1994-11-06T08:49:37Z",
    "Sun, 06 Nov 1994 08:49:37 GMT",
    "2024-06-30T11:49:37Z",
    "Sun, 30 Jun 2024 11:49:37 GMT",
    "Fri, 01 Mar 2024 00:00:00 GMT", // 2024 bisiesto
    "true",                          // round-trip epoch
    "true",                          // parse_iso8601 → epoch
    "true",                          // now_utc coherente con now()
    "1h2m3s",
    "45s",
    "500ms",
];

fn correr(flags: &[&str]) -> Vec<String> {
    let demo = format!("{}/examples/web/time_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta time_demo.ray");
    assert!(
        out.status.success(),
        "time_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn fechas_utc_interprete() {
    assert_eq!(correr(&[]), ESPERADO);
}

#[test]
fn fechas_utc_vm() {
    assert_eq!(correr(&["--vm"]), ESPERADO);
}
