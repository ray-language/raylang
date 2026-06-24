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

const APPEND_PROG: &str = r#"
fn n_de(r: Result<int, string>) -> int {
  match (r) { Result.Ok(n) => n, Result.Err(e) => 0 - 1, }
}
fn main() -> int {
  let ruta = args()[0];
  print(if (exists(ruta)) { "existe" } else { "no-existe" });   // no-existe
  let a = n_de(append_file(ruta, "uno\n"));
  let b = n_de(append_file(ruta, "dos\n"));
  print(if (exists(ruta)) { "existe" } else { "no-existe" });   // existe
  a + b
}
"#;

#[test]
fn exists_y_append_file_en_ambos_motores() {
    let mut prog = std::env::temp_dir();
    prog.push("ray_append_prog.ray");
    std::fs::File::create(&prog).unwrap().write_all(APPEND_PROG.as_bytes()).unwrap();

    for (i, vm) in [false, true].into_iter().enumerate() {
        let mut datos = std::env::temp_dir();
        datos.push(format!("ray_append_datos_{i}.txt"));
        let _ = std::fs::remove_file(&datos);

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
        if vm {
            cmd.arg("--vm");
        }
        let out = cmd.arg(&prog).arg(&datos).output().expect("ejecuta raylang");
        let stdout = String::from_utf8_lossy(&out.stdout);

        assert!(stdout.contains("no-existe"), "exists() es false antes de crear (vm={vm})\n{stdout}");
        assert!(stdout.contains("existe"), "exists() es true tras append (vm={vm})\n{stdout}");
        assert_eq!(out.status.code(), Some(8), "append_file devuelve nº de caracteres: 4+4 (vm={vm})");
        // append acumula (no sobrescribe): dos líneas en disco.
        assert_eq!(std::fs::read_to_string(&datos).unwrap(), "uno\ndos\n", "el archivo en disco (vm={vm})");
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

#[test]
fn list_dir_y_remove_file_en_ambos_motores() {
    // M11.7c: list_dir lista (ordenado) y remove_file borra. I/O real → subproceso, no oráculo.
    for vm in [false, true] {
        let mut dir = std::env::temp_dir();
        dir.push(format!("ray_listdir_{}", if vm { "vm" } else { "tw" }));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        std::fs::write(dir.join("b.txt"), "y").unwrap();
        let d = dir.to_string_lossy().replace('\\', "/");

        let src = format!(r#"
fn main() -> int {{
  match (list_dir("{d}")) {{
    Result.Ok(ns) => print(join(ns, ",")),
    Result.Err(e) => print("err: " + e),
  }}
  match (remove_file("{d}/a.txt")) {{
    Result.Ok(_) => print("borrado"),
    Result.Err(e) => print("err: " + e),
  }}
  match (list_dir("{d}")) {{
    Result.Ok(ns) => len(ns),
    Result.Err(_) => 0 - 1,
  }}
}}
"#);
        let mut path = std::env::temp_dir();
        path.push(format!("ray_listdir_prog_{}.ray", if vm { "vm" } else { "tw" }));
        std::fs::File::create(&path).unwrap().write_all(src.as_bytes()).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
        if vm { cmd.arg("--vm"); }
        let out = cmd.arg(&path).output().expect("ejecuta raylang");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("a.txt,b.txt"), "list_dir ordenado (vm={vm})\n{stdout}");
        assert!(stdout.contains("borrado"), "remove_file ok (vm={vm})\n{stdout}");
        assert_eq!(out.status.code(), Some(1), "tras borrar queda 1 entrada (vm={vm})");
    }
}

#[test]
fn handles_de_archivo_open_write_read_close() {
    // M11.8: abrir, escribir por partes, cerrar, reabrir y leer línea a línea. I/O real → subproceso.
    for vm in [false, true] {
        let mut dir = std::env::temp_dir();
        dir.push(format!("ray_handles_{}", if vm { "vm" } else { "tw" }));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("out.txt").to_string_lossy().replace('\\', "/");

        let src = format!(r#"
fn main() -> int {{
  match (open("{f}", "w")) {{
    Result.Ok(h) => {{
      write(h, "uno\n");
      write(h, "dos\n");
      close(h);
    }},
    Result.Err(e) => print("err: " + e),
  }}
  var total: int = 0;
  match (open("{f}", "r")) {{
    Result.Ok(h) => {{
      match (read_line(h)) {{ Option.Some(l) => print("L1=" + l), Option.None => print("vacio"), }}
      match (read_line(h)) {{ Option.Some(l) => print("L2=" + l), Option.None => print("vacio"), }}
      match (read_line(h)) {{ Option.Some(l) => print("hay mas"), Option.None => print("EOF"), }}
      close(h);
      total = 2;
    }},
    Result.Err(e) => print("err: " + e),
  }}
  total
}}
"#);
        let mut path = std::env::temp_dir();
        path.push(format!("ray_handles_prog_{}.ray", if vm { "vm" } else { "tw" }));
        std::fs::File::create(&path).unwrap().write_all(src.as_bytes()).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
        if vm { cmd.arg("--vm"); }
        let out = cmd.arg(&path).output().expect("ejecuta raylang");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("L1=uno"), "lee la 1ª línea (vm={vm})\n{stdout}");
        assert!(stdout.contains("L2=dos"), "lee la 2ª línea (vm={vm})\n{stdout}");
        assert!(stdout.contains("EOF"), "EOF tras la última (vm={vm})\n{stdout}");
        assert_eq!(out.status.code(), Some(2), "leídas 2 líneas (vm={vm})");
        // El archivo en disco tiene exactamente lo escrito por partes.
        assert_eq!(std::fs::read_to_string(dir.join("out.txt")).unwrap(), "uno\ndos\n");
    }
}

#[test]
fn open_de_archivo_inexistente_da_err() {
    let mut path = std::env::temp_dir();
    path.push("ray_open_err.ray");
    let src = r#"
fn main() -> int {
  match (open("/no/existe/para/nada.txt", "r")) {
    Result.Ok(h) => 0,
    Result.Err(e) => 1,
  }
}
"#;
    std::fs::File::create(&path).unwrap().write_all(src.as_bytes()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_raylang")).arg(&path).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "open de un archivo inexistente en modo r → Err");
}
