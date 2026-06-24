//! Oráculo del **back-end auto-alojado** (M14.4, self-hosting).
//!
//! El intérprete escrito en raylang (`selfhost/interpreter.ray`, vía `selfhost/run.ray`) debe producir
//! el mismo COMPORTAMIENTO que el runner de Rust: el mismo `stdout` (las salidas de `print`) y el mismo
//! CÓDIGO DE SALIDA (el `int` que devuelve `main`, enmascarado a 8 bits). A diferencia de lexer/parser/
//! checker (que comparaban texto canónico), aquí el oráculo es CONDUCTUAL (ver DESIGN §23.5): se ejecuta
//! la misma `.ray` por ambos pipelines y se comparan stdout + exit. `stderr` (errores) no se compara.
//!
//! Cobertura M14.4a: NÚCLEO — primitivos, aritmética/comparación/lógica, variables (let/var, ámbito,
//! mutación), if/while/block/return, llamadas a funciones nombradas + recursión, builtin `print`. El
//! corpus solo usa lo que aceptan AMBOS checkers (el de Rust y el auto-alojado: print/eprint/len/push);
//! datos (arreglos/structs/enums), closures y `to_string` llegan en M14.4b–d.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Ejecuta el runner de Rust sobre `archivo` (el ORÁCULO): devuelve (stdout, código de salida).
fn correr_rust(archivo: &str) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(archivo)
        .output()
        .expect("ejecuta el runner de Rust");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

/// Ejecuta el back-end auto-alojado (`selfhost/run.ray`) sobre `archivo`: (stdout, código de salida).
fn correr_selfhost(archivo: &str) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(repo_path("selfhost/run.ray"))
        .arg(archivo)
        .output()
        .expect("ejecuta el back-end auto-alojado");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

/// Compara los dos pipelines (Rust vs auto-alojado) sobre un archivo del repo.
fn comparar_archivo(rel: &str) {
    let abs = repo_path(rel);
    let abs = abs.to_str().expect("ruta utf-8");
    let (so_r, code_r) = correr_rust(abs);
    let (so_s, code_s) = correr_selfhost(abs);
    assert_eq!(so_s, so_r, "stdout difiere en {rel}");
    assert_eq!(code_s, code_r, "código de salida difiere en {rel}");
}

/// Compara los dos pipelines sobre una fuente concreta (escrita a un temporal).
fn comparar_fuente(src: &str, nombre_tmp: &str) {
    let mut ruta = std::env::temp_dir();
    ruta.push(nombre_tmp);
    let mut f = std::fs::File::create(&ruta).expect("crea el temporal");
    f.write_all(src.as_bytes()).expect("escribe el temporal");
    drop(f);
    let abs = ruta.to_str().expect("ruta utf-8");
    let (so_r, code_r) = correr_rust(abs);
    let (so_s, code_s) = correr_selfhost(abs);
    assert_eq!(so_s, so_r, "stdout difiere para:\n{src}");
    assert_eq!(code_s, code_r, "código de salida difiere para:\n{src}");
}

#[test]
fn corpus_nucleo() {
    // Los cuatro programas-objetivo del núcleo: recursión, while, if/else, %, return temprano.
    for rel in [
        "examples/fib.ray",
        "examples/fizzbuzz.ray",
        "examples/gcd.ray",
        "examples/primes.ray",
    ] {
        comparar_archivo(rel);
    }
}

#[test]
fn aritmetica_y_control() {
    comparar_fuente("fn main() -> int { print(2 + 3 * 4 - 1); 0 }", "in_arit.ray");
    comparar_fuente("fn main() -> int { print(17 / 5); print(17 % 5); 0 }", "in_divmod.ray");
    comparar_fuente("fn main() -> int { let x = 3.5; print(x + 2.0); print(x * 2.0); print(x - 1.0); 0 }", "in_float.ray");
    comparar_fuente("fn main() -> int { print(true && false); print(true || false); print(!false); 0 }", "in_logic.ray");
    comparar_fuente("fn main() -> int { print(1 < 2); print(2 <= 2); print(3 > 4); print('a' < 'b'); 0 }", "in_cmp.ray");
    comparar_fuente("fn main() -> int { print(\"foo\" + \"bar\"); 0 }", "in_concat.ray");
    comparar_fuente("fn main() -> int { if (1 == 1) { print(\"si\"); } else { print(\"no\"); } 0 }", "in_if.ray");
}

#[test]
fn variables_y_mutacion() {
    comparar_fuente(
        "fn main() -> int { var i = 0; var s = 0; while (i < 5) { s = s + i; i = i + 1; } print(s); 0 }",
        "in_mut.ray",
    );
    comparar_fuente(
        "fn main() -> int { let x = 10; { let x = 20; print(x); } print(x); 0 }",
        "in_shadow.ray",
    );
}

#[test]
fn llamadas_y_recursion() {
    comparar_fuente("fn doble(x: int) -> int { x * 2 } fn main() -> int { print(doble(21)); 0 }", "in_call.ray");
    comparar_fuente(
        "fn fact(n: int) -> int { if (n <= 1) { 1 } else { n * fact(n - 1) } } fn main() -> int { print(fact(6)); 0 }",
        "in_fact.ray",
    );
    // Recursión mutua.
    comparar_fuente(
        "fn par(n: int) -> bool { if (n == 0) { true } else { impar(n - 1) } } fn impar(n: int) -> bool { if (n == 0) { false } else { par(n - 1) } } fn main() -> int { print(par(10)); print(impar(7)); 0 }",
        "in_mutua.ray",
    );
}

#[test]
fn codigo_de_salida() {
    // El código de salida del runner es el int que devuelve main (0 si es unit).
    comparar_fuente("fn main() -> int { 42 }", "in_exit42.ray");
    comparar_fuente("fn main() { }", "in_unit.ray");
    // return temprano que decide el código de salida.
    comparar_fuente("fn pick(n: int) -> int { if (n > 5) { return 100; } n } fn main() -> int { print(pick(9)); pick(3) }", "in_return.ray");
}
