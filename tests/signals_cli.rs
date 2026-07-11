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
fn fontaneria_de_senales() {
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
fn correr_y_senalar(src: &str, nombre: &str, signal: &str) -> (String, i32) {
    let mut path = std::env::temp_dir();
    path.push(nombre);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(src.as_bytes()).unwrap();
    drop(f);
    let mut child = Command::new(BIN)
        .arg("--vm")
        .arg(&path)
        .stdout(Stdio::piped())
        .spawn()
        .expect("lanza el binario");
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut linea = String::new();
    reader.read_line(&mut linea).expect("lee 'listo'");
    assert_eq!(linea.trim(), "listo", "el programa anuncia el handler instalado");
    let st = Command::new("kill").arg(signal).arg(child.id().to_string()).status().unwrap();
    assert!(st.success(), "kill {signal}");
    let mut resto = String::new();
    let mut r = reader;
    std::io::Read::read_to_string(&mut r, &mut resto).unwrap();
    let code = child.wait().unwrap().code().unwrap_or(-1);
    (format!("{linea}{resto}"), code)
}

const PROG: &str = "fn main() -> int {\n    let sig = signals();\n    print(\"listo\");\n    match (recv(sig)) {\n        Option.Some(n) => print(\"señal \" + to_string(n)),\n        Option.None => print(\"cerrado\"),\n    }\n    0\n}\n";

/// SIGTERM llega como 15 por el canal; el programa drena y sale limpio (exit 0).
#[test]
fn sigterm_llega_al_canal_y_el_apagado_es_ordenado() {
    let (out, code) = correr_y_senalar(PROG, "sig_term.ray", "-TERM");
    assert_eq!(code, 0, "salida limpia\n{out}");
    assert!(out.contains("señal 15"), "{out}");
}

/// SIGINT llega como 2.
#[test]
fn sigint_llega_como_2() {
    let (out, code) = correr_y_senalar(PROG, "sig_int.ray", "-INT");
    assert_eq!(code, 0, "salida limpia\n{out}");
    assert!(out.contains("señal 2"), "{out}");
}

/// La composición NATIVA con select: el servicio drena su canal de trabajo O apaga —
/// el patrón microservicio. La señal despierta el select aunque haya trabajo en vuelo.
#[test]
fn select_compone_trabajo_y_apagado() {
    let src = "fn main() -> int {\n\
        let trabajo: Channel<int> = Channel.new();\n\
        let sig = signals();\n\
        spawn(fn() {\n\
            var i = 0;\n\
            while (i < 3) { send(trabajo, i); i = i + 1; }\n\
        });\n\
        var canales: [Channel<int>] = [trabajo, sig];\n\
        print(\"listo\");\n\
        var vivo = true;\n\
        while (vivo) {\n\
            let idx = select(canales);\n\
            if (idx == 0) {\n\
                match (recv(trabajo)) {\n\
                    Option.Some(x) => print(\"trabajo \" + to_string(x)),\n\
                    Option.None => { },\n\
                }\n\
            } else {\n\
                match (recv(sig)) {\n\
                    Option.Some(n) => { print(\"apagando por \" + to_string(n)); vivo = false; },\n\
                    Option.None => { vivo = false; },\n\
                }\n\
            }\n\
        }\n\
        0\n\
    }\n";
    let (out, code) = correr_y_senalar(src, "sig_select.ray", "-TERM");
    assert_eq!(code, 0, "salida limpia\n{out}");
    assert!(out.contains("apagando por 15"), "{out}");
}
