//! M464 (ray-daw #423) — std/midi. En macOS la batería corre de verdad contra CoreMIDI, sin
//! hardware: un puerto VIRTUAL creado por el programa aparece en la lista del sistema, se abre
//! como cualquier otro y lo enviado por un extremo llega por el otro. M466: lo mismo en Linux
//! contra el secuenciador de ALSA, cuando la máquina lo tiene (`/dev/snd/seq`; los runners de
//! CI no). Windows no tiene puertos virtuales: allí se comprueba que todo falla con un `Err`.
use std::process::Command;

/// ¿Hay un backend con puertos virtuales contra el que correr la batería?
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn virtual_ports_available() -> bool {
    if cfg!(target_os = "macos") {
        return true;
    }
    let d = workdir("probe", "import std/midi;\nfn main() -> int {\n    print(midi.watch().is_ok());\n    0\n}\n");
    let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).current_dir(&d).output().unwrap();
    let _ = std::fs::remove_dir_all(&d);
    let available = String::from_utf8_lossy(&o.stdout).trim() == "true";
    if !available {
        eprintln!("midi_cli: no ALSA sequencer here (/dev/snd/seq), skipping the virtual-port tests");
    }
    available
}

fn workdir(name: &str, src: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ray_midi_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("prog.ray"), src).unwrap();
    d
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Salida virtual → entrada: una nota va y vuelve con su marca de tiempo, en los tres motores.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_virtual_output_loops_back_through_an_input_in_the_three_engines() {
    if !virtual_ports_available() {
        return;
    }
    let src = r#"import std/midi;
fn main() -> int {
    print("inputs before: ${midi.inputs().len() >= 0}");
    let out = midi.create_output("ray test out").unwrap();
    var id = 0;
    var found = false;
    for p in midi.inputs() {
        if (p.name == "ray test out") { id = p.id; found = true; }
    }
    print("virtual output listed as input: ${found}");
    let inp = midi.open_input(id).unwrap();
    print(midi.send(out, b"\x90\x3c\x64"));
    let m = midi.read(inp).unwrap();
    print("${m.data} ${m.timestamp_ns > 0}");
    print(midi.send(out, b"\x80\x3c\x00"));
    let m2 = midi.read(inp).unwrap();
    print("${m2.data} ${m2.timestamp_ns >= m.timestamp_ns}");
    let _ = close(inp);
    let _ = close(out);
    0
}
"#;
    let want = "inputs before: true\nvirtual output listed as input: true\nResult.Ok(3)\n903c64 true\nResult.Ok(3)\n803c00 true\n";
    let d = workdir("loop", src);
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(&d).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stderr), "", "{args:?}");
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "{args:?}");
    }
    if has_rustc() {
        let bin = d.join("prog_bin");
        let b = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", "--native", "prog.ray", "-o", bin.to_str().unwrap()]).current_dir(&d).output().unwrap();
        assert!(b.status.success(), "build --native: {}", String::from_utf8_lossy(&b.stderr));
        assert_eq!(String::from_utf8_lossy(&Command::new(&bin).output().unwrap().stdout), want);
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// Entrada virtual, SysEx largo, abrir y cerrar muchas veces, un id inexistente y el aviso de
/// cambios al aparecer un puerto.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn virtual_inputs_sysex_churn_and_change_notifications() {
    if !virtual_ports_available() {
        return;
    }
    let src = r#"import std/midi;
import std/time;
fn main() -> int {
    // Entrada virtual: otro "programa" (nosotros) le envía por su puerto de salida.
    let inp = midi.create_input("ray test in").unwrap();
    var id = 0;
    var found = false;
    for p in midi.outputs() {
        if (p.name == "ray test in") { id = p.id; found = true; }
    }
    print("virtual input listed as output: ${found}");
    let out = midi.open_output(id).unwrap();
    print(midi.send(out, b"\xb0\x07\x7f"));
    print(midi.read(inp).unwrap().data);
    // Un SysEx más largo que un paquete corto.
    var sysex: [int] = [0xf0];
    var i = 0;
    while (i < 300) { sysex.push(i % 128); i = i + 1; }
    sysex.push(0xf7);
    print(midi.send(out, bytes_of(sysex)));
    var got = 0;
    while (got < 302) { got = got + midi.read(inp).unwrap().data.len(); }
    print("sysex bytes: ${got}");
    let _ = close(out);
    let _ = close(inp);
    // Abrir y cerrar muchas veces no agota nada.
    var n = 0;
    while (n < 40) {
        let h = midi.create_input("ray churn").unwrap();
        let _ = close(h);
        n = n + 1;
    }
    print("churn ok: ${midi.outputs().len() >= 0}");
    var leftover = false;
    for p in midi.outputs() { if (p.name == "ray churn" || p.name == "ray test in") { leftover = true; } }
    print("ports released: ${!leftover}");
    print(midi.open_input(123456789).is_err());
    // Aviso de cambios.
    let w = midi.watch().unwrap();
    let t = spawn(fn() {
        time.sleep(150);
        let h = midi.create_output("ray hotplug").unwrap();
        time.sleep(300);
        let _ = close(h);
    });
    print("change seen: ${midi.next_change(w).is_ok()}");
    t.join();
    let _ = close(w);
    0
}
"#;
    let want = "virtual input listed as output: true\nResult.Ok(3)\nb0077f\nResult.Ok(302)\nsysex bytes: 302\nchurn ok: true\nports released: true\ntrue\nchange seen: true\n";
    let d = workdir("full", src);
    let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).current_dir(&d).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stderr), "");
    assert_eq!(String::from_utf8_lossy(&o.stdout), want);
    let _ = std::fs::remove_dir_all(&d);
}

/// Un puerto que no existe da `Err` en cualquier sistema, haya o no backend, y listar no rompe.
#[test]
fn a_missing_port_is_an_error_everywhere() {
    let src = r#"import std/midi;
fn main() -> int {
    print("${midi.inputs().len() >= 0} ${midi.outputs().len() >= 0}");
    print(midi.open_input(987654321).is_err());
    print(midi.open_output(987654321).is_err());
    0
}
"#;
    let d = workdir("missing", src);
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(&d).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stderr), "", "{args:?}");
        assert_eq!(String::from_utf8_lossy(&o.stdout), "true true\ntrue\ntrue\n", "{args:?}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// M466: Windows (WinMM) no tiene puertos virtuales: crearlos da un `Err` que lo dice, y vigilar
/// los cambios funciona (sondea la lista de dispositivos).
#[cfg(windows)]
#[test]
fn windows_has_no_virtual_ports_and_watch_opens() {
    let src = r#"import std/midi;
fn main() -> int {
    match (midi.create_output("x")) {
        Result.Ok(_) => print("bad: created"),
        Result.Err(e) => print(e.contains("no virtual MIDI ports")),
    }
    print(midi.create_input("x").is_err());
    let w = midi.watch();
    print(w.is_ok());
    let _ = close(w.unwrap());
    0
}
"#;
    let d = workdir("windows", src);
    let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).current_dir(&d).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stderr), "");
    assert_eq!(String::from_utf8_lossy(&o.stdout), "true\ntrue\ntrue\n");
    let _ = std::fs::remove_dir_all(&d);
}

/// Revisión de 1.27.101: listar puertos sondeaba cada entrada abierta con un datagrama; en una
/// entrada que nadie leía llenaban la cola y los mensajes de verdad se perdían.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn listing_ports_does_not_fill_an_idle_input() {
    if !virtual_ports_available() {
        return;
    }
    let src = r#"import std/midi;
fn main() -> int {
    let inp = midi.create_input("ray idle in").unwrap();
    var id = 0;
    for p in midi.outputs() { if (p.name == "ray idle in") { id = p.id; } }
    let out = midi.open_output(id).unwrap();
    var n = 0;
    while (n < 6000) { let _ = midi.outputs(); n = n + 1; }
    print(midi.send(out, b"\x90\x40\x7f"));
    print(midi.read(inp).unwrap().data);
    let _ = close(out);
    let _ = close(inp);
    0
}
"#;
    let d = workdir("idle", src);
    let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).current_dir(&d).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stderr), "");
    assert_eq!(String::from_utf8_lossy(&o.stdout), "Result.Ok(3)\n90407f\n");
    let _ = std::fs::remove_dir_all(&d);
}
