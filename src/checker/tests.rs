//! Tests de `checker` (movimiento puro; usar `git log --follow`).

use super::*;

/// Lexea, parsea y verifica un fuente completo.
fn check_src(src: &str) -> Result<(), TypeError> {
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    check(&mut prog)
}

/// M45: etiquetas de `member_completion` sobre una fuente que YA lleva el centinela.
fn members(src: &str) -> Vec<String> {
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let (mut prog, _) = crate::parser::parse_all(tokens);
    member_completion(&mut prog).into_iter().map(|m| m.label).collect()
}

/// M52: recolecta los nombres de callee (Ident) de todas las llamadas del programa verificado.
fn call_targets(src: &str) -> Vec<String> {
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    check(&mut prog).expect("check ok");
    fn walk_block(b: &Block, acc: &mut Vec<String>) {
        for s in &b.statements {
            match &s.kind {
                StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } | StmtKind::Expr(value) => {
                    walk_expr(value, acc)
                }
                StmtKind::Assign { target, value } => {
                    walk_expr(target, acc);
                    walk_expr(value, acc);
                }
                StmtKind::Return { value: Some(v) } => walk_expr(v, acc),
                StmtKind::For { body, .. } => walk_block(body, acc),
                _ => {}
            }
        }
        if let Some(t) = &b.tail {
            walk_expr(t, acc);
        }
    }
    fn walk_expr(e: &Expr, acc: &mut Vec<String>) {
        match &e.kind {
            ExprKind::Call { callee, args } => {
                if let ExprKind::Ident(n) = &callee.kind {
                    acc.push(n.clone());
                }
                walk_expr(callee, acc);
                for a in args {
                    walk_expr(a, acc);
                }
            }
            ExprKind::Binary { left, right, .. } => {
                walk_expr(left, acc);
                walk_expr(right, acc);
            }
            ExprKind::While { cond, body } => {
                walk_expr(cond, acc);
                walk_block(body, acc);
            }
            ExprKind::Block(b) => walk_block(b, acc),
            ExprKind::If { cond, then_branch, else_branch } => {
                walk_expr(cond, acc);
                walk_block(then_branch, acc);
                if let Some(e2) = else_branch {
                    walk_expr(e2, acc);
                }
            }
            _ => {}
        }
    }
    // Solo `main`: el prelude contiene los cuerpos forwarder (que llaman `__x` legítimamente).
    let mut acc = Vec::new();
    for f in prog.functions.iter().filter(|f| f.name == "main") {
        walk_block(&f.body, &mut acc);
    }
    acc
}

#[test]
fn inline_forwarders_lower_push_and_len_to_builtin() {
    // M52: `a.push(i)` / `a.len()` (métodos de trait forwarder de M48.4) deben quedar
    // reescritos a la llamada al builtin (`__push`/`__len`), no al método manglado.
    let targets =
        call_targets("fn main() -> int {\n  var a: [int] = [];\n  a.push(1);\n  a.len()\n}");
    assert!(targets.iter().any(|t| t == "__push"), "push inlineado: {targets:?}");
    assert!(targets.iter().any(|t| t == "__len"), "len inlineado: {targets:?}");
    assert!(!targets.iter().any(|t| t.ends_with("#push") || t.ends_with("#len")),
        "sin calls al forwarder: {targets:?}");
}

#[test]
fn inline_forwarders_respects_a_shadowing_local() {
    // M52 (guarda de sonoridad): si el programa liga una variable `__push`, el inlining hacia
    // ese nombre se desactiva (el compilador resuelve local antes que builtin) y la llamada
    // sigue yendo al método manglado. `__len` no está ligado → sí se inlinea.
    let targets = call_targets(
        "fn main() -> int {\n  let __push = 5;\n  var a: [int] = [];\n  a.push(__push);\n  a.len()\n}",
    );
    assert!(!targets.iter().any(|t| t == "__push"), "push NO inlineado: {targets:?}");
    assert!(targets.iter().any(|t| t.ends_with("#push")), "va al forwarder: {targets:?}");
    assert!(targets.iter().any(|t| t == "__len"), "len sí inlineado: {targets:?}");
}

#[test]
fn hover_of_associated_function() {
    // M48.1/LSP: hover sobre el nombre asociado (`new`/`bounded`) → su firma del registro.
    let src = "fn main() -> int {\n  let m: Map<string, int> = Map.new();\n  m.len()\n}";
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    let idx = semantic_index(&mut prog);
    assert!(idx.hovers.iter().any(|h| h.line == 2 && h.text == "Map.new() -> Map<K, V>"),
        "hover de Map.new: {:?}", idx.hovers.iter().filter(|h| h.line == 2).map(|h| &h.text).collect::<Vec<_>>());
}

#[test]
fn trait_len_for_builtin_types() {
    // M48.4a: el trait `Len` (prelude) se implementa para string/[T]/Map/bytes → `.len()` despacha
    // por trait; funciona con un bound `T: Len` y con un tipo de usuario que lo implemente.
    assert!(check_src("fn main() -> int { \"hello\".len() + [1,2,3].len() + \"a\".to_bytes().len() }").is_ok());
    assert!(check_src("fn f<T: Len>(x: T) -> int { x.len() }\nfn main() -> int { f([1,2]) + f(\"ab\") }").is_ok());
    assert!(check_src("fn main() -> int { let m: Map<int, int> = [1: 2]; m.len() }").is_ok());
    // Un tipo del usuario puede implementar Len y usarse con el bound.
    assert!(check_src(
        "struct P { d: [int] }\nimpl Len for P { fn len(self) -> int { self.d.len() } }\n\
         fn f<T: Len>(x: T) -> int { x.len() }\nfn main() -> int { f(P { d: [1,2,3] }) }").is_ok());
    // Un tipo SIN Len no satisface el bound.
    let e = check_src("struct Q { x: int }\nfn f<T: Len>(x: T) -> int { x.len() }\nfn main() -> int { f(Q { x: 1 }) }").unwrap_err();
    assert!(format!("{e}").contains("Len"), "Q no implementa Len: {e}");
}

#[test]
fn traits_strops_bytesops() {
    // M48.4d: los métodos de string/bytes despachan por trait (StrOps/BytesOps).
    assert!(check_src(
        "fn main() -> int { let s = \"hello\"; \
         s.trim().split(\",\").len() + s.to_upper().len() + s.substring(0, 2).len() \
         + s.to_bytes().sub_bytes(0, 1).len() + s.chars().len() }").is_ok());
    // to_upper sobre un no-string → error.
    let e = check_src("fn main() -> int { (42).to_upper().len() }").unwrap_err();
    assert!(!format!("{e}").is_empty(), "int no has to_upper: {e}");
}

#[test]
fn trait_mapops() {
    // M48.4c: insert/contains_key/keys/values como métodos del trait MapOps.
    assert!(check_src(
        "fn main() -> int { let m: Map<int, int> = [1: 10]; m.insert(2, 20); \
         m.keys().len() + m.values()[0] + (if (m.contains_key(1)) { 1 } else { 0 }) }").is_ok());
    // clave del tipo equivocado → error.
    let e = check_src("fn main() { let m: Map<int, int> = [:]; m.insert(\"x\", 1); }").unwrap_err();
    assert!(format!("{e}").contains("clave") || format!("{e}").contains("int"), "{e}");
}

#[test]
fn traits_push_reverse_contains() {
    // M48.4b: los tres traits despachan como método; extensibles a tipos de usuario.
    assert!(check_src("fn main() -> int { var a = [1,2]; a.push(3); a.reverse().len() }").is_ok());
    assert!(check_src("fn ok(b: bool) -> int { if (b) { 1 } else { 0 } }\nfn main() -> int { ok([1,2,3].contains(2)) + ok(\"hello\".contains(\"la\")) }").is_ok());
    // Un tipo del usuario implementa Push<int>/Contains<int>.
    assert!(check_src(
        "struct C { d: [int] }\nimpl Push<int> for C { fn push(self, x: int) { self.d.push(x) } }\n\
         fn main() { let c = C { d: [] }; c.push(5); }").is_ok());
    // contains con el tipo de elemento equivocado → error.
    let e = check_src("fn main() -> int { if ([1,2,3].contains(\"x\")) { 1 } else { 0 } }").unwrap_err();
    assert!(format!("{e}").contains("string") || format!("{e}").contains("contains"), "{e}");
}

#[test]
fn impl_for_builtin_ty_must_be_generic() {
    // M48.4a: `impl X for [int]` (no plenamente genérico) se rechaza, como `impl X for Caja<int>`.
    let e = check_src("trait X { fn m(self) -> int; }\nimpl X for [int] { fn m(self) -> int { 0 } }\nfn main() -> int { 0 }").unwrap_err();
    assert!(format!("{e}").contains("distinct type parameters") || format!("{e}").contains("expects 1 type parameter"), "{e}");
}

#[test]
fn redefine_builtin_is_error() {
    // M48.3: un builtin del núcleo (print/to_string/panic…) no puede redefinirse como función libre.
    for name in ["print", "to_string", "panic"] {
        let src = format!("fn {name}(x: int) -> int {{ x }}\nfn main() -> int {{ 0 }}");
        let e = check_src(&src).unwrap_err();
        assert!(format!("{e}").contains(&format!("'{name}' is a language builtin")),
            "redefine {name}: {e}");
    }
    // M48.4e: los builtins de contenedor RETIRADOS (len/push/… → ahora métodos de trait) dejaron el
    // namespace libre → una función libre con ese nombre YA es legal (el footgun no dispara).
    for name in ["len", "push", "insert", "keys", "reverse", "contains", "split", "chars"] {
        let src = format!("fn {name}(x: int) -> int {{ x }}\nfn main() -> int {{ {name}(1) }}");
        assert!(check_src(&src).is_ok(), "'{name}' as a free function must now compile");
    }
    // Una función del PRELUDE (map/filter/fold/sort) SÍ puede redefinirse (override).
    assert!(check_src("fn map(x: int) -> int { x + 1 }\nfn main() -> int { map(5) }").is_ok());
    assert!(check_src("fn sort(x: int) -> int { x }\nfn main() -> int { sort(3) }").is_ok());
    // Y un nombre normal, obviamente, es válido.
    assert!(check_src("fn fold(x: int) -> int { x * 2 }\nfn main() -> int { fold(2) }").is_ok());
}

#[test]
fn map_literal() {
    // M48.2: `[k: v]` infiere `Map<K,V>`; `[:]` lo fija el esperado.
    assert!(check_src("fn main() -> int { let m = [1: \"a\", 2: \"b\"]; m.len() }").is_ok());
    assert!(check_src("fn main() { let m: Map<string, int> = [:]; }").is_ok());
    assert!(check_src("fn main() { let m: Map<int, string> = [1: \"a\"]; }").is_ok());
    // `[:]` sin anotar → error de "anota el tipo".
    let e = check_src("fn main() -> int { let m = [:]; 0 }").unwrap_err();
    assert!(format!("{e}").contains("cannot infer the type of [:]"), "{e}");
    // Claves/valores heterogéneos → error.
    let k = check_src("fn main() -> int { let m = [1: \"a\", \"b\": \"c\"]; 0 }").unwrap_err();
    assert!(format!("{k}").contains("the Map keys must be of the same type"), "{k}");
    let v = check_src("fn main() -> int { let m = [1: \"a\", 2: 3]; 0 }").unwrap_err();
    assert!(format!("{v}").contains("the Map values must be of the same type"), "{v}");
    // Clave no hashable (float) → error.
    let f = check_src("fn main() -> int { let m = [1.5: \"a\"]; 0 }").unwrap_err();
    assert!(format!("{f}").contains("Map key"), "{f}");
    // Contra un esperado que no es Map → error de tipos del `let`.
    assert!(check_src("fn main() { let xs: [int] = [1: 2]; }").is_err());
}

#[test]
fn associated_functions_of_ty() {
    // M48.1: `Map.new()`/`Channel.new()`/`Channel.bounded(n)` — el tipo lo fija el esperado.
    assert!(check_src("fn main() -> int { let m: Map<string, int> = Map.new(); m.len() }").is_ok());
    assert!(check_src("fn main() { let c: Channel<int> = Channel.new(); }").is_ok());
    assert!(check_src("fn main() { let c: Channel<int> = Channel.bounded(2); }").is_ok());
    // Sin tipo esperado → error de "anota el tipo".
    let e = check_src("fn main() -> int { let m = Map.new(); 0 }").unwrap_err();
    assert!(format!("{e}").contains("cannot infer the type of 'Map.new'"), "{e}");
    // Aridad: `Map.new` no recibe argumentos; `Channel.bounded` exige uno.
    let a = check_src("fn main() { let m: Map<int, int> = Map.new(1); }").unwrap_err();
    assert!(format!("{a}").contains("expects 0 argument(s)"), "{a}");
    // El argumento de `bounded` debe ser int.
    let b = check_src("fn main() { let c: Channel<int> = Channel.bounded(\"x\"); }").unwrap_err();
    assert!(format!("{b}").contains("must be int"), "{b}");
    // El tipo esperado debe casar la familia (Map.new no produce un Channel).
    let f = check_src("fn main() { let c: Channel<int> = Map.new(); }").unwrap_err();
    assert!(format!("{f}").contains("produces a Map"), "{f}");
}

#[test]
fn member_completion_fields_methods_y_builtins() {
    // Struct: campos + método de trait + UFCS del usuario; kinds correctos.
    let src = "struct P { x: int, y: int }\ntrait Ver { fn see(self) -> int; }\nimpl Ver for P { fn see(self) -> int { self.x } }\nfn fold(p: P) -> int { p.x }\nfn main() -> int { let p = P { x: 1, y: 2 }; p.__raycomplete__; 0 }\n";
    let m = members(src);
    for expected in ["x", "y", "see", "fold"] {
        assert!(m.contains(&expected.to_string()), "falta '{expected}': {m:?}");
    }
    // string: builtins de string, sin funciones de E/S sobre una ruta string.
    let s = members("fn main() -> int { let s = \"h\"; s.__raycomplete__; 0 }");
    assert!(s.contains(&"trim".to_string()) && s.contains(&"split".to_string()), "{s:?}");
    assert!(!s.contains(&"read_file".to_string()), "sin E/S about string: {s:?}");
    // array: builtins + orden superior del prelude.
    let a = members("fn main() -> int { let xs = [1,2,3]; xs.__raycomplete__; 0 }");
    for expected in ["len", "push", "map", "filter", "fold", "sort"] {
        assert!(a.contains(&expected.to_string()), "array falta '{expected}': {a:?}");
    }
    // receptor sin tipo conocido → sin miembros (sin pánico).
    assert!(members("fn main() -> int { unknown.__raycomplete__; 0 }").is_empty());
}

/// Atajo: ¿el mensaje de error contiene esta subcadena?
fn err_contains(src: &str, needle: &str) {
    let e = check_src(src).expect_err("debería fallar la verificación");
    assert!(
        e.msg.contains(needle),
        "mensaje '{}' no contiene '{}'",
        e.msg,
        needle
    );
}

#[test]
fn assigning_to_tuple_position_is_error() {
    // M34 (SPEC §5): las posiciones de tupla son de solo lectura. Antes esto era
    // un ICE (pasaba el checker sin bajarse y reventaba ambos motores).
    err_contains(
        "fn main() -> int { var t = (1, 2); t.0 = 9; t.0 }",
        "a tuple position is not assignable",
    );
    // La lectura y la desestructuración siguen funcionando.
    check_src("fn main() -> int { let t = (1, 2); let (a, b) = t; a + b + t.0 }")
        .expect("leer y desestructurar es válido");
}

    #[test]
fn check_all_accumulates_per_function() {
    // M33c: un error por cuerpo, todos reportados; el primero idéntico al fail-fast.
    let src = "fn f() -> int { 1 + true }\nfn g() -> int { \"x\" * 2 }\nfn main() -> int { f() + g() }";
    let toks = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(toks).expect("parse ok");
    let mut prog2 = prog.clone();
    let errs = check_all(&mut prog2);
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert!(errs[0].msg.contains("int and bool"), "{}", errs[0].msg);
    assert!(errs[1].msg.contains("string and int"), "{}", errs[1].msg);
    let solo = check(&mut prog).unwrap_err();
    assert_eq!(errs[0], solo, "el primer error must ser byte-idéntico (oráculos)");
}

#[test]
fn check_all_with_broken_early_pass_gives_one_error() {
    // Un error de la pre-pasada (función duplicada) es fail-fast → exactamente uno,
    // aunque haya además errores de cuerpo más abajo.
    let src = "fn f() -> int { 0 }\nfn f() -> int { 1 }\nfn g() -> int { 1 + true }\nfn main() -> int { 0 }";
    let toks = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(toks).expect("parse ok");
    let errs = check_all(&mut prog);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].msg.contains("declared twice"), "{}", errs[0].msg);
    // Y un tipo desconocido en una FIRMA se valida en la fase de cuerpos → acumula
    // junto a los demás (mejor: más errores de una tacada).
    let src = "fn f(a: NoExiste) -> int { 0 }\nfn g() -> int { 1 + true }\nfn main() -> int { 0 }";
    let toks = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(toks).expect("parse ok");
    let errs = check_all(&mut prog);
    assert_eq!(errs.len(), 2, "{errs:?}");
}

    #[test]
fn type_error_underlines_the_whole_expression() {
    // M33a-2: la extensión del error sale de la tabla de spans del parser.
    let e = check_src("fn main() -> int { let x = 1 + true; x }").unwrap_err();
    assert!(e.msg.contains("requires both operands"), "{}", e.msg);
    assert_eq!(e.len, "1 + true".chars().count(), "underscores la expresión entera");
    // Un argumento de tipo equivocado subraya el argumento (pasa por expression()).
    let e = check_src("fn f(a: int) -> int { a }\nfn main() -> int { f(\"dos\") }").unwrap_err();
    assert!(e.msg.contains("expected int"), "{}", e.msg);
    assert_eq!(e.len, "\"dos\"".chars().count());
}

// M9.4: bounds en parámetros de tipo de struct/enum (verificados en la construcción).
const BOUND_PRELUDE: &str = r#"
trait Show2 { fn see(self) -> string; }
struct P { n: int }
impl Show2 for P { fn see(self) -> string { "P" } }
struct Q { n: int }
"#;

#[test]
fn bound_struct_ok_con_impl() {
    let src = format!("{}struct Box<T: Show2> {{ v: T }}\nfn main() -> int {{ let c = Box {{ v: P {{ n: 1 }} }}; c.v.see(); 0 }}\n", BOUND_PRELUDE);
    check_src(&src).expect("P implementa Show2");
}

#[test]
fn bound_struct_fails_without_impl() {
    let src = format!("{}struct Box<T: Show2> {{ v: T }}\nfn main() -> int {{ let c = Box {{ v: Q {{ n: 1 }} }}; 0 }}\n", BOUND_PRELUDE);
    err_contains(&src, "requires that 'T' be 'Show2'");
}

#[test]
fn bound_struct_propagates_a_function_generic() {
    // Construir Box<U> exige que U lleve el bound: sin él, error; con él, OK.
    let bad = format!("{}struct Box<T: Show2> {{ v: T }}\nfn env<U>(x: U) -> Box<U> {{ Box {{ v: x }} }}\nfn main() -> int {{ 0 }}\n", BOUND_PRELUDE);
    err_contains(&bad, "requires that 'T' be 'Show2'");
    let good = format!("{}struct Box<T: Show2> {{ v: T }}\nfn env<U: Show2>(x: U) -> Box<U> {{ Box {{ v: x }} }}\nfn main() -> int {{ 0 }}\n", BOUND_PRELUDE);
    check_src(&good).expect("con U: Show2 la propagación se satisface");
}

#[test]
fn bound_enum_fails_without_impl() {
    let src = format!("{}enum Opt<T: Show2> {{ Nada, Algo(T) }}\nfn main() -> int {{ let x = Opt.Algo(Q {{ n: 1 }}); 0 }}\n", BOUND_PRELUDE);
    err_contains(&src, "requires that 'T' be 'Show2'");
}

#[test]
fn bound_struct_nonexistent_trait_is_error() {
    err_contains("struct Box<T: NoExiste> { v: T }\nfn main() -> int { 0 }\n", "trait 'NoExiste' not declared");
}

#[test]
fn fib_is_valid() {
    let src = r#"
fn fib(n: int) -> int {
if (n < 2) { n } else { fib(n - 1) + fib(n - 2) }
}
fn main() -> int {
var i: int = 0;
while (i < 10) {
    print(fib(i));
    i = i + 1;
}
0
}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn mixed_arithmetic_fails() {
    err_contains("fn main() -> int { 1 + true }", "requires both operands");
    err_contains("fn main() { let x: float = 1 + 2.0; }", "requires both operands");
}

#[test]
fn condition_must_be_bool() {
    err_contains("fn main() { if (1) { } }", "if condition must be bool");
    err_contains("fn main() { while (1) { } }", "while condition must be bool");
}

#[test]
fn ramas_del_if_same_ty() {
    err_contains(
        "fn main() -> int { if (true) { 1 } else { true } }",
        "if branches have different types",
    );
}

#[test]
fn if_without_else_must_be_unit() {
    err_contains("fn main() { if (true) { 5 } }", "without else has type unit");
}

#[test]
fn assigning_to_let_fails_but_to_var_ok() {
    err_contains(
        "fn main() { let x: int = 0; x = 1; }",
        "it is immutable",
    );
    assert!(check_src("fn main() { var x: int = 0; x = 1; }").is_ok());
}

#[test]
fn undeclared_variable() {
    err_contains("fn main() -> int { x }", "not declared");
    err_contains("fn main() { y = 1; }", "not declared");
}

#[test]
fn declaration_ty_must_match() {
    err_contains("fn main() { let x: int = true; }", "initialized with bool");
}

#[test]
fn return_val_incorrect() {
    err_contains("fn f() -> int { true } fn main() {}", "produces bool");
    err_contains("fn g() -> int { return true; } fn main() {}", "returning bool");
}

#[test]
fn early_return_without_final_value_is_valid() {
    // Gracias al análisis de divergencia, esto es válido aunque no tenga
    // expresión final: todos los caminos retornan.
    let src = r#"
fn sign(x: int) -> int {
if (x < 0) { return -1; } else { return 1; }
}
fn main() -> int { sign(3) }
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn calls_validan_arity_y_types() {
    err_contains(
        "fn add(a: int, b: int) -> int { a + b } fn main() -> int { add(1) }",
        "expects 2 argument",
    );
    err_contains(
        "fn add(a: int, b: int) -> int { a + b } fn main() -> int { add(1, true) }",
        "expected int, got bool",
    );
    err_contains("fn main() -> int { desconocida() }", "not declared");
}

#[test]
fn print_builtin() {
    assert!(check_src("fn main() { print(42); print(\"hello\"); print(true); }").is_ok());
    err_contains("fn main() { print(); }", "expects 1 argument");
    err_contains("fn main() { print(1, 2); }", "expects 1 argument");
}

#[test]
fn main_is_mandatory_and_well_formed() {
    err_contains("fn other() -> int { 0 }", "missing entry function 'main'");
    err_contains("fn main(x: int) -> int { x }", "must not take parameters");
    err_contains("fn main() -> bool { true }", "must return int or unit");
}

#[test]
fn shadowing_en_block_internal() {
    // Una variable interior puede tapar a una exterior con otro tipo.
    let src = r#"
fn main() -> int {
let x: int = 1;
{ let x: bool = true; print(x); }
x
}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn function_not_declared_twice() {
    err_contains("fn f() {} fn f() {} fn main() {}", "declared twice");
}

// ----- M3.1: arreglos -----

#[test]
fn arrays_valid_vals() {
    assert!(check_src("fn main() -> int { let a: [int] = [1, 2, 3]; a[0] }").is_ok());
    assert!(check_src("fn main() -> int { let a: [int] = []; a.push(1); a.len() }").is_ok());
    assert!(check_src("fn main() { var a: [int] = [1]; a[0] = 9; }").is_ok());
    // Arreglos anidados.
    assert!(check_src("fn main() -> int { let m: [[int]] = [[1, 2], [3, 4]]; m[1][0] }").is_ok());
}

#[test]
fn array_type_errors() {
    err_contains("fn main() -> int { let a: [int] = [1, true]; a[0] }", "must be int");
    err_contains("fn main() -> int { let a: [int] = [1]; a[true] }", "index must be int");
    err_contains("fn main() -> int { let x: int = 5; x[0] }", "not an array");
    err_contains("fn main() { let x: int = []; }", "cannot infer");
    err_contains("fn main() -> int { let a: [int] = [1]; a[0] = true; a[0] }", "is assigned bool");
    err_contains("fn main() -> int { 5.len() }", "no field or function 'len' applicable to int");
    err_contains("fn main() { let a: [int] = [1]; a.push(true); }", "'T' cannot be int and bool at the same time");
}

// ----- M3.2: structs -----

#[test]
fn structs_valid_vals() {
    assert!(check_src("struct P { x: int, y: int } fn main() -> int { let p: P = P { x: 1, y: 2 }; p.x + p.y }").is_ok());
    assert!(check_src("struct P { x: int } fn main() { var p: P = P { x: 1 }; p.x = 9; }").is_ok());
    // Campos en otro orden: válido.
    assert!(check_src("struct P { x: int, y: int } fn main() -> int { let p: P = P { y: 2, x: 1 }; p.x }").is_ok());
    // Structs anidados y como parámetro.
    assert!(check_src(
        "struct P { x: int } struct L { a: P, b: P }
         fn f(l: L) -> int { l.a.x } fn main() -> int { f(L { a: P { x: 1 }, b: P { x: 2 } }) }"
    ).is_ok());
}

#[test]
fn structs_errors() {
    err_contains("fn main() { let p: Foo = Foo { x: 1 }; }", "not declared");
    err_contains("struct P { x: int } fn main() -> int { let p: P = P { x: true }; p.x }", "expected int");
    err_contains("struct P { x: int, y: int } fn main() -> int { let p: P = P { x: 1 }; p.x }", "missing field");
    err_contains("struct P { x: int } fn main() -> int { let p: P = P { x: 1, z: 2 }; p.x }", "has no field");
    err_contains("struct P { x: int } fn main() -> int { let p: P = P { x: 1 }; p.y }", "has no field");
    err_contains("struct P { x: int } fn main() -> int { let n: int = 5; n.x }", "not a struct");
    err_contains("struct P {} struct P {} fn main() {}", "declared twice");
}

// ----- M4.1: funciones de primera clase -----

#[test]
fn functions_first_class_validas() {
    // Anónima en variable, con su tipo función.
    assert!(check_src("fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x + 1 }; f(2) }").is_ok());
    // De orden superior: recibe y aplica una función.
    assert!(check_src(
        "fn apply(f: fn(int) -> int, x: int) -> int { f(x) }
         fn main() -> int { apply(fn(n: int) -> int { n * n }, 3) }"
    ).is_ok());
    // Un nombre de función es un valor de tipo función.
    assert!(check_src(
        "fn inc(n: int) -> int { n + 1 }
         fn main() -> int { let g: fn(int) -> int = inc; g(4) }"
    ).is_ok());
    // Devolver una función.
    assert!(check_src(
        "fn dame() -> fn(int) -> int { fn(n: int) -> int { n } }
         fn main() -> int { let f: fn(int) -> int = dame(); f(5) }"
    ).is_ok());
    // Sin argumentos y retorno unit.
    assert!(check_src("fn main() { let f: fn() = fn() { print(1); }; f() }").is_ok());
}

#[test]
fn functions_first_class_errors() {
    // Tipo de la anónima no coincide con la anotación.
    err_contains(
        "fn main() { let f: fn(int) -> int = fn(x: bool) -> int { 0 }; }",
        "initialized with",
    );
    // Aridad incorrecta en una llamada indirecta.
    err_contains(
        "fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x }; f(1, 2) }",
        "expects 1 argument",
    );
    // Tipo de argumento incorrecto en una llamada indirecta.
    err_contains(
        "fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x }; f(true) }",
        "expected int, got bool",
    );
    // Llamar a algo que no es función.
    err_contains("fn main() -> int { let x: int = 3; x(1) }", "not a function");
    // El cuerpo de la anónima no respeta su tipo de retorno.
    err_contains(
        "fn main() { let f: fn() -> int = fn() -> int { true }; }",
        "produces bool",
    );
}

// ----- M4.2: closures (captura de entorno) -----

#[test]
fn closures_capture_the_environment() {
    // Captura de un `let` externo (lectura).
    assert!(check_src(
        "fn main() -> int { let b: int = 10; let f: fn(int) -> int = fn(x: int) -> int { x + b }; f(1) }"
    ).is_ok());
    // Captura de un `var` externo y su mutación.
    assert!(check_src(
        "fn counter() -> fn() -> int { var n: int = 0; fn() -> int { n = n + 1; n } }
         fn main() -> int { let c: fn() -> int = counter(); c() }"
    ).is_ok());
    // Captura transitiva (dos niveles).
    assert!(check_src(
        "fn adder(x: int) -> fn(int) -> int { fn(y: int) -> int { x + y } }
         fn main() -> int { let add5: fn(int) -> int = adder(5); add5(10) }"
    ).is_ok());
}

#[test]
fn closure_cannot_reassign_a_captured_let() {
    // Capturar no reata: asignar a un `let` externo sigue siendo error.
    err_contains(
        "fn main() { let b: int = 1; let f: fn() = fn() { b = 2; }; f() }",
        "it is immutable",
    );
}

#[test]
fn functions_no_son_comparables() {
    err_contains(
        "fn inc(n: int) -> int { n } fn main() -> int { if (inc == inc) { 1 } else { 0 } }",
        "same comparable type",
    );
}

// ----- M5.1: enums (tipos suma) y construcción -----

#[test]
fn enum_construction_validates() {
    let src = r#"
enum Shape { Circulo(float), Rect(float, float), Punto }
fn area(f: Shape) -> Shape { f }
fn main() {
let a: Shape = Shape.Circulo(2.0);
let b: Shape = Shape.Rect(3.0, 4.0);
let c: Shape = Shape.Punto;
print(a); print(b); print(c); print(area(a));
}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn recursive_enum_is_valid() {
    // Un enum puede portar su propio tipo: el norte de M5 (listas, árboles).
    let src = r#"
enum List { Cons(int, List), Nil }
fn main() { let xs: List = List.Cons(1, List.Cons(2, List.Nil)); print(xs); }
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn enum_variant_nonexistent() {
    err_contains("enum E { A, B } fn main() { let x: E = E.C; print(x); }", "has no variant 'C'");
}

#[test]
fn enum_arity_incorrect() {
    err_contains("enum E { A(int) } fn main() { let x: E = E.A(1, 2); print(x); }", "expects 1 argument");
}

#[test]
fn enum_payload_type_incorrect() {
    err_contains("enum E { A(int) } fn main() { let x: E = E.A(true); print(x); }", "expected int, got bool");
}

#[test]
fn enum_is_not_comparable() {
    err_contains(
        "enum E { A, B } fn main() -> int { let x: E = E.A; if (x == E.B) { 1 } else { 0 } }",
        "same comparable type",
    );
}

#[test]
fn enum_and_struct_cannot_share_a_name() {
    err_contains("enum E { A } struct E { x: int } fn main() {}", "cannot also be a struct");
}

#[test]
fn enum_variant_repeated() {
    err_contains("enum E { A, A } fn main() {}", "variant 'A' repeated");
}

#[test]
fn enum_declared_twice() {
    err_contains("enum E { A } enum E { B } fn main() {}", "declared twice");
}

#[test]
fn enum_as_unknown_ty() {
    // Anotar con un nombre que no es ni struct ni enum.
    err_contains("fn main() { let x: NoExiste = 1; print(x); }", "not declared");
}

// ----- M5.2: match y exhaustividad -----

#[test]
fn exhaustive_match_is_valid() {
    let src = r#"
enum List { Cons(int, List), Nil }
fn sum(xs: List) -> int {
match (xs) {
    List.Cons(h, t) => h + sum(t),
    List.Nil => 0,
}
}
fn main() -> int { sum(List.Cons(1, List.Nil)) }
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn match_with_wildcard_is_exhaustive() {
    let src = "enum E { A, B, C } fn f(e: E) -> int { match (e) { E.A => 1, _ => 0 } } fn main() {}";
    assert!(check_src(src).is_ok());
}

#[test]
fn match_no_exhaustive() {
    err_contains(
        "enum E { A, B, C } fn f(e: E) -> int { match (e) { E.A => 1, E.B => 2 } } fn main() {}",
        "non-exhaustive",
    );
}

#[test]
fn match_arms_of_different_types() {
    err_contains(
        "enum E { A, B } fn f(e: E) -> int { match (e) { E.A => 1, E.B => true } } fn main() {}",
        "different types",
    );
}

#[test]
fn match_variant_repeated() {
    err_contains(
        "enum E { A, B } fn f(e: E) -> int { match (e) { E.A => 1, E.A => 2, E.B => 3 } } fn main() {}",
        "is already covered",
    );
}

#[test]
fn match_branch_unreachable_after_catchall() {
    err_contains(
        "enum E { A, B } fn f(e: E) -> int { match (e) { other => 0, E.A => 1 } } fn main() {}",
        "unreachable",
    );
}

#[test]
fn match_binding_arity_incorrect() {
    err_contains(
        "enum E { A(int) } fn f(e: E) -> int { match (e) { E.A => 1 } } fn main() {}",
        "binds 0 value(s), but the variant has 1",
    );
}

#[test]
fn match_about_no_enum() {
    err_contains(
        "fn f(n: int) -> int { match (n) { _ => 0 } } fn main() {}",
        "match requires an enum",
    );
}

#[test]
fn match_pattern_from_other_enum() {
    err_contains(
        "enum E { A } enum F { B } fn f(e: E) -> int { match (e) { F.B => 1, _ => 0 } } fn main() {}",
        "is of enum 'F'",
    );
}

#[test]
fn match_binds_payload_for_the_body() {
    // El binding del payload debe estar disponible (y bien tipado) en el cuerpo.
    let src = "enum Box { Con(int), Vacia } fn val(c: Box) -> int { match (c) { Box.Con(n) => n + 1, Box.Vacia => 0 } } fn main() {}";
    assert!(check_src(src).is_ok());
}

// ----- M6.1: funciones genéricas e inferencia -----

#[test]
fn generic_identity_y_usage() {
    let src = r#"
fn identity<T>(x: T) -> T { x }
fn main() -> int {
let a: int = identity(5);
let b: bool = identity(true);
if (b) { a } else { 0 }
}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn generic_infers_from_several_arguments() {
    // [T] y fn(T)->U determinan T y U a la vez.
    let src = r#"
fn apply<T, U>(f: fn(T) -> U, x: T) -> U { f(x) }
fn double(n: int) -> int { n * 2 }
fn main() -> int { apply(double, 21) }
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn generic_t_inconsistent() {
    err_contains(
        "fn par<T>(a: T, b: T) -> T { a } fn main() -> int { par(1, true) }",
        "cannot be int and bool",
    );
}

#[test]
fn generic_t_not_inferable() {
    err_contains(
        "fn empty<T>() -> int { 0 } fn main() -> int { empty() }",
        "could not infer the type parameter 'T'",
    );
}

#[test]
fn generic_as_value_is_error() {
    err_contains(
        "fn id<T>(x: T) -> T { x } fn main() -> int { let f: fn(int) -> int = id; f(3) }",
        "generic function 'id' as a value",
    );
}

#[test]
fn generic_cannot_compare_a_type_parameter() {
    err_contains(
        "fn ig<T>(a: T, b: T) -> bool { a == b } fn main() {}",
        "same comparable type",
    );
}

#[test]
fn repeated_ty_parameter() {
    err_contains("fn f<T, T>(x: T) -> T { x } fn main() {}", "type parameter 'T' repeated");
}

#[test]
fn unknown_type_is_not_a_parameter() {
    err_contains("fn f(x: Desconocido) -> int { 0 } fn main() {}", "'Desconocido' not declared");
}

// ----- M6.2: tipos genéricos del usuario y chequeo bidireccional -----

#[test]
fn generic_enum_construction_and_match() {
    let src = r#"
enum Box<T> { Llena(T), Vacia }
fn val(c: Box<int>, def: int) -> int {
match (c) { Box.Llena(v) => v, Box.Vacia => def }
}
fn main() -> int {
let a: Box<int> = Box.Llena(7);   // T=int del argumento
let b: Box<int> = Box.Vacia;       // T=int del tipo esperado
val(a, 0) + val(b, 35)
}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn generic_struct_field_substituted() {
    let src = r#"
struct Par<A, B> { primero: A, second: B }
fn main() -> int {
let p: Par<int, bool> = Par { primero: 10, second: true };
if (p.second) { p.primero } else { 0 }
}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn generic_ty_argument_mismatch() {
    err_contains(
        "enum Box<T> { Llena(T), Vacia } fn main() { let b: Box<bool> = Box.Llena(7); print(b); }",
        "cannot be bool and int",
    );
}

#[test]
fn generic_type_args_arity() {
    err_contains(
        "enum Box<T> { Llena(T), Vacia } fn main() { let b: Box<int, bool> = Box.Vacia; print(b); }",
        "expects 1 type argument(s)",
    );
}

#[test]
fn generic_empty_not_inferable_without_context() {
    // Sin tipo esperado ni argumentos, T queda sin determinar.
    err_contains(
        "enum Box<T> { Llena(T), Vacia } fn main() { print(Box.Vacia); }",
        "could not infer",
    );
}

#[test]
fn repeated_enum_ty_parameter() {
    err_contains("enum E<T, T> { A(T) } fn main() {}", "type parameter 'T' repeated");
}

#[test]
fn empty_array_adopts_the_expected_ty() {
    // El chequeo bidireccional arregla la aspereza histórica del [] vacío.
    assert!(check_src("fn main() -> int { let xs: [int] = []; xs.len() }").is_ok());
}

// ----- M6.3: Option/Result (prelude) y el operador ? -----

#[test]
fn prelude_option_result_available() {
    // Sin declararlos, Option y Result existen (vienen del prelude).
    let src = r#"
fn f() -> Result<int, string> { Result.Ok(1) }
fn g() -> Option<int> { Option.None }
fn main() {}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn try_result_y_option_valid_vals() {
    let src = r#"
fn d(a: int, b: int) -> Result<int, string> {
if (b == 0) { Result.Err("cero") } else { Result.Ok(a / b) }
}
fn calc(x: int, y: int) -> Result<int, string> {
let q: int = d(x, y)?;
Result.Ok(q + 1)
}
fn raw(xs: [int]) -> Option<int> { if (xs.len() == 0) { Option.None } else { Option.Some(xs[0]) } }
fn first(xs: [int]) -> Option<int> {
let v: int = raw(xs)?;
Option.Some(v)
}
fn main() {}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn try_requires_result_or_option() {
    err_contains(
        "fn f() -> Result<int, string> { let x: int = 5?; Result.Ok(x) } fn main() {}",
        "requires a Result or an Option",
    );
}

#[test]
fn try_function_must_return_compatible_type() {
    err_contains(
        "fn d() -> Result<int, string> { Result.Ok(1) } fn g() -> int { let x: int = d()?; x } fn main() {}",
        "requires the function to return Result",
    );
}

#[test]
fn try_result_with_different_e() {
    err_contains(
        "fn d() -> Result<int, string> { Result.Ok(1) } fn f() -> Result<int, bool> { let x: int = d()?; Result.Ok(x) } fn main() {}",
        "Result<_, string>",
    );
}

// ----- UFCS (M7.1) -----

#[test]
fn ufcs_free_function_as_method() {
    // recv.f(args) ≡ f(recv, args). Builtin (len) y función del usuario (suma).
    let src = r#"
fn sum(a: int, b: int) -> int { a + b }
fn main() -> int {
let xs: [int] = [1, 2, 3];
let n: int = xs.len();      // len(xs)
let v: int = 10;
v.sum(n)                    // suma(10, 3)
}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn ufcs_non_field_uses_free_function() {
    // 'doble' no es campo de Point: se resuelve como UFCS doble(p).
    let src = r#"
struct Point { x: int, y: int }
fn double(p: Point) -> int { (p.x + p.y) * 2 }
fn main() -> int {
let p: Point = Point { x: 3, y: 4 };
p.double()
}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn ufcs_field_function_wins_over_free_function() {
    // 'op' ES un campo (de tipo función): c.op(x) llama al campo, no es UFCS, aunque
    // exista una función libre 'op' homónima con otra firma.
    let src = r#"
fn op(a: int, b: int) -> int { a - b }
struct Box { op: fn(int) -> int }
fn main() -> int {
let c: Box = Box { op: fn(x: int) -> int { x + 1 } };
c.op(41)                     // (c.op)(41) = 42, NO op(c, 41)
}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn ufcs_chained() {
    let src = r#"
fn double(x: int) -> int { x * 2 }
fn inc(x: int) -> int { x + 1 }
fn main() -> int {
let v: int = 5;
v.double().inc().double()      // doble(inc(doble(5)))
}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn ufcs_method_nonexistent() {
    err_contains(
        "fn main() -> int { let v: int = 5; v.frobnicate() }",
        "no field or function 'frobnicate' applicable to int",
    );
}

#[test]
fn ufcs_receiver_type_incorrect() {
    // El receptor se inserta como primer argumento: si su tipo no encaja, error.
    err_contains(
        "fn double(x: int) -> int { x * 2 } fn main() -> int { let b: bool = true; b.double() }",
        "expected int, got bool",
    );
}

#[test]
fn ufcs_generic_infers_from_receptor() {
    // El receptor cuenta para la inferencia de genéricos (M6) como cualquier arg.
    let src = r#"
fn first<T>(xs: [T]) -> T { xs[0] }
fn main() -> int {
let xs: [int] = [7, 8, 9];
xs.first()                 // first(xs) con T = int
}
"#;
    assert!(check_src(src).is_ok());
}

// ----- M7.3: stdlib (prelude de orden superior: map/filter/fold) -----

#[test]
fn prelude_map_filter_fold_type_check() {
    // Disponibles sin declararlas; se infieren los genéricos en cada uso.
    let src = r#"
fn double(x: int) -> int { x * 2 }
fn par(x: int) -> bool { x % 2 == 0 }
fn sum(a: int, b: int) -> int { a + b }
fn main() -> int {
let xs: [int] = [1, 2, 3, 4];
let ys: [int] = xs.map(double).filter(par);
ys.fold(0, sum)
}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn prelude_fold_to_different_ty() {
    // fold<T, A>: el acumulador A puede diferir del elemento T (aquí bool).
    let src = r#"
fn main() -> int {
let xs: [int] = [2, 4, 6];
let all: bool = xs.fold(true, fn(acc: bool, x: int) -> bool { acc && (x % 2 == 0) });
if (all) { 1 } else { 0 }
}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn prelude_map_requires_compatible_function() {
    // map<T,U>(xs:[T], f:fn(T)->U): una f con dominio incompatible hace que el
    // parámetro de tipo T se exija int (por xs) y bool (por f) a la vez: error.
    err_contains(
        "fn f(b: bool) -> int { 1 } fn main() -> int { let xs: [int] = [1]; let ys: [int] = xs.map(f); ys[0] }",
        "cannot be int and bool",
    );
}

#[test]
fn prelude_user_can_redefine() {
    // Si el usuario define 'map', el del prelude se omite (override).
    let src = r#"
fn map(x: int) -> int { x + 1 }
fn main() -> int { map(41) }
"#;
    assert!(check_src(src).is_ok());
}

// ----- M8.1: inferencia local (let/var sin anotación) -----

#[test]
fn infers_primitives_and_compounds() {
    let src = r#"
struct Point { x: int, y: int }
enum Box<T> { Llena(T), Vacia }
fn main() -> int {
let x = 3;                      // int
let f = 2.5;                    // float
let b = x > 1;                  // bool
let s = "hello";                 // string
let xs = [10, 20, 30];          // [int]
let p = Point { x: 7, y: 6 };   // Point
let c = Box.Llena(5);          // Box<int> (genéricos M6)
let cv = p.x + p.y;             // int, del campo inferido
let inside = match (c) { Box.Llena(v) => v, Box.Vacia => 0 };  // int
x + xs[0] + cv + inside
}
"#;
    assert!(check_src(src).is_ok());
}

#[test]
fn inferred_variable_keeps_its_ty() {
    // Una inferida como int no puede luego usarse como bool.
    err_contains(
        "fn main() -> int { let x = 3; if (x) { 0 } else { 1 } }",
        "if condition must be bool",
    );
}

#[test]
fn inferred_var_is_mutable_and_typed() {
    // 'var t = 0' infiere int y es mutable; asignarle bool falla.
    assert!(check_src("fn main() -> int { var t = 0; t = t + 1; t }").is_ok());
    err_contains(
        "fn main() -> int { var t = 0; t = true; t }",
        "is int but is assigned bool",
    );
}

#[test]
fn inferred_let_stays_immutable() {
    // La inferencia no cambia la mutabilidad: un 'let' inferido no se puede reasignar.
    err_contains(
        "fn main() -> int { let x = 3; x = 4; x }",
        "immutable",
    );
}

#[test]
fn inference_does_not_apply_to_the_indeterminate() {
    // Sin anotación, '[]' no se puede inferir: pide la anotación.
    err_contains(
        "fn main() -> int { let xs = []; xs.len() }",
        "cannot infer the type of []",
    );
}

#[test]
fn annotation_follows_validandose() {
    // Con anotación, un inicializador incompatible sigue siendo error.
    err_contains(
        "fn main() -> int { let x: int = true; x }",
        "initialized with bool",
    );
}

// ----- M9.1: traits -----

#[test]
fn trait_e_impl_valid_vals() {
    check_src(r#"
        trait Showable { fn show(self) -> string; }
        struct Point { x: int, y: int }
        impl Showable for Point { fn show(self) -> string { "p" } }
        fn main() -> int { let p = Point { x: 1, y: 2 }; print(p.show()); 0 }
    "#).expect("trait/impl válidos");
}

#[test]
fn trait_for_enum_and_primitive() {
    check_src(r#"
        trait Value { fn value(self) -> int; }
        enum Coin { Cara, Cruz }
        impl Value for Coin { fn value(self) -> int { match (self) { Coin.Cara => 1, Coin.Cruz => 0 } } }
        impl Value for int { fn value(self) -> int { self } }
        fn main() -> int { Coin.Cara.value() + 5.value() }
    "#).expect("impl para enum y primitivo");
}

#[test]
fn self_en_return_val_y_method_internal() {
    check_src(r#"
        trait P { fn add(self, o: Point) -> Point; fn double(self) -> Self; }
        struct Point { x: int, y: int }
        impl P for Point {
            fn add(self, o: Point) -> Point { Point { x: self.x + o.x, y: self.y + o.y } }
            fn double(self) -> Self { self.add(self) }
        }
        fn main() -> int { let p = Point { x: 1, y: 2 }; let q = p.double(); q.x }
    "#).expect("Self en return_val y self.method() internal");
}

#[test]
fn field_wins_over_trait_method() {
    // Un campo función del struct tiene prioridad sobre un método de trait homónimo.
    check_src(r#"
        trait T { fn f(self) -> int; }
        struct S { f: fn() -> int, x: int }
        impl T for S { fn f(self) -> int { self.x } }
        fn cero() -> int { 0 }
        fn main() -> int { let s = S { f: cero, x: 9 }; s.f() }
    "#).expect("el campo 'f' gana: se invoca el valor del campo, no el método");
}

#[test]
fn impl_does_not_cover_all_methods() {
    err_contains(
        r#"trait T { fn a(self) -> int; fn b(self) -> int; }
           struct S { x: int }
           impl T for S { fn a(self) -> int { self.x } }
           fn main() -> int { 0 }"#,
        "does not implement method 'b'",
    );
}

#[test]
fn impl_with_different_signature() {
    err_contains(
        r#"trait T { fn a(self) -> int; }
           struct S { x: int }
           impl T for S { fn a(self) -> bool { true } }
           fn main() -> int { 0 }"#,
        "returns bool, but the trait requires int",
    );
}

#[test]
fn method_ambiguous_between_two_traits() {
    err_contains(
        r#"trait A { fn f(self) -> int; }
           trait B { fn f(self) -> int; }
           struct S { x: int }
           impl A for S { fn f(self) -> int { 1 } }
           impl B for S { fn f(self) -> int { 2 } }
           fn main() -> int { 0 }"#,
        "ambiguo",
    );
}

#[test]
fn impl_of_nonexistent_trait() {
    err_contains(
        r#"struct S { x: int }
           impl NoExiste for S { fn f(self) -> int { 1 } }
           fn main() -> int { 0 }"#,
        "trait 'NoExiste' not declared",
    );
}

#[test]
fn concrete_impl_over_generic_ty_is_error() {
    // `impl T for Box` sin declarar los parámetros de tipo: M9.2b pide `impl<A> T for
    // Box<A>`. El error guía hacia esa forma.
    err_contains(
        r#"trait T { fn f(self) -> int; }
           struct Box<A> { v: A }
           impl T for Box { fn f(self) -> int { 1 } }
           fn main() -> int { 0 }"#,
        "is generic: declare its parameters in the impl",
    );
}

#[test]
fn semantic_index_hover_of_variable() {
    // M10.2b: el índice registra el tipo de un uso de identificador.
    let src = "fn main() -> int {\n  let x = 5;\n  x\n}";
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    let idx = semantic_index(&mut prog);
    let h = idx.hovers.iter().find(|h| h.line == 3 && h.col == 3).expect("hover de x");
    assert_eq!(h.text, "x: int");
    assert_eq!(h.len, 1);
    // Y registra su definición (el `let` de la línea 2).
    let d = idx.defs.iter().find(|d| d.line == 3 && d.col == 3).expect("def de x");
    assert_eq!((d.def_line, d.def_col), (2, 3));
}

#[test]
fn semantic_index_hover_of_ty() {
    // M10.2f: el índice registra el uso de un nombre de tipo en un literal de struct y la
    // construcción de un enum, con su posición de declaración (ir-a-definición).
    let src = "struct Point { x: int }\nenum Color { Rojo }\nfn main() -> int {\n  let p = Point { x: 1 };\n  let c = Color.Rojo;\n  p.x\n}";
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    let idx = semantic_index(&mut prog);
    // Hover del nombre `Point` en el literal (línea 4, col 11).
    let h = idx.hovers.iter().find(|h| h.line == 4 && h.col == 11).expect("hover de Point");
    assert_eq!(h.text, "struct Point");
    // Def → la declaración del struct (línea 1).
    let d = idx.defs.iter().find(|d| d.line == 4 && d.col == 11).expect("def de Point");
    assert_eq!(d.def_line, 1);
    // Hover del enum `Color` en la construcción (línea 5).
    let he = idx.hovers.iter().find(|h| h.line == 5 && h.text == "enum Color").expect("hover de Color");
    assert_eq!(he.line, 5);
    // Hover de la **variante** `Rojo` (tras el `.`): su firma. `Color.Rojo` no tiene payload.
    let hv = idx.hovers.iter().find(|h| h.line == 5 && h.text == "Color.Rojo").expect("hover de Rojo");
    assert!(hv.col > he.col, "la variant va after el enum: {} vs {}", hv.col, he.col);
}

#[test]
fn semantic_index_hover_in_interpolation() {
    // El `to_string(e)` sintético de `${e}` comparte posición con `e`; su hover NO debe taparlo.
    // Hover sobre `area` dentro de `${area(3.0)}` → la función, nunca `to_string`.
    let src = "fn area(r: float) -> float { r * r }\nfn main() {\n  print(\"x: ${area(3.0)}\");\n}";
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    let idx = semantic_index(&mut prog);
    // En la línea 3, NINGÚN hover debe ser de `to_string` (el wrapper sintético se omite).
    assert!(!idx.hovers.iter().any(|h| h.line == 3 && h.text.starts_with("to_string:")),
        "el to_string sintético no registra hover");
    // Y `area` sí tiene su hover de función.
    assert!(idx.hovers.iter().any(|h| h.line == 3 && h.text == "area: fn(float) -> float"),
        "hover de area en la interpolación");
}

#[test]
fn semantic_index_hover_in_string_ufcs() {
    // En una cadena `v.doble().inc().doble()` todos los eslabones comparten la posición del
    // receptor: las dos `.doble()` colisionaban en `field_name_pos` y la primera perdía su hover.
    // Ahora se registran ambas posiciones.
    let src = "fn double(x: int) -> int { x * 2 }\nfn inc(x: int) -> int { x + 1 }\nfn main() -> int {\n  let v: int = 5;\n  v.double().inc().double()\n}";
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    let idx = semantic_index(&mut prog);
    // Los dos `doble` de la línea 5 tienen hover en posiciones distintas.
    let cols: Vec<usize> = idx.hovers.iter()
        .filter(|h| h.line == 5 && h.text == "double: fn(int) -> int").map(|h| h.col).collect();
    assert!(cols.len() >= 2, "ambas `.double()` con hover: {cols:?}");
    assert!(cols[0] != cols[1], "en columnas distintas: {cols:?}");
}

#[test]
fn semantic_index_hover_in_match() {
    // M10.2f: dentro de un `match` el índice registra el escrutinio, el enum y la variante del
    // patrón, y los bindings que liga (tanto en el patrón como en el cuerpo).
    let src = "enum Shape { Circulo(float), Punto }\nfn area(f: Shape) -> float {\n  match (f) {\n    Shape.Circulo(r) => r,\n    Shape.Punto => 0.0,\n  }\n}\nfn main() -> int { 0 }";
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    let idx = semantic_index(&mut prog);
    // Enum y variante en el patrón (línea 4).
    assert!(idx.hovers.iter().any(|h| h.line == 4 && h.text == "enum Shape"), "hover enum en patrón");
    assert!(idx.hovers.iter().any(|h| h.line == 4 && h.text == "Shape.Circulo(float)"), "hover variant en patrón");
    // Binding `r` del patrón → su tipo.
    assert!(idx.hovers.iter().any(|h| h.line == 4 && h.text == "r: float"), "hover binding en patrón");
}

#[test]
fn impl_generic_valid() {
    // M9.2b-1: `impl<A> T for Box<A>` con un método que no usa A.
    assert!(check_src(
        r#"trait T { fn f(self) -> int; }
           struct Box<A> { v: A }
           impl<A> T for Box<A> { fn f(self) -> int { 1 } }
           fn main() -> int { let c = Box { v: 9 }; c.f() }"#,
    ).is_ok());
}

#[test]
fn generic_impl_malformed_target_is_error() {
    // El objetivo de un impl genérico debe ser `Box<A>` con sus propios parámetros.
    err_contains(
        r#"trait T { fn f(self) -> int; }
           struct Box<A> { v: A }
           impl<A> T for Box<int> { fn f(self) -> int { 1 } }
           fn main() -> int { 0 }"#,
        "must apply to 'Box<A>'",
    );
}

#[test]
fn self_outside_impl_is_error() {
    err_contains(
        "fn f(x: Self) -> int { 1 } fn main() -> int { 0 }",
        "'Self' is only valid inside a trait or impl",
    );
}

#[test]
fn nonexistent_method_is_not_field_or_function() {
    err_contains(
        r#"struct S { x: int }
           fn main() -> int { let s = S { x: 1 }; s.noexiste() }"#,
        "no field or function",
    );
}

// ----- M9.2: bounds de genéricos -----

#[test]
fn concrete_bound_and_forwarding() {
    check_src(r#"
        trait Value { fn value(self) -> int; }
        struct Point { x: int }
        impl Value for Point { fn value(self) -> int { self.x } }
        impl Value for int { fn value(self) -> int { self } }
        fn double<T: Value>(x: T) -> int { x.value() + x.value() }
        fn forward<T: Value>(x: T) -> int { double(x) }
        fn main() -> int {
            let p = Point { x: 5 };
            double(p) + double(9) + forward(p)
        }
    "#).expect("bound concreto + reenvío");
}

#[test]
fn bounds_multiples() {
    check_src(r#"
        trait A { fn a(self) -> int; }
        trait B { fn b(self) -> int; }
        struct S { x: int }
        impl A for S { fn a(self) -> int { self.x } }
        impl B for S { fn b(self) -> int { self.x } }
        fn usar<T: A + B>(x: T) -> int { x.a() + x.b() }
        fn main() -> int { let s = S { x: 1 }; usar(s) }
    "#).expect("T: A + B");
}

#[test]
fn bound_ty_does_not_implement() {
    err_contains(
        r#"trait Value { fn value(self) -> int; }
           struct Point { x: int }
           fn usar<T: Value>(x: T) -> int { x.value() }
           fn main() -> int { let p = Point { x: 1 }; usar(p) }"#,
        "Point does not implement 'Value'",
    );
}

#[test]
fn bound_method_outside_del_trait() {
    err_contains(
        r#"trait Value { fn value(self) -> int; }
           fn usar<T: Value>(x: T) -> int { x.other() }
           fn main() -> int { 0 }"#,
        "no field or function 'other'",
    );
}

#[test]
fn forwarding_without_bound_is_error() {
    err_contains(
        r#"trait Value { fn value(self) -> int; }
           fn usar<T: Value>(x: T) -> int { x.value() }
           fn forwarder<U>(y: U) -> int { usar(y) }
           fn main() -> int { 0 }"#,
        "is not bounded by 'Value'",
    );
}

#[test]
fn bound_a_trait_nonexistent() {
    err_contains(
        "fn usar<T: NoExiste>(x: T) -> int { 0 } fn main() -> int { 0 }",
        "trait 'NoExiste' not declared",
    );
}

// ----- M9.3a: métodos por defecto -----

#[test]
fn default_method_inherited_and_overridden() {
    check_src(r#"
        trait Value {
            fn base(self) -> int;
            fn double(self) -> int { self.base() + self.base() }
        }
        struct A { n: int }
        impl Value for A { fn base(self) -> int { self.n } }
        struct B { n: int }
        impl Value for B { fn base(self) -> int { self.n } fn double(self) -> int { 0 } }
        fn main() -> int {
            let a = A { n: 1 };
            let b = B { n: 2 };
            a.double() + b.double()
        }
    "#).expect("default heredado por A, redefinido por B");
}

#[test]
fn required_method_without_default_stays_mandatory() {
    err_contains(
        r#"trait T { fn req(self) -> int; fn opt(self) -> int { 0 } }
           struct S { x: int }
           impl T for S { fn opt(self) -> int { self.x } }
           fn main() -> int { 0 }"#,
        "does not implement method 'req'",
    );
}

#[test]
fn default_method_via_bound() {
    check_src(r#"
        trait Greeting {
            fn name(self) -> int;
            fn double(self) -> int { self.name() + self.name() }
        }
        struct P { v: int }
        impl Greeting for P { fn name(self) -> int { self.v } }
        fn usar<T: Greeting>(x: T) -> int { x.double() }
        fn main() -> int { let p = P { v: 1 }; usar(p) }
    "#).expect("default invocado vía bound");
}

// ----- M9.3b: trait objects -----

#[test]
fn trait_object_coercion_y_dispatch() {
    check_src(r#"
        trait Shape { fn area(self) -> int; }
        struct Square { lado: int }
        impl Shape for Square { fn area(self) -> int { self.lado * self.lado } }
        struct Rect { ancho: int, alto: int }
        impl Shape for Rect { fn area(self) -> int { self.ancho * self.alto } }
        fn total(xs: [dyn Shape]) -> int {
            var s = 0; var i = 0;
            while (i < xs.len()) { s = s + xs[i].area(); i = i + 1; }
            s
        }
        fn main() -> int {
            let fs: [dyn Shape] = [Square { lado: 2 }, Rect { ancho: 3, alto: 4 }];
            total(fs)
        }
    "#).expect("array heterogéneo de trait objects + dispatch");
}

#[test]
fn trait_object_ty_does_not_implement() {
    err_contains(
        r#"trait Shape { fn area(self) -> int; }
           struct P { x: int }
           fn main() -> int { let f: dyn Shape = P { x: 1 }; 0 }"#,
        "does not implement 'Shape'",
    );
}

#[test]
fn trait_object_object_safety() {
    err_contains(
        r#"trait Clone { fn copy(self) -> Self; }
           struct P { x: int }
           impl Clone for P { fn copy(self) -> Self { P { x: self.x } } }
           fn usar(p: dyn Clone) -> int { let q = p.copy(); 0 }
           fn main() -> int { 0 }"#,
        "uses 'Self': it is not callable on 'dyn Clone'",
    );
}

#[test]
fn dyn_of_nonexistent_trait() {
    err_contains(
        "fn f(x: dyn NoExiste) -> int { 0 } fn main() -> int { 0 }",
        "trait 'NoExiste' not declared",
    );
}

// ----- M10.1: anotaciones -----

#[test]
fn test_valid() {
    check_src(r#"
        @test
        fn ok() -> bool { 1 + 1 == 2 }
        fn main() -> int { 0 }
    "#).expect("@test con signature () -> bool");
}

#[test]
fn test_signature_incorrect() {
    err_contains(
        "@test fn malo() -> int { 1 } fn main() -> int { 0 }",
        "must return bool",
    );
}

#[test]
fn test_with_parameters() {
    err_contains(
        "@test fn malo(x: int) -> bool { true } fn main() -> int { 0 }",
        "must not take parameters",
    );
}

#[test]
fn unknown_annotation() {
    err_contains(
        "@magia fn f() -> bool { true } fn main() -> int { 0 }",
        "unknown annotation: '@magia'",
    );
}

#[test]
fn test_annotation_on_struct_is_error() {
    err_contains(
        "@test struct S { x: int } fn main() -> int { 0 }",
        "'@test' is only allowed on functions",
    );
}

#[test]
fn derive_eq_struct_y_enum() {
    check_src(r#"
        @derive(Eq)
        struct Point { x: int, y: int }
        @derive(Eq)
        enum Color { Rojo, Verde, Azul }
        @derive(Eq)
        enum Form { Circulo(int), Rect(int, int) }
        fn main() -> int {
            let p = Point { x: 1, y: 2 };
            let c = Color.Rojo;
            let f = Form.Rect(1, 2);
            if (p.eq(p)) { 0 } else { 1 }
        }
    "#).expect("@derive(Eq) para struct y enum (unit y con payload)");
}

#[test]
fn derive_eq_compone_con_bound() {
    check_src(r#"
        @derive(Eq)
        enum Color { Rojo, Verde }
        fn equal<T: Eq>(a: T, b: T) -> bool { a.eq(b) }
        fn main() -> int { if (equal(Color.Rojo, Color.Rojo)) { 0 } else { 1 } }
    "#).expect("un type derivado satisface el bound T: Eq");
}

#[test]
fn derive_trait_not_supported() {
    err_contains(
        "@derive(Ord) struct P { x: int } fn main() -> int { 0 }",
        "cannot derive 'Ord'",
    );
}

#[test]
fn derive_on_generic_type_is_error() {
    err_contains(
        "@derive(Eq) struct Box<T> { v: T } fn main() -> int { 0 }",
        "generic type",
    );
}

#[test]
fn derive_show_struct_y_enum() {
    check_src(r#"
        @derive(Show)
        struct Point { x: int, y: int }
        @derive(Show)
        enum Color { Rojo, RGB(int, int, int) }
        @derive(Show)
        struct Label { name: string, location: Point, color: Color }
        fn main() -> int {
            let e = Label { name: "o", location: Point { x: 1, y: 2 }, color: Color.Rojo };
            print(e.show());
            0
        }
    "#).expect("@derive(Show) para struct, enum y struct nested");
}

#[test]
fn derive_eq_and_show_together() {
    check_src(r#"
        @derive(Eq, Show)
        struct P { x: int }
        fn main() -> int { if (P { x: 1 }.eq(P { x: 1 })) { 0 } else { 1 } }
    "#).expect("@derive(Eq, Show) genera ambos impls");
}

#[test]
fn derive_show_unsupported_field_is_error() {
    err_contains(
        "@derive(Show) struct S { xs: [int] } fn main() -> int { 0 }",
        "cannot derive Show for a field of type [int]",
    );
}
