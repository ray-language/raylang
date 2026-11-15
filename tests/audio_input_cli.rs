//! M463 (ray-daw #422) — std/audio: ENTRADA de audio. La batería corre contra la fuente de
//! pruebas (`RAY_AUDIO_SOURCE=tone`: un La de 440 Hz a ritmo de tiempo real; CI no tiene
//! micrófono) en la VM, el intérprete y el binario nativo. Lo que se comprueba: los bloques
//! llegan alineados a frame, a su ritmo (leer medio segundo tarda medio segundo: `read` aparca
//! la fibra, no gira), el reloj de captura avanza aunque nadie lea, y lo que no se lee a tiempo
//! se descarta por frames enteros.
#![cfg(unix)]

use std::process::Command;

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

const PROG: &str = r#"import std/audio;
import std/time;
import std/binary;

fn main() -> int {
    match (audio.open_input(999, 1, "s16le", 0)) {
        Result.Ok(_) => print("bad: 999 Hz accepted"),
        Result.Err(e) => print("rate rejected: ${e.contains("unsupported sample rate")}"),
    }
    match (audio.open_input(48000, 2, "s8", 0)) {
        Result.Ok(_) => print("bad: s8 accepted"),
        Result.Err(_) => print("format rejected: true"),
    }
    let h = audio.open_input(48000, 2, "s16le", 40).unwrap();
    // Medio segundo de estéreo s16: 24000 frames de 4 octetos.
    var total = 0;
    var peak = 0;
    var aligned = true;
    let t0 = time.now();
    while (total < 24000 * 4) {
        let b = audio.read(h).unwrap();
        if (b.len() == 0 || b.len() % 4 != 0) {
            aligned = false;
            total = 24000 * 4;
        } else {
            var i = 0;
            while (i < b.len()) {
                let v = binary.i16le(b, i);
                if (v > peak) { peak = v; }
                i = i + 2;
            }
            total = total + b.len();
        }
    }
    let ms = time.now() - t0;
    print("aligned: ${aligned}");
    print("tone heard: ${peak > 16000 && peak < 16500}");
    print("paced: ${ms >= 400 && ms < 2500}");
    // Sin leer durante 1,4 s el reloj sigue; la cola guarda ~1 s y descarta el resto, entero.
    time.sleep(1400);
    let clock = audio.recorded_ms(h).unwrap();
    print("clock runs: ${clock >= 1800}");
    var backlog = 0;
    var still_aligned = true;
    let t1 = time.now();
    while (time.now() - t1 < 300) {
        let b = audio.read(h).unwrap();
        if (b.len() % 4 != 0) { still_aligned = false; }
        backlog = backlog + b.len();
    }
    print("backlog bounded: ${backlog <= 48000 * 4 * 16 / 10}");
    print("aligned after overrun: ${still_aligned}");
    let _ = close(h);
    print("clock after close: ${audio.recorded_ms(h).is_err()}");
    0
}
"#;

const WANT: &str = "rate rejected: true\nformat rejected: true\naligned: true\ntone heard: true\npaced: true\nclock runs: true\nbacklog bounded: true\naligned after overrun: true\nclock after close: true\n";

fn workdir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ray_audio_in_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("prog.ray"), PROG).unwrap();
    d
}

#[test]
fn input_reads_paced_aligned_frames_on_the_vm_and_the_interpreter() {
    let d = workdir("vm");
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).env("RAY_AUDIO_SOURCE", "tone").current_dir(&d).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stderr), "", "{args:?}");
        assert_eq!(String::from_utf8_lossy(&o.stdout), WANT, "{args:?}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn input_reads_paced_aligned_frames_natively() {
    if !has_rustc() {
        return;
    }
    let d = workdir("native");
    let bin = d.join("prog_bin");
    let b = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", "--native", "prog.ray", "-o", bin.to_str().unwrap()]).current_dir(&d).output().unwrap();
    assert!(b.status.success(), "build --native: {}", String::from_utf8_lossy(&b.stderr));
    let o = Command::new(&bin).env("RAY_AUDIO_SOURCE", "tone").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout), WANT);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn silence_source_and_other_formats() {
    // `null` entrega silencio; el formato pedido es el que llega (f32le: 4 octetos por muestra).
    let d = workdir("null");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/audio;
fn main() -> int {
    let h = audio.open_input(44100, 1, "f32le", 0).unwrap();
    var total = 0;
    var silent = true;
    while (total < 44100 / 5 * 4) {
        let b = audio.read(h).unwrap();
        var i = 0;
        while (i < b.len()) {
            if (b[i] != 0) { silent = false; }
            i = i + 1;
        }
        total = total + b.len();
    }
    print("${total % 4 == 0} ${silent}");
    let _ = close(h);
    0
}
"#,
    )
    .unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).env("RAY_AUDIO_SOURCE", "null").current_dir(&d).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stderr), "");
    assert_eq!(String::from_utf8_lossy(&o.stdout), "true true\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn every_read_is_whole_frames_whatever_the_frame_size() {
    // Revisión de 1.27.101: con un flujo, la lectura de 64 KiB del runtime partía los frames
    // que no dividen 65536 (s24le estéreo: 6 octetos) en cuanto había atraso. Cada bloque viaja
    // ahora como un datagrama.
    let d = workdir("frames");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/audio;
import std/time;
fn main() -> int {
    let h = audio.open_input(48000, 2, "s24le", 40).unwrap();
    time.sleep(700);
    var reads = 0;
    var misaligned = 0;
    var total = 0;
    while (total < 48000 * 6 * 9 / 10) {
        let b = audio.read(h).unwrap();
        if (b.len() == 0 || b.len() % 6 != 0) { misaligned = misaligned + 1; }
        total = total + b.len();
        reads = reads + 1;
    }
    print("${reads > 10} ${misaligned}");
    let _ = close(h);
    0
}
"#,
    )
    .unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).env("RAY_AUDIO_SOURCE", "tone").current_dir(&d).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stderr), "");
    assert_eq!(String::from_utf8_lossy(&o.stdout), "true 0\n");
    let _ = std::fs::remove_dir_all(&d);
}
