//! Pruebas de la I/O interactiva (M11.2a) sobre el binario: escriben un `.ray` temporal,
//! ejecutan `raylang [--vm] <archivo>` alimentando stdin, y comprueban stdout/stderr/código.
//! La entrada/lectura no es determinista para el oráculo (depende de stdin), así que se prueba
//! aquí por subproceso, como el REPL.

use std::io::Write;
use std::process::{Command, Stdio};

/// Escribe `src` en un `.ray` temporal, ejecuta el binario (con `--vm` opcional) alimentando
/// `stdin`, y devuelve `(stdout, stderr, código)`.
fn run_io(name: &str, src: &str, stdin: &str, vm: bool) -> (String, String, i32) {
    let mut path = std::env::temp_dir();
    path.push(format!("{name}.ray"));
    std::fs::File::create(&path).expect("crea").write_all(src.as_bytes()).expect("escribe");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    if vm {
        cmd.arg("--vm");
    }
    let mut child = cmd
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanza raylang");
    child.stdin.take().expect("stdin").write_all(stdin.as_bytes()).expect("escribe stdin");
    let out = child.wait_with_output().expect("espera al proceso");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

const PROG: &str = r#"
fn saludo(o: Option<string>) -> string {
  match (o) {
    Option.Some(s) => "hola, " + s,
    Option.None => "EOF",
  }
}
fn main() -> int {
  print(saludo(input()));
  eprint("aviso a stderr");
  match (read_int()) {
    Option.Some(n) => n,
    Option.None => -1,
  }
}
"#;

#[test]
fn input_y_read_int_leen_de_stdin_en_ambos_motores() {
    for vm in [false, true] {
        let (out, err, code) = run_io("ray_io_ok", PROG, "mundo\n7\n", vm);
        assert!(out.contains("hola, mundo"), "input() leyó la primera línea (vm={vm})\n{out}");
        assert!(err.contains("aviso a stderr"), "eprint fue a stderr (vm={vm})\n{err}");
        assert_eq!(code, 7, "read_int() leyó 7 (vm={vm})");
    }
}

#[test]
fn eof_inmediato_da_none() {
    // Sin entrada: input() -> None ("EOF"); read_int() -> None -> -1 (& 0xFF = 255).
    let (out, _, code) = run_io("ray_io_eof", PROG, "", false);
    assert!(out.contains("EOF"), "input() en EOF es None\n{out}");
    assert_eq!(code, 255, "read_int() en EOF es None -> -1");
}

#[test]
fn read_int_con_texto_no_entero_da_none() {
    // Primera línea cualquiera; segunda no es entero -> read_int None -> -1.
    let (out, _, code) = run_io("ray_io_noint", PROG, "x\nabc\n", false);
    assert!(out.contains("hola, x"), "input() leyó 'x'\n{out}");
    assert_eq!(code, 255, "read_int() de 'abc' es None -> -1");
}
