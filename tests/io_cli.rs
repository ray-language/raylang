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

const ARGS_ENV_PROG: &str = r#"
fn main() -> int {
  let xs = args();
  var i = 0;
  while (i < len(xs)) { print(xs[i]); i = i + 1; }
  match (env("RAY_TEST_VAR")) {
    Option.Some(v) => print("env=" + v),
    Option.None => print("env=?"),
  }
  len(xs)
}
"#;

#[test]
fn args_y_env_en_ambos_motores() {
    let mut path = std::env::temp_dir();
    path.push("ray_args_env.ray");
    std::fs::File::create(&path).unwrap().write_all(ARGS_ENV_PROG.as_bytes()).unwrap();

    for vm in [false, true] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
        if vm {
            cmd.arg("--vm");
        }
        // raylang [--vm] <archivo> uno dos   con RAY_TEST_VAR=hola en el entorno.
        let out = cmd
            .arg(&path)
            .arg("uno")
            .arg("dos")
            .env("RAY_TEST_VAR", "hola")
            .output()
            .expect("ejecuta raylang");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("uno") && stdout.contains("dos"), "args() ve los CLI args (vm={vm})\n{stdout}");
        assert!(stdout.contains("env=hola"), "env() lee la variable (vm={vm})\n{stdout}");
        assert_eq!(out.status.code(), Some(2), "len(args) = 2 (vm={vm})");
    }
}

const FILE_PROG: &str = r#"
fn cuerpo(r: Result<string, string>) -> string {
  match (r) {
    Result.Ok(s) => s,
    Result.Err(e) => "ERR:" + e,
  }
}
fn main() -> int {
  let ruta = args()[0];
  match (write_file(ruta, "hola\nmundo")) {
    Result.Ok(n) => print("escritos:" + n.to_string()),
    Result.Err(e) => print("err:" + e),
  }
  print(cuerpo(read_file(ruta)));
  match (read_file(ruta + ".noexiste")) {
    Result.Ok(_) => print("inesperado"),
    Result.Err(_) => print("err-al-leer-inexistente"),
  }
  0
}
"#;

#[test]
fn read_write_file_ida_y_vuelta_en_ambos_motores() {
    let mut prog = std::env::temp_dir();
    prog.push("ray_file_prog.ray");
    std::fs::File::create(&prog).unwrap().write_all(FILE_PROG.as_bytes()).unwrap();

    for (i, vm) in [false, true].into_iter().enumerate() {
        let mut datos = std::env::temp_dir();
        datos.push(format!("ray_file_datos_{i}.txt"));
        let _ = std::fs::remove_file(&datos);

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
        if vm {
            cmd.arg("--vm");
        }
        let out = cmd.arg(&prog).arg(&datos).output().expect("ejecuta raylang");
        let stdout = String::from_utf8_lossy(&out.stdout);

        assert!(stdout.contains("escritos:10"), "write_file devuelve nº de caracteres (vm={vm})\n{stdout}");
        assert!(stdout.contains("hola\nmundo"), "read_file recupera el contenido escrito (vm={vm})\n{stdout}");
        assert!(stdout.contains("err-al-leer-inexistente"), "leer inexistente es Err (vm={vm})\n{stdout}");
        // Y el archivo existe de verdad en disco con el contenido correcto.
        assert_eq!(std::fs::read_to_string(&datos).unwrap(), "hola\nmundo", "el archivo en disco (vm={vm})");
    }
}

#[test]
fn env_no_definida_da_none() {
    let mut path = std::env::temp_dir();
    path.push("ray_args_env2.ray");
    std::fs::File::create(&path).unwrap().write_all(ARGS_ENV_PROG.as_bytes()).unwrap();
    // Sin args ni la variable: args() = [] y env() = None.
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(&path)
        .env_remove("RAY_TEST_VAR")
        .output()
        .expect("ejecuta raylang");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("env=?"), "env() de una variable ausente es None\n{stdout}");
    assert_eq!(out.status.code(), Some(0), "len(args) = 0 sin argumentos");
}
