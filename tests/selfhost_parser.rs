//! Oráculo del **parser auto-alojado** (M14.2, self-hosting).
//!
//! El parser escrito en raylang (`selfhost/parser.ray`, vía el driver `selfhost/parse_dump.ray`)
//! debe producir EXACTAMENTE el mismo AST que el parser de Rust (`src/parser.rs`). Como no exponemos
//! los tipos internos de Rust a raylang, comparamos un **volcado canónico** (S-expression, con
//! posición `@línea:col` en cada nodo): el lado raylang lo imprime; el lado Rust (este archivo) lo
//! reconstruye con `dump_program`. Si difieren, el parser auto-alojado no es fiel.
//!
//! Cobertura M14.2a: expresiones (toda la precedencia), sentencias (let/var/assign/return/expr) y
//! funciones de nivel superior. Los ítems no cubiertos (structs/enums/match/genéricos/…) no aparecen
//! en el corpus de prueba; el volcado de Rust hace `panic!` si los encuentra (red de seguridad).

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use raylang::ast::*;

/// Re-escapa una cadena igual que `escape` en `parse_dump.ray` (idéntico carácter a carácter).
fn escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out
}

fn pos(line: usize, col: usize) -> String {
    format!("@{}:{}", line, col)
}

fn dump_type(t: &Type) -> String {
    match t {
        Type::Int => "int".into(),
        Type::Float => "float".into(),
        Type::Bool => "bool".into(),
        Type::String => "string".into(),
        Type::Char => "char".into(),
        Type::Unit => "unit".into(),
        Type::Array(e) => format!("[{}]", dump_type(e)),
        Type::Fn(ps, r) => {
            let joined = ps.iter().map(dump_type).collect::<Vec<_>>().join(", ");
            format!("fn({}) -> {}", joined, dump_type(r))
        }
        // El parser (sin checker) produce `Struct(name, args)` para todo identificador en posición de
        // tipo (incl. `Map<K,V>`, sin reclasificar). El receptor `self` de un método sí llega como
        // `SelfType`; tanto él como un `Self` escrito (que es `Struct("Self",[])`) se vuelcan "Self".
        Type::Struct(name, args) => {
            if args.is_empty() {
                name.clone()
            } else {
                format!("{}<{}>", name, args.iter().map(dump_type).collect::<Vec<_>>().join(", "))
            }
        }
        Type::SelfType => "Self".into(),
        Type::Dyn(traits) => format!("dyn {}", traits.join(" + ")),
        other => panic!("el parser no debería producir el tipo {:?}", other),
    }
}

/// Segmento de genéricos ` (generics T U (bound T A) (bound U B))`, o "" si no hay ninguno.
fn dump_generics(tp: &[String], bounds: &[(String, String)]) -> String {
    if tp.is_empty() && bounds.is_empty() {
        return String::new();
    }
    let mut s = String::from(" (generics");
    for t in tp {
        s.push_str(&format!(" {}", t));
    }
    for (p, tr) in bounds {
        s.push_str(&format!(" (bound {} {})", p, tr));
    }
    s.push(')');
    s
}

fn dump_binop(op: &BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "add",
        BinaryOp::Sub => "sub",
        BinaryOp::Mul => "mul",
        BinaryOp::Div => "div",
        BinaryOp::Rem => "rem",
        BinaryOp::Eq => "eq",
        BinaryOp::Ne => "ne",
        BinaryOp::Lt => "lt",
        BinaryOp::Le => "le",
        BinaryOp::Gt => "gt",
        BinaryOp::Ge => "ge",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
        // M19.3a: bit a bit. El parser auto-alojado aún no los reconoce; fuera del corpus,
        // pero el `match` debe ser exhaustivo.
        BinaryOp::BitAnd => "bitand",
        BinaryOp::BitOr => "bitor",
        BinaryOp::BitXor => "bitxor",
        BinaryOp::Shl => "shl",
        BinaryOp::Shr => "shr",
    }
}

fn dump_unop(op: &UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "neg",
        UnaryOp::Not => "not",
        UnaryOp::BitNot => "bitnot", // M19.3a
    }
}

fn dump_expr(e: &Expr) -> String {
    let pp = pos(e.line, e.col);
    match &e.kind {
        ExprKind::Int(n) => format!("(int {}){}", n, pp),
        ExprKind::Float(f) => format!("(float {}){}", f, pp),
        ExprKind::Bool(b) => format!("(bool {}){}", b, pp),
        ExprKind::Str(s) => format!("(str \"{}\"){}", escape(s), pp),
        ExprKind::Char(c) => format!("(char '{}'){}", escape(&c.to_string()), pp),
        ExprKind::Ident(n) => format!("(ident {}){}", n, pp),
        ExprKind::Unary { op, expr } => format!("(unary {} {}){}", dump_unop(op), dump_expr(expr), pp),
        ExprKind::Binary { op, left, right } => {
            format!("(binary {} {} {}){}", dump_binop(op), dump_expr(left), dump_expr(right), pp)
        }
        ExprKind::Call { callee, args } => {
            format!("(call {}{}){}", dump_expr(callee), dump_exprs(args), pp)
        }
        ExprKind::Index { array, index } => {
            format!("(index {} {}){}", dump_expr(array), dump_expr(index), pp)
        }
        ExprKind::Field { object, name } => format!("(field {} {}){}", dump_expr(object), name, pp),
        ExprKind::ArrayLit(elems) => format!("(array{}){}", dump_exprs(elems), pp),
        ExprKind::If { cond, then_branch, else_branch } => {
            format!(
                "(if {} {}{}){}",
                dump_expr(cond),
                dump_block(then_branch),
                dump_opt_expr(else_branch),
                pp
            )
        }
        ExprKind::While { cond, body } => format!("(while {} {}){}", dump_expr(cond), dump_block(body), pp),
        ExprKind::Block(b) => dump_block(b),
        ExprKind::StructLit { name, fields } => {
            let inits: String = fields
                .iter()
                .map(|(n, e)| format!(" (init {} {})", n, dump_expr(e)))
                .collect();
            format!("(struct-lit {}{}){}", name, inits, pp)
        }
        ExprKind::Func(fe) => dump_fnexpr(fe),
        ExprKind::Match { scrutinee, arms } => {
            let body: String = arms
                .iter()
                .map(|a| {
                    format!(
                        " (arm {} {}){}",
                        dump_pattern(&a.pattern),
                        dump_expr(&a.body),
                        pos(a.line, a.col)
                    )
                })
                .collect();
            format!("(match {}{}){}", dump_expr(scrutinee), body, pp)
        }
        ExprKind::Try(inner) => format!("(try {}){}", dump_expr(inner), pp),
        // EnumLit lo genera el checker, no el parser; no debería aparecer.
        other => panic!("el parser no debería producir la expresión {:?}", other),
    }
}

/// Marca ` pub` si el ítem es público, o "".
fn dump_pub(is_pub: bool) -> &'static str {
    if is_pub {
        " pub"
    } else {
        ""
    }
}

/// Segmento de anotaciones ` (anns (ann derive Eq Show) ...)`, o "" si no hay.
fn dump_anns(anns: &[Annotation]) -> String {
    if anns.is_empty() {
        return String::new();
    }
    let mut s = String::from(" (anns");
    for a in anns {
        let args: String = a.args.iter().map(|x| format!(" {}", x)).collect();
        s.push_str(&format!(" (ann {}{})", a.name, args));
    }
    s.push(')');
    s
}

fn dump_fnexpr(fe: &FnExpr) -> String {
    format!(
        "(fn-expr {} (params{}) {} {}){}",
        fe.id,
        dump_params(&fe.params),
        dump_type(&fe.return_type),
        dump_block(&fe.body),
        pos(fe.line, fe.col)
    )
}

fn dump_pattern(pat: &Pattern) -> String {
    let pp = pos(pat.line, pat.col);
    match &pat.kind {
        PatternKind::Wildcard => format!("(wild){}", pp),
        PatternKind::Binding(n) => format!("(bind {}){}", n, pp),
        PatternKind::Variant { enum_name, variant, bindings } => {
            let binds: String = bindings
                .iter()
                .map(|o| match o {
                    Some(n) => format!(" {}", n),
                    None => " _".into(),
                })
                .collect();
            format!("(variant {} {}{}){}", enum_name, variant, binds, pp)
        }
    }
}

fn dump_struct(s: &StructDef) -> String {
    let inner: String = s
        .fields
        .iter()
        .map(|(n, t)| format!(" (field {} {})", n, dump_type(t)))
        .collect();
    format!(
        "(struct{}{} {}{} (fields{})){}",
        dump_pub(s.is_pub),
        dump_anns(&s.annotations),
        s.name,
        dump_generics(&s.type_params, &s.bounds),
        inner,
        pos(s.line, s.col)
    )
}

fn dump_enum(e: &EnumDef) -> String {
    let inner: String = e
        .variants
        .iter()
        .map(|v| {
            let payload: String = v.payload.iter().map(|t| format!(" {}", dump_type(t))).collect();
            format!(" (variant {}{}){}", v.name, payload, pos(v.line, v.col))
        })
        .collect();
    format!(
        "(enum{}{} {}{}{}){}",
        dump_pub(e.is_pub),
        dump_anns(&e.annotations),
        e.name,
        dump_generics(&e.type_params, &e.bounds),
        inner,
        pos(e.line, e.col)
    )
}

/// Cada expresión precedida de un espacio (listas: args, elementos de arreglo).
fn dump_exprs(es: &[Expr]) -> String {
    es.iter().map(|e| format!(" {}", dump_expr(e))).collect()
}

fn dump_opt_expr(o: &Option<Box<Expr>>) -> String {
    match o {
        Some(e) => format!(" {}", dump_expr(e)),
        None => String::new(),
    }
}

fn dump_opt_type(o: &Option<Type>) -> String {
    match o {
        Some(t) => dump_type(t),
        None => "_".into(),
    }
}

fn dump_stmt(st: &Stmt) -> String {
    let pp = pos(st.line, st.col);
    match &st.kind {
        StmtKind::Let { name, ty, value, mutable } => {
            let kw = if *mutable { "var" } else { "let" };
            format!("({} {} {} {}){}", kw, name, dump_opt_type(ty), dump_expr(value), pp)
        }
        StmtKind::Assign { target, value } => {
            format!("(assign {} {}){}", dump_expr(target), dump_expr(value), pp)
        }
        StmtKind::Return { value } => match value {
            Some(e) => format!("(return {}){}", dump_expr(e), pp),
            None => format!("(return){}", pp),
        },
        StmtKind::Expr(e) => format!("(expr {}){}", dump_expr(e), pp),
    }
}

fn dump_block(b: &Block) -> String {
    let mut s = String::from("(block");
    for st in &b.statements {
        s.push(' ');
        s.push_str(&dump_stmt(st));
    }
    if let Some(t) = &b.tail {
        s.push(' ');
        s.push_str(&dump_expr(t));
    }
    s.push(')');
    s.push_str(&pos(b.line, b.col));
    s
}

fn dump_params(ps: &[Param]) -> String {
    ps.iter()
        .map(|pm| format!(" (param {} {}){}", pm.name, dump_type(&pm.ty), pos(pm.line, pm.col)))
        .collect()
}

fn dump_func(f: &Function) -> String {
    format!(
        "(fn{}{} {}{} (params{}) {} {}){}",
        dump_pub(f.is_pub),
        dump_anns(&f.annotations),
        f.name,
        dump_generics(&f.type_params, &f.bounds),
        dump_params(&f.params),
        dump_type(&f.return_type),
        dump_block(&f.body),
        pos(f.line, f.col)
    )
}

fn dump_methodsig(m: &MethodSig) -> String {
    let mut s = format!(
        "(method {} (params{}) {}",
        m.name,
        dump_params(&m.params),
        dump_type(&m.return_type)
    );
    if let Some(b) = &m.default_body {
        s.push_str(&format!(" {}", dump_block(b)));
    }
    s.push(')');
    s.push_str(&pos(m.line, m.col));
    s
}

fn dump_trait(t: &TraitDef) -> String {
    let inner: String = t.methods.iter().map(|m| format!(" {}", dump_methodsig(m))).collect();
    format!("(trait{} {}{}){}", dump_pub(t.is_pub), t.name, inner, pos(t.line, t.col))
}

fn dump_import(d: &ImportDecl) -> String {
    let mut s = format!("(import {}", d.module);
    if let Some(a) = &d.alias {
        s.push_str(&format!(" as {}", a));
    }
    s.push(')');
    s.push_str(&pos(d.line, d.col));
    s
}

fn dump_from_import(fi: &FromImport) -> String {
    let mut names = String::new();
    for n in &fi.names {
        let mut one = format!("(name {}", n.name);
        if let Some(a) = &n.alias {
            one.push_str(&format!(" as {}", a));
        }
        one.push(')');
        names.push_str(&format!(" {}{}", one, pos(n.line, n.col)));
    }
    format!("(from{} {}{}){}", dump_pub(fi.is_pub), fi.module, names, pos(fi.line, fi.col))
}

fn dump_impl(b: &ImplBlock) -> String {
    let inner: String = b.methods.iter().map(|m| format!(" {}", dump_func(m))).collect();
    format!(
        "(impl {}{} {}{}){}",
        b.trait_name,
        dump_generics(&b.type_params, &b.bounds),
        dump_type(&b.target),
        inner,
        pos(b.line, b.col)
    )
}

/// El volcado canónico de un Program (el oráculo). Orden fijo (el driver raylang usa el mismo):
/// imports, from-imports, funciones, structs, enums, traits, impls. M14.2c-2 cierra el parser:
/// ya cubre todo el lenguaje, así que no hay red de seguridad de asserts.
fn dump_program(prog: &Program) -> String {
    let mut out: Vec<String> = Vec::new();
    out.extend(prog.imports.iter().map(dump_import));
    out.extend(prog.from_imports.iter().map(dump_from_import));
    out.extend(prog.functions.iter().map(dump_func));
    out.extend(prog.structs.iter().map(dump_struct));
    out.extend(prog.enums.iter().map(dump_enum));
    out.extend(prog.traits.iter().map(dump_trait));
    out.extend(prog.impls.iter().map(dump_impl));
    out.join("\n")
}

/// La salida canónica del parser de Rust (el oráculo) para una fuente. En el caso feliz, el volcado
/// del AST; ante un error de sintaxis, el `Display` de `ParseError` (`error de sintaxis en L:C: msg`)
/// — exactamente lo que imprime `parse_dump.ray`.
fn canonical(src: &str) -> String {
    let tokens = raylang::lexer::lex(src).expect("el oráculo lexea sin error");
    match raylang::parser::parse(tokens) {
        Ok(prog) => dump_program(&prog),
        Err(e) => format!("{e}"),
    }
}

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Ejecuta el parser auto-alojado sobre `src`: lo escribe a un temporal y corre
/// `raylang selfhost/parse_dump.ray <temporal>`. Devuelve su stdout (sin el salto final).
fn parse_dump(src: &str, nombre_tmp: &str) -> String {
    let mut tmp = std::env::temp_dir();
    tmp.push(nombre_tmp);
    let mut f = std::fs::File::create(&tmp).expect("crea el temporal");
    f.write_all(src.as_bytes()).expect("escribe el temporal");
    drop(f);

    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(repo_path("selfhost/parse_dump.ray"))
        .arg(&tmp)
        .output()
        .expect("ejecuta el parser auto-alojado");
    assert!(
        out.status.success(),
        "parse_dump falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

/// Compara el parser auto-alojado con el oráculo para una fuente concreta.
fn comparar(src: &str, nombre_tmp: &str) {
    let esperado = canonical(src);
    let obtenido = parse_dump(src, nombre_tmp);
    assert_eq!(obtenido, esperado, "el parser auto-alojado difiere del oráculo para:\n{src}");
}

#[test]
fn funcion_minima() {
    comparar("fn main() -> int { 0 }", "sp_min.ray");
    comparar("fn nada() { }", "sp_unit.ray");
}

#[test]
fn precedencia_y_asociatividad() {
    // `1 + 2 * 3` → `1 + (2 * 3)`;  `1 - 2 - 3` → `(1 - 2) - 3`.
    comparar("fn f() -> int { 1 + 2 * 3 }", "sp_prec.ray");
    comparar("fn f() -> int { 1 - 2 - 3 }", "sp_asoc.ray");
    comparar("fn f() -> bool { !a && b || c == d }", "sp_logic.ray");
    comparar("fn f() -> int { -(-x) }", "sp_unary.ray");
}

#[test]
fn llamadas_indexacion_y_campos() {
    comparar("fn f() -> int { g(1, 2) }", "sp_call.ray");
    comparar("fn f() -> int { a[i][j] }", "sp_index.ray");
    comparar("fn f() -> int { p.x.y }", "sp_field.ray");
    comparar("fn f() -> int { obj.metodo(1).campo }", "sp_chain.ray");
    comparar("fn f() -> int { g() }", "sp_call0.ray");
}

#[test]
fn sentencias() {
    comparar("fn f() -> int { let x = 1; let y: int = 2; x + y }", "sp_let.ray");
    comparar("fn f() { var i = 0; i = i + 1; }", "sp_assign.ray");
    comparar("fn f() -> int { return 7; }", "sp_ret.ray");
    comparar("fn f() { return; }", "sp_ret0.ray");
    comparar("fn f() -> int { a[0] = 9; p.x = 1; 0 }", "sp_assign_lvalue.ray");
}

#[test]
fn control_de_flujo() {
    comparar("fn f() -> int { if (c) { 1 } else { 2 } }", "sp_if.ray");
    comparar("fn f() -> int { if (a) { 1 } else if (b) { 2 } else { 3 } }", "sp_elseif.ray");
    comparar("fn f() { while (i < 10) { i = i + 1; } }", "sp_while.ray");
    comparar("fn f() -> int { { let x = 1; x } }", "sp_block.ray");
}

#[test]
fn literales() {
    comparar("fn f() -> [int] { [1, 2, 3] }", "sp_array.ray");
    comparar("fn f() -> [int] { [] }", "sp_array0.ray");
    comparar("fn f() -> float { 3.14 }", "sp_float.ray");
    comparar("fn f() -> string { \"hola\\nmundo\" }", "sp_str.ray");
    comparar("fn f() -> char { 'a' }", "sp_char.ray");
    comparar("fn f() -> bool { true }", "sp_bool.ray");
}

#[test]
fn tipos() {
    comparar("fn f(g: fn(int, bool) -> int, xs: [string]) -> [bool] { [] }", "sp_types.ray");
    comparar("fn f(c: Color) -> Punto { p }", "sp_named.ray");
}

#[test]
fn varias_funciones() {
    comparar(
        "fn a() -> int { 1 }\nfn b(x: int) -> int { x }\nfn main() -> int { a() + b(2) }",
        "sp_multi.ray",
    );
}

#[test]
fn structs_y_enums() {
    comparar("struct Punto { x: int, y: int }", "sp_struct.ray");
    comparar("struct Vacio { }", "sp_struct0.ray");
    comparar("enum Color { Rojo, Verde, Azul }", "sp_enum_unit.ray");
    comparar("enum Figura { Circulo(float), Rect(float, float), Nada }", "sp_enum_payload.ray");
}

#[test]
fn literal_de_struct() {
    comparar("fn f() -> Punto { Punto { x: 1, y: 2 } }", "sp_structlit.ray");
    comparar("fn f() -> Vacio { Vacio { } }", "sp_structlit0.ray");
    comparar("fn f() -> Caja { Caja { v: g(1) + 2 } }", "sp_structlit_expr.ray");
}

#[test]
fn funciones_anonimas() {
    comparar("fn f() -> int { let g = fn(x: int) -> int { x + 1 }; g(2) }", "sp_fnexpr.ray");
    // Anidadas: los ids son densos en pre-orden (exterior < interior).
    comparar("fn f() { let h = fn() { let k = fn() { 0 }; k() }; }", "sp_fnexpr_nested.ray");
    comparar("fn f() { let g = fn() { }; }", "sp_fnexpr_unit.ray");
}

#[test]
fn match_y_patrones() {
    comparar(
        "fn f(o: Figura) -> float { match (o) { Figura.Circulo(r) => r, Figura.Nada => 0.0, _ => 1.0 } }",
        "sp_match.ray",
    );
    comparar(
        "fn f(o: Par) -> int { match (o) { Par.Dos(a, _) => a, x => 0 } }",
        "sp_match_bind.ray",
    );
}

#[test]
fn genericos_en_declaraciones() {
    comparar("fn id<T>(x: T) -> T { x }", "sp_gen_fn.ray");
    comparar("fn dos<A, B>(a: A, b: B) -> A { a }", "sp_gen_fn2.ray");
    comparar("fn f<T: Show + Eq>(x: T) -> T { x }", "sp_gen_bounds.ray");
    comparar("struct Caja<T: Show> { v: T }", "sp_gen_struct.ray");
    comparar("enum Lista<T> { Vacia, Cons(T, Lista<T>) }", "sp_gen_enum.ray");
}

#[test]
fn tipos_genericos_dyn_y_map() {
    comparar("fn f(c: Caja<int>, p: Par<A, [bool]>) -> Map<string, int> { m }", "sp_targs.ray");
    // dyn: el conjunto es canónico (ordenado, sin duplicados): `dyn Show + Eq` → `dyn Eq + Show`.
    comparar("fn f(x: dyn Show + Eq) -> int { 0 }", "sp_dyn.ray");
    comparar("fn f(x: dyn Show) -> int { 0 }", "sp_dyn1.ray");
}

#[test]
fn traits_e_impls() {
    comparar("trait Show { fn mostrar(self) -> string; }", "sp_trait.ray");
    comparar(
        "trait Saludo { fn hola(self) -> string { \"hola\" } fn chau(self) -> string; }",
        "sp_trait_default.ray",
    );
    comparar("impl Show for int { fn mostrar(self) -> string { \"i\" } }", "sp_impl.ray");
    comparar(
        "impl<T: Show> Show for Caja<T> { fn mostrar(self) -> string { self.v.mostrar() } }",
        "sp_impl_gen.ray",
    );
}

#[test]
fn imports_y_modulos() {
    comparar("import geo;\nfn main() -> int { 0 }", "sp_import.ray");
    comparar("import geo/formas/circulo as c;\nfn main() -> int { 0 }", "sp_import_path.ray");
    comparar("from mates import doble, triple as tri;\nfn main() -> int { 0 }", "sp_from.ray");
    comparar("pub from util import area, Punto;\nfn main() -> int { 0 }", "sp_pubfrom.ray");
}

#[test]
fn anotaciones_y_pub() {
    comparar("@test\nfn prueba() -> bool { true }", "sp_ann.ray");
    comparar("@derive(Eq, Show)\nstruct Punto { x: int, y: int }", "sp_derive.ray");
    comparar("pub fn expuesta() -> int { 0 }", "sp_pub_fn.ray");
    comparar("pub struct S { x: int }", "sp_pub_struct.ray");
    comparar("pub enum E { A, B }", "sp_pub_enum.ray");
    comparar("pub trait T { fn m(self) -> int; }", "sp_pub_trait.ray");
}

#[test]
fn azucar_try_y_pipeline() {
    comparar("fn f() -> int { g()? }", "sp_try.ray");
    comparar("fn f() -> int { a()?.b()? }", "sp_try_chain.ray");
    // pipeline desazucara: `x |> g() |> h(1)` ≡ `h(g(x), 1)`.
    comparar("fn f() -> int { x |> g() |> h(1) }", "sp_pipe.ray");
    comparar("fn f() -> int { x |> g }", "sp_pipe_noargs.ray");
}

#[test]
fn referencias_calificadas_por_modulo() {
    comparar("fn f(p: M.Punto) -> M.Color { p }", "sp_qual_type.ray");
    comparar("fn f() -> M.Punto { M.Punto { x: 1 } }", "sp_qual_lit.ray");
    comparar("fn f(c: M.Color) -> int { match (c) { M.Color.Rojo => 0, _ => 1 } }", "sp_qual_pat.ray");
}

/// M14.2d: errores como valores. Ante una entrada inválida, el parser auto-alojado debe producir el
/// MISMO mensaje y ubicación que el de Rust (no abortar con `panic`).
#[test]
fn errores_de_sintaxis_igual_que_el_oraculo() {
    comparar("fn f() -> int { let x = ; }", "spe_expr.ray"); // se esperaba una expresión, … Semicolon
    comparar("1 + 2", "spe_toplevel.ray"); // se esperaba 'fn'
    comparar("fn f( {", "spe_param.ray"); // se esperaba el nombre de un parámetro
    comparar("fn f()", "spe_body.ray"); // se esperaba '{'
    comparar("struct S { x }", "spe_field.ray"); // se esperaba ':' tras el nombre del campo
    comparar("fn f() -> { 0 }", "spe_type.ray"); // se esperaba un tipo (...)
    comparar("fn f() { 1 ", "spe_unclosed.ray"); // se esperaba '}' para cerrar el bloque
    comparar("impl Foo bar Tipo { }", "spe_for.ray"); // se esperaba 'for', no 'bar'
    comparar("@derive(Eq)\ntrait T { }", "spe_ann_trait.ray"); // no se permiten anotaciones sobre un trait
    comparar("pub impl T for X { }", "spe_pub_impl.ray"); // 'pub' no se admite en un impl …
    comparar("fn f() -> int { a b }", "spe_semi.ray"); // se esperaba ';' después de la expresión
}

/// El test fuerte de fidelidad: parsear TODOS los ejemplos reales y los PROPIOS fuentes del
/// self-hosting (el parser se parsea a sí mismo, como el lexer se lexea a sí mismo), exigiendo que
/// el parser en raylang coincida con el de Rust nodo a nodo (posiciones incluidas).
#[test]
fn parsea_archivos_reales_igual_que_el_oraculo() {
    let mut archivos: Vec<String> = Vec::new();
    // El toolchain auto-alojado (lexer/parser/checker/intérprete en raylang) **no soporta `bytes`**
    // (M16 fue solo del lado de Rust; `bytes` en self-hosting es un diferido). Los ejemplos que usan
    // `bytes`/`b"..."` quedan fuera de este corpus, como en su día quedó `mapa.ray` hasta M14.6.
    // M19.3a: tampoco soporta los **operadores bit a bit** (`& | ^ ~ << >>`), también diferidos →
    // se excluyen las librerías cripto de M19.3b (`sha1.ray`/`base64.ray`/`crypto_demo.ray`).
    const DIFERIDOS_SELFHOST: &[&str] = &[
        "binario.ray", "http.ray", "webserver.ray",
        "sha1.ray", "base64.ray", "crypto_demo.ray",
        "sha256.ray", "sha256_demo.ray", // M20.1: SHA-256, usa `bytes` + bitops
        "hex.ray", "hmac.ray", "hmac_demo.ray", // M20.2: HMAC/hex, usa `bytes` + bitops
        "jwt.ray", "jwt_demo.ray", "uuid.ray", // M20.3: JWT/UUID, usa `bytes` + bitops
        "url.ray", "cookie.ray", "url_demo.ray", // M20.4: url/cookies, usa `bytes` + bitops
        "udp.ray", "udp_demo.ray", // M20.8: UDP, usa `bytes`
        "sigv4.ray", "sigv4_demo.ray", // M20.9: AWS SigV4, usa `bytes` + bitops (vía hmac/sha256/url)
        "inflate.ray", "inflate_demo.ray", // M20.10: INFLATE/gzip, usa `bytes` + bitops
        "websocket.ray", "websocket_demo.ray", "websocket_echo.ray",
        "framework.ray", // micro-framework web: usa `bytes` (diferido en el toolchain auto-alojado)
    ];
    // Los ejemplos viven en subdirectorios por categoría (basics/, types/, web/, …) → se recorre
    // `examples/` **recursivamente**. Se saltan los directorios de ejemplos de MÓDULOS (multi-archivo,
    // con fragmentos `mod.ray` no parseables sueltos), que prueba `modules_cli`.
    const DIRS_EXCLUIDOS: &[&str] = &["capsula", "modulos", "proyecto"];
    fn recolectar(dir: &std::path::Path, fuera: &[&str], difer: &[&str], out: &mut Vec<String>) {
        let mut entradas: Vec<_> = std::fs::read_dir(dir).expect("lee dir").filter_map(|e| e.ok()).map(|e| e.path()).collect();
        entradas.sort();
        for p in entradas {
            if p.is_dir() {
                let nombre = p.file_name().unwrap().to_string_lossy().to_string();
                if !fuera.contains(&nombre.as_str()) {
                    recolectar(&p, fuera, difer, out);
                }
            } else if p.extension().map(|x| x == "ray").unwrap_or(false) {
                let nombre = p.file_name().unwrap().to_string_lossy().to_string();
                if !difer.contains(&nombre.as_str()) {
                    // Ruta relativa a la raíz del repo (p. ej. "examples/basics/fib.ray").
                    let raiz = repo_path("");
                    out.push(p.strip_prefix(&raiz).unwrap().to_string_lossy().to_string());
                }
            }
        }
    }
    recolectar(&repo_path("examples"), DIRS_EXCLUIDOS, DIFERIDOS_SELFHOST, &mut archivos);
    // Los propios fuentes del self-hosting: el parser se parsea a sí mismo.
    archivos.push("selfhost/lexer.ray".into());
    archivos.push("selfhost/lex_dump.ray".into());
    archivos.push("selfhost/parser.ray".into());
    archivos.push("selfhost/parse_dump.ray".into());

    for rel in &archivos {
        let src = std::fs::read_to_string(repo_path(rel)).unwrap_or_else(|e| panic!("lee {rel}: {e}"));
        let esperado = canonical(&src);
        let nombre_tmp = format!("sp_real_{}.ray", rel.replace('/', "_"));
        let obtenido = parse_dump(&src, &nombre_tmp);
        assert_eq!(obtenido, esperado, "el parser auto-alojado difiere del oráculo en {rel}");
    }
}
