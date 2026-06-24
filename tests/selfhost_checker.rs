//! Oráculo del **checker auto-alojado** (M14.3, self-hosting).
//!
//! El checker escrito en raylang (`selfhost/checker.ray`, vía `selfhost/check_dump.ray`) debe dar el
//! mismo VEREDICTO que el checker de Rust (`src/checker.rs`): `ok` para programas válidos, o
//! `error de tipos en L:C: msg` byte-idéntico para los inválidos. Es un VALIDADOR (no reproduce el
//! lowering de M9; ver DESIGN §23.4): solo se compara el veredicto.
//!
//! Estrategia: la misma fuente por ambos pipelines (Rust: lex→parse→check; raylang: self-lex→
//! self-parse→self-check). El lado raylang lo imprime el driver; el lado Rust lo reconstruye
//! `canonical`.
//!
//! Cobertura M14.3a: núcleo monomórfico (operadores, variables, llamadas, if/while/return, builtin
//! print). M14.3b: datos (arreglos `[T]`/índice/`len`/`push`, structs def/literal/campo/asignación,
//! enums construcción/`match`/exhaustividad/patrones). El corpus evita genéricos/traits (→ M14.3c–d).

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

/// El veredicto del front-end de Rust (el oráculo) para una fuente: `ok` o el `Display` del primer
/// error (léxico, sintáctico o de tipos), exactamente lo que imprime `check_dump.ray`.
fn canonical(src: &str) -> String {
    let tokens = match raylang::lexer::lex(src) {
        Ok(t) => t,
        Err(e) => return format!("{e}"),
    };
    let mut prog = match raylang::parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => return format!("{e}"),
    };
    match raylang::checker::check(&mut prog) {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("{e}"),
    }
}

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Ejecuta el checker auto-alojado sobre `src`: lo escribe a un temporal y corre
/// `raylang selfhost/check_dump.ray <temporal>`. Devuelve su stdout (sin el salto final).
fn check_dump(src: &str, nombre_tmp: &str) -> String {
    let mut tmp = std::env::temp_dir();
    tmp.push(nombre_tmp);
    let mut f = std::fs::File::create(&tmp).expect("crea el temporal");
    f.write_all(src.as_bytes()).expect("escribe el temporal");
    drop(f);

    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(repo_path("selfhost/check_dump.ray"))
        .arg(&tmp)
        .output()
        .expect("ejecuta el checker auto-alojado");
    assert!(
        out.status.success(),
        "check_dump falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

/// Compara el checker auto-alojado con el oráculo para una fuente concreta.
fn comparar(src: &str, nombre_tmp: &str) {
    let esperado = canonical(src);
    let obtenido = check_dump(src, nombre_tmp);
    assert_eq!(obtenido, esperado, "el checker auto-alojado difiere del oráculo para:\n{src}");
}

#[test]
fn programas_validos() {
    comparar("fn main() -> int { 0 }", "sc_min.ray");
    comparar("fn main() { }", "sc_unit.ray");
    comparar("fn main() -> int { let x = 3; let y = x + 1; y }", "sc_let.ray");
    comparar("fn main() -> int { var i = 0; while (i < 10) { i = i + 1; } i }", "sc_while.ray");
    comparar("fn main() -> int { if (1 < 2) { 1 } else { 2 } }", "sc_if.ray");
    comparar("fn doble(x: int) -> int { x * 2 } fn main() -> int { doble(21) }", "sc_call.ray");
    comparar("fn main() -> int { print(\"hola\"); print(42); 0 }", "sc_print.ray");
    comparar("fn main() -> int { let b = true && false || 1 == 2; if (b) { 1 } else { 0 } }", "sc_logic.ray");
}

#[test]
fn errores_de_tipo() {
    // Operadores.
    comparar("fn main() -> int { 1 + true }", "sce_add.ray");
    comparar("fn main() -> int { if (1) { 1 } else { 2 } }", "sce_cond.ray");
    comparar("fn main() -> bool { 1 < true }", "sce_order.ray");
    comparar("fn main() -> int { -true }", "sce_neg.ray");
    comparar("fn main() -> int { if (!1) { 0 } else { 1 } }", "sce_not.ray");
    // Variables.
    comparar("fn main() -> int { x }", "sce_undecl.ray");
    comparar("fn main() -> int { let x = 3; x = 4; 0 }", "sce_immut.ray");
    comparar("fn main() -> int { let x: int = true; x }", "sce_annot.ray");
    // Retorno.
    comparar("fn main() -> int { true }", "sce_ret.ray");
    comparar("fn f() -> int { } fn main() -> int { 0 }", "sce_ret_unit.ray");
    // Llamadas.
    comparar("fn g(x: int) -> int { x } fn main() -> int { g() }", "sce_arity.ray");
    comparar("fn g(x: int) -> int { x } fn main() -> int { g(true) }", "sce_argty.ray");
    comparar("fn main() -> int { h(1) }", "sce_nofn.ray");
    comparar("fn main() -> int { print(1, 2) }", "sce_print_arity.ray");
    // if sin else con valor.
    comparar("fn main() -> int { if (true) { 1 } 0 }", "sce_if_noelse.ray");
    // main mal.
    comparar("fn main(x: int) -> int { 0 }", "sce_main_params.ray");
    comparar("fn main() -> bool { true }", "sce_main_ret.ray");
    comparar("fn f() -> int { 1 }", "sce_no_main.ray");
    // tipo desconocido.
    comparar("fn main() -> Foo { 0 }", "sce_unknown_type.ray");
}

#[test]
fn datos_validos() {
    // Structs: definición, literal, acceso a campo, asignación a campo (incl. anidada).
    comparar("struct P { x: int, y: int } fn area(p: P) -> int { p.x + p.y } fn main() -> int { let p = P { x: 1, y: 2 }; area(p) }", "scd_struct.ray");
    comparar("struct P { x: int } fn main() -> int { var p = P { x: 1 }; p.x = 9; p.x }", "scd_fassign.ray");
    comparar("struct Q { p: int } struct R { q: Q } fn main() -> int { var r = R { q: Q { p: 1 } }; r.q.p = 5; r.q.p }", "scd_nested.ray");
    // Arreglos: literal anotado vacío, push, len, índice y asignación por índice.
    comparar("fn main() -> int { var xs: [int] = []; push(xs, 3); xs[0] = 4; len(xs) }", "scd_arr.ray");
    comparar("fn main() -> int { let m: [[int]] = [[1, 2], [3, 4]]; m[1][0] }", "scd_matriz.ray");
    // Enums: construcción con/sin payload, match con bindings, comodín, catch-all.
    comparar("enum E { A, B(int) } fn f(e: E) -> int { match (e) { E.A => 0, E.B(n) => n } } fn main() -> int { f(E.B(7)) }", "scd_match.ray");
    comparar("enum E { A, B, C } fn f(e: E) -> int { match (e) { E.A => 1, _ => 0 } } fn main() -> int { f(E.C) }", "scd_catchall.ray");
    comparar("enum F { C(float), R(float, float) } fn area(f: F) -> float { match (f) { F.C(r) => r * r, F.R(w, h) => w * h } } fn main() { print(area(F.C(2.0))); }", "scd_payload.ray");
}

#[test]
fn errores_de_datos() {
    // Structs.
    comparar("fn main() -> int { let p = Q { x: 1 }; 0 }", "scde_unk_struct.ray");
    comparar("struct P { x: int, y: int } fn main() -> int { let p = P { x: 1 }; 0 }", "scde_missing.ray");
    comparar("struct P { x: int, y: int } fn main() -> int { let p = P { x: 1, z: 2, y: 3 }; 0 }", "scde_unkfield.ray");
    comparar("struct P { x: int, y: int } fn main() -> int { let p = P { x: 1, x: 2, y: 3 }; 0 }", "scde_repfield.ray");
    comparar("struct P { x: int, y: int } fn main() -> int { let p = P { x: true, y: 2 }; 0 }", "scde_fieldty.ray");
    comparar("fn main() -> int { let n = 3; n.x }", "scde_field_nonst.ray");
    comparar("struct P { x: int } fn main() -> int { let p = P { x: 1 }; p.z }", "scde_field_unk.ray");
    comparar("struct P { x: int } fn main() -> int { var p = P { x: 1 }; p.x = true; 0 }", "scde_fassign_ty.ray");
    // Arreglos.
    comparar("fn main() -> int { let xs = []; 0 }", "scde_empty.ray");
    comparar("fn main() -> int { let xs = [1, true]; 0 }", "scde_arrty.ray");
    comparar("fn main() -> int { let xs: [int] = [1]; xs[true] }", "scde_idxty.ray");
    comparar("fn main() -> int { let n = 3; n[0] }", "scde_idx_nonarr.ray");
    comparar("fn main() -> int { var xs: [int] = [1]; xs[0] = true; 0 }", "scde_iassign_ty.ray");
    comparar("fn main() -> int { var xs: [int] = []; push(xs, true); 0 }", "scde_push_ty.ray");
    comparar("fn main() -> int { push(3, 1); 0 }", "scde_push_nonarr.ray");
    comparar("fn main() -> int { len(3) }", "scde_len_nonarr.ray");
    // Enums.
    comparar("enum E { A } fn main() -> int { let x = E.C; 0 }", "scde_unkvariant.ray");
    comparar("enum F { C(float) } fn main() -> int { let x = F.C(); 0 }", "scde_enum_arity.ray");
    comparar("enum F { C(float) } fn main() -> int { let x = F.C(true); 0 }", "scde_payty.ray");
    // match.
    comparar("fn main() -> int { match (3) { _ => 0 } }", "scde_match_nonen.ray");
    comparar("enum E { A, B } fn main() -> int { match (E.A) { E.A => 1 } }", "scde_nonexh.ray");
    comparar("enum E { A, B } fn main() -> int { match (E.A) { _ => 1, E.A => 2 } }", "scde_unreach.ray");
    comparar("enum E { A, B } fn main() -> int { match (E.A) { E.A => 1, E.A => 2, E.B => 3 } }", "scde_covered.ray");
    comparar("enum F { C(float) } fn main() -> int { match (F.C(1.0)) { F.C(a, b) => 0 } }", "scde_pat_arity.ray");
    comparar("enum E { A, B } fn main() -> int { match (E.A) { E.A => 1, E.B => true } }", "scde_arm_ty.ray");
    comparar("enum E { A } enum G { Z } fn main() -> int { match (E.A) { G.Z => 0 } }", "scde_pat_enum.ray");
    // Definiciones de tipos (el checker las detecta directamente; en el pipeline completo lo haría el loader).
    comparar("enum E { A } enum E { B } fn main() -> int { 0 }", "scde_dup_enum.ray");
    comparar("struct P { x: int } struct P { y: int } fn main() -> int { 0 }", "scde_dup_struct.ray");
    comparar("enum E { A } struct E { x: int } fn main() -> int { 0 }", "scde_struct_enum.ray");
    comparar("enum E { A, A } fn main() -> int { 0 }", "scde_repvariant.ray");
}

#[test]
fn genericos_validos() {
    // Función genérica mínima; inferencia de T desde el argumento.
    comparar("fn id<T>(x: T) -> T { x } fn main() -> int { let a: int = id(5); a }", "scg_id.ray");
    // Genérica de orden superior (tipo función con T, U).
    comparar("fn ap<T, U>(f: fn(T) -> U, x: T) -> U { f(x) } fn doble(n: int) -> int { n * 2 } fn main() -> int { ap(doble, 21) }", "scg_ap.ray");
    // Genérica sobre arreglos.
    comparar("fn ultimo<T>(xs: [T]) -> T { xs[len(xs) - 1] } fn main() -> int { ultimo([1, 2, 3]) }", "scg_last.ray");
    comparar("fn par<T>(a: T, b: T) -> [T] { [a, b] } fn main() -> int { let xs: [int] = par(10, 20); xs[0] }", "scg_par.ray");
}

#[test]
fn errores_genericos() {
    comparar("fn id<T>(x: T) -> T { x } fn main() -> int { id(1, 2) }", "scge_arity.ray");
    comparar("fn par<T>(a: T, b: T) -> [T] { [a, b] } fn main() -> int { let xs = par(1, true); 0 }", "scge_consist.ray");
    comparar("fn raro<T>(x: int) -> int { x } fn main() -> int { raro(3) }", "scge_uninfer.ray");
    comparar("fn f<T, T>(x: T) -> T { x } fn main() -> int { 0 }", "scge_duptp.ray");
    comparar("fn ap<T, U>(f: fn(T) -> U, x: T) -> U { f(x) } fn negar(b: bool) -> bool { !b } fn main() -> int { ap(negar, 3) }", "scge_fnarg.ray");
}

#[test]
fn tipos_genericos_validos() {
    // Struct genérico: infiere A, B de los valores; acceso a campo sustituye el tipo.
    comparar("struct Par<A, B> { primero: A, segundo: B } fn main() -> int { let p: Par<int, bool> = Par { primero: 7, segundo: true }; if (p.segundo) { p.primero } else { 0 } }", "sct_par.ray");
    // Enum genérico: T del payload y T del tipo esperado (Vacia); match sustituye el payload.
    comparar("enum Caja<T> { Llena(T), Vacia } fn val(c: Caja<int>, d: int) -> int { match (c) { Caja.Llena(v) => v, Caja.Vacia => d } } fn main() -> int { let a: Caja<int> = Caja.Llena(35); let b: Caja<int> = Caja.Vacia; val(a, 0) + val(b, 9) }", "sct_caja.ray");
    // Enum genérico recursivo + función genérica que lo recorre.
    comparar("enum Lista<T> { Cons(T, Lista<T>), Nil } fn longitud<T>(xs: Lista<T>) -> int { match (xs) { Lista.Cons(_, r) => 1 + longitud(r), Lista.Nil => 0 } } fn main() -> int { let ns: Lista<string> = Lista.Cons(\"a\", Lista.Cons(\"b\", Lista.Nil)); longitud(ns) }", "sct_lista.ray");
    // Inferencia local de un enum genérico (sin anotación) + match.
    comparar("enum Caja<T> { Llena(T), Vacia } fn main() -> int { let c = Caja.Llena(5); match (c) { Caja.Llena(v) => v, Caja.Vacia => 0 } }", "sct_infer.ray");
}

#[test]
fn errores_tipos_genericos() {
    comparar("struct Par<A, B> { primero: A, segundo: B } fn main() -> int { let p: Par<int, bool> = Par { primero: true, segundo: true }; 0 }", "scte_fieldty.ray");
    comparar("enum Caja<T> { Llena(T), Vacia } fn main() -> int { let c = Caja.Vacia; 0 }", "scte_vacia.ray");
    comparar("enum Caja<T> { Llena(T), Vacia } fn f(c: Caja<int, bool>) -> int { 0 } fn main() -> int { 0 }", "scte_arity.ray");
    comparar("enum Par2<T> { P(T, T) } fn main() -> int { let x = Par2.P(1, true); 0 }", "scte_payincon.ray");
    comparar("struct Caja<T> { v: T } fn f(c: Caja) -> int { 0 } fn main() -> int { 0 }", "scte_noargs.ray");
}

#[test]
fn prelude_y_try_validos() {
    // Result del prelude: construcción + match (con el tipo esperado fijando T y E).
    comparar("fn div(a: int, b: int) -> Result<int, string> { if (b == 0) { Result.Err(\"cero\") } else { Result.Ok(a / b) } } fn main() -> int { match (div(6, 2)) { Result.Ok(v) => v, Result.Err(_) => -1 } }", "scp_result.ray");
    // El operador `?` sobre Result.
    comparar("fn div(a: int, b: int) -> Result<int, string> { if (b == 0) { Result.Err(\"cero\") } else { Result.Ok(a / b) } } fn ev(a: int, b: int) -> Result<int, string> { let p: int = div(a, b)?; Result.Ok(p + 1) } fn main() -> int { match (ev(6, 2)) { Result.Ok(v) => v, Result.Err(_) => -1 } }", "scp_try.ray");
    // Option del prelude + `?` sobre Option.
    comparar("fn primero(xs: [int]) -> Option<int> { if (len(xs) == 0) { Option.None } else { Option.Some(xs[0]) } } fn main() -> int { match (primero([7])) { Option.Some(v) => v, Option.None => -1 } }", "scp_option.ray");
    comparar("fn g(xs: [int]) -> Option<int> { if (len(xs) == 0) { Option.None } else { Option.Some(xs[0]) } } fn f(xs: [int]) -> Option<int> { let x: int = g(xs)?; Option.Some(x + 1) } fn main() -> int { match (f([3])) { Option.Some(v) => v, Option.None => -1 } }", "scp_tryopt.ray");
}

#[test]
fn errores_prelude_y_try() {
    comparar("fn f() -> Result<int, string> { let x: int = 5?; Result.Ok(x) } fn main() -> int { 0 }", "scpe_nores.ray");
    comparar("fn div() -> Result<int, string> { Result.Ok(1) } fn f() -> int { let x: int = div()?; x } fn main() -> int { 0 }", "scpe_badret.ray");
    comparar("fn div() -> Result<int, string> { Result.Ok(1) } fn f() -> Result<int, bool> { let x: int = div()?; Result.Ok(x) } fn main() -> int { 0 }", "scpe_errmis.ray");
    comparar("fn primero(xs: [int]) -> Option<int> { Option.Algun(xs[0]) } fn main() -> int { 0 }", "scpe_optvar.ray");
    comparar("fn f() -> int { let r = Result.Ok(5); 0 } fn main() -> int { 0 }", "scpe_uninf.ray");
}

#[test]
fn ufcs_y_closures_validos() {
    // UFCS: función libre como método (encadenada), receptor como primer argumento.
    comparar("fn doble(x: int) -> int { x * 2 } fn main() -> int { let v = 5; v.doble().doble() }", "scu_ufcs.ray");
    // Builtin como método.
    comparar("fn main() -> int { let xs: [int] = [1, 2, 3]; xs.len() }", "scu_builtin.ray");
    // Campo de struct (de tipo función) gana sobre la función libre homónima.
    comparar("struct M { op: fn(int) -> int } fn op(a: int, b: int) -> int { a - b } fn main() -> int { let m = M { op: fn(x: int) -> int { x + 1 } }; m.op(41) }", "scu_field.ray");
    // UFCS sobre una función genérica: el receptor cuenta para la inferencia.
    comparar("fn cabeza<T>(xs: [T]) -> T { xs[0] } fn main() -> int { let xs: [int] = [10, 20]; xs.cabeza() }", "scu_gen.ray");
    // Función anónima como valor + captura del entorno (closure).
    comparar("fn aplica(f: fn(int) -> int, x: int) -> int { f(x) } fn main() -> int { let g = fn(n: int) -> int { n + 1 }; aplica(g, 41) }", "scu_closure.ray");
    comparar("fn main() -> int { let base = 10; let f = fn(x: int) -> int { x + base }; f(5) }", "scu_capture.ray");
}

#[test]
fn errores_ufcs_y_closures() {
    comparar("fn main() -> int { let v = 5; v.inexistente() }", "scue_noufcs.ray");
    comparar("fn doble(x: int) -> int { x * 2 } fn main() -> int { let b = true; b.doble() }", "scue_ufcsty.ray");
    comparar("fn main() -> int { let f = fn(x: int) -> int { true }; 0 }", "scue_anonret.ray");
}

/// El test fuerte: los ejemplos reales deben dar el mismo veredicto (`ok`) que Rust.
#[test]
fn ejemplos_reales_validos() {
    let archivos = ["examples/fib.ray", "examples/fizzbuzz.ray", "examples/gcd.ray", "examples/primes.ray",
        "examples/structs.ray", "examples/match_figuras.ray", "examples/enums.ray", "examples/arrays.ray",
        "examples/matriz.ray", "examples/genericos.ray", "examples/tipos_genericos.ray", "examples/opcional.ray",
        "examples/errores.ray", "examples/ufcs.ray", "examples/closures.ray"];
    for rel in archivos {
        let src = std::fs::read_to_string(repo_path(rel)).unwrap_or_else(|e| panic!("lee {rel}: {e}"));
        let esperado = canonical(&src);
        let nombre_tmp = format!("sc_real_{}.ray", rel.replace('/', "_"));
        let obtenido = check_dump(&src, &nombre_tmp);
        assert_eq!(obtenido, esperado, "el checker auto-alojado difiere del oráculo en {rel}");
    }
}
