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
        let bin = base.join("prog_bin");
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
