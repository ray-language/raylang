//! Hallazgos atendidos en 1.27.74 (ronda 1.27.73 de las apps): #223 el `stream_push` del runtime
//! perdía datos con más de un trozo; #226 la política de otoño de `cron/local`.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ray_f1_27_74_{name}_{}", std::process::id()));
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

/// rayzip #223: un stream muy comprimible (3 MB de `a` en 2,9 KB) y uno de texto, troceados en
/// 100, 500, 1000, 2000 y 64 KiB octetos, dan la salida entera y `finished` — en 1.27.73 salían
/// truncados (a veces con `finished = true`) o con `corrupt DEFLATE stream`.
#[test]
fn stream_push_keeps_every_byte_whatever_the_chunking() {
    let d = tmp("stream_chunks");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/deflate;
import std/inflate;
fn feed(raw: bytes, chunk: int) -> (int, bool) {
    let s = inflate.inflate_stream();
    inflate.stream_set_limit(s, 1 << 40);
    var total = 0;
    var pos = 0;
    while (pos < raw.len()) {
        var end = pos + chunk;
        if (end > raw.len()) { end = raw.len(); }
        match (inflate.stream_push(s, raw.sub_bytes(pos, end))) {
            Result.Ok(b) => { total = total + b.len(); },
            Result.Err(e) => { print("err: " + e); return (total, false); },
        }
        pos = end;
    }
    (total, inflate.stream_finished(s))
}
fn main() -> int {
    let raw = deflate.deflate_raw("a".repeat(3000000).to_bytes());
    for c in [100, 500, 1000, 2000, raw.len()] {
        let (n, fin) = feed(raw, c);
        print("${n} ${fin}");
    }
    var lines: [string] = [];
    for i in 0..20000 {
        lines.push("line ${i}: the quick brown fox jumps over the lazy dog ${i * 7919 % 1000}");
    }
    let text = join(lines, "\n").to_bytes();
    let (n, fin) = feed(deflate.deflate_raw(text), 65536);
    print(n == text.len() && fin);
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "3000000 true\n3000000 true\n3000000 true\n3000000 true\n3000000 true\ntrue\n");
}

/// raycron #226: en el cambio de otoño de Santiago (fixture no commiteado: se usa el zoneinfo del
/// sistema si está) `*/7` recorre la segunda pasada entera sin `Err`, y `*/15` sus cuatro cuartos.
#[cfg(unix)]
#[test]
fn cron_local_walks_the_whole_repeated_hour_in_santiago() {
    if !std::path::Path::new("/usr/share/zoneinfo/America/Santiago").exists() {
        return;
    }
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let d = tmp("cron_santiago");
    std::fs::write(
        d.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nentry = \"prog.ray\"\n\n[dependencies]\ncron = \"path:{}\"\ntz = \"path:{}\"\n",
            repo.join("packages/cron").display(),
            repo.join("packages/tz").display()
        ),
    )
    .unwrap();
    std::fs::write(
        d.join("prog.ray"),
        r#"import cron/cron;
import cron/local;
import tz/tz;
fn count(expr: string, from: int, until: int) -> string {
    let z = tz.load("America/Santiago").unwrap();
    let s = cron.parse(expr).unwrap();
    var a = from;
    var n = 0;
    while (true) {
        match (local.next_after_in(s, z, a)) {
            Result.Ok(t) => { if (t >= until) { break; } n = n + 1; a = t; },
            Result.Err(e) => { return "ERR " + e; },
        }
    }
    to_string(n)
}
fn main() -> int {
    // 5 abr 2026: 00:00 −03 vuelve a 23:00 −04 (03:00Z). De 02:00Z a 04:00Z hay dos horas reales.
    print(count("*/7 * * * *", 1775354400000, 1775361600000));
    print(count("*/15 * * * *", 1775354400000, 1775361600000));
    print(count("0 23 * * *", 1775340000000, 1775426400000));
    0
}
"#,
    )
    .unwrap();
    let (out, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!(code, 0, "{out}\n{err}");
    // Estrictamente después de 02:00Z (= 23:00 −03, que no cuenta): */7 da 8 en la primera pasada
    // (23:07…23:56 −03) + 9 en la segunda (23:00…23:56 −04); */15, 3 + 4; `0 23` fijado: uno.
    assert_eq!(out, "17\n7\n1\n");
}
