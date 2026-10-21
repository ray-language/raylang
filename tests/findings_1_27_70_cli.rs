//! Hallazgos atendidos en 1.27.70 (rayzip #189, M415): el inflate incremental de `std/inflate` corre
//! en el runtime (miniz) con un handle por stream — `stream_close`, tope por push, cortes en
//! cualquier octeto y `stream_remaining`, iguales en VM, intérprete y nativo.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ray_f1_27_70_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Corre `prog.ray` de `dir` en VM, intérprete y (si hay rustc) nativo; los tres deben dar `want`.
fn three_engines(dir: &PathBuf, want: &str) {
    for flags in [&[][..], &["--interp"][..]] {
        let mut args = vec!["run"];
        args.extend_from_slice(flags);
        args.push("prog.ray");
        let (out, err, code) = ray(dir, &args);
        assert_eq!(code, 0, "{flags:?}: {out}\n{err}");
        assert_eq!(out, want, "{flags:?}");
    }
    if has_rustc() {
        let bin = dir.join("prog_native");
        let (out, err, code) = ray(dir, &["build", "--native", "prog.ray", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build: {out}\n{err}");
        let o = Command::new(&bin).current_dir(dir).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "nativo");
    }
}

/// rayzip #189 (M415): un stream de sesión decodifica por trozos arbitrarios lo mismo que
/// `inflate_raw` de una vez; el tope por push es `Err` pegajoso; `stream_close` apaga un stream a
/// medias (más pushes dan vacío) y es inocuo tras el final; `stream_remaining` sigue valiendo.
#[test]
fn the_incremental_decoder_runs_in_the_runtime_with_close_and_limit() {
    let d = tmp("inflate_stream_rt");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/deflate;
import std/inflate;
fn main() -> int {
    var plain = b"";
    var i = 0;
    while (i < 3000) {
        plain = plain + b"raylang stream " + to_string(i % 97).to_bytes() + b"; ";
        i = i + 1;
    }
    let z = deflate.deflate_raw(plain) + b"TAIL";
    // (a) cortes arbitrarios → idéntico al de una vez, y lo que seguía al bloque final queda.
    let s = inflate.inflate_stream();
    var out = b"";
    var pos = 0;
    let sizes = [1, 7, 100, 3, 5000, 33, 2, 9999, 1, 1];
    var k = 0;
    while (pos < z.len()) {
        var n = sizes[k % sizes.len()];
        if (pos + n > z.len()) { n = z.len() - pos; }
        match (inflate.stream_push(s, z.sub_bytes(pos, pos + n))) {
            Result.Ok(b) => { out = out + b; },
            Result.Err(e) => { print("err: " + e); return 1; },
        }
        pos = pos + n;
        k = k + 1;
    }
    print(out == plain);
    print(inflate.stream_finished(s));
    print(inflate.stream_remaining(s));
    inflate.stream_close(s);
    // (b) tope por push: rebasarlo es Err y el stream queda fallido.
    let t = inflate.inflate_stream();
    inflate.stream_set_limit(t, 100);
    match (inflate.stream_push(t, z)) { Result.Ok(b) => print("no error"), Result.Err(e) => print(e), }
    match (inflate.stream_push(t, b"\x00")) { Result.Ok(b) => print("no error"), Result.Err(e) => print("sticky"), }
    // (c) cerrar a medias: lo que venga después se ignora.
    let u = inflate.inflate_stream();
    let first = match (inflate.stream_push(u, z.sub_bytes(0, 40))) { Result.Ok(b) => b.len() > 0, Result.Err(e) => false, };
    inflate.stream_close(u);
    match (inflate.stream_push(u, z.sub_bytes(40, z.len()))) { Result.Ok(b) => print(b.len()), Result.Err(e) => print(e), }
    print(inflate.stream_finished(u));
    print(first);
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "true\ntrue\n5441494c\ndecompressed output exceeds the limit (possible decompression bomb)\nsticky\n0\ntrue\ntrue\n");
}
