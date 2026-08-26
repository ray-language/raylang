//! Pruebas de `ray upgrade` (M137). Las de aquí son **offline**: un tag explícito evita la
//! consulta de la última release, y ninguna llega a descargar (igual versión, `--check`, o
//! error de uso). El camino con red real (consultar/descargar la release publicada) es el
//! test `#[ignore]` del final: `cargo test --test upgrade_cli -- --ignored`.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn ray(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(BIN).args(args).output().expect("lanza el binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn upgrade_to_current_version_is_a_noop() {
    // El tag de la versión ya instalada (con y sin `v`) → "al día", sin red ni escritura.
    for tag in [format!("v{VERSION}"), VERSION.to_string()] {
        let (out, err, code) = ray(&["upgrade", &tag]);
        assert_eq!(code, 0, "misma versión = éxito\n{err}");
        assert!(out.contains("up to date"), "informa que ya está al día:\n{out}");
    }
}

#[test]
fn upgrade_check_reports_without_installing() {
    // `--check` con un tag distinto: informa y sale 1 (hay versión "nueva"), sin descargar.
    let (out, _err, code) = ray(&["upgrade", "v999.0.0", "--check"]);
    assert_eq!(code, 1, "--check con versión distinta sale 1");
    assert!(out.contains("999.0.0") && out.contains(VERSION), "informa ambas versiones:\n{out}");
    // Y con la actual sale 0.
    let (out, _err, code) = ray(&["upgrade", VERSION, "--check"]);
    assert_eq!(code, 0);
    assert!(out.contains("up to date"), "{out}");
}

#[test]
fn upgrade_usage_error() {
    let (_out, err, code) = ray(&["upgrade", "v1.0.0", "v2.0.0"]);
    assert_eq!(code, 64, "dos tags = error de uso");
    assert!(err.contains("usage: ray upgrade"), "{err}");
}

#[test]
fn upgrade_nonexistent_tag_fails_cleanly() {
    // Un tag que no existe: la descarga falla con mensaje claro y NO toca los binarios.
    // (Sí toca la red para el intento de descarga; si no hay red, curl falla igual → mismo exit.)
    let (_out, err, code) = ray(&["upgrade", "v999.0.0"]);
    assert_eq!(code, 69, "descarga imposible = EX_UNAVAILABLE\n{err}");
    assert!(err.contains("could not download"), "mensaje claro:\n{err}");
}

/// Red real: consulta la última release publicada del repo oficial. `--check` debe reportar
/// coherente con la versión compilada (0 al día / 1 nueva disponible), nunca otro código.
#[test]
#[ignore]
fn live_check_against_official_releases() {
    let (out, err, code) = ray(&["upgrade", "--check"]);
    assert!(code == 0 || code == 1, "0 o 1, nunca error: {code}\n{err}");
    assert!(out.contains(VERSION), "menciona la versión instalada:\n{out}");
}
