//! M145 — std/audio: salida PCM. La batería corre contra el SUMIDERO NULO (`RAY_AUDIO_SINK=null`,
//! consume a ritmo de tiempo real — CI no tiene tarjeta de sonido) en los TRES motores, y la
//! aserción clave es de TIEMPO: escribir 300 ms de audio + drain debe tardar al menos ~300 ms —
//! la prueba de que la contrapresión (pipe lleno → fibra aparcada) marca el pacing de verdad.

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

fn tmp(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("ray_audio_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

const PROG: &str = r#"import std/audio;

fn main() {
    match (audio.open(999, 1)) {
        Result.Ok(_) => print("bad: 999 Hz accepted"),
        Result.Err(e) => print("rate rejected: " + to_string(e.contains("unsupported sample rate"))),
    }
    match (audio.open(8000, 1)) {
        Result.Err(e) => print("open failed: " + e),
        Result.Ok(h) => {
            // 300 ms de PCM (8000 Hz mono s16 = 16000 octetos/s → 4800 octetos).
            var samples: [int] = [];
            var i = 0;
            while (i < 4800) {
                samples.push(i % 256);
                i = i + 1;
            }
            print("write ok: " + to_string(audio.write(h, bytes_of(samples)).is_ok()));
            print("drain ok: " + to_string(audio.drain(h).is_ok()));
            let _ = close(h);
            print("drain after close: " + to_string(audio.drain(h).is_err()));
        },
    }
}
"#;

const WANT: &str = "rate rejected: true\nwrite ok: true\ndrain ok: true\ndrain after close: true\n";

fn run_timed(cmd: &mut Command) -> (String, i32, u128) {
    cmd.env("RAY_AUDIO_SINK", "null");
    let t0 = Instant::now();
    let out = cmd.stdin(std::process::Stdio::null()).output().expect("corre");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
        t0.elapsed().as_millis(),
    )
}

#[test]
fn null_sink_battery_and_pacing_match_on_all_three_engines() {
    let base = tmp("battery");
    std::fs::write(base.join("prog.ray"), PROG).unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, code, ms) =
            run_timed(Command::new(env!("CARGO_BIN_EXE_ray")).args([engine, "prog.ray"]).current_dir(&base));
        assert_eq!(code, 0, "{engine}: exit 0");
        assert_eq!(out, WANT, "{engine}: salida exacta");
        // 300 ms de audio: el sumidero consume a tiempo real → menos que eso = no hubo pacing.
        assert!(ms >= 280, "{engine}: la contrapresión debe marcar el paso (tardó {ms} ms)");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let (out, code, ms) = run_timed(&mut Command::new(&bin));
        assert_eq!(code, 0, "nativo: exit 0");
        assert_eq!(out, WANT, "nativo ≡ VM");
        assert!(ms >= 280, "nativo: la contrapresión debe marcar el paso (tardó {ms} ms)");
    }
}

/// M158 (§79b): open_latency valida el hint (20–1000, 0 = default) y played_ms avanza de
/// verdad sobre el sumidero de tiempo real. 3 motores, salida exacta (los booleanos absorben
/// la variación de reloj).
const V2_PROG: &str = r#"import std/audio;

fn main() {
    match (audio.open_latency(44100, 2, 5)) {
        Result.Ok(_) => print("bad: 5ms accepted"),
        Result.Err(e) => print("latency rejected: " + to_string(e.contains("20"))),
    }
    let h = match (audio.open_latency(8000, 1, 50)) {
        Result.Ok(h) => h,
        Result.Err(e) => {
            print("open failed: " + e);
            return;
        },
    };
    print("early played: " + to_string(match (audio.played_ms(h)) {
        Result.Ok(ms) => ms == 0,
        Result.Err(_) => false,
    }));
    var chunk = b"";
    var i = 0;
    while (i < 3200) {
        chunk = chunk + b"\x00\x00";
        i = i + 1;
    }
    let _ = audio.write(h, chunk);
    let _ = audio.drain(h);
    match (audio.played_ms(h)) {
        Result.Ok(ms) => print("played advanced: " + to_string(ms > 100 && ms <= 500)),
        Result.Err(e) => print("played err: " + e),
    }
    let _ = close(h);
    match (audio.played_ms(h)) {
        Result.Ok(_) => print("bad: closed handle answered"),
        Result.Err(e) => print("closed rejected: " + to_string(e.contains("not an open audio output"))),
    }
}
"#;

#[test]
fn latency_hint_and_played_position_match_on_all_three_engines() {
    const WANT: &str =
        "latency rejected: true\nearly played: true\nplayed advanced: true\nclosed rejected: true\n";
    let base = tmp("v2");
    std::fs::write(base.join("prog.ray"), V2_PROG).unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, code, _) = run_timed(
            Command::new(env!("CARGO_BIN_EXE_ray")).args([engine, "prog.ray"]).current_dir(&base),
        );
        assert_eq!(code, 0, "{engine}: exit 0\n{out}");
        assert_eq!(out, WANT, "{engine}: exact output");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("native build");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let (out, code, _) = run_timed(&mut Command::new(&bin));
        assert_eq!(code, 0, "native: exit 0\n{out}");
        assert_eq!(out, WANT, "native ≡ VM");
    }
}

/// M298 (findings 1.27.11 #21 y #22). [21] La cola entre el programa y el alimentador es un
/// `socketpair` acotado a la latencia pedida: a 8000 Hz mono con 20 ms el buffer es el mínimo
/// (2 KiB ≈ 128 ms), así que escribir 600 ms de audio BLOQUEA ~470 ms — antes el pipe del SO
/// (64 KiB = 4 s a esa tasa) se lo tragaba sin aparcar. [22] Seis ciclos open/write/drain/close
/// seguidos: `played_ms` responde en todos (el alimentador viejo ya no borra la entrada de una
/// salida nueva que heredó su fd).
const V3_PROG: &str = r#"import std/audio;
import std/time;

fn main() {
    var chunk = b"";
    var i = 0;
    while (i < 4800) {
        chunk = chunk + b"\x00\x00";
        i = i + 1;
    }
    let h = match (audio.open_latency(8000, 1, 20)) {
        Result.Ok(h) => h,
        Result.Err(e) => {
            print("open failed: " + e);
            return;
        },
    };
    let t0 = time.monotonic();
    let _ = audio.write(h, chunk);
    let dt = time.monotonic() - t0;
    print("write paced by the latency hint: " + to_string(dt >= 300));
    let _ = close(h);

    var small = b"";
    i = 0;
    while (i < 800) {
        small = small + b"\x00\x00";
        i = i + 1;
    }
    var ok = true;
    var n = 0;
    while (n < 6) {
        match (audio.open(8000, 1)) {
            Result.Err(e) => {
                print("open failed: " + e);
                return;
            },
            Result.Ok(h2) => {
                let _ = audio.write(h2, small);
                let _ = audio.drain(h2);
                match (audio.played_ms(h2)) {
                    Result.Ok(ms) => { if (ms <= 0) { ok = false; } },
                    Result.Err(_) => { ok = false; },
                }
                let _ = close(h2);
            },
        }
        n = n + 1;
    }
    print("played_ms after reopen: " + to_string(ok));
}
"#;

#[test]
fn queue_is_bounded_by_the_latency_hint_and_reopen_keeps_played_ms() {
    const WANT: &str = "write paced by the latency hint: true\nplayed_ms after reopen: true\n";
    let base = tmp("v3");
    std::fs::write(base.join("prog.ray"), V3_PROG).unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, code, _) = run_timed(
            Command::new(env!("CARGO_BIN_EXE_ray")).args([engine, "prog.ray"]).current_dir(&base),
        );
        assert_eq!(code, 0, "{engine}: exit 0\n{out}");
        assert_eq!(out, WANT, "{engine}: exact output");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let (out, code, _) = run_timed(&mut Command::new(&bin));
        assert_eq!(code, 0, "nativo: exit 0\n{out}");
        assert_eq!(out, WANT, "nativo ≡ VM");
    }
}
