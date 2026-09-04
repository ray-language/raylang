//! Pruebas de **std/io** (M107.1): escritura a stdout/stderr SIN salto de línea + flush.
//!
//! Lo delicado no es escribir: es el ORDEN. `print` y `io.write` deben intercalarse en orden de
//! programa en los TRES motores — y en el binario nativo `print` es asíncrono (M96f: un hilo
//! escritor consume un canal), así que `io.write` va por el mismo canal; esta suite lo asevera
//! comparando la salida EXACTA byte a byte, stdout y stderr por separado.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_io_{name}"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("lanza ray");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

const PROG: &str = "import std/io;\n\
\n\
fn main() -> int {\n\
    let _ = io.write(\"a\");\n\
    let _ = io.write(\"b\");\n\
    let _ = io.flush();\n\
    print(\"\");\n\
    let _ = io.write_bytes(b\"\\x41\\x42\");\n\
    print(\"\");\n\
    let _ = io.ewrite(\"E1\");\n\
    let _ = io.ewrite(\"E2\");\n\
    eprint(\"\");\n\
    match (io.write(\"n=3\")) {\n\
        Result.Ok(n) => print(n),\n\
        Result.Err(e) => eprint(e),\n\
    }\n\
    0\n\
}\n";

// La salida esperada: `write`s pegados, el `print(\"\")` cierra la línea EN ORDEN aunque el write
// no haya hecho flush (mismo buffer), y el conteo de `write` es el nº de caracteres.
const WANT_OUT: &str = "ab\nAB\nn=33\n";
const WANT_ERR: &str = "E1E2\n";

#[test]
fn io_writes_interleave_with_print_in_program_order() {
    let base = tmp("orden");
    std::fs::write(base.join("prog.ray"), PROG).unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, err, code) = ray(&base, &[engine, "prog.ray"]);
        assert_eq!(code, 0, "{engine}: exit 0\n{err}");
        assert_eq!(out, WANT_OUT, "{engine}: stdout exacto");
        assert_eq!(err, WANT_ERR, "{engine}: stderr exacto");
    }
}

#[test]
fn io_native_matches_the_vm_byte_for_byte() {
    // El caso que motivó el diseño: el primer intento escribía `io.write` directo a
    // `std::io::stdout` y los saltos de `print` (asíncrono) llegaban DESPUÉS de todos los writes.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando io nativo: rustc no disponible");
        return;
    }
    let base = tmp("nativo");
    std::fs::write(base.join("prog.ray"), PROG).unwrap();
    let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
    let (_o, berr, bcode) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(bcode, 0, "build --native ok\n{berr}");
    let native = Command::new(&bin).output().expect("corre el binario nativo");
    assert_eq!(String::from_utf8_lossy(&native.stdout), WANT_OUT, "nativo ≡ VM (stdout)");
    assert_eq!(String::from_utf8_lossy(&native.stderr), WANT_ERR, "nativo ≡ VM (stderr)");
    assert_eq!(native.status.code(), Some(0));
}

#[test]
fn write_bytes_emits_raw_bytes() {
    // `write_bytes` no pasa por UTF-8: un byte inválido (\xff) llega intacto al stdout.
    let base = tmp("crudo");
    std::fs::write(
        base.join("prog.ray"),
        "import std/io;\nfn main() -> int {\n    let _ = io.write_bytes(b\"\\x1b[2J\\xff\");\n    let _ = io.flush();\n    0\n}\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["run", "prog.ray"])
        .current_dir(&base)
        .output()
        .expect("lanza ray");
    assert_eq!(out.stdout, b"\x1b[2J\xff", "los bytes crudos llegan intactos (VM)");
}

// ── M107.2: lectura de stdin por bytes ──────────────────────────────────────────────────────────

const ECHO_PROG: &str = "import std/io;\n\
\n\
fn main() -> int {\n\
    var total = 0;\n\
    var done = false;\n\
    while (!done) {\n\
        match (io.read(8)) {\n\
            Option.Some(b) => {\n\
                total = total + b.len();\n\
                let _ = io.write_bytes(b);\n\
            },\n\
            Option.None => { done = true; },\n\
        }\n\
    }\n\
    let _ = io.flush();\n\
    print(\"\");\n\
    print(total);\n\
    0\n\
}\n";

/// Corre `prog` con `stdin` alimentado por pipe (todo el contenido y cierre) y devuelve stdout.
fn run_with_stdin(dir: &PathBuf, args: &[&str], stdin: &[u8]) -> (String, String, i32) {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("lanza ray");
    child.stdin.take().expect("stdin").write_all(stdin).expect("escribe stdin");
    let out = child.wait_with_output().expect("espera");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn stdin_read_by_bytes_until_eof() {
    // Eco por bytes hasta EOF, en los dos motores y en nativo: mismos bytes, mismo conteo.
    let base = tmp("read_eco");
    std::fs::write(base.join("prog.ray"), ECHO_PROG).unwrap();
    let want = "hola mundo\n10\n";
    for engine in ["--vm", "--interp"] {
        let (out, err, code) = run_with_stdin(&base, &[engine, "prog.ray"], b"hola mundo");
        assert_eq!(code, 0, "{engine}: exit 0\n{err}");
        assert_eq!(out, want, "{engine}: eco + conteo");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let (_o, berr, bcode) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(bcode, 0, "build --native ok\n{berr}");
        use std::io::Write;
        let mut child = Command::new(&bin)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("corre el binario nativo");
        child.stdin.take().unwrap().write_all(b"hola mundo").unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "nativo ≡ VM");
    }
}

#[test]
fn stdin_read_parks_the_fiber_not_the_vm() {
    // EL test del arco: mientras main espera un byte de stdin, las demás fibras siguen corriendo.
    // Determinista por HANDSHAKE (sin temporizadores, que el arranque en frío del binario falsea):
    // la hija imprime sus ticks, el padre los LEE por el pipe y solo entonces envía el byte —
    // si la lectura de stdin bloquease la VM entera, los ticks jamás llegarían y esto colgaría.
    let src = "import std/io;\n\
fn main() -> int {\n\
    let t = spawn(fn() {\n\
        var i = 0;\n\
        while (i < 3) { print(\"tick \" + to_string(i)); i = i + 1; }\n\
    });\n\
    match (io.read(16)) {\n\
        Option.Some(b) => print(\"got \" + to_string(b.len())),\n\
        Option.None => print(\"eof\"),\n\
    }\n\
    join(t);\n\
    0\n\
}\n";
    let base = tmp("read_park");
    std::fs::write(base.join("prog.ray"), src).unwrap();

    let mut cmds: Vec<(String, Command)> = Vec::new();
    let mut vm = Command::new(env!("CARGO_BIN_EXE_ray"));
    vm.args(["--vm", "--deterministic", "prog.ray"]).current_dir(&base);
    cmds.push(("VM".into(), vm));
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let (_o, berr, bcode) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(bcode, 0, "build --native ok\n{berr}");
        cmds.push(("nativo".into(), Command::new(&bin)));
    }
    for (label, mut cmd) in cmds {
        use std::io::{BufRead, Write};
        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("lanza");
        let mut stdin = child.stdin.take().unwrap();
        let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
        // Los tres ticks llegan MIENTRAS main está aparcado en stdin (aún no se envió nada).
        for i in 0..3 {
            let line = lines.next().expect("línea").expect("lee");
            assert_eq!(line, format!("tick {i}"), "{label}: la fibra hermana corre durante la espera");
        }
        stdin.write_all(b"x").unwrap();
        drop(stdin);
        assert_eq!(lines.next().expect("línea").expect("lee"), "got 1", "{label}: el byte llega tras los ticks");
        assert!(child.wait().unwrap().success(), "{label}: exit 0");
    }
}

#[test]
fn stdin_read_timeout_distinguishes_data_eof_and_timeout() {
    let src = "import std/io;\n\
fn main() -> int {\n\
    match (io.read_timeout(8, 80)) {\n\
        io.ReadResult.Data(b) => print(\"data \" + to_string(b.len())),\n\
        io.ReadResult.Eof => print(\"eof\"),\n\
        io.ReadResult.TimedOut => print(\"timeout\"),\n\
    }\n\
    0\n\
}\n";
    let base = tmp("read_tmo");
    std::fs::write(base.join("prog.ray"), src).unwrap();
    for engine in ["--vm", "--interp"] {
        // datos ya presentes → Data
        let (out, err, code) = run_with_stdin(&base, &[engine, "prog.ray"], b"abc");
        assert_eq!(code, 0, "{engine}\n{err}");
        assert_eq!(out, "data 3\n", "{engine}: datos");
        // stdin cerrado de entrada → Eof
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args([engine, "prog.ray"])
            .current_dir(&base)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("lanza");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "eof\n", "{engine}: EOF");
        // pipe abierto y mudo → TimedOut (el padre retiene el extremo de escritura sin escribir)
        let mut child = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args([engine, "prog.ray"])
            .current_dir(&base)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("lanza");
        // OJO: `wait_with_output` CIERRA el stdin del hijo (dropea el pipe) → parecería EOF.
        // Retener el extremo de escritura durante la espera es el punto de este caso.
        let stdin_keep = child.stdin.take();
        let out = child.wait_with_output().expect("espera");
        drop(stdin_keep);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "timeout\n", "{engine}: plazo vencido");
    }
}

#[test]
fn closed_stdout_pipe_exits_quietly_with_141() {
    // La verruga de M108 (DESIGN §99): `programa | head` reventaba con un pánico de Rust
    // disfrazado de ICE ("failed printing to stdout: Broken pipe") — Rust ignora SIGPIPE y
    // `println!` paniquea. Ahora sigue la convención Unix: exit 141 (128+SIGPIPE) EN SILENCIO,
    // en los tres motores (el nativo, vía su hilo escritor).
    let src = "fn main() -> int {\n    var i = 0;\n    while (i < 200000) {\n        print(\"line \" + to_string(i));\n        i = i + 1;\n    }\n    0\n}\n";
    let base = tmp("epipe");
    std::fs::write(base.join("prog.ray"), src).unwrap();

    let mut cmds: Vec<(String, Command)> = Vec::new();
    for engine in ["--vm", "--interp"] {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ray"));
        c.args([engine, "prog.ray"]).current_dir(&base);
        cmds.push((engine.to_string(), c));
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let (_o, berr, bcode) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(bcode, 0, "build --native ok\n{berr}");
        cmds.push(("nativo".into(), Command::new(&bin)));
    }
    for (label, mut cmd) in cmds {
        use std::io::Read;
        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("lanza");
        // Lee un poco y CIERRA el extremo de lectura (el `head` del caso real).
        let mut out = child.stdout.take().unwrap();
        let mut first = [0u8; 64];
        let _ = out.read(&mut first);
        drop(out);
        let status = child.wait().expect("espera");
        let mut err = String::new();
        child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
        assert_eq!(status.code(), Some(141), "{label}: convención Unix (128+SIGPIPE)\n{err}");
        assert!(!err.contains("panicked") && !err.contains("ICE"), "{label}: sin ruido en stderr: {err}");
    }
}

#[test]
fn cli_output_to_a_closed_pipe_also_exits_quietly_with_141() {
    // El residuo de #126: aquel fix cubrió la salida del PROGRAMA (print/eprint), pero la del
    // propio CLI (`ray fmt x | head`, `ray doc | less`) seguía paniqueando con el ICE de
    // "failed printing to stdout: Broken pipe". Ahora la red central de ICEs distingue ese
    // pánico de la libstd y aplica la misma convención Unix: exit 141 en silencio. (La salida
    // debe superar el buffer del pipe —64 KiB— o el EPIPE nunca ocurre.)
    use std::io::Read;
    let base = tmp("cli_epipe");
    let mut src = String::from("fn main() -> int {\n");
    for i in 0..6000 {
        src.push_str(&format!("    let variable_number_{i} = {i};\n"));
    }
    src.push_str("    0\n}\n");
    std::fs::write(base.join("big.ray"), src).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["fmt", "big.ray"])
        .current_dir(&base)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("lanza ray fmt");
    let mut out = child.stdout.take().unwrap();
    let mut first = [0u8; 128];
    let _ = out.read(&mut first);
    drop(out); // ← el `head` del caso real: cierra el extremo de lectura
    let status = child.wait().unwrap();
    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert_eq!(status.code(), Some(141), "convención Unix\n{err}");
    assert!(
        !err.contains("panicked") && !err.contains("ICE"),
        "sin traza de pánico ni banner de ICE: {err}"
    );
}
