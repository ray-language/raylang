//! M175 (docs/windows.md W6) — `std/process` en Windows. El gemelo de `process_cli.rs` (que es
//! `#![cfg(unix)]`: usa `sh`/`cat`), con los comandos de Windows: `cmd /c` para el contrato de
//! `run` (exit≠0 es Ok, stdin, ENOENT como `Err`, timeout con Output PARCIAL, merge, tope con
//! `truncated`) y el propio `ray` como hijo interactivo para la sesión con stdin abierto (los
//! filtros de Windows —`findstr`, `sort`— bufferizan su salida bajo un pipe y no sirven de `cat`
//! línea a línea; `print` de raylang sí escribe al momento). VM, intérprete y binario nativo.
#![cfg(windows)]

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

fn tmp(name: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("ray_process_win_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

/// `ray <args>` en `dir`, con plazo: un cuelgue mata al hijo y falla con lo impreso hasta ahí.
fn ray(dir: &std::path::Path, args: &[&str], secs: u64) -> (String, String, Option<i32>) {
    let mut child = Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("lanza ray");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        if child.try_wait().expect("try_wait").is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let out = child.wait_with_output().expect("espera");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code(),
    )
}

/// El contrato de `run` con `cmd /c`. Las salidas se normalizan en el propio programa (CRLF →
/// LF, trim) para que la esperada sea la misma en los tres motores.
const RUN_SRC: &str = r#"import std/process;

fn show_exit(e: process.Exit) -> string {
    match (e) {
        process.Exit.Code(c) => "code:" + c.to_string(),
        process.Exit.Signal(s) => "signal:" + s.to_string(),
    }
}

fn text(b: bytes) -> string {
    match (from_utf8(b)) {
        Result.Ok(s) => s.replace("\r", "").trim(),
        Result.Err(_) => "binario",
    }
}

fn show(label: string, r: Result<process.Output, string>) {
    match (r) {
        Result.Ok(o) => {
            var line = label + " " + show_exit(o.exit);
            if (o.timed_out) { line = line + " timed_out"; }
            if (o.truncated) { line = line + " truncated"; }
            print(line + " [" + text(o.stdout) + "]");
        },
        Result.Err(_) => print(label + " no se pudo lanzar"),
    }
}

fn main() -> int {
    show("echo", process.run("cmd", ["/c", "echo hi&exit 3"]));
    show("stdin", process.cmd("findstr", ["/r", "."]).stdin("b\na\n".to_bytes()).run());
    show("missing", process.run("definitely-missing-program-ray", []));
    // Plazo: el hijo sobrevive al plazo y muere por la escalera; el Output vuelve PARCIAL.
    match (process.cmd("cmd", ["/c", "echo partial&ping -n 30 127.0.0.1 >nul"]).timeout_ms(400).run()) {
        Result.Ok(o) => print("timeout timed_out:" + to_string(o.timed_out) + " [" + text(o.stdout) + "]"),
        Result.Err(_) => print("timeout no se pudo lanzar"),
    }
    show("merge", process.cmd("cmd", ["/c", "echo out&echo err 1>&2"]).merge_output().run());
    show("cap", process.cmd("cmd", ["/c", "echo yyyyyyyyyyyyyyyyyyyy"]).max_output(5).run());
    show("env", process.cmd("cmd", ["/c", "echo %RAY_TEST_VAR%"]).env("RAY_TEST_VAR", "v1").run());
    0
}
"#;

const RUN_EXPECTED: &str = "echo code:3 [hi]\n\
    stdin code:0 [b\na]\n\
    missing no se pudo lanzar\n\
    timeout timed_out:true [partial]\n\
    merge code:0 [out\nerr]\n\
    cap code:0 truncated [yyyyy]\n\
    env code:0 [v1]\n";

#[test]
fn run_contract_holds_on_windows_on_all_engines() {
    let base = tmp("run");
    std::fs::write(base.join("prog.ray"), RUN_SRC).unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, err, code) = ray(&base, &[engine, "prog.ray"], 60);
        assert_eq!(code, Some(0), "{engine}: exit 0\nstdout={out}\nstderr={err}");
        assert_eq!(out, RUN_EXPECTED, "{engine}: contrato de run");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join("prog_bin.exe");
        let (_o, berr, bcode) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()], 600);
        assert_eq!(bcode, Some(0), "build --native ok (M169 ya no rechaza std/process en Windows)\n{berr}");
        let native = Command::new(&bin).current_dir(&base).stdin(std::process::Stdio::null()).output().expect("nativo");
        assert_eq!(native.status.code(), Some(0), "nativo exit 0\n{}", String::from_utf8_lossy(&native.stderr));
        assert_eq!(String::from_utf8_lossy(&native.stdout), RUN_EXPECTED, "nativo ≡ VM");
    }
}

/// El hijo interactivo: un `cat` en raylang (lee stdin por trozos y los imprime al momento).
const ECHO_CHILD: &str = r#"import std/io;

fn main() -> int {
    var go = true;
    while (go) {
        match (io.read(64)) {
            Option.Some(b) => {
                match (from_utf8(b)) {
                    Result.Ok(s) => print(s.trim()),
                    Result.Err(_) => print("?"),
                }
            },
            Option.None => { go = false; },
        }
    }
    0
}
"#;

/// La sesión con stdin abierto (M100 v3), como en unix: tres peticiones, cierre de stdin, `wait`
/// y una escritura tras la muerte del hijo que debe ser `Err`. `RAY` se sustituye por la ruta del
/// binario (con `/`: dentro de un literal raylang las barras invertidas serían escapes).
const SESSION_SRC: &str = r#"import std/process;

fn main() -> int {
    match (process.cmd("RAY", ["run", "echo_child.ray"]).stdin_pipe().stream()) {
        Result.Err(e) => { print("spawn: " + e); 1 },
        Result.Ok(p) => {
            var i = 1;
            while (i <= 3) {
                match (p.write("req ${i}\n".to_bytes())) {
                    Result.Err(e) => { print("write: " + e); },
                    Result.Ok(n) => { print("-> ${n}"); },
                }
                match (recv(p.out)) {
                    Option.Some(chunk) => { print("<- " + from_utf8(chunk).unwrap_or("?").trim()); },
                    Option.None => { print("<- (closed)"); },
                }
                i = i + 1;
            }
            p.close_stdin();
            match (p.wait()) {
                process.Exit.Code(c) => { print("exit ${c}"); },
                process.Exit.Signal(s) => { print("signal ${s}"); },
            }
            match (p.write(b"tarde")) {
                Result.Err(_) => print("write-after-exit: err"),
                Result.Ok(_) => print("write-after-exit: OK (INESPERADO)"),
            }
            0
        },
    }
}
"#;

const SESSION_EXPECTED: &str = "-> 6\n<- req 1\n-> 6\n<- req 2\n-> 6\n<- req 3\nexit 0\nwrite-after-exit: err\n";

#[test]
fn writable_stdin_keeps_a_session_alive_on_windows() {
    let base = tmp("session");
    std::fs::write(base.join("echo_child.ray"), ECHO_CHILD).unwrap();
    let ray_path = BIN.replace('\\', "/");
    std::fs::write(base.join("session.ray"), SESSION_SRC.replace("RAY", &ray_path)).unwrap();

    let (out, err, code) = ray(&base, &["run", "session.ray"], 60);
    assert_eq!(code, Some(0), "VM: exit 0\nstdout={out}\nstderr={err}");
    assert_eq!(out, SESSION_EXPECTED, "VM: sesión");

    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join("session_bin.exe");
        let (_o, berr, bcode) = ray(&base, &["build", "session.ray", "--native", "-o", bin.to_str().unwrap()], 600);
        assert_eq!(bcode, Some(0), "build --native ok\n{berr}");
        let native = Command::new(&bin).current_dir(&base).stdin(std::process::Stdio::null()).output().expect("nativo");
        assert_eq!(native.status.code(), Some(0), "nativo exit 0\n{}", String::from_utf8_lossy(&native.stderr));
        assert_eq!(String::from_utf8_lossy(&native.stdout), SESSION_EXPECTED, "nativo ≡ VM");
    }
}
