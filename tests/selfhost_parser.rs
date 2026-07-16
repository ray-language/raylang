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
        Type::Tuple(ts) => format!("(tuple{})", ts.iter().map(|t| format!(" {}", dump_type(t))).collect::<String>()),
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
        other => panic!("el parser no debería producir el type {:?}", other),
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
        ExprKind::Cast { expr, ty } => format!("(as {} {}){}", dump_expr(expr), dump_type(ty), pp),
        ExprKind::Field { object, name } => format!("(field {} {}){}", dump_expr(object), name, pp),
        ExprKind::ArrayLit(elems) => format!("(array{}){}", dump_exprs(elems), pp),
        ExprKind::TupleLit(elems) => format!("(tuple{}){}", dump_exprs(elems), pp),
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
        PatternKind::Variant { enum_name, variant, subpatterns } => {
            // Los sub-patrones planos (binding/`_`) se vuelcan como ` nombre`/` _`, igual que el
            // parser auto-alojado (que no tiene patrones anidados; el corpus no los usa, M40.1c).
            let binds: String = subpatterns
                .iter()
                .map(|sp| match &sp.kind {
                    PatternKind::Binding(n) => format!(" {}", n),
                    PatternKind::Wildcard => " _".into(),
                    _ => format!(" {}", dump_pattern(sp)),
                })
                .collect();
            format!("(variant {} {}{}){}", enum_name, variant, binds, pp)
        }
        // M40.1d: patrón de struct (el toolchain auto-alojado no lo soporta; fuera del corpus).
        PatternKind::Struct { name, fields } => {
            let fs: String = fields.iter().map(|(f, p)| format!(" ({} {})", f, dump_pattern(p))).collect();
            format!("(struct-pat {}{}){}", name, fs, pp)
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
        StmtKind::LetTuple { names, value, mutable } => {
            let kw = if *mutable { "var" } else { "let" };
            let ns: String = names.iter().map(|n| format!(" {}", n.clone().unwrap_or_else(|| "_".to_string()))).collect();
            format!("({} (tuple{}) {}){}", kw, ns, dump_expr(value), pp)
        }
        StmtKind::For { pat, iter, body } => {
            let p = match pat {
                raylang::ast::ForPat::Single(n) => n.clone(),
                raylang::ast::ForPat::Tuple(ns) => format!("(tuple{})", ns.iter().map(|n| format!(" {}", n.clone().unwrap_or_else(|| "_".to_string()))).collect::<String>()),
            };
            let it = match iter {
                raylang::ast::ForIter::Range { start, end } => format!("(range {} {})", dump_expr(start), dump_expr(end)),
                raylang::ast::ForIter::In(e) => dump_expr(e),
                // M40.2: `Iter` lo produce el lowering del checker; el parser (crudo) nunca lo emite.
                raylang::ast::ForIter::Iter { expr, .. } => dump_expr(expr),
            };
            format!("(for {} {} {}){}", p, it, dump_block(body), pp)
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
fn parse_dump(src: &str, name_tmp: &str) -> String {
    let mut tmp = std::env::temp_dir();
    tmp.push(name_tmp);
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
fn compare(src: &str, name_tmp: &str) {
    let expected = canonical(src);
    let obtained = parse_dump(src, name_tmp);
    assert_eq!(obtained, expected, "el parser auto-alojado difiere del oráculo para:\n{src}");
}

#[test]
fn function_minima() {
    compare("fn main() -> int { 0 }", "sp_min.ray");
    compare("fn nada() { }", "sp_unit.ray");
}

#[test]
fn precedence_y_asociatividad() {
    // `1 + 2 * 3` → `1 + (2 * 3)`;  `1 - 2 - 3` → `(1 - 2) - 3`.
    compare("fn f() -> int { 1 + 2 * 3 }", "sp_prec.ray");
    compare("fn f() -> int { 1 - 2 - 3 }", "sp_asoc.ray");
    compare("fn f() -> bool { !a && b || c == d }", "sp_logic.ray");
    compare("fn f() -> int { -(-x) }", "sp_unary.ray");
}

#[test]
fn calls_indexacion_y_fields() {
    compare("fn f() -> int { g(1, 2) }", "sp_call.ray");
    compare("fn f() -> int { a[i][j] }", "sp_index.ray");
    compare("fn f() -> int { p.x.y }", "sp_field.ray");
    compare("fn f() -> int { obj.method(1).campo }", "sp_chain.ray");
    compare("fn f() -> int { g() }", "sp_call0.ray");
}

#[test]
fn statements() {
    compare("fn f() -> int { let x = 1; let y: int = 2; x + y }", "sp_let.ray");
    compare("fn f() { var i = 0; i = i + 1; }", "sp_assign.ray");
    compare("fn f() -> int { return 7; }", "sp_ret.ray");
    compare("fn f() { return; }", "sp_ret0.ray");
    compare("fn f() -> int { a[0] = 9; p.x = 1; 0 }", "sp_assign_lvalue.ray");
}

#[test]
fn control_de_flujo() {
    compare("fn f() -> int { if (c) { 1 } else { 2 } }", "sp_if.ray");
    compare("fn f() -> int { if (a) { 1 } else if (b) { 2 } else { 3 } }", "sp_elseif.ray");
    compare("fn f() { while (i < 10) { i = i + 1; } }", "sp_while.ray");
    compare("fn f() -> int { { let x = 1; x } }", "sp_block.ray");
}

#[test]
fn literals() {
    compare("fn f() -> [int] { [1, 2, 3] }", "sp_array.ray");
    compare("fn f() -> [int] { [] }", "sp_array0.ray");
    compare("fn f() -> float { 3.14 }", "sp_float.ray");
    compare("fn f() -> string { \"hello\\nmundo\" }", "sp_str.ray");
    compare("fn f() -> char { 'a' }", "sp_char.ray");
    compare("fn f() -> bool { true }", "sp_bool.ray");
}

#[test]
fn types() {
    compare("fn f(g: fn(int, bool) -> int, xs: [string]) -> [bool] { [] }", "sp_types.ray");
    compare("fn f(c: Color) -> Punto { p }", "sp_named.ray");
}

#[test]
fn various_functions() {
    compare(
        "fn a() -> int { 1 }\nfn b(x: int) -> int { x }\nfn main() -> int { a() + b(2) }",
        "sp_multi.ray",
    );
}

#[test]
fn structs_y_enums() {
    compare("struct Punto { x: int, y: int }", "sp_struct.ray");
    compare("struct Vacio { }", "sp_struct0.ray");
    compare("enum Color { Rojo, Verde, Azul }", "sp_enum_unit.ray");
    compare("enum Figura { Circulo(float), Rect(float, float), Nada }", "sp_enum_payload.ray");
}

#[test]
fn literal_de_struct() {
    compare("fn f() -> Punto { Punto { x: 1, y: 2 } }", "sp_structlit.ray");
    compare("fn f() -> Vacio { Vacio { } }", "sp_structlit0.ray");
    compare("fn f() -> Caja { Caja { v: g(1) + 2 } }", "sp_structlit_expr.ray");
}

#[test]
fn functions_anonimas() {
    compare("fn f() -> int { let g = fn(x: int) -> int { x + 1 }; g(2) }", "sp_fnexpr.ray");
    // Anidadas: los ids son densos en pre-orden (exterior < interior).
    compare("fn f() { let h = fn() { let k = fn() { 0 }; k() }; }", "sp_fnexpr_nested.ray");
    compare("fn f() { let g = fn() { }; }", "sp_fnexpr_unit.ray");
}

#[test]
fn match_y_patterns() {
    compare(
        "fn f(o: Figura) -> float { match (o) { Figura.Circulo(r) => r, Figura.Nada => 0.0, _ => 1.0 } }",
        "sp_match.ray",
    );
    compare(
        "fn f(o: Par) -> int { match (o) { Par.Dos(a, _) => a, x => 0 } }",
        "sp_match_bind.ray",
    );
}

#[test]
fn generics_en_declaraciones() {
    compare("fn id<T>(x: T) -> T { x }", "sp_gen_fn.ray");
    compare("fn dos<A, B>(a: A, b: B) -> A { a }", "sp_gen_fn2.ray");
    compare("fn f<T: Show + Eq>(x: T) -> T { x }", "sp_gen_bounds.ray");
    compare("struct Caja<T: Show> { v: T }", "sp_gen_struct.ray");
    compare("enum Lista<T> { Vacia, Cons(T, Lista<T>) }", "sp_gen_enum.ray");
}

#[test]
fn types_generics_dyn_y_map() {
    compare("fn f(c: Caja<int>, p: Par<A, [bool]>) -> Map<string, int> { m }", "sp_targs.ray");
    // dyn: el conjunto es canónico (ordenado, sin duplicados): `dyn Show + Eq` → `dyn Eq + Show`.
    compare("fn f(x: dyn Show + Eq) -> int { 0 }", "sp_dyn.ray");
    compare("fn f(x: dyn Show) -> int { 0 }", "sp_dyn1.ray");
}

#[test]
fn traits_e_impls() {
    compare("trait Show { fn show(self) -> string; }", "sp_trait.ray");
    compare(
        "trait Saludo { fn hello(self) -> string { \"hello\" } fn chau(self) -> string; }",
        "sp_trait_default.ray",
    );
    compare("impl Show for int { fn show(self) -> string { \"i\" } }", "sp_impl.ray");
    compare(
        "impl<T: Show> Show for Caja<T> { fn show(self) -> string { self.v.show() } }",
        "sp_impl_gen.ray",
    );
}

#[test]
fn imports_y_modules() {
    compare("import geo;\nfn main() -> int { 0 }", "sp_import.ray");
    compare("import geo/formas/circle as c;\nfn main() -> int { 0 }", "sp_import_path.ray");
    compare("from mates import double, triple as tri;\nfn main() -> int { 0 }", "sp_from.ray");
    compare("pub from util import area, Punto;\nfn main() -> int { 0 }", "sp_pubfrom.ray");
}

#[test]
fn annotations_y_pub() {
    compare("@test\nfn prueba() -> bool { true }", "sp_ann.ray");
    compare("@derive(Eq, Show)\nstruct Punto { x: int, y: int }", "sp_derive.ray");
    compare("pub fn expuesta() -> int { 0 }", "sp_pub_fn.ray");
    compare("pub struct S { x: int }", "sp_pub_struct.ray");
    compare("pub enum E { A, B }", "sp_pub_enum.ray");
    compare("pub trait T { fn m(self) -> int; }", "sp_pub_trait.ray");
}

#[test]
fn azucar_try_y_pipeline() {
    compare("fn f() -> int { g()? }", "sp_try.ray");
    compare("fn f() -> int { a()?.b()? }", "sp_try_chain.ray");
    // pipeline desazucara: `x |> g() |> h(1)` ≡ `h(g(x), 1)`.
    compare("fn f() -> int { x |> g() |> h(1) }", "sp_pipe.ray");
    compare("fn f() -> int { x |> g }", "sp_pipe_noargs.ray");
}

#[test]
fn references_calificadas_por_modulo() {
    compare("fn f(p: M.Punto) -> M.Color { p }", "sp_qual_type.ray");
    compare("fn f() -> M.Punto { M.Punto { x: 1 } }", "sp_qual_lit.ray");
    compare("fn f(c: M.Color) -> int { match (c) { M.Color.Rojo => 0, _ => 1 } }", "sp_qual_pat.ray");
}

/// M14.2d: errores como valores. Ante una entrada inválida, el parser auto-alojado debe producir el
/// MISMO mensaje y ubicación que el de Rust (no abortar con `panic`).
#[test]
fn errors_de_syntax_equal_what_el_oracle() {
    compare("fn f() -> int { let x = ; }", "spe_expr.ray"); // se esperaba una expresión, … Semicolon
    compare("1 + 2", "spe_toplevel.ray"); // se esperaba 'fn'
    compare("fn f( {", "spe_param.ray"); // se esperaba el nombre de un parámetro
    compare("fn f()", "spe_body.ray"); // se esperaba '{'
    compare("struct S { x }", "spe_field.ray"); // se esperaba ':' tras el nombre del campo
    compare("fn f() -> { 0 }", "spe_type.ray"); // se esperaba un tipo (...)
    compare("fn f() { 1 ", "spe_unclosed.ray"); // se esperaba '}' para cerrar el bloque
    compare("impl Foo bar Tipo { }", "spe_for.ray"); // se esperaba 'for', no 'bar'
    compare("@derive(Eq)\ntrait T { }", "spe_ann_trait.ray"); // no se permiten anotaciones sobre un trait
    compare("pub impl T for X { }", "spe_pub_impl.ray"); // 'pub' no se admite en un impl …
    compare("fn f() -> int { a b }", "spe_semi.ray"); // se esperaba ';' después de la expresión
}

/// El test fuerte de fidelidad: parsear TODOS los ejemplos reales y los PROPIOS fuentes del
/// self-hosting (el parser se parsea a sí mismo, como el lexer se lexea a sí mismo), exigiendo que
/// el parser en raylang coincida con el de Rust nodo a nodo (posiciones incluidas).
#[test]
fn parses_files_reales_equal_what_el_oracle() {
    let mut files: Vec<String> = Vec::new();
    // El toolchain auto-alojado (lexer/parser/checker/intérprete en raylang) **no soporta `bytes`**
    // (M16 fue solo del lado de Rust; `bytes` en self-hosting es un diferido). Los ejemplos que usan
    // `bytes`/`b"..."` quedan fuera de este corpus, como en su día quedó `mapa.ray` hasta M14.6.
    // M19.3a: tampoco soporta los **operadores bit a bit** (`& | ^ ~ << >>`), también diferidos →
    // se excluyen las librerías cripto de M19.3b (`sha1.ray`/`base64.ray`/`crypto_demo.ray`).
    const DIFERIDOS_SELFHOST: &[&str] = &[
        "binario.ray", "http.ray", "webserver.ray",
        "tuplas.ray", // M27.1: tuplas (el toolchain auto-alojado aún no las soporta)
        "for_bucles.ray", // M27.2: bucle `for` (el toolchain auto-alojado aún no lo soporta)
        "interpolacion.ray", // M27.3: interpolación de strings (idem)
        "casts.ray", // M27.4: casts `as` (idem; usa `for`/interpolación también)
        "constantes.ray", // M27.5: const de nivel superior (idem)
        "operadores.ray", // M28.1: sobrecarga de operadores (el toolchain auto-alojado aún no la soporta)
        "conversion_error.ray", // M28.2: `?` con From<S> / traits con params de tipo (idem)
        "enteros.ray", // M28.3: enteros con tamaño u8/u32/u64 + `for` (idem)
        "regex.ray", // M29.1c: el motor de regex usa tuplas `Option<(int,int)>` (M27.1, idem)
        "regex_demo.ray", // M29.1: demo del motor de regex; usa interpolación f"..." (idem)
        "regex_captures_demo.ray", // M81: capturas; usa tuplas `p.0` (M27.1, idem) — rojo pre-existente
                                   // descubierto en la limpieza ES→EN (nunca estuvo en la lista)
        "chacha20.ray", // M30.1a: ChaCha20 usa enteros con tamaño u32 (M28.3, idem)
        "chacha20_demo.ray", // M30.1a: demo usa `for`/u32 (idem)
        "poly1305.ray", // M30.1b: Poly1305 usa enteros con tamaño u64 (M28.3, idem)
        "poly1305_demo.ray", // M30.1b: demo usa `for`/u64 (idem)
        "chacha20poly1305.ray", // M30.1c: AEAD (importa chacha20/poly1305 → u32/u64, idem)
        "chacha20poly1305_demo.ray", // M30.1c: demo usa `for`/u32 (idem)
        "sha512.ray", // M30.2a: SHA-512 usa enteros con tamaño u64 (M28.3, idem)
        "sha512_demo.ray", // M30.2a: demo usa `for` (idem)
        "ed25519.ray", // M30.2: importa sha512 (u64); i64 puro pero depende de u64 (idem)
        "ed25519_demo.ray", // M30.2: demo usa `for`/importa ed25519 (idem)
        "jwt_eddsa.ray", // M30.3: JWT EdDSA, importa ed25519 (bitops/u64, idem)
        "jwt_eddsa_demo.ray", // M30.3: demo importa ed25519 (idem)
        "huffman.ray", // M31.1: HPACK-Huffman, usa bitops (& | << >>, idem)
        "huffman_demo.ray", // M31.1: demo usa `for`/importa huffman (idem)
        "http2_client.ray", // M31.2b: cliente HTTP/2, importa http2/hpack (bytes/bitops, idem)
        "http2_get_demo.ray", // M31.2b: demo importa http2_client (idem)
        "h2_alpn_demo.ray", // M31.2a: usa args()/tls_connect_h2 (fuera del subset auto-alojado)
        "grpc_client.ray", // M31.3: cliente gRPC, importa http2/hpack/protobuf (bytes/bitops, idem)
        "grpc_call_demo.ray", // M31.3: demo importa grpc_client (idem)
        "scram.ray", // M32.1a: SCRAM-SHA-256, importa hmac/sha256 (bytes/bitops, idem)
        "scram_demo.ray", // M32.1a: demo importa scram (idem)
        "postgres.ray", // M32.1b: cliente PostgreSQL, importa scram (bytes/bitops, idem)
        "postgres_demo.ray", // M32.1b: demo importa postgres (idem). También cubre, por basename,
                             // examples/db/postgres_demo.ray (M53.2), que usa `for`/paquete `db`.
        "mysql_demo.ray", // M53.1: demo del cliente MySQL (examples/db); usa `for` (M27.2, idem)
        "sqlite_demo.ray", // M53.4: demo del cliente SQLite (examples/db); usa `for` (M27.2, idem)
        "mongo_demo.ray", // M54.3: demo del cliente MongoDB (examples/db); usa `for` (M27.2, idem)
        "log.ray", // M70: el escape \uXXXX de controles usa bitops (>> &) — misma clase que sha1
        "redis.ray", // M69: el framing RESP migró a `bytes` (b"") — misma clase que sha256
        "sha1.ray", "base64.ray", "base64_demo.ray", "crypto_demo.ray",
        "sha256.ray", "sha256_demo.ray", // M20.1: SHA-256, usa `bytes` + bitops
        "hex.ray", "hmac.ray", "hmac_demo.ray", // M20.2: HMAC/hex, usa `bytes` + bitops
        "jwt.ray", "jwt_demo.ray", "uuid.ray", // M20.3: JWT/UUID, usa `bytes` + bitops
        "jwt_eddsa.ray", // M30.3 (M60): anota `bytes` en posición de tipo (frontera base64url)
        "url.ray", "cookie.ray", "url_demo.ray", // M20.4: url/cookies, usa `bytes` + bitops
        "udp.ray", "udp_demo.ray", "udp_yield_demo.ray", // M20.8/M20.11: UDP, usa `bytes`
        "dns.ray", "dns_demo.ray", // M22: cliente DNS, usa `bytes` + bitops
        "dns_cache.ray", "dns_cache_demo.ray", // M22.1: caché DNS (importa dns → bytes/bitops)
        "sigv4.ray", "sigv4_demo.ray", // M20.9: AWS SigV4, usa `bytes` + bitops (vía hmac/sha256/url)
        "inflate.ray", "inflate_demo.ray", // M20.10: INFLATE/gzip, usa `bytes` + bitops
        "deflate.ray", "deflate_demo.ray", // M20-final: encoder DEFLATE, usa `bytes` + bitops
        "websocket.ray", "websocket_demo.ray", "websocket_echo.ray",
        "websocket_client.ray", "websocket_client_demo.ray", // M24: cliente WS, usa `bytes` + bitops
        "framework.ray", // micro-framework web: usa `bytes` (diferido en el toolchain auto-alojado)
        "metrics_server_demo.ray", // M21.3: monta metrics sobre webserver → usa `bytes`
        "oauth2.ray", "oauth2_demo.ray", // M23: cliente OAuth2 (importa http → bytes)
        "protobuf.ray", "protobuf_demo.ray", // M25: códec protobuf, usa `bytes` + bitops
        "http2.ray", "hpack.ray", "http2_demo.ray", // M26: HTTP/2 framing + HPACK, usa `bytes` + bitops
        "iterador.ray", // M40.2: `impl Iterator<int> for …` — args de tipo en el trait de un impl,
                        // aún no soportados por el parser auto-alojado (M14.2c).
        "libm.ray",     // M41: bloques `extern "lib" { … }` (FFI), aún no en el parser auto-alojado.
        "cstrings.ray", // M41: idem.
        "mapa_literal.ray", // M48.2: literal de Map `[k: v]`/`[:]`, aún no en el parser auto-alojado.
    ];
    // Los ejemplos viven en subdirectorios por categoría (basics/, types/, web/, …) → se recorre
    // `examples/` **recursivamente**. Se saltan los directorios de ejemplos de MÓDULOS (multi-archivo,
    // con fragmentos `mod.ray` no parseables sueltos), que prueba `modules_cli`.
    const DIRS_EXCLUIDOS: &[&str] = &["capsule", "modules", "project", "ssr"];
    fn recolectar(dir: &std::path::Path, outside: &[&str], difer: &[&str], out: &mut Vec<String>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).expect("lee dir").filter_map(|e| e.ok()).map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                if !outside.contains(&name.as_str()) {
                    recolectar(&p, outside, difer, out);
                }
            } else if p.extension().map(|x| x == "ray").unwrap_or(false) {
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                if !difer.contains(&name.as_str()) {
                    // Ruta relativa a la raíz del repo (p. ej. "examples/basics/fib.ray").
                    let root = repo_path("");
                    out.push(p.strip_prefix(&root).unwrap().to_string_lossy().to_string());
                }
            }
        }
    }
    recolectar(&repo_path("examples"), DIRS_EXCLUIDOS, DIFERIDOS_SELFHOST, &mut files);
    // Los propios fuentes del self-hosting: el parser se parsea a sí mismo.
    files.push("selfhost/lexer.ray".into());
    files.push("selfhost/lex_dump.ray".into());
    files.push("selfhost/parser.ray".into());
    files.push("selfhost/parse_dump.ray".into());

    for rel in &files {
        let src = std::fs::read_to_string(repo_path(rel)).unwrap_or_else(|e| panic!("lee {rel}: {e}"));
        let expected = canonical(&src);
        let name_tmp = format!("sp_real_{}.ray", rel.replace('/', "_"));
        let obtained = parse_dump(&src, &name_tmp);
        assert_eq!(obtained, expected, "el parser auto-alojado difiere del oráculo en {rel}");
    }
}
