//! Oráculo de la **VM auto-alojada** (M14.5, self-hosting).
//!
//! La VM escrita en raylang (`selfhost/vm.ray`, alimentada por `selfhost/compiler.ray`, vía
//! `selfhost/run_vm.ray`) debe producir el mismo COMPORTAMIENTO que el runner de Rust: el mismo
//! `stdout` y el mismo CÓDIGO DE SALIDA. Es el oráculo conductual de M14.4 aplicado al segundo motor:
//! se ejecuta la misma `.ray` por ambos caminos y se comparan stdout + exit. `stderr` no se compara.
//!
//! M14.5a cubre el NÚCLEO: escalares, aritmética/comparación/lógica (con cortocircuito), variables
//! locales, if/while, llamadas nombradas + builtins escalares, recursión. El corpus evita datos
//! (arreglos/structs/enums), closures y el prelude (map/filter/fold), que llegan en M14.5b–d.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn correr_rust(archivo: &str) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(archivo)
        .output()
        .expect("ejecuta el runner de Rust");
    (String::from_utf8_lossy(&out.stdout).to_string(), out.status.code().unwrap_or(-1))
}

fn correr_vm(archivo: &str) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(repo_path("selfhost/run_vm.ray"))
        .arg(archivo)
        .output()
        .expect("ejecuta la VM auto-alojada");
    (String::from_utf8_lossy(&out.stdout).to_string(), out.status.code().unwrap_or(-1))
}

fn comparar_archivo(rel: &str) {
    let abs = repo_path(rel);
    let abs = abs.to_str().expect("ruta utf-8");
    let (so_r, code_r) = correr_rust(abs);
    let (so_v, code_v) = correr_vm(abs);
    assert_eq!(so_v, so_r, "stdout difiere en {rel}");
    assert_eq!(code_v, code_r, "código de salida difiere en {rel}");
}

fn comparar_fuente(src: &str, nombre_tmp: &str) {
    let mut ruta = std::env::temp_dir();
    ruta.push(nombre_tmp);
    let mut f = std::fs::File::create(&ruta).expect("crea el temporal");
    f.write_all(src.as_bytes()).expect("escribe el temporal");
    drop(f);
    let abs = ruta.to_str().expect("ruta utf-8");
    let (so_r, code_r) = correr_rust(abs);
    let (so_v, code_v) = correr_vm(abs);
    assert_eq!(so_v, so_r, "stdout difiere para:\n{src}");
    assert_eq!(code_v, code_r, "código de salida difiere para:\n{src}");
}

#[test]
fn corpus_nucleo() {
    // Los cuatro programas-objetivo del núcleo, por la VM auto-alojada.
    for rel in ["examples/fib.ray", "examples/fizzbuzz.ray", "examples/gcd.ray", "examples/primes.ray"] {
        comparar_archivo(rel);
    }
}

#[test]
fn aritmetica_y_control() {
    comparar_fuente(
        "fn main() -> int { print(2 + 3 * 4); print(17 % 5); print(10 / 3); print(0 - 7); print(2.5 + 1.0); 0 }",
        "vm_arit.ray",
    );
    // if/else como expresión + comparación.
    comparar_fuente(
        "fn signo(n: int) -> int { if (n > 0) { 1 } else { if (n < 0) { 0 - 1 } else { 0 } } } \
         fn main() -> int { print(signo(5)); print(signo(0 - 3)); print(signo(0)); 0 }",
        "vm_if.ray",
    );
    // Cortocircuito de && y ||.
    comparar_fuente(
        "fn main() -> int { print(true && false); print(false || true); print(1 < 2 && 2 < 3); 0 }",
        "vm_corto.ray",
    );
}

#[test]
fn variables_y_recursion() {
    // var, mutación, while.
    comparar_fuente(
        "fn main() -> int { var s = 0; var i = 1; while (i <= 100) { s = s + i; i = i + 1; } print(s); 0 }",
        "vm_suma.ray",
    );
    // Recursión mutua.
    comparar_fuente(
        "fn par(n: int) -> bool { if (n == 0) { true } else { impar(n - 1) } } \
         fn impar(n: int) -> bool { if (n == 0) { false } else { par(n - 1) } } \
         fn main() -> int { print(par(10)); print(impar(7)); 0 }",
        "vm_mutua.ray",
    );
    // Shadowing en bloques anidados.
    comparar_fuente(
        "fn main() -> int { let x = 1; { let x = 2; print(x); } print(x); 0 }",
        "vm_shadow.ray",
    );
}

#[test]
fn builtins_escalares() {
    // print/eprint/to_string + concatenación de string.
    comparar_fuente(
        "fn main() -> int { print(to_string(42) + \"!\"); eprint(\"a stderr\"); print(\"hola\"); 0 }",
        "vm_builtins.ray",
    );
}

#[test]
fn codigo_de_salida() {
    comparar_fuente("fn main() -> int { 42 }", "vm_exit42.ray");
    comparar_fuente("fn main() { }", "vm_unit.ray");
    comparar_fuente("fn pick(n: int) -> int { if (n > 5) { return 100; } n } fn main() -> int { print(pick(9)); pick(3) }", "vm_return.ray");
}

// ---------------------------------------------------------------------
// M14.5b — datos: arreglos, structs, enums, match. Mismo corpus que el
// oráculo del intérprete auto-alojado (M14.4b), ahora por la VM.
// ---------------------------------------------------------------------

#[test]
fn corpus_datos() {
    for rel in [
        "examples/structs.ray",
        "examples/enums.ray",
        "examples/match_figuras.ray",
        "examples/arrays.ray",
        "examples/matriz.ray",
    ] {
        comparar_archivo(rel);
    }
}

#[test]
fn structs_snippets() {
    comparar_fuente(
        "struct P { x: int, y: int } fn main() -> int { let p = P { x: 3, y: 4 }; print(p); print(p.x + p.y); 0 }",
        "vm_struct.ray",
    );
    // Mutación de campo + aliasing por semántica de referencia.
    comparar_fuente(
        "struct P { x: int, y: int } fn main() -> int { var p = P { x: 1, y: 2 }; let q = p; p.x = 9; print(q.x); 0 }",
        "vm_alias.ray",
    );
    // Arreglo de structs + mutación por índice.
    comparar_fuente(
        "struct P { x: int, y: int } fn main() -> int { var ps: [P] = []; push(ps, P { x: 1, y: 2 }); ps[0].x = 99; print(ps[0]); print(len(ps)); 0 }",
        "vm_arrstruct.ray",
    );
}

#[test]
fn enums_y_match_snippets() {
    comparar_fuente(
        "enum Dir { N, S, E, O } fn dx(d: Dir) -> int { match (d) { Dir.E => 1, Dir.O => 0 - 1, _ => 0 } } fn main() -> int { print(dx(Dir.E) + dx(Dir.O)); print(dx(Dir.N)); 0 }",
        "vm_match.ray",
    );
    // Variantes con payload + binding + comodín de payload.
    comparar_fuente(
        "enum E { A(int, int), B(string), C } fn f(e: E) -> int { match (e) { E.A(x, y) => x + y, E.B(_) => 0, E.C => 0 - 1 } } fn main() -> int { print(f(E.A(3, 4))); print(f(E.B(\"z\"))); print(f(E.C)); 0 }",
        "vm_payload.ray",
    );
    // Enum recursivo (lista enlazada) + Display anidado.
    comparar_fuente(
        "enum L { Nil, Cons(int, L) } fn suma(l: L) -> int { match (l) { L.Nil => 0, L.Cons(h, t) => h + suma(t) } } fn main() -> int { let l = L.Cons(1, L.Cons(2, L.Nil)); print(suma(l)); print(l); 0 }",
        "vm_list.ray",
    );
}

#[test]
fn arreglos_snippets() {
    comparar_fuente(
        "fn main() -> int { let a = [3, 1, 4, 1, 5]; print(a); print(a[2]); print(len(a)); 0 }",
        "vm_array.ray",
    );
    // Construcción dinámica + asignación por índice.
    comparar_fuente(
        "fn main() -> int { var a: [int] = []; var i = 0; while (i < 4) { push(a, i * i); i = i + 1; } a[0] = 100; print(a); 0 }",
        "vm_arrpush.ray",
    );
    // Arreglos anidados con indexación encadenada.
    comparar_fuente(
        "fn main() -> int { let m = [[1, 2], [3, 4]]; print(m[1][0]); print(m); 0 }",
        "vm_nested.ray",
    );
    // Índice fuera de rango → error de ejecución (mismo código de salida).
    comparar_fuente(
        "fn main() -> int { let a = [1, 2, 3]; print(a[5]); 0 }",
        "vm_oob.ray",
    );
}
