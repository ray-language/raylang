//! M468 (ray-daw #424) — std/audio: `devices()` y abrir un dispositivo por su id. La lista
//! depende de la máquina (en CI no hay tarjeta de sonido y sale vacía), así que se comprueba su
//! forma — cada dispositivo tiene id y al menos una dirección — y que un id inexistente da `Err`
//! en la salida y en la entrada, igual en los tres motores.
#![cfg(any(unix, windows))]

use std::process::Command;

const PROG: &str = r#"import std/audio;

fn main() -> int {
    var shaped = true;
    for d in audio.devices() {
        if (d.id.len() == 0 || d.input_channels + d.output_channels == 0) { shaped = false; }
        if (d.input_channels < 0 || d.output_channels < 0 || d.sample_rate < 0) { shaped = false; }
    }
    print("devices are well formed: ${shaped}");
    print("unknown output: ${audio.open_device("no-such-device", 48000, 2, "s16le", 0).is_err()}");
    print("unknown input: ${audio.open_input_device("no-such-device", 48000, 1, "s16le", 0).is_err()}");
    match (audio.open_device("no-such-device", 48000, 2, "s8", 0)) {
        Result.Ok(_) => print("bad: s8 accepted"),
        Result.Err(e) => print("format checked first: ${e.contains("unsupported format 's8'")}"),
    }
    0
}
"#;

const WANT: &str = "devices are well formed: true\nunknown output: true\nunknown input: true\nformat checked first: true\n";

fn workdir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ray_audio_dev_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("prog.ray"), PROG).unwrap();
    d
}

#[test]
fn devices_are_listed_and_an_unknown_id_is_an_error_on_the_vm_and_the_interpreter() {
    let d = workdir("vm");
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).env_remove("RAY_AUDIO_SINK").env_remove("RAY_AUDIO_SOURCE").current_dir(&d).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stderr), "", "{args:?}");
        assert_eq!(String::from_utf8_lossy(&o.stdout), WANT, "{args:?}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn devices_are_listed_and_an_unknown_id_is_an_error_natively() {
    if !Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        return;
    }
    let d = workdir("native");
    let bin = d.join("prog_bin");
    let b = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", "--native", "prog.ray", "-o", bin.to_str().unwrap()]).current_dir(&d).output().unwrap();
    assert!(b.status.success(), "build --native: {}", String::from_utf8_lossy(&b.stderr));
    let o = Command::new(&bin).env_remove("RAY_AUDIO_SINK").env_remove("RAY_AUDIO_SOURCE").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout), WANT);
    let _ = std::fs::remove_dir_all(&d);
}

/// Con el sumidero y la fuente de pruebas no hay dispositivos: el id se acepta y se ignora, de
/// modo que un programa que elige dispositivo se puede probar en CI.
#[test]
fn the_test_sink_and_source_accept_any_device() {
    let d = workdir("null");
    std::fs::write(
        d.join("prog.ray"),
        "import std/audio;\nfn main() -> int {\n    let o = audio.open_device(\"whatever\", 48000, 2, \"s16le\", 0);\n    print(o.is_ok());\n    let _ = close(o.unwrap());\n    let i = audio.open_input_device(\"whatever\", 48000, 1, \"s16le\", 0);\n    print(i.is_ok());\n    let _ = close(i.unwrap());\n    0\n}\n",
    )
    .unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).env("RAY_AUDIO_SINK", "null").env("RAY_AUDIO_SOURCE", "null").current_dir(&d).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stderr), "");
    assert_eq!(String::from_utf8_lossy(&o.stdout), "true\ntrue\n");
    let _ = std::fs::remove_dir_all(&d);
}
