//! Pruebas de **std/term** (M107.3). Sin terminal en CI, se prueba lo probable:
//!
//! - el DECODIFICADOR de teclas (`term.decode`), que es puro (bytes → tecla) a propósito: la
//!   batería (`tests/fixtures/term_decoder.ray`) cubre ASCII, controles, CSI (flechas/Home/End/
//!   `~`-teclas/F1..F12/modificadores), SS3, Shift-Tab, UTF-8 de 2/3/4 octetos y los prefijos
//!   INCOMPLETOS — en los tres motores, contra salida exacta;
//! - los caminos sin-tty: `is_tty` false, `size` None, y `raw` que falla LIMPIO sin correr `f`.
//!
//! El modo crudo real (termios) no se puede ejercitar bajo pipes: su smoke es manual
//! (`examples/term/keys.ray` en una terminal).

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_term_{name}"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("lanza ray");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Corre `src` en VM + intérprete + (si hay rustc) nativo, y asevera la salida EXACTA en los tres.
fn assert_on_all_engines(name: &str, src: &str, want: &str) {
    let base = tmp(name);
    std::fs::write(base.join("prog.ray"), src).unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, err, code) = ray(&base, &[engine, "prog.ray"]);
        assert_eq!(code, 0, "{engine}: exit 0\n{err}");
        assert_eq!(out, want, "{engine}: salida exacta");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join("prog_bin");
        let (_o, berr, bcode) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(bcode, 0, "build --native ok\n{berr}");
        let native = Command::new(&bin).stdin(std::process::Stdio::null()).output().expect("nativo");
        assert_eq!(String::from_utf8_lossy(&native.stdout), want, "nativo ≡ VM");
        assert_eq!(native.status.code(), Some(0));
    }
}

#[test]
fn decoder_battery_matches_on_all_three_engines() {
    assert_on_all_engines(
        "decoder",
        include_str!("fixtures/term_decoder.ray"),
        include_str!("fixtures/term_decoder.out"),
    );
}

#[test]
fn without_a_tty_everything_degrades_cleanly() {
    // Bajo pipes (stdin /dev/null, stdout capturado): nada es tty → is_tty false, size None, y
    // `raw` devuelve Err SIN correr f (una TUI exige terminal; si f corriera, saldría "CORRIO").
    assert_on_all_engines(
        "no_tty",
        include_str!("fixtures/term_no_tty.ray"),
        "false\nno-size\nraw-err\n",
    );
}
