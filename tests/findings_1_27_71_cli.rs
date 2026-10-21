//! Hallazgos atendidos en 1.27.71 (rayzip #200, raycron #202; M416): hora LOCAL del sistema —
//! `time.local_offset_ms`/`to_local`/`now_local` en `std/time` (localtime_r / la zona activa de
//! Windows), `tz.from_os()` y el `tz.system()` que ya no falla, y el solape de otoño de `cron/local`
//! que dispara dos veces para un schedule horario. En los tres motores.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ray_f1_27_71_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `tz` vacío = sin `TZ` en el entorno (la zona del sistema de verdad; `TZ=""` sería UTC en libc).
fn ray(dir: &PathBuf, args: &[&str], tz: &str) -> (String, String, i32) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ray"));
    cmd.args(args).current_dir(dir);
    if tz.is_empty() {
        cmd.env_remove("TZ");
    } else {
        cmd.env("TZ", tz);
    }
    let out = cmd.output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Corre `prog.ray` de `dir` con `TZ=tz` en VM, intérprete y (si hay rustc) nativo; los tres deben dar `want`.
fn three_engines(dir: &PathBuf, tz: &str, want: &str) {
    for flags in [&[][..], &["--interp"][..]] {
        let mut args = vec!["run"];
        args.extend_from_slice(flags);
        args.push("prog.ray");
        let (out, err, code) = ray(dir, &args, tz);
        assert_eq!(code, 0, "{flags:?}: {out}\n{err}");
        assert_eq!(out, want, "{flags:?}");
    }
    if has_rustc() {
        let bin = dir.join("prog_native");
        let (out, err, code) = ray(dir, &["build", "--native", "prog.ray", "-o", bin.to_str().unwrap()], tz);
        assert_eq!(code, 0, "build: {out}\n{err}");
        let o = Command::new(&bin).env("TZ", tz).current_dir(dir).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "nativo");
    }
}

fn project(d: &PathBuf) {
    let tz = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/tz");
    let cron = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/cron");
    std::fs::write(
        d.join("ray.toml"),
        format!(
            "[package]\nname = \"lo\"\nversion = \"0.1.0\"\nentry = \"prog.ray\"\n\n[dependencies]\ntz = \"path:{}\"\ncron = \"path:{}\"\n",
            tz.display(),
            cron.display()
        ),
    )
    .unwrap();
}

/// rayzip #200 (M416): `time.local_offset_ms` da el desplazamiento del SO con DST (Madrid: +1 h en
/// enero, +2 h en julio; Santiago: −3 h en enero, −4 h en julio), `to_local` lo aplica, y la zona
/// del SO de `tz` resuelve Gap/Ambiguous y `is_dst_at` sin ningún TZif. En unix el SO honra `TZ`.
#[cfg(unix)]
#[test]
fn the_local_offset_comes_from_the_os_on_all_engines() {
    let d = tmp("local_offset");
    project(&d);
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/time;
import tz/tz;
fn main() -> int {
    let jan = 1768478400000;
    let jul = 1784116800000;
    print(time.local_offset_ms(jan));
    print(time.local_offset_ms(jul));
    print(time.to_iso8601(time.to_local(jul)));
    let z = tz.from_os();
    print(tz.name(z));
    print(tz.offset_at(z, jan) == time.local_offset_ms(jan));
    print(tz.abbrev_at(z, jul));
    print(tz.is_dst_at(z, jul));
    print(tz.is_dst_at(z, jan));
    match (tz.to_utc(z, time.DateTime { year: 2026, month: 10, day: 25, hour: 2, minute: 30, second: 0, weekday: 0 })) {
        tz.LocalResult.Ambiguous(a, b) => print("ambiguous " + to_string(b - a)),
        tz.LocalResult.Single(t) => print("single"),
        tz.LocalResult.Gap => print("gap"),
    }
    match (tz.to_utc(z, time.DateTime { year: 2026, month: 3, day: 29, hour: 2, minute: 30, second: 0, weekday: 0 })) {
        tz.LocalResult.Ambiguous(a, b) => print("ambiguous"),
        tz.LocalResult.Single(t) => print("single"),
        tz.LocalResult.Gap => print("gap"),
    }
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "Europe/Madrid", "3600000\n7200000\n2026-07-15T14:00:00Z\nsystem\ntrue\nUTC+02:00\ntrue\nfalse\nambiguous 3600000\ngap\n");
    three_engines(&d, "America/Santiago", "-10800000\n-14400000\n2026-07-15T08:00:00Z\nsystem\ntrue\nUTC-04:00\nfalse\ntrue\nsingle\nsingle\n");
}

/// Siempre y en todas partes: `tz.system()` ya no falla (cae a la zona del SO) y el desplazamiento
/// local es un valor plausible (±14 h) — lo que en Windows es la única hora local.
#[test]
fn tz_system_always_resolves() {
    let d = tmp("tz_system");
    project(&d);
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/time;
import tz/tz;
fn main() -> int {
    let off = time.local_offset_ms(time.now());
    print(off >= 0 - 14 * 3600000 && off <= 14 * 3600000);
    match (tz.system()) {
        Result.Ok(z) => print(tz.offset_at(z, time.now()) == off),
        Result.Err(e) => print(e),
    }
    0
}
"#,
    )
    .unwrap();
    let (out, err, code) = ray(&d, &["run", "prog.ray"], "");
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(out, "true\ntrue\n");
}
