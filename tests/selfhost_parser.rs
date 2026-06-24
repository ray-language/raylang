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
        // El parser (sin checker) produce `Struct(name, args)` para todo identificador en posición
        // de tipo. En M14.2a no hay argumentos de tipo (genéricos → M14.2c).
        Type::Struct(name, args) if args.is_empty() => name.clone(),
        other => panic!("M14.2a no cubre el tipo {:?}", other),
    }
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
    }
}

fn dump_unop(op: &UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "neg",
        UnaryOp::Not => "not",
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
        other => panic!("M14.2b no cubre la expresión {:?}", other),
    }
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
    format!("(struct {} (fields{})){}", s.name, inner, pos(s.line, s.col))
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
    format!("(enum {}{}){}", e.name, inner, pos(e.line, e.col))
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
        "(fn {} (params{}) {} {}){}",
        f.name,
        dump_params(&f.params),
        dump_type(&f.return_type),
        dump_block(&f.body),
        pos(f.line, f.col)
    )
}

/// El volcado canónico de un Program (el oráculo). Orden fijo: funciones, structs, enums (el driver
/// raylang usa el mismo). M14.2b cubre esos tres; traits/impls/genéricos/imports → M14.2c, así que el
/// corpus no debe traerlos (red de seguridad).
fn dump_program(prog: &Program) -> String {
    assert!(prog.traits.is_empty(), "M14.2b: el corpus no debe tener traits");
    assert!(prog.impls.is_empty(), "M14.2b: el corpus no debe tener impls");
    assert!(prog.imports.is_empty(), "M14.2b: el corpus no debe tener imports");
    assert!(prog.from_imports.is_empty(), "M14.2b: el corpus no debe tener from-imports");
    // Las definiciones de tipo de M14.2b no llevan genéricos/anotaciones/pub (→ M14.2c).
    for s in &prog.structs {
        assert!(s.type_params.is_empty() && s.annotations.is_empty() && !s.is_pub, "M14.2b: struct simple");
    }
    for e in &prog.enums {
        assert!(e.type_params.is_empty() && e.annotations.is_empty() && !e.is_pub, "M14.2b: enum simple");
    }
    let mut out: Vec<String> = Vec::new();
    out.extend(prog.functions.iter().map(dump_func));
    out.extend(prog.structs.iter().map(dump_struct));
    out.extend(prog.enums.iter().map(dump_enum));
    out.join("\n")
}

/// La salida canónica del parser de Rust (el oráculo) para una fuente.
fn canonical(src: &str) -> String {
    let tokens = raylang::lexer::lex(src).expect("el oráculo lexea sin error");
    let prog = raylang::parser::parse(tokens).expect("el oráculo parsea sin error");
    dump_program(&prog)
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

/// El test fuerte: parsear archivos REALES (los ejemplos que solo usan features de M14.2a/b) y exigir
/// que el parser en raylang coincida con el de Rust nodo a nodo (posiciones incluidas).
#[test]
fn parsea_archivos_reales_igual_que_el_oraculo() {
    let archivos = [
        "examples/fib.ray",
        "examples/fizzbuzz.ray",
        "examples/enums.ray",
        "examples/match_figuras.ray",
    ];
    for rel in archivos {
        let src = std::fs::read_to_string(repo_path(rel)).unwrap_or_else(|e| panic!("lee {rel}: {e}"));
        let esperado = canonical(&src);
        let nombre_tmp = format!("sp_real_{}.ray", rel.replace('/', "_"));
        let obtenido = parse_dump(&src, &nombre_tmp);
        assert_eq!(obtenido, esperado, "el parser auto-alojado difiere del oráculo en {rel}");
    }
}
