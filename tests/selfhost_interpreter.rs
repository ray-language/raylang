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
fn corpus_datos() {
    // M14.4b: structs, enums, match, arreglos (incl. anidados).
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
        "in_struct.ray",
    );
    // Mutación de campo + aliasing por semántica de referencia.
    comparar_fuente(
        "struct P { x: int, y: int } fn main() -> int { var p = P { x: 1, y: 2 }; let q = p; p.x = 9; print(q.x); 0 }",
        "in_alias.ray",
    );
    // Arreglo de structs + mutación por índice.
    comparar_fuente(
        "struct P { x: int, y: int } fn main() -> int { var ps: [P] = []; push(ps, P { x: 1, y: 2 }); ps[0].x = 99; print(ps[0]); print(len(ps)); 0 }",
        "in_arrstruct.ray",
    );
}

#[test]
fn enums_y_match_snippets() {
    comparar_fuente(
        "enum Dir { N, S, E, O } fn dx(d: Dir) -> int { match (d) { Dir.E => 1, Dir.O => 0 - 1, _ => 0 } } fn main() -> int { print(dx(Dir.E) + dx(Dir.O)); print(dx(Dir.N)); 0 }",
        "in_match.ray",
    );
    // Variantes con payload + binding + comodín de payload.
    comparar_fuente(
        "enum E { A(int, int), B(string), C } fn f(e: E) -> int { match (e) { E.A(x, y) => x + y, E.B(_) => 0, E.C => 0 - 1 } } fn main() -> int { print(f(E.A(3, 4))); print(f(E.B(\"z\"))); print(f(E.C)); 0 }",
        "in_payload.ray",
    );
    // Enum recursivo (lista enlazada) + Display anidado.
    comparar_fuente(
        "enum L { Nil, Cons(int, L) } fn suma(l: L) -> int { match (l) { L.Nil => 0, L.Cons(h, t) => h + suma(t) } } fn main() -> int { let l = L.Cons(1, L.Cons(2, L.Nil)); print(suma(l)); print(l); 0 }",
        "in_list.ray",
    );
}

#[test]
fn arreglos_snippets() {
    comparar_fuente(
        "fn main() -> int { let a = [3, 1, 4, 1, 5]; print(a); print(a[2]); print(len(a)); 0 }",
        "in_array.ray",
    );
    // Construcción dinámica + asignación por índice.
    comparar_fuente(
        "fn main() -> int { var a: [int] = []; var i = 0; while (i < 4) { push(a, i * i); i = i + 1; } a[0] = 100; print(a); 0 }",
        "in_arrpush.ray",
    );
    // Arreglos anidados con indexación encadenada.
    comparar_fuente(
        "fn main() -> int { let m = [[1, 2], [3, 4]]; print(m[1][0]); print(m); 0 }",
        "in_nested.ray",
    );
}

#[test]
fn corpus_primera_clase() {
    // M14.4c: closures (captura por celda), funciones como valor, ?, Option/Result.
    for rel in [
        "examples/closures.ray",
        "examples/errores.ray",
        "examples/opcional.ray",
    ] {
        comparar_archivo(rel);
    }
}

#[test]
fn closures_snippets() {
    // Funciones como valor pasadas a orden superior definido por el usuario.
    comparar_fuente(
        "fn doble(x: int) -> int { x * 2 } fn aplica(f: fn(int) -> int, x: int) -> int { f(x) } fn main() -> int { print(aplica(doble, 21)); 0 }",
        "in_hof.ray",
    );
    // Closure que captura un `let` de main.
    comparar_fuente(
        "fn main() -> int { let base = 1000; let f = fn(d: int) -> int { base + d }; print(f(7)); 0 }",
        "in_capture.ray",
    );
    // Estado por celda compartida: instancias independientes, mutación persistente.
    comparar_fuente(
        "fn acc(start: int) -> fn(int) -> int { var s = start; fn(d: int) -> int { s = s + d; s } } fn main() -> int { let a = acc(0); let b = acc(100); print(a(1)); print(a(2)); print(b(10)); print(a(3)); 0 }",
        "in_state.ray",
    );
}

#[test]
fn option_result_y_try() {
    // ? que desempaqueta y propaga, con Result.
    comparar_fuente(
        "fn div(a: int, b: int) -> Result<int, string> { if (b == 0) { Result.Err(\"cero\") } else { Result.Ok(a / b) } } fn ev(a: int, b: int, c: int) -> Result<int, string> { let p = div(a, b)?; let q = div(p, c)?; Result.Ok(q) } fn show(r: Result<int, string>) -> int { match (r) { Result.Ok(v) => v, Result.Err(_) => 0 - 1 } } fn main() -> int { print(show(ev(100, 5, 2))); print(show(ev(100, 0, 2))); 0 }",
        "in_result.ray",
    );
    // ? con Option encadenado y None.
    comparar_fuente(
        "fn mitad(n: int) -> Option<int> { if (n % 2 == 0) { Option.Some(n / 2) } else { Option.None } } fn dv(n: int) -> Option<int> { let a = mitad(n)?; let b = mitad(a)?; Option.Some(b) } fn show(o: Option<int>) -> int { match (o) { Option.Some(v) => v, Option.None => 0 - 1 } } fn main() -> int { print(show(dv(20))); print(show(dv(6))); 0 }",
        "in_option.ray",
    );
}

#[test]
fn corpus_despacho_dinamico() {
    // M14.4d-1: UFCS, métodos de trait, bounds (no-op), impls genéricos, dyn, @derive.
    for rel in [
        "examples/ufcs.ray",
        "examples/traits.ray",
        "examples/bounds.ray",
        "examples/metodos_por_defecto.ray",
        "examples/impls_genericos.ray",
        "examples/trait_objects.ray",
        "examples/anotaciones.ray",
    ] {
        comparar_archivo(rel);
    }
}

#[test]
fn ufcs_snippets() {
    // UFCS a builtin (len), a función libre, y a genérica; campo-función gana sobre función libre.
    comparar_fuente(
        "fn doble(x: int) -> int { x * 2 } fn cabeza<T>(xs: [T]) -> T { xs[0] } fn main() -> int { let xs = [10, 20, 30]; print(xs.len()); print(xs.cabeza()); print((5).doble().doble()); 0 }",
        "in_ufcs.ray",
    );
    comparar_fuente(
        "fn op(a: int, b: int) -> int { a - b } struct M { op: fn(int) -> int } fn main() -> int { let m = M { op: fn(x: int) -> int { x + 1 } }; print(m.op(41)); 0 }",
        "in_field_fn.ray",
    );
}

#[test]
fn traits_y_bounds_snippets() {
    // Despacho de método sobre struct/enum/primitivo.
    comparar_fuente(
        "trait V { fn v(self) -> int; } struct P { x: int } impl V for P { fn v(self) -> int { self.x } } impl V for int { fn v(self) -> int { self } } fn main() -> int { print(P { x: 7 }.v()); print((42).v()); 0 }",
        "in_trait.ray",
    );
    // Bound como no-op: despacho por tipo concreto en runtime.
    comparar_fuente(
        "trait V { fn v(self) -> int; } struct P { x: int } impl V for P { fn v(self) -> int { self.x } } impl V for int { fn v(self) -> int { self } } fn dv<T: V>(x: T) -> int { x.v() + x.v() } fn main() -> int { print(dv(P { x: 5 })); print(dv(10)); 0 }",
        "in_bound.ray",
    );
    // Método por defecto que llama a otro método del trait.
    comparar_fuente(
        "trait S { fn nombre(self) -> string; fn saludar(self) -> string { self.nombre() } } struct A { } impl S for A { fn nombre(self) -> string { \"a\" } } struct B { } impl S for B { fn nombre(self) -> string { \"b\" } fn saludar(self) -> string { \"hola\" } } fn main() -> int { print(A { }.saludar()); print(B { }.saludar()); 0 }",
        "in_default.ray",
    );
}

#[test]
fn dyn_y_derive_snippets() {
    // dyn: valor concreto + despacho por etiqueta + método por defecto sobre el objeto.
    comparar_fuente(
        "trait An { fn voz(self) -> string; fn di(self) -> int { print(self.voz()); 1 } } struct Pe { } impl An for Pe { fn voz(self) -> string { \"guau\" } } struct Ga { } impl An for Ga { fn voz(self) -> string { \"miau\" } } fn coro(xs: [dyn An]) -> int { var n = 0; var i = 0; while (i < len(xs)) { n = n + xs[i].di(); i = i + 1; } n } fn main() -> int { let xs: [dyn An] = [Pe { }, Ga { }]; coro(xs) }",
        "in_dyn.ray",
    );
    // @derive(Eq, Show) anidado: igual recursivo, mostrar recursivo.
    comparar_fuente(
        "@derive(Eq, Show) struct P { x: int, y: int } @derive(Eq, Show) enum F { C(int), B(P) } fn main() -> int { let a = F.B(P { x: 1, y: 2 }); let b = F.B(P { x: 1, y: 2 }); print(a.mostrar()); print(a.igual(b)); print(a.igual(F.C(5))); 0 }",
        "in_derive.ray",
    );
}

#[test]
fn corpus_prelude_y_genericos() {
    // M14.4d-2: map/filter/fold del prelude (UFCS + pipelines + closures) y genéricos (no-op en runtime).
    for rel in [
        "examples/stdlib.ray",
        "examples/genericos.ray",
        "examples/tipos_genericos.ray",
    ] {
        comparar_archivo(rel);
    }
}

#[test]
fn map_filter_fold_snippets() {
    // map/filter/fold por UFCS encadenado.
    comparar_fuente(
        "fn doble(x: int) -> int { x * 2 } fn par(x: int) -> bool { x % 2 == 0 } fn suma(a: int, b: int) -> int { a + b } fn main() -> int { let xs = [1, 2, 3, 4]; print(xs.map(doble).filter(par).fold(0, suma)); 0 }",
        "in_mff_ufcs.ray",
    );
    // Pipeline + closures anónimos en línea.
    comparar_fuente(
        "fn suma(a: int, b: int) -> int { a + b } fn main() -> int { let xs = [1, 2, 3, 4, 5]; print(xs |> map(fn(x: int) -> int { x * x }) |> filter(fn(x: int) -> bool { x > 5 }) |> fold(0, suma)); 0 }",
        "in_mff_pipe.ray",
    );
    // El usuario redefine map → su versión gana (override del prelude).
    comparar_fuente(
        "fn map(x: int) -> int { x + 1 } fn main() -> int { print(map(41)); 0 }",
        "in_override.ray",
    );
}

#[test]
fn string_builtins() {
    // M14.6a: familia de string (to_string/to_upper/to_lower/len/starts_with/ends_with/contains/
    // replace/substring/repeat/trim/split/join/chars) por UFCS y llamada directa.
    comparar_fuente(
        "fn main() -> int { \
            let s = \"Hola, Mundo\"; \
            print(to_string(42) + \"-\" + to_string(3.5) + \"-\" + to_string(true)); \
            print(s.to_upper()); print(s.to_lower()); print(s.len()); \
            print(s.starts_with(\"Hola\")); print(s.ends_with(\"do\")); print(s.contains(\"Mun\")); \
            print(s.replace(\"o\", \"0\")); print(s.substring(0, 4)); print(\"ab\".repeat(3)); \
            print(\"  x  \".trim()); \
            let parts = \"a,b,c\".split(\",\"); print(parts); print(parts.len()); print(join(parts, \"-\")); \
            let cs = chars(\"xyz\"); print(cs); print(cs[1]); \
            print([1, 2, 3].contains(2)); \
            0 \
        }",
        "in_strings.ray",
    );
    // chars + indexar string + recorrido.
    comparar_fuente(
        "fn main() -> int { let s = \"abc\"; var i = 0; while (i < s.len()) { print(s[i]); i = i + 1; } 0 }",
        "in_str_idx.ray",
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
