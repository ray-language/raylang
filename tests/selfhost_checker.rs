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
fn check_dump(src: &str, name_tmp: &str) -> String {
    let mut tmp = std::env::temp_dir();
    tmp.push(name_tmp);
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
fn compare(src: &str, name_tmp: &str) {
    let expected = canonical(src);
    let obtained = check_dump(src, name_tmp);
    assert_eq!(obtained, expected, "el checker auto-alojado difiere del oráculo para:\n{src}");
}

/// Como [`comparar`], pero solo exige que ambos pipelines **rechacen en la misma posición** (mismo
/// prefijo `error de tipos en L:C`), tolerando el texto del mensaje. M48.4e: al retirar los builtins de
/// contenedor, el checker de Rust los ve como **métodos de trait** (`xs.push(true)` → error de inferencia
/// de `[]#push`; `(3).push(1)` → "no existe campo ni función 'push' aplicable a int"), mientras este
/// validador auto-alojado —un subconjunto que los sigue modelando como **builtins de arreglo/string**—
/// da su propio mensaje. Ambos rechazan el mismo error en el mismo sitio; la redacción difiere por diseño.
fn compare_pos(src: &str, name_tmp: &str) {
    let expected = canonical(src);
    let obtained = check_dump(src, name_tmp);
    let pos = |s: &str| s.split(": ").next().unwrap_or("").to_string();
    assert!(expected.starts_with("type error at"), "el oráculo no rechazó:\n{src}\n  {expected}");
    assert!(obtained.starts_with("type error at"), "el auto-alojado no rechazó:\n{src}\n  {obtained}");
    assert_eq!(pos(&obtained), pos(&expected),
        "posición del rechazo difiere para:\n{src}\n  auto: {obtained}\n  rust: {expected}");
}

#[test]
fn programas_valid_vals() {
    compare("fn main() -> int { 0 }", "sc_min.ray");
    compare("fn main() { }", "sc_unit.ray");
    compare("fn main() -> int { let x = 3; let y = x + 1; y }", "sc_let.ray");
    compare("fn main() -> int { var i = 0; while (i < 10) { i = i + 1; } i }", "sc_while.ray");
    compare("fn main() -> int { if (1 < 2) { 1 } else { 2 } }", "sc_if.ray");
    compare("fn double(x: int) -> int { x * 2 } fn main() -> int { double(21) }", "sc_call.ray");
    compare("fn main() -> int { print(\"hello\"); print(42); 0 }", "sc_print.ray");
    compare("fn main() -> int { let b = true && false || 1 == 2; if (b) { 1 } else { 0 } }", "sc_logic.ray");
    // `unit` escrito en posición de tipo (SPEC §3): válido, y con args de tipo NO (tipo desconocido).
    compare("fn side() -> unit { print(1); } fn main() -> int { side(); 0 }", "sc_unit_written.ray");
    compare("fn f() -> unit<int> { } fn main() -> int { 0 }", "sce_unit_args.ray");
}

#[test]
fn type_errors() {
    // Operadores.
    compare("fn main() -> int { 1 + true }", "sce_add.ray");
    compare("fn main() -> int { if (1) { 1 } else { 2 } }", "sce_cond.ray");
    compare("fn main() -> bool { 1 < true }", "sce_order.ray");
    compare("fn main() -> int { -true }", "sce_neg.ray");
    compare("fn main() -> int { if (!1) { 0 } else { 1 } }", "sce_not.ray");
    // Variables.
    compare("fn main() -> int { x }", "sce_undecl.ray");
    compare("fn main() -> int { let x = 3; x = 4; 0 }", "sce_immut.ray");
    compare("fn main() -> int { let x: int = true; x }", "sce_annot.ray");
    // Retorno.
    compare("fn main() -> int { true }", "sce_ret.ray");
    compare("fn f() -> int { } fn main() -> int { 0 }", "sce_ret_unit.ray");
    // Llamadas.
    compare("fn g(x: int) -> int { x } fn main() -> int { g() }", "sce_arity.ray");
    compare("fn g(x: int) -> int { x } fn main() -> int { g(true) }", "sce_argty.ray");
    compare("fn main() -> int { h(1) }", "sce_nofn.ray");
    compare("fn main() -> int { print(1, 2) }", "sce_print_arity.ray");
    // if sin else con valor.
    compare("fn main() -> int { if (true) { 1 } 0 }", "sce_if_noelse.ray");
    // main mal.
    compare("fn main(x: int) -> int { 0 }", "sce_main_params.ray");
    compare("fn main() -> bool { true }", "sce_main_ret.ray");
    compare("fn f() -> int { 1 }", "sce_no_main.ray");
    // tipo desconocido.
    compare("fn main() -> Foo { 0 }", "sce_unknown_type.ray");
}

#[test]
fn data_valid_vals() {
    // Structs: definición, literal, acceso a campo, asignación a campo (incl. anidada).
    compare("struct P { x: int, y: int } fn area(p: P) -> int { p.x + p.y } fn main() -> int { let p = P { x: 1, y: 2 }; area(p) }", "scd_struct.ray");
    compare("struct P { x: int } fn main() -> int { var p = P { x: 1 }; p.x = 9; p.x }", "scd_fassign.ray");
    compare("struct Q { p: int } struct R { q: Q } fn main() -> int { var r = R { q: Q { p: 1 } }; r.q.p = 5; r.q.p }", "scd_nested.ray");
    // Arreglos: literal anotado vacío, push, len, índice y asignación por índice.
    compare("fn main() -> int { var xs: [int] = []; xs.push(3); xs[0] = 4; xs.len() }", "scd_arr.ray");
    compare("fn main() -> int { let m: [[int]] = [[1, 2], [3, 4]]; m[1][0] }", "scd_matriz.ray");
    // Enums: construcción con/sin payload, match con bindings, comodín, catch-all.
    compare("enum E { A, B(int) } fn f(e: E) -> int { match (e) { E.A => 0, E.B(n) => n } } fn main() -> int { f(E.B(7)) }", "scd_match.ray");
    compare("enum E { A, B, C } fn f(e: E) -> int { match (e) { E.A => 1, _ => 0 } } fn main() -> int { f(E.C) }", "scd_catchall.ray");
    compare("enum F { C(float), R(float, float) } fn area(f: F) -> float { match (f) { F.C(r) => r * r, F.R(w, h) => w * h } } fn main() { print(area(F.C(2.0))); }", "scd_payload.ray");
}

#[test]
fn data_errors() {
    // Structs.
    compare("fn main() -> int { let p = Q { x: 1 }; 0 }", "scde_unk_struct.ray");
    compare("struct P { x: int, y: int } fn main() -> int { let p = P { x: 1 }; 0 }", "scde_missing.ray");
    compare("struct P { x: int, y: int } fn main() -> int { let p = P { x: 1, z: 2, y: 3 }; 0 }", "scde_unkfield.ray");
    compare("struct P { x: int, y: int } fn main() -> int { let p = P { x: 1, x: 2, y: 3 }; 0 }", "scde_repfield.ray");
    compare("struct P { x: int, y: int } fn main() -> int { let p = P { x: true, y: 2 }; 0 }", "scde_fieldty.ray");
    compare("fn main() -> int { let n = 3; n.x }", "scde_field_nonst.ray");
    compare("struct P { x: int } fn main() -> int { let p = P { x: 1 }; p.z }", "scde_field_unk.ray");
    compare("struct P { x: int } fn main() -> int { var p = P { x: 1 }; p.x = true; 0 }", "scde_fassign_ty.ray");
    // Arreglos.
    compare("fn main() -> int { let xs = []; 0 }", "scde_empty.ray");
    compare("fn main() -> int { let xs = [1, true]; 0 }", "scde_arrty.ray");
    compare("fn main() -> int { let xs: [int] = [1]; xs[true] }", "scde_idxty.ray");
    compare("fn main() -> int { let n = 3; n[0] }", "scde_idx_nonarr.ray");
    compare("fn main() -> int { var xs: [int] = [1]; xs[0] = true; 0 }", "scde_iassign_ty.ray");
    // M48.4e: builtins de contenedor retirados → métodos de trait en Rust; el validador auto-alojado los
    // sigue modelando como builtins de arreglo. Ambos rechazan en el mismo sitio, con distinta redacción.
    compare_pos("fn main() -> int { var xs: [int] = []; xs.push(true); 0 }", "scde_push_ty.ray");
    compare_pos("fn main() -> int { (3).push(1); 0 }", "scde_push_nonarr.ray");
    compare_pos("fn main() -> int { (3).len() }", "scde_len_nonarr.ray");
    // Enums.
    compare("enum E { A } fn main() -> int { let x = E.C; 0 }", "scde_unkvariant.ray");
    compare("enum F { C(float) } fn main() -> int { let x = F.C(); 0 }", "scde_enum_arity.ray");
    compare("enum F { C(float) } fn main() -> int { let x = F.C(true); 0 }", "scde_payty.ray");
    // match.
    compare("fn main() -> int { match (3) { _ => 0 } }", "scde_match_nonen.ray");
    compare("enum E { A, B } fn main() -> int { match (E.A) { E.A => 1 } }", "scde_nonexh.ray");
    compare("enum E { A, B } fn main() -> int { match (E.A) { _ => 1, E.A => 2 } }", "scde_unreach.ray");
    compare("enum E { A, B } fn main() -> int { match (E.A) { E.A => 1, E.A => 2, E.B => 3 } }", "scde_covered.ray");
    compare("enum F { C(float) } fn main() -> int { match (F.C(1.0)) { F.C(a, b) => 0 } }", "scde_pat_arity.ray");
    compare("enum E { A, B } fn main() -> int { match (E.A) { E.A => 1, E.B => true } }", "scde_arm_ty.ray");
    compare("enum E { A } enum G { Z } fn main() -> int { match (E.A) { G.Z => 0 } }", "scde_pat_enum.ray");
    // Definiciones de tipos (el checker las detecta directamente; en el pipeline completo lo haría el loader).
    compare("enum E { A } enum E { B } fn main() -> int { 0 }", "scde_dup_enum.ray");
    compare("struct P { x: int } struct P { y: int } fn main() -> int { 0 }", "scde_dup_struct.ray");
    compare("enum E { A } struct E { x: int } fn main() -> int { 0 }", "scde_struct_enum.ray");
    compare("enum E { A, A } fn main() -> int { 0 }", "scde_repvariant.ray");
}

#[test]
fn generics_valid_vals() {
    // Función genérica mínima; inferencia de T desde el argumento.
    compare("fn id<T>(x: T) -> T { x } fn main() -> int { let a: int = id(5); a }", "scg_id.ray");
    // Genérica de orden superior (tipo función con T, U).
    compare("fn ap<T, U>(f: fn(T) -> U, x: T) -> U { f(x) } fn double(n: int) -> int { n * 2 } fn main() -> int { ap(double, 21) }", "scg_ap.ray");
    // Genérica sobre arreglos.
    compare("fn ultimo<T>(xs: [T]) -> T { xs[xs.len() - 1] } fn main() -> int { ultimo([1, 2, 3]) }", "scg_last.ray");
    compare("fn par<T>(a: T, b: T) -> [T] { [a, b] } fn main() -> int { let xs: [int] = par(10, 20); xs[0] }", "scg_par.ray");
}

#[test]
fn errors_generics() {
    compare("fn id<T>(x: T) -> T { x } fn main() -> int { id(1, 2) }", "scge_arity.ray");
    compare("fn par<T>(a: T, b: T) -> [T] { [a, b] } fn main() -> int { let xs = par(1, true); 0 }", "scge_consist.ray");
    compare("fn raro<T>(x: int) -> int { x } fn main() -> int { raro(3) }", "scge_uninfer.ray");
    compare("fn f<T, T>(x: T) -> T { x } fn main() -> int { 0 }", "scge_duptp.ray");
    compare("fn ap<T, U>(f: fn(T) -> U, x: T) -> U { f(x) } fn negar(b: bool) -> bool { !b } fn main() -> int { ap(negar, 3) }", "scge_fnarg.ray");
}

#[test]
fn types_generics_valid_vals() {
    // Struct genérico: infiere A, B de los valores; acceso a campo sustituye el tipo.
    compare("struct Par<A, B> { primero: A, segundo: B } fn main() -> int { let p: Par<int, bool> = Par { primero: 7, segundo: true }; if (p.segundo) { p.primero } else { 0 } }", "sct_par.ray");
    // Enum genérico: T del payload y T del tipo esperado (Vacia); match sustituye el payload.
    compare("enum Caja<T> { Llena(T), Vacia } fn val(c: Caja<int>, d: int) -> int { match (c) { Caja.Llena(v) => v, Caja.Vacia => d } } fn main() -> int { let a: Caja<int> = Caja.Llena(35); let b: Caja<int> = Caja.Vacia; val(a, 0) + val(b, 9) }", "sct_caja.ray");
    // Enum genérico recursivo + función genérica que lo recorre.
    compare("enum Lista<T> { Cons(T, Lista<T>), Nil } fn length<T>(xs: Lista<T>) -> int { match (xs) { Lista.Cons(_, r) => 1 + length(r), Lista.Nil => 0 } } fn main() -> int { let ns: Lista<string> = Lista.Cons(\"a\", Lista.Cons(\"b\", Lista.Nil)); length(ns) }", "sct_list.ray");
    // Inferencia local de un enum genérico (sin anotación) + match.
    compare("enum Caja<T> { Llena(T), Vacia } fn main() -> int { let c = Caja.Llena(5); match (c) { Caja.Llena(v) => v, Caja.Vacia => 0 } }", "sct_infer.ray");
}

#[test]
fn errors_types_generics() {
    compare("struct Par<A, B> { primero: A, segundo: B } fn main() -> int { let p: Par<int, bool> = Par { primero: true, segundo: true }; 0 }", "scte_fieldty.ray");
    compare("enum Caja<T> { Llena(T), Vacia } fn main() -> int { let c = Caja.Vacia; 0 }", "scte_empty.ray");
    compare("enum Caja<T> { Llena(T), Vacia } fn f(c: Caja<int, bool>) -> int { 0 } fn main() -> int { 0 }", "scte_arity.ray");
    compare("enum Par2<T> { P(T, T) } fn main() -> int { let x = Par2.P(1, true); 0 }", "scte_payincon.ray");
    compare("struct Caja<T> { v: T } fn f(c: Caja) -> int { 0 } fn main() -> int { 0 }", "scte_noargs.ray");
}

#[test]
fn prelude_y_try_valid_vals() {
    // Result del prelude: construcción + match (con el tipo esperado fijando T y E).
    compare("fn div(a: int, b: int) -> Result<int, string> { if (b == 0) { Result.Err(\"cero\") } else { Result.Ok(a / b) } } fn main() -> int { match (div(6, 2)) { Result.Ok(v) => v, Result.Err(_) => -1 } }", "scp_result.ray");
    // El operador `?` sobre Result.
    compare("fn div(a: int, b: int) -> Result<int, string> { if (b == 0) { Result.Err(\"cero\") } else { Result.Ok(a / b) } } fn ev(a: int, b: int) -> Result<int, string> { let p: int = div(a, b)?; Result.Ok(p + 1) } fn main() -> int { match (ev(6, 2)) { Result.Ok(v) => v, Result.Err(_) => -1 } }", "scp_try.ray");
    // Option del prelude + `?` sobre Option.
    compare("fn primero(xs: [int]) -> Option<int> { if (xs.len() == 0) { Option.None } else { Option.Some(xs[0]) } } fn main() -> int { match (primero([7])) { Option.Some(v) => v, Option.None => -1 } }", "scp_option.ray");
    compare("fn g(xs: [int]) -> Option<int> { if (xs.len() == 0) { Option.None } else { Option.Some(xs[0]) } } fn f(xs: [int]) -> Option<int> { let x: int = g(xs)?; Option.Some(x + 1) } fn main() -> int { match (f([3])) { Option.Some(v) => v, Option.None => -1 } }", "scp_tryopt.ray");
}

#[test]
fn errors_prelude_y_try() {
    compare("fn f() -> Result<int, string> { let x: int = 5?; Result.Ok(x) } fn main() -> int { 0 }", "scpe_nores.ray");
    compare("fn div() -> Result<int, string> { Result.Ok(1) } fn f() -> int { let x: int = div()?; x } fn main() -> int { 0 }", "scpe_badret.ray");
    compare("fn div() -> Result<int, string> { Result.Ok(1) } fn f() -> Result<int, bool> { let x: int = div()?; Result.Ok(x) } fn main() -> int { 0 }", "scpe_errmis.ray");
    compare("fn primero(xs: [int]) -> Option<int> { Option.Algun(xs[0]) } fn main() -> int { 0 }", "scpe_optvar.ray");
    compare("fn f() -> int { let r = Result.Ok(5); 0 } fn main() -> int { 0 }", "scpe_uninf.ray");
}

#[test]
fn ufcs_y_closures_valid_vals() {
    // UFCS: función libre como método (encadenada), receptor como primer argumento.
    compare("fn double(x: int) -> int { x * 2 } fn main() -> int { let v = 5; v.double().double() }", "scu_ufcs.ray");
    // Builtin como método.
    compare("fn main() -> int { let xs: [int] = [1, 2, 3]; xs.len() }", "scu_builtin.ray");
    // Campo de struct (de tipo función) gana sobre la función libre homónima.
    compare("struct M { op: fn(int) -> int } fn op(a: int, b: int) -> int { a - b } fn main() -> int { let m = M { op: fn(x: int) -> int { x + 1 } }; m.op(41) }", "scu_field.ray");
    // UFCS sobre una función genérica: el receptor cuenta para la inferencia.
    compare("fn head<T>(xs: [T]) -> T { xs[0] } fn main() -> int { let xs: [int] = [10, 20]; xs.head() }", "scu_gen.ray");
    // Función anónima como valor + captura del entorno (closure).
    compare("fn applies(f: fn(int) -> int, x: int) -> int { f(x) } fn main() -> int { let g = fn(n: int) -> int { n + 1 }; applies(g, 41) }", "scu_closure.ray");
    compare("fn main() -> int { let base = 10; let f = fn(x: int) -> int { x + base }; f(5) }", "scu_capture.ray");
}

#[test]
fn errors_ufcs_y_closures() {
    compare("fn main() -> int { let v = 5; v.nonexistent() }", "scue_noufcs.ray");
    compare("fn double(x: int) -> int { x * 2 } fn main() -> int { let b = true; b.double() }", "scue_ufcsty.ray");
    compare("fn main() -> int { let f = fn(x: int) -> int { true }; 0 }", "scue_anonret.ray");
}

#[test]
fn traits_valid_vals() {
    // Despacho estático sobre struct, enum y primitivo; Self en el retorno.
    compare("trait V { fn valor(self) -> int; } struct P { x: int } impl V for P { fn valor(self) -> int { self.x } } fn main() -> int { let p = P { x: 7 }; p.valor() }", "sctr_struct.ray");
    compare("trait V { fn valor(self) -> int; } enum M { Cara, Cruz } impl V for M { fn valor(self) -> int { match (self) { M.Cara => 1, M.Cruz => 0 } } } fn main() -> int { M.Cara.valor() }", "sctr_enum.ray");
    compare("trait V { fn valor(self) -> int; } impl V for int { fn valor(self) -> int { self } } fn main() -> int { 42.valor() }", "sctr_prim.ray");
    compare("struct P { x: int } trait Pt { fn double(self) -> Self; } impl Pt for P { fn double(self) -> Self { P { x: self.x + self.x } } } fn main() -> int { let p = P { x: 3 }; p.double().x }", "sctr_self.ray");
    // Método por defecto (heredado y redefinido).
    compare("trait S { fn name(self) -> string; fn greet(self) -> string { self.name() } } struct P { n: string } impl S for P { fn name(self) -> string { self.n } } fn main() -> int { let p = P { n: \"a\" }; print(p.greet()); 0 }", "sctr_default.ray");
    compare("trait S { fn name(self) -> string; fn greet(self) -> string { self.name() } } struct R { id: int } impl S for R { fn name(self) -> string { \"robot\" } fn greet(self) -> string { \"beep\" } } fn main() -> int { let r = R { id: 1 }; print(r.greet()); 0 }", "sctr_overr.ray");
}

#[test]
fn errors_traits() {
    compare("impl Foo for int { fn f(self) -> int { self } } fn main() -> int { 0 }", "sctre_notrait.ray");
    compare("trait V { fn valor(self) -> int; } struct P { x: int } impl V for P { } fn main() -> int { 0 }", "sctre_missing.ray");
    compare("trait V { fn valor(self) -> int; } struct P { x: int } impl V for P { fn valor(self) -> int { self.x } fn other(self) -> int { 0 } } fn main() -> int { 0 }", "sctre_extra.ray");
    compare("trait V { fn valor(self) -> int; } struct P { x: int } impl V for P { fn valor(self) -> bool { true } } fn main() -> int { 0 }", "sctre_sigret.ray");
    compare("trait V { fn valor(self) -> int; } struct P { x: int } impl V for P { fn valor(self) -> int { self.x } } fn main() -> int { let p = P { x: 1 }; p.nonexistent() }", "sctre_nomethod.ray");
    compare("struct T { x: int } trait T { fn f(self) -> int; } fn main() -> int { 0 }", "sctre_duptype.ray");
    compare("trait V { fn valor(self) -> int; } struct C<T> { v: T } impl V for C { fn valor(self) -> int { 0 } } fn main() -> int { 0 }", "sctre_genimpl.ray");
}

#[test]
fn bounds_valid_vals() {
    let t = "trait V { fn valor(self) -> int; } struct P { x: int } impl V for P { fn valor(self) -> int { self.x } } impl V for int { fn valor(self) -> int { self } } ";
    // Genérico acotado: x.valor() legal porque T: V; sirve para Punto y para int.
    compare(&format!("{t}fn dv<T: V>(x: T) -> int {{ x.valor() + x.valor() }} fn main() -> int {{ let p = P {{ x: 3 }}; dv(p) + dv(10) }}"), "scb_bound.ray");
    // Reenvío de diccionario: un genérico acotado llama a otro con su propio T.
    compare(&format!("{t}fn dv<T: V>(x: T) -> int {{ x.valor() }} fn res<T: V>(a: T, b: T) -> int {{ dv(a) + b.valor() }} fn main() -> int {{ let p = P {{ x: 3 }}; res(p, p) }}"), "scb_forward.ray");
    // Varios bounds a la vez (T: A + B).
    compare(&format!("{t}trait E {{ fn eti(self) -> string; }} impl E for P {{ fn eti(self) -> string {{ \"p\" }} }} fn des<T: V + E>(x: T) -> int {{ print(x.eti()); x.valor() }} fn main() -> int {{ des(P {{ x: 3 }}) }}"), "scb_multi.ray");
    // Método por defecto a través de un bound.
    compare("trait S { fn name(self) -> string; fn s(self) -> string { self.name() } } struct P { nom: string } impl S for P { fn name(self) -> string { self.nom } } fn an<T: S>(x: T) -> int { print(x.s()); 0 } fn main() -> int { an(P { nom: \"a\" }) }", "scb_default.ray");
}

#[test]
fn errors_bounds() {
    let t = "trait V { fn valor(self) -> int; } struct P { x: int } impl V for P { fn valor(self) -> int { self.x } } impl V for int { fn valor(self) -> int { self } } ";
    compare(&format!("{t}fn dv<T: V>(x: T) -> int {{ x.valor() }} fn main() -> int {{ dv(true) }}"), "scbe_unsat.ray");
    compare("fn f<T: NoExiste>(x: T) -> int { 0 } fn main() -> int { 0 }", "scbe_badbound.ray");
    compare("trait V { fn valor(self) -> int; } fn f<T: V>(x: int) -> int { 0 } fn g<U: V>(y: U) -> int { f(y) } fn main() -> int { 0 }", "scbe_boundtp.ray");
    compare(&format!("{t}fn dv<T: V>(x: T) -> int {{ x.other() }} fn main() -> int {{ 0 }}"), "scbe_nomethod.ray");
}

#[test]
fn impls_generics_valid_vals() {
    let m = "struct C<T> { v: T } trait M { fn measure(self) -> int; } impl M for int { fn measure(self) -> int { self } } impl<T: M> M for C<T> { fn measure(self) -> int { self.v.measure() + 1 } } ";
    // Impl genérico SIN bounds (el método no usa T): vale para cualquier C<T>.
    compare("struct C<T> { v: T } trait Cont { fn count(self) -> int; } impl<T> Cont for C<T> { fn count(self) -> int { 1 } } fn main() -> int { let ci = C { v: 42 }; let cs = C { v: \"x\" }; ci.count() + cs.count() }", "scgi_nobound.ray");
    // Impl genérico ACOTADO (el cuerpo usa T.medir()).
    compare(&format!("{m}fn main() -> int {{ let c = C {{ v: 10 }}; c.measure() }}"), "scgi_bound.ray");
    // Caja<Caja<int>>: diccionario anidado (satisfacción recursiva, validada en el sitio).
    compare(&format!("{m}fn main() -> int {{ let c2 = C {{ v: C {{ v: 100 }} }}; c2.measure() }}"), "scgi_nested.ray");
    // Pasar un Caja<int> a otro genérico acotado.
    compare(&format!("{m}fn md<X: M>(a: X) -> int {{ a.measure() }} fn main() -> int {{ let c = C {{ v: 10 }}; md(c) }}"), "scgi_pass.ray");
}

#[test]
fn errors_impls_generics() {
    compare("struct C<T> { v: T } trait V { fn f(self) -> int; } impl<T, U> V for C<T> { fn f(self) -> int { 0 } } fn main() -> int { 0 }", "scgie_arity.ray");
    compare("struct C<T> { v: T } trait V { fn f(self) -> int; } impl<T> V for C<int> { fn f(self) -> int { 0 } } fn main() -> int { 0 }", "scgie_shape.ray");
    compare("struct C<T> { v: T } trait V { fn f(self) -> int; } impl<T: NoExiste> V for C<T> { fn f(self) -> int { 0 } } fn main() -> int { 0 }", "scgie_implbnd.ray");
}

#[test]
fn dyn_valid_vals() {
    let f = "trait F { fn area(self) -> int; } struct C { l: int } impl F for C { fn area(self) -> int { self.l * self.l } } ";
    // Coerción concreto→dyn en un arreglo + despacho dinámico.
    compare(&format!("{f}fn total(fs: [dyn F]) -> int {{ var s = 0; var i = 0; while (i < fs.len()) {{ s = s + fs[i].area(); i = i + 1; }} s }} fn main() -> int {{ let fs: [dyn F] = [C {{ l: 3 }}, C {{ l: 2 }}]; total(fs) }}"), "scd_coerce.ray");
    // Método por defecto despachado sobre el objeto.
    compare("trait F { fn area(self) -> int; fn d(self) -> int { self.area() } } struct C { l: int } impl F for C { fn area(self) -> int { self.l } } fn main() -> int { let x: dyn F = C { l: 5 }; x.d() }", "scd_default.ray");
    // dyn multi-trait (dyn A + B).
    compare("trait A { fn a(self) -> int; } trait B { fn b(self) -> int; } struct S {} impl A for S { fn a(self) -> int { 1 } } impl B for S { fn b(self) -> int { 2 } } fn main() -> int { let x: dyn A + B = S {}; x.a() + x.b() }", "scd_multi.ray");
    // Upcasting dyn A + B → dyn A.
    compare("trait A { fn a(self) -> int; } trait B { fn b(self) -> int; } struct S {} impl A for S { fn a(self) -> int { 1 } } impl B for S { fn b(self) -> int { 2 } } fn f(x: dyn A) -> int { x.a() } fn main() -> int { let x: dyn A + B = S {}; f(x) }", "scd_upcast.ray");
}

#[test]
fn errors_dyn() {
    let f = "trait F { fn area(self) -> int; } struct C { l: int } impl F for C { fn area(self) -> int { self.l * self.l } } ";
    compare(&format!("{f}struct D {{}} fn main() -> int {{ let x: dyn F = D {{}}; 0 }}"), "scde_noimpl.ray");
    compare(&format!("{f}fn main() -> int {{ let x: dyn F = C {{ l: 1 }}; x.nope() }}"), "scde_nomethod.ray");
    compare("struct S {} fn main() -> int { let x: dyn NoExiste = S {}; 0 }", "scde_notrait.ray");
    compare("trait F { fn dup(self) -> Self; } struct C {} impl F for C { fn dup(self) -> Self { C {} } } fn main() -> int { let x: dyn F = C {}; x.dup(); 0 }", "scde_objsafe.ray");
    compare("trait A { fn a(self) -> int; } trait B { fn b(self) -> int; } struct S {} impl A for S { fn a(self) -> int { 1 } } impl B for S { fn b(self) -> int { 2 } } fn f(x: dyn A + B) -> int { x.a() } fn main() -> int { let x: dyn A = S {}; f(x) }", "scde_baddown.ray");
}

#[test]
fn prelude_map_filter_fold() {
    let d = "fn double(x: int) -> int { x * 2 } fn par(x: int) -> bool { x % 2 == 0 } fn sum(a: int, b: int) -> int { a + b } ";
    // map/filter/fold del prelude, vía UFCS encadenado.
    compare(&format!("{d}fn main() -> int {{ let xs: [int] = [1, 2, 3]; xs.map(double).filter(par).fold(0, sum) }}"), "scpre_chain.ray");
    // Vía pipelines.
    compare(&format!("{d}fn main() -> int {{ let xs: [int] = [1, 2, 3, 4]; xs |> filter(par) |> map(double) |> fold(0, sum) }}"), "scpre_pipe.ray");
    // Con closures inline.
    compare(&format!("{d}fn main() -> int {{ let xs: [int] = [1, 2, 3]; xs |> map(fn(x: int) -> int {{ x * x }}) |> fold(0, sum) }}"), "scpre_clos.ray");
    // El usuario puede redefinir 'map' (override del prelude).
    compare("fn map(x: int) -> int { x + 1 } fn main() -> int { map(5) }", "scpre_override.ray");
    // Error: el predicado/función no casa con el elemento.
    compare("fn negar(b: bool) -> bool { !b } fn main() -> int { let xs: [int] = [1, 2]; let ys = xs.map(negar); 0 }", "scpre_mapty.ray");
}

#[test]
fn derive_y_annotations_valid_vals() {
    // @derive(Eq): .eq() sobre struct.
    compare("@derive(Eq) struct P { x: int } fn main() -> int { let a = P { x: 1 }; let b = P { x: 1 }; if (a.eq(b)) { 1 } else { 0 } }", "scan_eq.ray");
    // @derive(Show): .show() sobre enum.
    compare("@derive(Show) enum C { Rojo, Azul } fn main() -> int { print(C.Rojo.show()); 0 }", "scan_show.ray");
    // @derive(Eq, Show) a la vez.
    compare("@derive(Eq, Show) struct P { x: int, y: int } fn main() -> int { let p = P { x: 1, y: 2 }; print(p.show()); if (p.eq(p)) { 1 } else { 0 } }", "scan_both.ray");
    // El tipo derivado satisface un bound T: Eq.
    compare("@derive(Eq) enum C { A, B } fn ci<T: Eq>(a: T, b: T) -> int { if (a.eq(b)) { 1 } else { 0 } } fn main() -> int { ci(C.A, C.A) }", "scan_bound.ray");
    // Eq/Show del prelude sobre primitivos.
    compare("fn main() -> int { print(42.show()); if (1.eq(1)) { 0 } else { 1 } }", "scan_prim.ray");
    // @test (función de prueba).
    compare("@test fn t() -> bool { true } fn main() -> int { 0 }", "scan_test.ray");
}

#[test]
fn errors_derive_y_annotations() {
    compare("@frobnicate fn f() -> int { 0 } fn main() -> int { 0 }", "scane_unk.ray");
    compare("@test fn t(x: int) -> bool { true } fn main() -> int { 0 }", "scane_testpar.ray");
    compare("@test fn t() -> int { 0 } fn main() -> int { 0 }", "scane_testret.ray");
    compare("@derive(Eq) fn f() -> int { 0 } fn main() -> int { 0 }", "scane_derfn.ray");
    compare("@test struct S {} fn main() -> int { 0 }", "scane_testty.ray");
    compare("@derive(Frob) struct S { x: int } fn main() -> int { 0 }", "scane_derunk.ray");
    compare("@derive(Eq) struct C<T> { v: T } fn main() -> int { 0 }", "scane_dergen.ray");
    compare("@derive() struct S { x: int } fn main() -> int { 0 }", "scane_derempty.ray");
}

#[test]
fn panic_y_parse() {
    // M14.6c-1: válidos.
    // parse_int/parse_float (envoltorios del prelude) → Option<int>/Option<float>.
    compare("fn main() -> int { match (parse_int(\"5\")) { Option.Some(n) => n, Option.None => 0 } }", "scpp_pi.ray");
    compare("fn main() -> int { match (parse_float(\"5.0\")) { Option.Some(f) => 1, Option.None => 0 } }", "scpp_pf.ray");
    // panic como expresión divergente: cede el tipo al otro brazo del if.
    compare("fn pos(n: int) -> int { if (n >= 0) { n } else { panic(\"neg\") } } fn main() -> int { pos(3) }", "scpp_pan.ray");
    // Errores (mensajes byte-idénticos a Rust).
    compare("fn main() -> int { panic(5); 0 }", "scpp_pantype.ray");
    compare("fn main() -> int { panic(); 0 }", "scpp_panarity.ray");
    compare("fn main() -> int { match (parse_int(42)) { Option.Some(n) => n, Option.None => 0 } }", "scpp_pitype.ray");
    // M14.6c-2: assert/assert_eq/sort (sobre los traits del prelude Eq/Show/Ord).
    compare("fn main() -> int { assert(true); 0 }", "scpp_assert.ray");
    compare("fn main() -> int { assert_eq(1, 1); 0 }", "scpp_asserteq.ray");
    compare("fn main() -> int { let xs = sort([3, 1, 2]); xs[0] }", "scpp_sort.ray");
    compare("fn main() -> int { let xs = sort([\"b\", \"a\"]); 0 }", "scpp_sortstr.ray");
    // sort sobre un tipo sin Ord → error de bound (mensaje byte-idéntico a Rust).
    compare("struct P { v: int } fn main() -> int { let xs = sort([P { v: 1 }]); 0 }", "scpp_sortnoord.ray");
    // M14.6d (M50.1: fs → std/fs; el oráculo es pre-loader → usa los primitivos __x): __read_file/
    // __write_file (arreglo etiquetado [string]) + __exists (bool).
    compare("fn main() -> int { let r = __read_file(\"x\"); if (r[0] == \"ok\") { 0 } else { 1 } }", "scpp_rf.ray");
    compare("fn main() -> int { let r = __write_file(\"x\", \"y\"); if (r[0] == \"ok\") { 0 } else { 1 } }", "scpp_wf.ray");
    compare("fn main() -> int { if (__exists(\"x\")) { 1 } else { 0 } }", "scpp_exists.ray");
    // Error de tipo en __exists (mensaje byte-idéntico a Rust).
    compare("fn main() -> int { if (__exists(5)) { 1 } else { 0 } }", "scpp_existstype.ray");
}

/// El test fuerte: los ejemplos reales deben dar el mismo veredicto (`ok`) que Rust.
#[test]
fn real_examples_valid_values() {
    let files = ["examples/basics/fib.ray", "examples/basics/fizzbuzz.ray", "examples/basics/gcd.ray", "examples/basics/primes.ray",
        "examples/data/structs.ray", "examples/data/match_figuras.ray", "examples/data/enums.ray", "examples/data/arrays.ray",
        "examples/data/matriz.ray", "examples/types/genericos.ray", "examples/types/tipos_genericos.ray", "examples/types/opcional.ray",
        "examples/types/errores.ray", "examples/stdlib/ufcs.ray", "examples/stdlib/closures.ray", "examples/types/traits.ray",
        "examples/types/bounds.ray", "examples/types/metodos_por_defecto.ray", "examples/types/impls_genericos.ray",
        "examples/types/trait_objects.ray", "examples/stdlib/stdlib.ray", "examples/types/anotaciones.ray"];
    for rel in files {
        let src = std::fs::read_to_string(repo_path(rel)).unwrap_or_else(|e| panic!("lee {rel}: {e}"));
        let expected = canonical(&src);
        let name_tmp = format!("sc_real_{}.ray", rel.replace('/', "_"));
        let obtained = check_dump(&src, &name_tmp);
        assert_eq!(obtained, expected, "el checker auto-alojado difiere del oráculo en {rel}");
    }
}

/// M87 — el diagnóstico del gotcha §55, byte-idéntico entre checkers: una cola que
/// empieza con `(`/`[` tras un if/while/match-sentencia se parsea como llamada/indexación
/// de su valor; el error lo dice, con la pista de separarla con 'return' o 'let'.
#[test]
fn gotcha_55_has_a_hint() { // es-ok: "gotcha" es jerga técnica inglesa aceptada, no español
    // Llamada: el "callee" es un while-expresión (unit).
    compare(
        "fn main() { var i = 0; while (i < 3) { i = i + 1; } (2); }",
        "sc_g55_call.ray",
    );
    // Llamada con un if-sentencia como callee.
    compare("fn main() { if (true) { print(1); } (2); }", "sc_g55_if.ray");
    // Indexación: la cola `[0]` tras el if.
    compare("fn main() { if (true) { print(1); } [0]; }", "sc_g55_index.ray");
    // Control: llamar un valor no-función SIN forma de bloque NO lleva la pista.
    compare("fn main() { let x = 3; x(1); }", "sc_g55_plain.ray");
}
