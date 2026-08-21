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
    let bin = base.join("prog_bin");
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
