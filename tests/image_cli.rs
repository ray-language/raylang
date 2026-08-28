//! M144 — std/image: decodificación PNG. La batería (filtros None/Sub/Up/Paeth, gris 1-bit,
//! paleta 2-bit + tRNS, gris+alfa, RGB de 16 bits reducido, entrelazado/CRC/truncado → Err)
//! corre byte-idéntica en los TRES motores; los PNG van embebidos como literales b"…"
//! (generados con zlib de Python — el oráculo externo del formato).

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("ray_image_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
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

#[test]
fn png_battery_matches_on_all_three_engines() {
    let src = include_str!("fixtures/image_decode.ray");
    let want = include_str!("fixtures/image_decode.out");
    let base = tmp("battery");
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
