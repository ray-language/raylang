//! M88.1 — `signals() -> Channel<int>`: el canal de señales del SO para el apagado
//! ordenado de servicios (SIGTERM/SIGINT). Solo VM (como toda la concurrencia); el
//! self-pipe del handler entra al poller del scheduler, y las fibras aparcadas en el
//! canal NO cuentan como deadlock (esperan al exterior). Unix solamente.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// La fontanería host en aislamiento: handler + self-pipe + bandera.
#[test]
fn signal_plumbing() {
    let fd = raylang::builtins::signals_install().expect("install");
    unsafe { libc_kill(std::process::id() as i32, 15) };
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(raylang::builtins::signals_pending(), "bandera pendiente");
    assert_eq!(raylang::builtins::signals_read_one(fd), Some(15), "la señal viaja por el pipe");
    assert_eq!(raylang::builtins::signals_read_one(fd), None, "el pipe queda drenado (no bloquea)");
}
unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

/// Lanza `src` con --vm, espera la línea "listo" (el handler ya instalado), manda la
/// señal y devuelve (stdout completo, exit code).
fn run_and_signal(src: &str, name: &str, signal: &str) -> (String, i32) {
    let mut path = std::env::temp_dir();
    path.push(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(src.as_bytes()).unwrap();
    drop(f);
    let mut child = Command::new(BIN)
        .arg("--vm")
        .arg(&path)
        .stdout(Stdio::piped())
        .spawn()
        .expect("lanza el binary");
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).expect("lee 'listo'");
    assert_eq!(line.trim(), "listo", "el program anuncia el handler instalado");
    let st = Command::new("kill").arg(signal).arg(child.id().to_string()).status().unwrap();
    assert!(st.success(), "kill {signal}");
    let mut rest = String::new();
    let mut r = reader;
    std::io::Read::read_to_string(&mut r, &mut rest).unwrap();
    let code = child.wait().unwrap().code().unwrap_or(-1);
    (format!("{line}{rest}"), code)
}

const PROG: &str = "fn main() -> int {\n    let sig = signals();\n    print(\"listo\");\n    match (recv(sig)) {\n        Option.Some(n) => print(\"señal \" + to_string(n)),\n        Option.None => print(\"closed\"),\n    }\n    0\n}\n";

/// SIGTERM llega como 15 por el canal; el programa drena y sale limpio (exit 0).
#[test]
fn sigterm_reaches_the_channel_and_shutdown_is_orderly() {
    let (out, code) = run_and_signal(PROG, "sig_term.ray", "-TERM");
    assert_eq!(code, 0, "output limpia\n{out}");
    assert!(out.contains("señal 15"), "{out}");
}

/// SIGINT llega como 2.
#[test]
fn sigint_arrives_as_2() {
    let (out, code) = run_and_signal(PROG, "sig_int.ray", "-INT");
    assert_eq!(code, 0, "output limpia\n{out}");
    assert!(out.contains("señal 2"), "{out}");
}

/// La composición NATIVA con select: el servicio drena su canal de trabajo O apaga —
/// el patrón microservicio. La señal despierta el select aunque haya trabajo en vuelo.
#[test]
fn select_composes_work_and_shutdown() {
    let src = "fn main() -> int {\n\
        let trabajo: Channel<int> = Channel.new();\n\
        let sig = signals();\n\
        spawn(fn() {\n\
            var i = 0;\n\
            while (i < 3) { send(trabajo, i); i = i + 1; }\n\
        });\n\
        var canales: [Channel<int>] = [trabajo, sig];\n\
        print(\"listo\");\n\
        var live = true;\n\
        while (live) {\n\
            let idx = select(canales);\n\
            if (idx == 0) {\n\
                match (recv(trabajo)) {\n\
                    Option.Some(x) => print(\"trabajo \" + to_string(x)),\n\
                    Option.None => { },\n\
                }\n\
            } else {\n\
                match (recv(sig)) {\n\
                    Option.Some(n) => { print(\"apagando por \" + to_string(n)); live = false; },\n\
                    Option.None => { live = false; },\n\
                }\n\
            }\n\
        }\n\
        0\n\
    }\n";
    let (out, code) = run_and_signal(src, "sig_select.ray", "-TERM");
    assert_eq!(code, 0, "output limpia\n{out}");
    assert!(out.contains("apagando por 15"), "{out}");
}

/// M107.4: SIGWINCH (cambio de tamaño del terminal) llega como 28 — con `select` sobre
/// `signals()` + `term.size()`, una TUI se re-maqueta al redimensionar.
#[test]
fn sigwinch_arrives_as_28() {
    let (out, code) = run_and_signal(PROG, "sig_winch.ray", "-WINCH");
    assert_eq!(code, 0, "output limpia\n{out}");
    assert!(out.contains("señal 28"), "{out}");
}

/// M107.4: el binario NATIVO también entrega SIGWINCH (la doc decía "VM only" y estaba rancia:
/// `__ray_signals` existe desde M88.1 — aquí queda asegurado con señal real).
#[test]
fn sigwinch_reaches_the_native_binary_too() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando sigwinch nativo: rustc no disponible");
        return;
    }
    let dir = std::env::temp_dir().join("ray_sigwinch_native");
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(dir.join("prog.ray"), PROG).unwrap();
    let bin = dir.join("prog_bin");
    let st = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
        .current_dir(&dir)
        .output()
        .expect("build");
    assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
    let mut child = Command::new(&bin).stdout(Stdio::piped()).spawn().expect("corre");
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).expect("lee 'listo'");
    assert_eq!(line.trim(), "listo");
    let st = Command::new("kill").arg("-WINCH").arg(child.id().to_string()).status().unwrap();
    assert!(st.success());
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut reader, &mut rest).unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert!(rest.contains("señal 28"), "nativo: {rest}");
}
