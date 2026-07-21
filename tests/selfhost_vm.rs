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

fn run_rust(file: &str) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(file)
        .output()
        .expect("ejecuta el runner de Rust");
    (String::from_utf8_lossy(&out.stdout).to_string(), out.status.code().unwrap_or(-1))
}

fn run_vm(file: &str) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(repo_path("selfhost/run_vm.ray"))
        .arg(file)
        .output()
        .expect("ejecuta la VM auto-alojada");
    (String::from_utf8_lossy(&out.stdout).to_string(), out.status.code().unwrap_or(-1))
}

fn compare_file(rel: &str) {
    let abs = repo_path(rel);
    let abs = abs.to_str().expect("path utf-8");
    let (so_r, code_r) = run_rust(abs);
    let (so_v, code_v) = run_vm(abs);
    assert_eq!(so_v, so_r, "stdout difiere en {rel}");
    assert_eq!(code_v, code_r, "código de output difiere en {rel}");
}

fn compare_source(src: &str, name_tmp: &str) {
    let mut path = std::env::temp_dir();
    path.push(name_tmp);
    let mut f = std::fs::File::create(&path).expect("crea el temporal");
    f.write_all(src.as_bytes()).expect("escribe el temporal");
    drop(f);
    let abs = path.to_str().expect("path utf-8");
    let (so_r, code_r) = run_rust(abs);
    let (so_v, code_v) = run_vm(abs);
    assert_eq!(so_v, so_r, "stdout difiere para:\n{src}");
    assert_eq!(code_v, code_r, "código de output difiere para:\n{src}");
}

#[test]
fn core_corpus() {
    // Los cuatro programas-objetivo del núcleo, por la VM auto-alojada.
    for rel in ["examples/basics/fib.ray", "examples/basics/fizzbuzz.ray", "examples/basics/gcd.ray", "examples/basics/primes.ray"] {
        compare_file(rel);
    }
}

#[test]
fn arithmetic_y_control() {
    compare_source(
        "fn main() -> int { print(2 + 3 * 4); print(17 % 5); print(10 / 3); print(0 - 7); print(2.5 + 1.0); 0 }",
        "vm_arit.ray",
    );
    // if/else como expresión + comparación.
    compare_source(
        "fn sign(n: int) -> int { if (n > 0) { 1 } else { if (n < 0) { 0 - 1 } else { 0 } } } \
         fn main() -> int { print(sign(5)); print(sign(0 - 3)); print(sign(0)); 0 }",
        "vm_if.ray",
    );
    // Cortocircuito de && y ||.
    compare_source(
        "fn main() -> int { print(true && false); print(false || true); print(1 < 2 && 2 < 3); 0 }",
        "vm_corto.ray",
    );
}

#[test]
fn variables_y_recursion() {
    // var, mutación, while.
    compare_source(
        "fn main() -> int { var s = 0; var i = 1; while (i <= 100) { s = s + i; i = i + 1; } print(s); 0 }",
        "vm_sum.ray",
    );
    // Recursión mutua.
    compare_source(
        "fn par(n: int) -> bool { if (n == 0) { true } else { impar(n - 1) } } \
         fn impar(n: int) -> bool { if (n == 0) { false } else { par(n - 1) } } \
         fn main() -> int { print(par(10)); print(impar(7)); 0 }",
        "vm_mutua.ray",
    );
    // Shadowing en bloques anidados.
    compare_source(
        "fn main() -> int { let x = 1; { let x = 2; print(x); } print(x); 0 }",
        "vm_shadow.ray",
    );
}

#[test]
fn scalar_builtins() {
    // print/eprint/to_string + concatenación de string.
    compare_source(
        "fn main() -> int { print(to_string(42) + \"!\"); eprint(\"a stderr\"); print(\"hello\"); 0 }",
        "vm_builtins.ray",
    );
}

#[test]
fn exit_code() {
    compare_source("fn main() -> int { 42 }", "vm_exit42.ray");
    compare_source("fn main() { }", "vm_unit.ray");
    compare_source("fn pick(n: int) -> int { if (n > 5) { return 100; } n } fn main() -> int { print(pick(9)); pick(3) }", "vm_return.ray");
}

// ---------------------------------------------------------------------
// M14.5b — datos: arreglos, structs, enums, match. Mismo corpus que el
// oráculo del intérprete auto-alojado (M14.4b), ahora por la VM.
// ---------------------------------------------------------------------

#[test]
fn corpus_data() {
    for rel in [
        "examples/data/structs.ray",
        "examples/data/enums.ray",
        "examples/data/match_figuras.ray",
        "examples/data/arrays.ray",
        "examples/data/matriz.ray",
    ] {
        compare_file(rel);
    }
}

#[test]
fn structs_snippets() {
    compare_source(
        "struct P { x: int, y: int } fn main() -> int { let p = P { x: 3, y: 4 }; print(p); print(p.x + p.y); 0 }",
        "vm_struct.ray",
    );
    // Mutación de campo + aliasing por semántica de referencia.
    compare_source(
        "struct P { x: int, y: int } fn main() -> int { var p = P { x: 1, y: 2 }; let q = p; p.x = 9; print(q.x); 0 }",
        "vm_alias.ray",
    );
    // Arreglo de structs + mutación por índice.
    compare_source(
        "struct P { x: int, y: int } fn main() -> int { var ps: [P] = []; ps.push(P { x: 1, y: 2 }); ps[0].x = 99; print(ps[0]); print(ps.len()); 0 }",
        "vm_arrstruct.ray",
    );
}

#[test]
fn enums_y_match_snippets() {
    compare_source(
        "enum Dir { N, S, E, O } fn dx(d: Dir) -> int { match (d) { Dir.E => 1, Dir.O => 0 - 1, _ => 0 } } fn main() -> int { print(dx(Dir.E) + dx(Dir.O)); print(dx(Dir.N)); 0 }",
        "vm_match.ray",
    );
    // Variantes con payload + binding + comodín de payload.
    compare_source(
        "enum E { A(int, int), B(string), C } fn f(e: E) -> int { match (e) { E.A(x, y) => x + y, E.B(_) => 0, E.C => 0 - 1 } } fn main() -> int { print(f(E.A(3, 4))); print(f(E.B(\"z\"))); print(f(E.C)); 0 }",
        "vm_payload.ray",
    );
    // Enum recursivo (lista enlazada) + Display anidado.
    compare_source(
        "enum L { Nil, Cons(int, L) } fn sum(l: L) -> int { match (l) { L.Nil => 0, L.Cons(h, t) => h + sum(t) } } fn main() -> int { let l = L.Cons(1, L.Cons(2, L.Nil)); print(sum(l)); print(l); 0 }",
        "vm_list.ray",
    );
}

#[test]
fn arrays_snippets() {
    compare_source(
        "fn main() -> int { let a = [3, 1, 4, 1, 5]; print(a); print(a[2]); print(a.len()); 0 }",
        "vm_array.ray",
    );
    // Construcción dinámica + asignación por índice.
    compare_source(
        "fn main() -> int { var a: [int] = []; var i = 0; while (i < 4) { a.push(i * i); i = i + 1; } a[0] = 100; print(a); 0 }",
        "vm_arrpush.ray",
    );
    // Arreglos anidados con indexación encadenada.
    compare_source(
        "fn main() -> int { let m = [[1, 2], [3, 4]]; print(m[1][0]); print(m); 0 }",
        "vm_nested.ray",
    );
    // Índice fuera de rango → error de ejecución (mismo código de salida).
    compare_source(
        "fn main() -> int { let a = [1, 2, 3]; print(a[5]); 0 }",
        "vm_oob.ray",
    );
}

// ---------------------------------------------------------------------
// M14.5c — primera clase: funciones como valor, closures (captura por
// celda), llamada indirecta y el operador `?`. Mismo corpus que el
// oráculo del intérprete auto-alojado (M14.4c), ahora por la VM.
// ---------------------------------------------------------------------

#[test]
fn corpus_first_class() {
    for rel in [
        "examples/stdlib/closures.ray",
        "examples/types/errores.ray",
        "examples/types/opcional.ray",
    ] {
        compare_file(rel);
    }
}

#[test]
fn closures_snippets() {
    // Función nombrada como valor pasada a una de orden superior definida por el usuario.
    compare_source(
        "fn double(x: int) -> int { x * 2 } fn applies(f: fn(int) -> int, x: int) -> int { f(x) } fn main() -> int { print(applies(double, 21)); 0 }",
        "vm_hof.ray",
    );
    // Closure que captura un `let` de main.
    compare_source(
        "fn main() -> int { let base = 1000; let f = fn(d: int) -> int { base + d }; print(f(7)); 0 }",
        "vm_capture.ray",
    );
    // Estado por celda compartida: instancias independientes, mutación persistente.
    compare_source(
        "fn acc(start: int) -> fn(int) -> int { var s = start; fn(d: int) -> int { s = s + d; s } } fn main() -> int { let a = acc(0); let b = acc(100); print(a(1)); print(a(2)); print(b(10)); print(a(3)); 0 }",
        "vm_state.ray",
    );
    // Captura transitiva: la closure interna captura el parámetro de la externa.
    compare_source(
        "fn adder(x: int) -> fn(int) -> int { fn(y: int) -> int { x + y } } fn main() -> int { let s = adder(10); print(s(5)); 0 }",
        "vm_transitiva.ray",
    );
}

#[test]
fn option_result_y_try() {
    // ? que desempaqueta y propaga, con Result.
    compare_source(
        "fn div(a: int, b: int) -> Result<int, string> { if (b == 0) { Result.Err(\"cero\") } else { Result.Ok(a / b) } } fn ev(a: int, b: int, c: int) -> Result<int, string> { let p = div(a, b)?; let q = div(p, c)?; Result.Ok(q) } fn show(r: Result<int, string>) -> int { match (r) { Result.Ok(v) => v, Result.Err(_) => 0 - 1 } } fn main() -> int { print(show(ev(100, 5, 2))); print(show(ev(100, 0, 2))); 0 }",
        "vm_result.ray",
    );
    // ? con Option encadenado y None.
    compare_source(
        "fn half(n: int) -> Option<int> { if (n % 2 == 0) { Option.Some(n / 2) } else { Option.None } } fn dv(n: int) -> Option<int> { let a = half(n)?; let b = half(a)?; Option.Some(b) } fn show(o: Option<int>) -> int { match (o) { Option.Some(v) => v, Option.None => 0 - 1 } } fn main() -> int { print(show(dv(20))); print(show(dv(6))); 0 }",
        "vm_option.ray",
    );
}

// ---------------------------------------------------------------------
// M14.5d — despacho dinámico: métodos de trait, UFCS, dyn, @derive,
// bounds (no-op). Mismo corpus que el oráculo del intérprete (M14.4d),
// ahora por la VM. La resolución es por la etiqueta del valor en runtime.
// ---------------------------------------------------------------------

#[test]
fn corpus_dispatch_dynamic() {
    for rel in [
        "examples/stdlib/ufcs.ray",
        "examples/types/traits.ray",
        "examples/types/bounds.ray",
        "examples/types/metodos_por_defecto.ray",
        "examples/types/impls_genericos.ray",
        "examples/types/trait_objects.ray",
        "examples/types/anotaciones.ray",
    ] {
        compare_file(rel);
    }
}

// El prelude COMPLETO ya compila por la VM (M14.5d): map/filter/fold (llamadas indirectas, M14.5c) +
// sort/assert_eq (métodos `.less()`/`.eq()`/`.show()`, resueltos por ODispatch). `run_vm.ray` lo
// fusiona como `run.ray`.
#[test]
fn corpus_prelude() {
    for rel in [
        "examples/stdlib/stdlib.ray",   // map/filter/fold
        "examples/data/mapa.ray",     // Map + assert_eq + sort
    ] {
        compare_file(rel);
    }
}

#[test]
fn methods_y_ufcs_snippets() {
    // Método de trait con despacho estático (resuelto por la etiqueta del valor en runtime).
    compare_source(
        "trait Saludo { fn hello(self) -> string } struct P { n: string } impl Saludo for P { fn hello(self) -> string { \"hello \" + self.n } } fn main() -> int { let p = P { n: \"ray\" }; print(p.hello()); 0 }",
        "vm_method.ray",
    );
    // UFCS: una función libre llamada con sintaxis de método (el receptor es el primer argumento).
    compare_source(
        "fn square(x: int) -> int { x * x } fn main() -> int { print(5.square()); 0 }",
        "vm_ufcs.ray",
    );
    // @derive(Eq, Show): igualdad e impresión derivadas (resueltas por valor, sin tabla de métodos).
    compare_source(
        "@derive(Eq, Show) struct Punto { x: int, y: int } fn main() -> int { let a = Punto { x: 1, y: 2 }; let b = Punto { x: 1, y: 2 }; print(a.eq(b)); print(a.show()); 0 }",
        "vm_derive.ray",
    );
    // sort del prelude sobre un arreglo de int (usa `.less()` de Ord sobre primitivos).
    compare_source(
        "fn main() -> int { let xs = sort([3, 1, 2]); print(xs); 0 }",
        "vm_sort.ray",
    );
}

// ---------------------------------------------------------------------
// M14.5e — TCO (recursión de cola en O(1) marcos). Una recursión de cola
// profunda corre por la VM auto-alojada reutilizando el marco, sin
// desbordar, igual que en Rust (que tiene TCO desde M13.3b).
//
// `#[ignore]`: profundidades de 200k DOBLE-interpretadas (la VM auto-alojada
// corre sobre la VM del host) son lentas (~1 min). Es el oráculo THOROUGH —
// la prueba real de O(1) marcos—, como el run-on-run meta-circular:
//   cargo test --test selfhost_vm -- --ignored
// La corrección de los opcodes Tail* también la cubre el resto del corpus
// (gcd/primes/sort... tienen llamadas en cola), que sí corre por defecto.
// ---------------------------------------------------------------------

#[test]
#[ignore]
fn tail_recursion() {
    // Recursión de cola DIRECTA profunda (sin TCO crecería el stack de marcos sin límite → desbordaría).
    compare_source(
        "fn account(n: int, acc: int) -> int { if (n == 0) { acc } else { account(n - 1, acc + n) } } fn main() -> int { print(account(1000000, 0)); 0 }",
        "vm_tco_directa.ray",
    );
    // Recursión de cola MUTUA profunda.
    compare_source(
        "fn par(n: int) -> bool { if (n == 0) { true } else { impar(n - 1) } } fn impar(n: int) -> bool { if (n == 0) { false } else { par(n - 1) } } fn main() -> int { if (par(1000000)) { print(1); } else { print(0); } 0 }",
        "vm_tco_mutua.ray",
    );
}
