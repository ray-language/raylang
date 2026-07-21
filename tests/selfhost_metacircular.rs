//! Oráculo de la **META-CIRCULARIDAD** (M14.7c, self-hosting).
//!
//! El compilador auto-alojado (lexer/parser/checker/intérprete, escritos en raylang) corre **sobre el
//! intérprete auto-alojado** vía `selfhost/run.ray`, y debe producir el mismo COMPORTAMIENTO que cuando
//! lo corre Rust directamente. Para cada driver del self-hosting (`lex_dump`/`parse_dump`/`check_dump`) y
//! para `run.ray` mismo, se compara stdout + código de salida de ambos caminos sobre la misma entrada:
//!
//!   Rust:           raylang <driver> <input>
//!   auto-alojado:   raylang selfhost/run.ray <driver> <input>   (el driver corre SOBRE el intérprete)
//!
//! Que coincidan demuestra que raylang lexea/parsea/chequea/EJECUTA raylang con raylang corriendo en
//! raylang. El último caso (`run.ray` corriendo `run.ray`) es **run-on-run**: el compilador entero,
//! incluido el back-end, ejecutándose sobre sí mismo. Ver DESIGN §23.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Escribe `src` a un archivo temporal y devuelve su ruta absoluta.
fn temp_input(name: &str, src: &str) -> String {
    let mut path = std::env::temp_dir();
    path.push(name);
    let mut f = std::fs::File::create(&path).expect("crea el temporal");
    f.write_all(src.as_bytes()).expect("escribe el temporal");
    path.to_str().expect("path utf-8").to_string()
}

/// Corre `args` con el binario de raylang; devuelve (stdout, código de salida).
fn run(args: &[&str]) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(args)
        .output()
        .expect("ejecuta raylang");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

/// Compara un driver del self-hosting corrido por Rust vs corrido SOBRE el intérprete auto-alojado.
fn compare_driver(driver_rel: &str, input_abs: &str, label: &str) {
    let driver = repo_path(driver_rel);
    let driver = driver.to_str().expect("path utf-8");
    let run_path = repo_path("selfhost/run.ray");
    let run_path = run_path.to_str().expect("path utf-8");
    let (so_r, code_r) = run(&[driver, input_abs]);
    let (so_s, code_s) = run(&[run_path, driver, input_abs]);
    assert_eq!(so_s, so_r, "stdout difiere ({label})");
    assert_eq!(code_s, code_r, "código de output difiere ({label})");
}

// Fuentes pequeñas (los drivers grandes corren un intérprete sobre un intérprete → entradas chicas).
const FIB: &str = "fn fib(n: int) -> int { if (n < 2) { n } else { fib(n - 1) + fib(n - 2) } }\n\
                   fn main() -> int { var i = 0; while (i <= 6) { print(fib(i)); i = i + 1; } 0 }\n";
const DATOS: &str = "struct P { x: int, y: int }\nenum F { C(int), R(int, int) }\n\
                     fn area(f: F) -> int { match (f) { F.C(r) => 3 * r * r, F.R(a, b) => a * b } }\n\
                     fn main() -> int { let p = P { x: 3, y: 4 }; print(p.x + p.y); print(area(F.C(2))); print(area(F.R(3, 5))); 0 }\n";

#[test]
fn lexer_metacircular() {
    // El lexer auto-alojado, sobre el intérprete auto-alojado, lexea idéntico a Rust.
    let inp = temp_input("mc_lex_fib.ray", FIB);
    compare_driver("selfhost/lex_dump.ray", &inp, "lex_dump/fib");
    let inp = temp_input("mc_lex_data.ray", DATOS);
    compare_driver("selfhost/lex_dump.ray", &inp, "lex_dump/data");
}

#[test]
fn parser_metacircular() {
    // El parser auto-alojado, sobre el intérprete auto-alojado, produce el mismo AST que Rust.
    let inp = temp_input("mc_parse_fib.ray", FIB);
    compare_driver("selfhost/parse_dump.ray", &inp, "parse_dump/fib");
    let inp = temp_input("mc_parse_data.ray", DATOS);
    compare_driver("selfhost/parse_dump.ray", &inp, "parse_dump/data");
}

#[test]
fn checker_metacircular() {
    // El checker auto-alojado, sobre el intérprete auto-alojado, da el mismo veredicto que Rust.
    let inp = temp_input("mc_check_fib.ray", FIB);
    compare_driver("selfhost/check_dump.ray", &inp, "check_dump/fib");
    let inp = temp_input("mc_check_data.ray", DATOS);
    compare_driver("selfhost/check_dump.ray", &inp, "check_dump/data");
    // Un programa con error de tipos: el veredicto (mensaje) también debe coincidir.
    let inp = temp_input("mc_check_err.ray", "fn main() -> int { let x: int = true; 0 }\n");
    compare_driver("selfhost/check_dump.ray", &inp, "check_dump/error");
}

// run-on-run corre DOS niveles de tree-walking (el intérprete auto-alojado ejecutando el compilador
// auto-alojado completo), así que en debug tarda ~1 min → `#[ignore]`. Ejecútalo con:
//   cargo test --test selfhost_metacircular -- --ignored
#[test]
#[ignore]
fn run_on_run_metacircular() {
    // run-on-run: `run.ray` corriendo SOBRE el intérprete auto-alojado ejecuta el programa con el mismo
    // comportamiento que Rust ejecutándolo directamente → el compilador entero corre sobre sí mismo.
    let inp = temp_input("mc_ror_fib.ray", FIB);
    compare_driver("selfhost/run.ray", &inp, "run-on-run/fib");
    let inp = temp_input("mc_ror_data.ray", DATOS);
    compare_driver("selfhost/run.ray", &inp, "run-on-run/data");
}
