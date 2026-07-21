//! Oráculo de la **META-CIRCULARIDAD por la VM** (M14.5f, self-hosting).
//!
//! Gemelo de `selfhost_metacircular.rs` (que usa el intérprete auto-alojado, `run.ray`), pero ejercitando
//! el SEGUNDO back-end: la VM auto-alojada (`run_vm.ray`, compilador `compiler.ray` + máquina de pila
//! `vm.ray`). El compilador auto-alojado (lexer/parser/checker, escritos en raylang) corre **sobre la VM
//! auto-alojada** vía `run_vm.ray`, y debe producir el mismo COMPORTAMIENTO que cuando lo corre Rust:
//!
//!   Rust:           raylang <driver> <input>
//!   sobre la VM:    raylang selfhost/run_vm.ray <driver> <input>   (el driver se COMPILA y corre en la VM)
//!
//! Que coincidan demuestra que raylang lexea/parsea/chequea/EJECUTA raylang con el compilador+VM de raylang
//! corriendo sobre la VM de raylang. El último caso (`run_vm.ray` corriendo `run_vm.ray`) es **run-on-run**:
//! la VM auto-alojada ejecutándose a sí misma (la VM compilando y corriendo la VM que compila y corre el
//! programa). Ver DESIGN §23.6.

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

/// Compara un driver del self-hosting corrido por Rust vs COMPILADO Y CORRIDO sobre la VM auto-alojada.
fn compare_driver_vm(driver_rel: &str, input_abs: &str, label: &str) {
    let driver = repo_path(driver_rel);
    let driver = driver.to_str().expect("path utf-8");
    let run_vm = repo_path("selfhost/run_vm.ray");
    let run_vm = run_vm.to_str().expect("path utf-8");
    let (so_r, code_r) = run(&[driver, input_abs]);
    let (so_v, code_v) = run(&[run_vm, driver, input_abs]);
    assert_eq!(so_v, so_r, "stdout difiere ({label})");
    assert_eq!(code_v, code_r, "código de output difiere ({label})");
}

// Fuentes pequeñas (la VM auto-alojada corre sobre la VM del host → entradas chicas).
const FIB: &str = "fn fib(n: int) -> int { if (n < 2) { n } else { fib(n - 1) + fib(n - 2) } }\n\
                   fn main() -> int { var i = 0; while (i <= 6) { print(fib(i)); i = i + 1; } 0 }\n";
const DATOS: &str = "struct P { x: int, y: int }\nenum F { C(int), R(int, int) }\n\
                     fn area(f: F) -> int { match (f) { F.C(r) => 3 * r * r, F.R(a, b) => a * b } }\n\
                     fn main() -> int { let p = P { x: 3, y: 4 }; print(p.x + p.y); print(area(F.C(2))); print(area(F.R(3, 5))); 0 }\n";

#[test]
fn lexer_metacircular_vm() {
    // El lexer auto-alojado, COMPILADO y corrido sobre la VM auto-alojada, lexea idéntico a Rust.
    let inp = temp_input("mcvm_lex_fib.ray", FIB);
    compare_driver_vm("selfhost/lex_dump.ray", &inp, "lex_dump/fib");
    let inp = temp_input("mcvm_lex_data.ray", DATOS);
    compare_driver_vm("selfhost/lex_dump.ray", &inp, "lex_dump/data");
}

#[test]
fn parser_metacircular_vm() {
    // El parser auto-alojado, sobre la VM auto-alojada, produce el mismo AST que Rust.
    let inp = temp_input("mcvm_parse_fib.ray", FIB);
    compare_driver_vm("selfhost/parse_dump.ray", &inp, "parse_dump/fib");
    let inp = temp_input("mcvm_parse_data.ray", DATOS);
    compare_driver_vm("selfhost/parse_dump.ray", &inp, "parse_dump/data");
}

#[test]
fn checker_metacircular_vm() {
    // El checker auto-alojado, sobre la VM auto-alojada, da el mismo veredicto que Rust.
    let inp = temp_input("mcvm_check_fib.ray", FIB);
    compare_driver_vm("selfhost/check_dump.ray", &inp, "check_dump/fib");
    let inp = temp_input("mcvm_check_data.ray", DATOS);
    compare_driver_vm("selfhost/check_dump.ray", &inp, "check_dump/data");
    // Un programa con error de tipos: el veredicto (mensaje) también debe coincidir.
    let inp = temp_input("mcvm_check_err.ray", "fn main() -> int { let x: int = true; 0 }\n");
    compare_driver_vm("selfhost/check_dump.ray", &inp, "check_dump/error");
}

// run-on-run de la VM corre TRES niveles (Rust → VM auto-alojada compilando+corriendo run_vm.ray → esa VM
// compilando+corriendo el programa), así que es muy lento (~varios min en debug) → `#[ignore]`. Ejecútalo:
//   cargo test --test selfhost_metacircular_vm -- --ignored
#[test]
#[ignore]
fn run_on_run_metacircular_vm() {
    // run-on-run: `run_vm.ray` compilando y corriendo `run_vm.ray` sobre la VM auto-alojada ejecuta el
    // programa con el mismo comportamiento que Rust → la VM auto-alojada ejecutándose a sí misma.
    let inp = temp_input("mcvm_ror_fib.ray", "fn fib(n: int) -> int { if (n < 2) { n } else { fib(n - 1) + fib(n - 2) } }\n\
                                              fn main() -> int { var i = 0; while (i <= 4) { print(fib(i)); i = i + 1; } 0 }\n");
    compare_driver_vm("selfhost/run_vm.ray", &inp, "run-on-run-vm/fib");
}
