//! Formateador canónico (`rayfmt`, M29.2): reescribe un `.ray` en un estilo único e **idempotente**,
//! sin configuración (estilo `gofmt`). **Cliente externo**: reusa `lexer`+`parser` y hace *pretty-print*
//! del AST; no toca el núcleo (checker/motores). `raylang --fmt <archivo>` imprime la versión formateada.
//!
//! Como trabaja sobre el **AST**, el formateador **normaliza**: descarta comentarios (el lexer no los
//! guarda) y desazucara lo que el parser desazucara (interpolación `"…${x}…"` → `+ to_string(...)`,
//! pipelines `|>` → llamadas). El resultado siempre es válido y `fmt(fmt(x)) == fmt(x)`.

use crate::ast::*;

const INDENT: &str = "    "; // 4 espacios

/// Formatea el código fuente. Devuelve el texto canónico, o un error de lexer/parser (ya formateado).
pub fn format_source(src: &str) -> Result<String, String> {
    let tokens = crate::lexer::lex(src).map_err(|e| e.to_string())?;
    let program = crate::parser::parse(tokens).map_err(|e| e.to_string())?;
    Ok(format_program(&program))
}

/// Un ítem de nivel superior con su línea de origen (para emitirlos en el orden del archivo).
struct TopItem {
    line: usize,
    text: String,
}

fn format_program(p: &Program) -> String {
    let mut items: Vec<TopItem> = Vec::new();

    for it in &p.imports {
        items.push(TopItem { line: it.line, text: fmt_import(it) });
    }
    for it in &p.from_imports {
        items.push(TopItem { line: it.line, text: fmt_from_import(it) });
    }
    for it in &p.consts {
        items.push(TopItem { line: it.line, text: fmt_const(it) });
    }
    for it in &p.structs {
        items.push(TopItem { line: it.line, text: fmt_struct(it) });
    }
    for it in &p.enums {
        items.push(TopItem { line: it.line, text: fmt_enum(it) });
    }
    for it in &p.traits {
        items.push(TopItem { line: it.line, text: fmt_trait(it) });
    }
    for it in &p.impls {
        items.push(TopItem { line: it.line, text: fmt_impl(it) });
    }
    for it in &p.functions {
        items.push(TopItem { line: it.line, text: fmt_function(it) });
    }

    // Orden del archivo: por la línea de origen (los ítems están bucketizados por categoría en el AST).
    items.sort_by_key(|it| it.line);

    let mut out = String::new();
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            out.push('\n'); // línea en blanco entre ítems de nivel superior
        }
        out.push_str(&it.text);
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Imports / const
// ---------------------------------------------------------------------------

fn fmt_import(it: &ImportDecl) -> String {
    match &it.alias {
        Some(a) => format!("import {} as {};", it.module, a),
        None => format!("import {};", it.module),
    }
}

fn fmt_from_import(it: &FromImport) -> String {
    let nombres: Vec<String> = it.names.iter().map(|n| match &n.alias {
        Some(a) => format!("{} as {}", n.name, a),
        None => n.name.clone(),
    }).collect();
    let pref = if it.is_pub { "pub " } else { "" };
    format!("{}from {} import {};", pref, it.module, nombres.join(", "))
}

fn fmt_const(it: &ConstDef) -> String {
    let pref = if it.is_pub { "pub " } else { "" };
    format!("{}const {}: {} = {};", pref, it.name, fmt_type(&it.ty), fmt_expr(&it.value, 0))
}

// ---------------------------------------------------------------------------
// Tipos, genséricos, anotaciones
// ---------------------------------------------------------------------------

/// Los tipos ya se imprimen en sintaxis de fuente por su `Display` (int, `[T]`, `Map<K, V>`,
/// `fn(...) -> R`, `Nombre<A, B>`, `dyn A + B`, `Self`, `u8`…). El AST crudo trae los nombres como
/// `Struct(name, args)`, cuyo `Display` es exactamente la sintaxis del programador.
fn fmt_type(t: &Type) -> String {
    t.to_string()
}

/// `<T, U>` o `<T: A + B, U>` a partir de los parámetros de tipo y sus bounds.
fn fmt_generics(type_params: &[String], bounds: &[(String, String)]) -> String {
    if type_params.is_empty() {
        return String::new();
    }
    let partes: Vec<String> = type_params.iter().map(|tp| {
        let bs: Vec<&str> = bounds.iter().filter(|(p, _)| p == tp).map(|(_, t)| t.as_str()).collect();
        if bs.is_empty() {
            tp.clone()
        } else {
            format!("{}: {}", tp, bs.join(" + "))
        }
    }).collect();
    format!("<{}>", partes.join(", "))
}

fn fmt_annotations(anns: &[Annotation]) -> String {
    let mut s = String::new();
    for a in anns {
        if a.args.is_empty() {
            s.push_str(&format!("@{}\n", a.name));
        } else {
            s.push_str(&format!("@{}({})\n", a.name, a.args.join(", ")));
        }
    }
    s
}

fn fmt_params(params: &[Param]) -> String {
    let partes: Vec<String> = params.iter().map(|p| {
        // El receptor `self` de un método se imprime sin tipo.
        if p.name == "self" && matches!(p.ty, Type::SelfType) {
            "self".to_string()
        } else {
            format!("{}: {}", p.name, fmt_type(&p.ty))
        }
    }).collect();
    partes.join(", ")
}

/// `-> T` salvo que el retorno sea unit (se omite, canónico).
fn fmt_return(ret: &Type) -> String {
    if matches!(ret, Type::Unit) {
        String::new()
    } else {
        format!(" -> {}", fmt_type(ret))
    }
}

// ---------------------------------------------------------------------------
// struct / enum / trait / impl / función
// ---------------------------------------------------------------------------

fn fmt_struct(it: &StructDef) -> String {
    let mut s = fmt_annotations(&it.annotations);
    let pref = if it.is_pub { "pub " } else { "" };
    let gens = fmt_generics(&it.type_params, &it.bounds);
    if it.fields.is_empty() {
        s.push_str(&format!("{}struct {}{} {{ }}", pref, it.name, gens));
        return s;
    }
    s.push_str(&format!("{}struct {}{} {{\n", pref, it.name, gens));
    for (n, ty) in &it.fields {
        s.push_str(&format!("{}{}: {},\n", INDENT, n, fmt_type(ty)));
    }
    s.push('}');
    s
}

fn fmt_enum(it: &EnumDef) -> String {
    let mut s = fmt_annotations(&it.annotations);
    let pref = if it.is_pub { "pub " } else { "" };
    let gens = fmt_generics(&it.type_params, &it.bounds);
    if it.variants.is_empty() {
        s.push_str(&format!("{}enum {}{} {{ }}", pref, it.name, gens));
        return s;
    }
    s.push_str(&format!("{}enum {}{} {{\n", pref, it.name, gens));
    for v in &it.variants {
        if v.payload.is_empty() {
            s.push_str(&format!("{}{},\n", INDENT, v.name));
        } else {
            let tys: Vec<String> = v.payload.iter().map(fmt_type).collect();
            s.push_str(&format!("{}{}({}),\n", INDENT, v.name, tys.join(", ")));
        }
    }
    s.push('}');
    s
}

fn fmt_trait(it: &TraitDef) -> String {
    let pref = if it.is_pub { "pub " } else { "" };
    let gens = fmt_generics(&it.type_params, &[]);
    if it.methods.is_empty() {
        return format!("{}trait {}{} {{ }}", pref, it.name, gens);
    }
    let mut s = format!("{}trait {}{} {{\n", pref, it.name, gens);
    for m in &it.methods {
        s.push_str(INDENT);
        s.push_str(&fmt_method_sig(m));
        s.push('\n');
    }
    s.push('}');
    s
}

fn fmt_method_sig(m: &MethodSig) -> String {
    let head = format!("fn {}({}){}", m.name, fmt_params(&m.params), fmt_return(&m.return_type));
    match &m.default_body {
        Some(body) => format!("{} {}", head, fmt_block(body, 1)),
        None => format!("{};", head),
    }
}

fn fmt_impl(it: &ImplBlock) -> String {
    let gens = fmt_generics(&it.type_params, &it.bounds);
    let trait_args = if it.trait_args.is_empty() {
        String::new()
    } else {
        let a: Vec<String> = it.trait_args.iter().map(fmt_type).collect();
        format!("<{}>", a.join(", "))
    };
    let head = format!("impl{} {}{} for {}", gens, it.trait_name, trait_args, fmt_type(&it.target));
    if it.methods.is_empty() {
        return format!("{} {{ }}", head);
    }
    let mut s = format!("{} {{\n", head);
    for (i, m) in it.methods.iter().enumerate() {
        if i > 0 {
            s.push('\n');
        }
        s.push_str(&indent_lines(&fmt_function(m), 1));
        s.push('\n');
    }
    s.push('}');
    s
}

fn fmt_function(f: &Function) -> String {
    let mut s = fmt_annotations(&f.annotations);
    let pref = if f.is_pub { "pub " } else { "" };
    let gens = fmt_generics(&f.type_params, &f.bounds);
    s.push_str(&format!(
        "{}fn {}{}({}){} {}",
        pref, f.name, gens, fmt_params(&f.params), fmt_return(&f.return_type), fmt_block(&f.body, 0)
    ));
    s
}

// ---------------------------------------------------------------------------
// Bloques y sentencias
// ---------------------------------------------------------------------------

/// Formatea un bloque. `base` es el nivel de indentación de la línea que abre el `{`; el contenido
/// va a `base + 1`, y el `}` de cierre vuelve a `base`.
fn fmt_block(b: &Block, base: usize) -> String {
    if b.statements.is_empty() && b.tail.is_none() {
        return "{ }".to_string();
    }
    let inner = INDENT.repeat(base + 1);
    let mut s = String::from("{\n");
    for st in &b.statements {
        s.push_str(&inner);
        s.push_str(&fmt_stmt(st, base + 1));
        s.push('\n');
    }
    if let Some(tail) = &b.tail {
        s.push_str(&inner);
        s.push_str(&fmt_value(tail, base + 1));
        s.push('\n');
    }
    s.push_str(&INDENT.repeat(base));
    s.push('}');
    s
}

fn fmt_stmt(st: &Stmt, indent: usize) -> String {
    match &st.kind {
        StmtKind::Let { name, ty, value, mutable } => {
            let kw = if *mutable { "var" } else { "let" };
            let anno = match ty {
                Some(t) => format!(": {}", fmt_type(t)),
                None => String::new(),
            };
            format!("{} {}{} = {};", kw, name, anno, fmt_value(value, indent))
        }
        StmtKind::LetTuple { names, value, mutable } => {
            let kw = if *mutable { "var" } else { "let" };
            let ns: Vec<String> = names.iter().map(|n| n.clone().unwrap_or_else(|| "_".to_string())).collect();
            format!("{} ({}) = {};", kw, ns.join(", "), fmt_value(value, indent))
        }
        StmtKind::For { pat, iter, body } => {
            let p = match pat {
                ForPat::Single(n) => n.clone(),
                ForPat::Tuple(names) => {
                    let ns: Vec<String> = names.iter().map(|n| n.clone().unwrap_or_else(|| "_".to_string())).collect();
                    format!("({})", ns.join(", "))
                }
            };
            let it = match iter {
                ForIter::Range { start, end } => format!("{}..{}", fmt_expr(start, 0), fmt_expr(end, 0)),
                ForIter::In(e) => fmt_expr(e, 0),
            };
            format!("for {} in {} {}", p, it, fmt_block(body, indent))
        }
        StmtKind::Assign { target, value } => {
            format!("{} = {};", fmt_expr(target, 0), fmt_value(value, indent))
        }
        StmtKind::Return { value } => match value {
            Some(e) => format!("return {};", fmt_value(e, indent)),
            None => "return;".to_string(),
        },
        StmtKind::Expr(e) => {
            // Las formas con bloque (if/while/match/bloque) como sentencia no llevan `;`.
            if is_block_form(e) {
                fmt_expr_indented(e, indent)
            } else {
                format!("{};", fmt_expr(e, 0))
            }
        }
    }
}

fn is_block_form(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::If { .. } | ExprKind::While { .. } | ExprKind::Match { .. } | ExprKind::Block(_))
}

/// Formatea una expresión en **posición de valor** (tail de bloque, inicializador de `let`, valor de
/// asignación/`return`, brazo de `match`): si es una forma con bloque se indenta a `ind`; si no, se
/// imprime en línea.
fn fmt_value(e: &Expr, ind: usize) -> String {
    if is_block_form(e) {
        fmt_expr_indented(e, ind)
    } else {
        fmt_expr(e, 0)
    }
}

// ---------------------------------------------------------------------------
// Expresiones (con precedencia para insertar el mínimo de paréntesis)
// ---------------------------------------------------------------------------

/// Precedencia de un operador binario (mayor = liga más fuerte). Espeja la jerarquía del parser.
fn bin_prec(op: BinaryOp) -> u8 {
    use BinaryOp::*;
    match op {
        Or => 1,
        And => 2,
        BitOr => 3,
        BitXor => 4,
        BitAnd => 5,
        Eq | Ne => 6,
        Lt | Le | Gt | Ge => 7,
        Shl | Shr => 8,
        Add | Sub => 9,
        Mul | Div | Rem => 10,
    }
}

/// La precedencia "propia" de una expresión (para decidir paréntesis en el padre).
fn expr_prec(e: &Expr) -> u8 {
    match &e.kind {
        ExprKind::Binary { op, .. } => bin_prec(*op),
        ExprKind::Cast { .. } => 11,
        ExprKind::Unary { .. } => 12,
        ExprKind::Call { .. } | ExprKind::Index { .. } | ExprKind::Field { .. }
        | ExprKind::Try(_) | ExprKind::EnumLit { .. } => 13,
        _ => 100, // primarias (literales, ident, array, tupla, struct lit, func, if/while/match/block)
    }
}

/// Formatea `e`; si su precedencia es menor que `min_prec`, lo envuelve en paréntesis.
fn fmt_expr(e: &Expr, min_prec: u8) -> String {
    let s = fmt_expr_raw(e);
    if expr_prec(e) < min_prec {
        format!("({})", s)
    } else {
        s
    }
}

fn bin_op_str(op: BinaryOp) -> &'static str {
    use BinaryOp::*;
    match op {
        Add => "+", Sub => "-", Mul => "*", Div => "/", Rem => "%",
        Eq => "==", Ne => "!=", Lt => "<", Le => "<=", Gt => ">", Ge => ">=",
        And => "&&", Or => "||",
        BitAnd => "&", BitOr => "|", BitXor => "^", Shl => "<<", Shr => ">>",
    }
}

fn unary_op_str(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "!",
        UnaryOp::BitNot => "~",
    }
}

fn fmt_expr_raw(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Int(n) => n.to_string(),
        ExprKind::Float(f) => fmt_float(*f),
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Str(s) => fmt_string_lit(s),
        ExprKind::Char(c) => fmt_char_lit(*c),
        ExprKind::Bytes(b) => fmt_bytes_lit(b),
        ExprKind::Ident(n) => n.clone(),
        ExprKind::Unary { op, expr } => {
            format!("{}{}", unary_op_str(*op), fmt_expr(expr, 12))
        }
        ExprKind::Binary { op, left, right } => {
            let p = bin_prec(*op);
            // Asociativo a la izquierda: la izquierda admite igual precedencia; la derecha, mayor.
            format!("{} {} {}", fmt_expr(left, p), bin_op_str(*op), fmt_expr(right, p + 1))
        }
        ExprKind::Call { callee, args } => {
            let a: Vec<String> = args.iter().map(|x| fmt_expr(x, 0)).collect();
            format!("{}({})", fmt_expr(callee, 13), a.join(", "))
        }
        ExprKind::ArrayLit(elems) => {
            let a: Vec<String> = elems.iter().map(|x| fmt_expr(x, 0)).collect();
            format!("[{}]", a.join(", "))
        }
        ExprKind::TupleLit(elems) => {
            let a: Vec<String> = elems.iter().map(|x| fmt_expr(x, 0)).collect();
            format!("({})", a.join(", "))
        }
        ExprKind::Index { array, index } => {
            format!("{}[{}]", fmt_expr(array, 13), fmt_expr(index, 0))
        }
        ExprKind::Cast { expr, ty } => {
            format!("{} as {}", fmt_expr(expr, 12), fmt_type(ty))
        }
        ExprKind::StructLit { name, fields } => {
            if fields.is_empty() {
                format!("{} {{ }}", name)
            } else {
                let fs: Vec<String> = fields.iter().map(|(n, v)| format!("{}: {}", n, fmt_expr(v, 0))).collect();
                format!("{} {{ {} }}", name, fs.join(", "))
            }
        }
        ExprKind::Field { object, name } => {
            format!("{}.{}", fmt_expr(object, 13), name)
        }
        ExprKind::EnumLit { enum_name, variant, args } => {
            if args.is_empty() {
                format!("{}.{}", enum_name, variant)
            } else {
                let a: Vec<String> = args.iter().map(|x| fmt_expr(x, 0)).collect();
                format!("{}.{}({})", enum_name, variant, a.join(", "))
            }
        }
        ExprKind::Func(fe) => {
            format!("fn({}){} {}", fmt_params(&fe.params), fmt_return(&fe.return_type), fmt_block(&fe.body, 0))
        }
        ExprKind::Try(inner) => format!("{}?", fmt_expr(inner, 13)),
        ExprKind::Match { .. } | ExprKind::If { .. } | ExprKind::While { .. } | ExprKind::Block(_) => {
            // Estas formas son multilínea; se formatean con indentación explícita (nivel 0 aquí).
            fmt_expr_indented(e, 0)
        }
    }
}

/// Formatea una forma con bloque (if/while/match/block) con la indentación `base` (la de su línea).
fn fmt_expr_indented(e: &Expr, base: usize) -> String {
    match &e.kind {
        ExprKind::If { cond, then_branch, else_branch } => {
            let mut s = format!("if ({}) {}", fmt_expr(cond, 0), fmt_block(then_branch, base));
            if let Some(eb) = else_branch {
                match &eb.kind {
                    // `else if ...`: se encadena sin bloque intermedio.
                    ExprKind::If { .. } => {
                        s.push_str(" else ");
                        s.push_str(&fmt_expr_indented(eb, base));
                    }
                    ExprKind::Block(b) => {
                        s.push_str(" else ");
                        s.push_str(&fmt_block(b, base));
                    }
                    _ => {
                        // Un else con una expresión no-bloque: envolver en bloque canónico.
                        s.push_str(&format!(" else {{\n{}{}\n{}}}",
                            INDENT.repeat(base + 1), fmt_expr(eb, 0), INDENT.repeat(base)));
                    }
                }
            }
            s
        }
        ExprKind::While { cond, body } => {
            format!("while ({}) {}", fmt_expr(cond, 0), fmt_block(body, base))
        }
        ExprKind::Block(b) => fmt_block(b, base),
        ExprKind::Match { scrutinee, arms } => {
            let inner = INDENT.repeat(base + 1);
            let mut s = format!("match ({}) {{\n", fmt_expr(scrutinee, 0));
            for arm in arms {
                let body = if is_block_form(&arm.body) {
                    fmt_expr_indented(&arm.body, base + 1)
                } else {
                    fmt_expr(&arm.body, 0)
                };
                s.push_str(&format!("{}{} => {},\n", inner, fmt_pattern(&arm.pattern), body));
            }
            s.push_str(&INDENT.repeat(base));
            s.push('}');
            s
        }
        _ => fmt_expr(e, 0),
    }
}

fn fmt_pattern(p: &Pattern) -> String {
    match &p.kind {
        PatternKind::Wildcard => "_".to_string(),
        PatternKind::Binding(n) => n.clone(),
        PatternKind::Variant { enum_name, variant, bindings } => {
            if bindings.is_empty() {
                format!("{}.{}", enum_name, variant)
            } else {
                let bs: Vec<String> = bindings.iter().map(|b| b.clone().unwrap_or_else(|| "_".to_string())).collect();
                format!("{}.{}({})", enum_name, variant, bs.join(", "))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Literales y utilidades
// ---------------------------------------------------------------------------

/// Reproduce un flotante de forma que el lexer lo relea como flotante (siempre con `.`).
fn fmt_float(f: f64) -> String {
    let s = format!("{:?}", f); // el Debug de f64 conserva el punto decimal (3.0, no "3")
    s
}

fn escape_char(c: char, out: &mut String, en_comillas_dobles: bool) {
    match c {
        '\\' => out.push_str("\\\\"),
        '\n' => out.push_str("\\n"),
        '\t' => out.push_str("\\t"),
        '\r' => out.push_str("\\r"),
        '"' if en_comillas_dobles => out.push_str("\\\""),
        '\'' if !en_comillas_dobles => out.push_str("\\'"),
        other => out.push(other),
    }
}

fn fmt_string_lit(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        escape_char(c, &mut out, true);
    }
    out.push('"');
    out
}

fn fmt_char_lit(c: char) -> String {
    let mut out = String::from("'");
    escape_char(c, &mut out, false);
    out.push('\'');
    out
}

fn fmt_bytes_lit(b: &[u8]) -> String {
    let mut out = String::from("b\"");
    for &byte in b {
        escape_char(byte as char, &mut out, true);
    }
    out.push('"');
    out
}

/// Reindenta cada línea no vacía de `text` añadiendo `levels` niveles de sangría. Se usa para meter el
/// cuerpo de un método dentro de un `impl`.
fn indent_lines(text: &str, levels: usize) -> String {
    let pad = INDENT.repeat(levels);
    let mut out = String::new();
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.is_empty() {
            // deja las líneas en blanco sin sangría
        } else {
            out.push_str(&pad);
            out.push_str(line);
        }
    }
    out
}
