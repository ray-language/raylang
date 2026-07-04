//! Formateador canónico (`rayfmt`, M29.2): reescribe un `.ray` en un estilo único e **idempotente**,
//! sin configuración (estilo `gofmt`). **Cliente externo**: reusa `lexer`+`parser` y hace *pretty-print*
//! del AST; no toca el núcleo (checker/motores). `ray fmt <archivo>` imprime la versión formateada.
//!
//! Como trabaja sobre el **AST**, el formateador **normaliza**: desazucara lo que el parser desazucara
//! (interpolación `"…${x}…"` → `+ to_string(...)`, pipelines `|>` → llamadas). El resultado siempre es
//! válido y `fmt(fmt(x)) == fmt(x)`.
//!
//! **Comentarios.** El lexer los descarta, así que se recolectan aparte (`collect_comments`, respetando
//! cadenas/chars) y se **re-insertan** durante la emisión (`Cur`): antes de cada ítem/sentencia/miembro
//! se vuelcan los comentarios de las líneas anteriores (con su sangría), y un comentario al final de una
//! línea de código (*trailing*) se re-pega a esa línea. Como el AST **no guarda la posición del `}`**, un
//! comentario tras la última sentencia de un bloque no puede acotarse a ese bloque y se reubica justo
//! **tras** el `}` (el resto va bien). La invariante fuerte: **ningún comentario se pierde**.

use crate::ast::*;

const INDENT: &str = "    "; // 4 espacios

/// Formatea el código fuente. Devuelve el texto canónico, o un error de lexer/parser (ya formateado).
pub fn format_source(src: &str) -> Result<String, String> {
    let tokens = crate::lexer::lex(src).map_err(|e| e.to_string())?;
    let program = crate::parser::parse(tokens).map_err(|e| e.to_string())?;
    let mut cur = Cur::new(src);
    Ok(format_program(&program, &mut cur))
}

// ---------------------------------------------------------------------------
// Comentarios: recolección y re-inserción
// ---------------------------------------------------------------------------

/// Un comentario `//…` del fuente: su línea (1-basada), su texto (desde `//`, sin espacios al final) y
/// si es **trailing** (había código antes en la misma línea) o **suelto** (línea solo de comentario).
struct Comment {
    line: usize,
    text: String,
    trailing: bool,
}

/// Recolecta los comentarios `//` del fuente, respetando cadenas `"…"` y chars `'…'` (un `//` dentro de
/// un literal no es comentario). raylang no tiene comentarios de bloque, así que cada uno llega al fin de
/// línea. Como los literales de cadena no cruzan líneas, el estado se reinicia en cada `\n` (defensivo).
fn collect_comments(src: &str) -> Vec<Comment> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1usize;
    let mut line_tiene_codigo = false;
    let (mut en_str, mut en_char) = (false, false);
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            line_tiene_codigo = false;
            en_str = false;
            en_char = false;
            i += 1;
            continue;
        }
        if en_str || en_char {
            if c == '\\' {
                i += 2; // salta el carácter escapado
                continue;
            }
            if (en_str && c == '"') || (en_char && c == '\'') {
                en_str = false;
                en_char = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                en_str = true;
                line_tiene_codigo = true;
                i += 1;
            }
            '\'' => {
                en_char = true;
                line_tiene_codigo = true;
                i += 1;
            }
            '/' if i + 1 < chars.len() && chars[i + 1] == '/' => {
                let mut j = i;
                while j < chars.len() && chars[j] != '\n' {
                    j += 1;
                }
                let text: String = chars[i..j].iter().collect::<String>().trim_end().to_string();
                out.push(Comment { line, text, trailing: line_tiene_codigo });
                i = j; // al `\n` (o EOF)
            }
            _ => {
                if !c.is_whitespace() {
                    line_tiene_codigo = true;
                }
                i += 1;
            }
        }
    }
    out
}

/// Cursor sobre los comentarios recolectados, consumidos **en orden de fuente** conforme el formateador
/// emite los constructos (que se recorren en ese mismo orden). Ver el `//!` del módulo.
struct Cur {
    items: Vec<Comment>,
    i: usize,
}

impl Cur {
    fn new(src: &str) -> Self {
        Cur { items: collect_comments(src), i: 0 }
    }

    /// Vuelca los comentarios **sueltos** de las líneas anteriores a `line` (los aún no emitidos), cada
    /// uno en su propia línea con la sangría `pad`. Es lo que va **encima** de un constructo. (Un
    /// *trailing* no consumido de un constructo multilínea también cae aquí, en su propia línea → nunca
    /// se pierde.)
    fn flush_before(&mut self, line: usize, pad: &str) -> String {
        let mut s = String::new();
        while self.i < self.items.len() && self.items[self.i].line < line {
            s.push_str(pad);
            s.push_str(&self.items[self.i].text);
            s.push('\n');
            self.i += 1;
        }
        s
    }

    /// Un comentario **trailing** en la línea `line` (código antes en esa línea). Se consume y se
    /// devuelve como `  // …` (dos espacios), para re-pegarlo al final de la línea de código emitida.
    fn trailing_on(&mut self, line: usize) -> String {
        if self.i < self.items.len() && self.items[self.i].line == line && self.items[self.i].trailing {
            let t = format!("  {}", self.items[self.i].text);
            self.i += 1;
            t
        } else {
            String::new()
        }
    }

    /// Vuelca **todos** los comentarios restantes (fin de archivo), cada uno en su línea con sangría `pad`.
    fn flush_rest(&mut self, pad: &str) -> String {
        let mut s = String::new();
        while self.i < self.items.len() {
            s.push_str(pad);
            s.push_str(&self.items[self.i].text);
            s.push('\n');
            self.i += 1;
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Programa (ítems de nivel superior)
// ---------------------------------------------------------------------------

/// Una referencia a un ítem de nivel superior, con su línea de origen, para emitirlos en orden de
/// archivo **y** avanzar el cursor de comentarios monótonamente (por eso se ordena ANTES de emitir).
enum Top<'a> {
    Import(&'a ImportDecl),
    FromImport(&'a FromImport),
    Const(&'a ConstDef),
    Struct(&'a StructDef),
    Enum(&'a EnumDef),
    Trait(&'a TraitDef),
    Impl(&'a ImplBlock),
    Fn(&'a Function),
    Extern(String), // nombre de la librería (bloque `extern "lib" { … }`)
}

fn format_program(p: &Program, cur: &mut Cur) -> String {
    // 1. Recolectar (línea, ítem) SIN formatear todavía, para ordenarlos por línea.
    let mut tops: Vec<(usize, Top)> = Vec::new();
    for it in &p.imports {
        tops.push((it.line, Top::Import(it)));
    }
    for it in &p.from_imports {
        tops.push((it.line, Top::FromImport(it)));
    }
    for it in &p.consts {
        tops.push((it.line, Top::Const(it)));
    }
    for it in &p.structs {
        tops.push((it.line, Top::Struct(it)));
    }
    for it in &p.enums {
        tops.push((it.line, Top::Enum(it)));
    }
    for it in &p.traits {
        tops.push((it.line, Top::Trait(it)));
    }
    for it in &p.impls {
        tops.push((it.line, Top::Impl(it)));
    }
    for it in &p.functions {
        tops.push((it.line, Top::Fn(it)));
    }
    // M41: los `extern "lib" { … }` se reagrupan por librería (orden de primera aparición).
    for (lib, line) in extern_libs_en_orden(&p.externs) {
        tops.push((line, Top::Extern(lib)));
    }
    tops.sort_by_key(|(line, _)| *line);

    // 2. Emitir en orden de fuente, volcando los comentarios de encima de cada ítem (y el trailing si el
    //    ítem cabe en una línea). El cursor avanza monótonamente porque recorremos por línea creciente.
    let mut out = String::new();
    for (idx, (line, top)) in tops.iter().enumerate() {
        let comentarios = cur.flush_before(*line, "");
        if idx > 0 {
            out.push('\n'); // línea en blanco entre ítems de nivel superior
        }
        out.push_str(&comentarios);
        let text = match top {
            Top::Import(it) => fmt_import(it),
            Top::FromImport(it) => fmt_from_import(it),
            Top::Const(it) => fmt_const(cur, it),
            Top::Struct(it) => fmt_struct(cur, it),
            Top::Enum(it) => fmt_enum(cur, it),
            Top::Trait(it) => fmt_trait(cur, it),
            Top::Impl(it) => fmt_impl(cur, it),
            Top::Fn(it) => fmt_function(cur, it),
            Top::Extern(lib) => fmt_extern_block(lib, &p.externs),
        };
        out.push_str(&text);
        if !text.contains('\n') {
            out.push_str(&cur.trailing_on(*line));
        }
        out.push('\n');
    }
    // 3. Comentarios sueltos al final del archivo (tras el último ítem).
    if tops.is_empty() {
        out.push_str(&cur.flush_rest(""));
    } else {
        let cola = cur.flush_rest("");
        if !cola.is_empty() {
            out.push('\n');
            out.push_str(&cola);
        }
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

fn fmt_const(cur: &mut Cur, it: &ConstDef) -> String {
    let pref = if it.is_pub { "pub " } else { "" };
    format!("{}const {}: {} = {};", pref, it.name, fmt_type(&it.ty), fmt_expr(cur, &it.value, 0))
}

/// Las librerías de los bloques `extern` en orden de primera aparición, con la línea de esa primera
/// firma (para ordenar el bloque entre los ítems de nivel superior). (M41)
fn extern_libs_en_orden(externs: &[ExternFn]) -> Vec<(String, usize)> {
    let mut vistos: Vec<(String, usize)> = Vec::new();
    for e in externs {
        if !vistos.iter().any(|(l, _)| *l == e.lib) {
            vistos.push((e.lib.clone(), e.line));
        }
    }
    vistos
}

/// Formatea un bloque `extern "lib" { fn …; … }` con todas las firmas de esa librería. (M41)
fn fmt_extern_block(lib: &str, externs: &[ExternFn]) -> String {
    let mut s = format!("extern {:?} {{\n", lib);
    for e in externs.iter().filter(|e| e.lib == lib) {
        s.push_str(&format!(
            "{}fn {}({}){};\n",
            INDENT, e.name, fmt_params(&e.params), fmt_return(&e.return_type)
        ));
    }
    s.push('}');
    s
}

// ---------------------------------------------------------------------------
// Tipos, genéricos, anotaciones
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

fn fmt_struct(_cur: &mut Cur, it: &StructDef) -> String {
    // Nota: los campos del struct son `(nombre, tipo)` **sin posición** en el AST, así que un comentario
    // entre campos no puede acotarse a su campo → lo volcará el contexto tras el `}` (nunca se pierde).
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

fn fmt_enum(cur: &mut Cur, it: &EnumDef) -> String {
    let mut s = fmt_annotations(&it.annotations);
    let pref = if it.is_pub { "pub " } else { "" };
    let gens = fmt_generics(&it.type_params, &it.bounds);
    if it.variants.is_empty() {
        s.push_str(&format!("{}enum {}{} {{ }}", pref, it.name, gens));
        return s;
    }
    s.push_str(&format!("{}enum {}{} {{\n", pref, it.name, gens));
    for v in &it.variants {
        s.push_str(&cur.flush_before(v.line, INDENT)); // comentarios encima de la variante
        if v.payload.is_empty() {
            s.push_str(&format!("{}{},", INDENT, v.name));
        } else {
            let tys: Vec<String> = v.payload.iter().map(fmt_type).collect();
            s.push_str(&format!("{}{}({}),", INDENT, v.name, tys.join(", ")));
        }
        s.push_str(&cur.trailing_on(v.line)); // comentario al final de la línea de la variante
        s.push('\n');
    }
    s.push('}');
    s
}

fn fmt_trait(cur: &mut Cur, it: &TraitDef) -> String {
    let pref = if it.is_pub { "pub " } else { "" };
    let gens = fmt_generics(&it.type_params, &[]);
    if it.methods.is_empty() {
        return format!("{}trait {}{} {{ }}", pref, it.name, gens);
    }
    let mut s = format!("{}trait {}{} {{\n", pref, it.name, gens);
    for m in &it.methods {
        s.push_str(&cur.flush_before(m.line, INDENT)); // comentarios encima de la firma
        s.push_str(INDENT);
        let sig = fmt_method_sig(cur, m);
        s.push_str(&sig);
        if !sig.contains('\n') {
            s.push_str(&cur.trailing_on(m.line));
        }
        s.push('\n');
    }
    s.push('}');
    s
}

fn fmt_method_sig(cur: &mut Cur, m: &MethodSig) -> String {
    // M40.2c: métodos genéricos — renderizar los parámetros de tipo propios (`fn map<U>`).
    let gens = fmt_generics(&m.type_params, &m.bounds);
    let head = format!("fn {}{}({}){}", m.name, gens, fmt_params(&m.params), fmt_return(&m.return_type));
    match &m.default_body {
        Some(body) => format!("{} {}", head, fmt_block(cur, body, 1)),
        None => format!("{};", head),
    }
}

fn fmt_impl(cur: &mut Cur, it: &ImplBlock) -> String {
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
        let comentarios = cur.flush_before(m.line, INDENT); // comentarios encima del método (1 nivel)
        if i > 0 {
            s.push('\n');
        }
        s.push_str(&comentarios);
        s.push_str(&indent_lines(&fmt_function(cur, m), 1));
        s.push('\n');
    }
    s.push('}');
    s
}

fn fmt_function(cur: &mut Cur, f: &Function) -> String {
    let mut s = fmt_annotations(&f.annotations);
    let pref = if f.is_pub { "pub " } else { "" };
    let gens = fmt_generics(&f.type_params, &f.bounds);
    s.push_str(&format!(
        "{}fn {}{}({}){} {}",
        pref, f.name, gens, fmt_params(&f.params), fmt_return(&f.return_type), fmt_block(cur, &f.body, 0)
    ));
    s
}

// ---------------------------------------------------------------------------
// Bloques y sentencias
// ---------------------------------------------------------------------------

/// Formatea un bloque. `base` es el nivel de indentación de la línea que abre el `{`; el contenido
/// va a `base + 1`, y el `}` de cierre vuelve a `base`. Vuelca los comentarios que van **encima** de
/// cada sentencia (con la sangría del cuerpo) y re-pega los *trailing* de sentencias de una línea.
fn fmt_block(cur: &mut Cur, b: &Block, base: usize) -> String {
    if b.statements.is_empty() && b.tail.is_none() {
        return "{ }".to_string();
    }
    let inner = INDENT.repeat(base + 1);
    let mut s = String::from("{\n");
    for st in &b.statements {
        s.push_str(&cur.flush_before(st.line, &inner));
        let text = fmt_stmt(cur, st, base + 1);
        s.push_str(&inner);
        s.push_str(&text);
        if !text.contains('\n') {
            s.push_str(&cur.trailing_on(st.line));
        }
        s.push('\n');
    }
    if let Some(tail) = &b.tail {
        s.push_str(&cur.flush_before(tail.line, &inner));
        let text = fmt_value(cur, tail, base + 1);
        s.push_str(&inner);
        s.push_str(&text);
        if !text.contains('\n') {
            s.push_str(&cur.trailing_on(tail.line));
        }
        s.push('\n');
    }
    s.push_str(&INDENT.repeat(base));
    s.push('}');
    s
}

fn fmt_stmt(cur: &mut Cur, st: &Stmt, indent: usize) -> String {
    match &st.kind {
        StmtKind::Let { name, ty, value, mutable } => {
            let kw = if *mutable { "var" } else { "let" };
            let anno = match ty {
                Some(t) => format!(": {}", fmt_type(t)),
                None => String::new(),
            };
            format!("{} {}{} = {};", kw, name, anno, fmt_value(cur, value, indent))
        }
        StmtKind::LetTuple { names, value, mutable } => {
            let kw = if *mutable { "var" } else { "let" };
            let ns: Vec<String> = names.iter().map(|n| n.clone().unwrap_or_else(|| "_".to_string())).collect();
            format!("{} ({}) = {};", kw, ns.join(", "), fmt_value(cur, value, indent))
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
                ForIter::Range { start, end } => format!("{}..{}", fmt_expr(cur, start, 0), fmt_expr(cur, end, 0)),
                ForIter::In(e) => fmt_expr(cur, e, 0),
                ForIter::Iter { expr, .. } => fmt_expr(cur, expr, 0),
            };
            format!("for {} in {} {}", p, it, fmt_block(cur, body, indent))
        }
        StmtKind::Assign { target, value } => {
            format!("{} = {};", fmt_expr(cur, target, 0), fmt_value(cur, value, indent))
        }
        StmtKind::Return { value } => match value {
            Some(e) => format!("return {};", fmt_value(cur, e, indent)),
            None => "return;".to_string(),
        },
        StmtKind::Expr(e) => {
            // Las formas con bloque (if/while/match/bloque) como sentencia no llevan `;`.
            if is_block_form(e) {
                fmt_expr_indented(cur, e, indent)
            } else {
                format!("{};", fmt_expr(cur, e, 0))
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
fn fmt_value(cur: &mut Cur, e: &Expr, ind: usize) -> String {
    if is_block_form(e) {
        fmt_expr_indented(cur, e, ind)
    } else {
        fmt_expr(cur, e, 0)
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
fn fmt_expr(cur: &mut Cur, e: &Expr, min_prec: u8) -> String {
    let s = fmt_expr_raw(cur, e);
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

fn fmt_expr_raw(cur: &mut Cur, e: &Expr) -> String {
    match &e.kind {
        ExprKind::Int(n) => n.to_string(),
        ExprKind::Float(f) => fmt_float(*f),
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Str(s) => fmt_string_lit(s),
        ExprKind::Char(c) => fmt_char_lit(*c),
        ExprKind::Bytes(b) => fmt_bytes_lit(b),
        ExprKind::Ident(n) => n.clone(),
        ExprKind::Unary { op, expr } => {
            format!("{}{}", unary_op_str(*op), fmt_expr(cur, expr, 12))
        }
        ExprKind::Binary { op, left, right } => {
            let p = bin_prec(*op);
            // Asociativo a la izquierda: la izquierda admite igual precedencia; la derecha, mayor.
            let l = fmt_expr(cur, left, p);
            let r = fmt_expr(cur, right, p + 1);
            format!("{} {} {}", l, bin_op_str(*op), r)
        }
        ExprKind::Call { callee, args } => {
            let c = fmt_expr(cur, callee, 13);
            let a: Vec<String> = args.iter().map(|x| fmt_expr(cur, x, 0)).collect();
            format!("{}({})", c, a.join(", "))
        }
        ExprKind::ArrayLit(elems) => {
            let a: Vec<String> = elems.iter().map(|x| fmt_expr(cur, x, 0)).collect();
            format!("[{}]", a.join(", "))
        }
        ExprKind::TupleLit(elems) => {
            let a: Vec<String> = elems.iter().map(|x| fmt_expr(cur, x, 0)).collect();
            format!("({})", a.join(", "))
        }
        ExprKind::Index { array, index } => {
            let arr = fmt_expr(cur, array, 13);
            let idx = fmt_expr(cur, index, 0);
            format!("{}[{}]", arr, idx)
        }
        ExprKind::Cast { expr, ty } => {
            format!("{} as {}", fmt_expr(cur, expr, 12), fmt_type(ty))
        }
        ExprKind::StructLit { name, fields } => {
            if fields.is_empty() {
                format!("{} {{ }}", name)
            } else {
                let fs: Vec<String> = fields.iter().map(|(n, v)| format!("{}: {}", n, fmt_expr(cur, v, 0))).collect();
                format!("{} {{ {} }}", name, fs.join(", "))
            }
        }
        ExprKind::Field { object, name } => {
            format!("{}.{}", fmt_expr(cur, object, 13), name)
        }
        ExprKind::EnumLit { enum_name, variant, args } => {
            if args.is_empty() {
                format!("{}.{}", enum_name, variant)
            } else {
                let a: Vec<String> = args.iter().map(|x| fmt_expr(cur, x, 0)).collect();
                format!("{}.{}({})", enum_name, variant, a.join(", "))
            }
        }
        ExprKind::Func(fe) => {
            format!("fn({}){} {}", fmt_params(&fe.params), fmt_return(&fe.return_type), fmt_block(cur, &fe.body, 0))
        }
        ExprKind::Try(inner) => format!("{}?", fmt_expr(cur, inner, 13)),
        ExprKind::Match { .. } | ExprKind::If { .. } | ExprKind::While { .. } | ExprKind::Block(_) => {
            // Estas formas son multilínea; se formatean con indentación explícita (nivel 0 aquí).
            fmt_expr_indented(cur, e, 0)
        }
    }
}

/// Formatea una forma con bloque (if/while/match/block) con la indentación `base` (la de su línea).
fn fmt_expr_indented(cur: &mut Cur, e: &Expr, base: usize) -> String {
    match &e.kind {
        ExprKind::If { cond, then_branch, else_branch } => {
            let c = fmt_expr(cur, cond, 0);
            let mut s = format!("if ({}) {}", c, fmt_block(cur, then_branch, base));
            if let Some(eb) = else_branch {
                match &eb.kind {
                    // `else if ...`: se encadena sin bloque intermedio.
                    ExprKind::If { .. } => {
                        s.push_str(" else ");
                        s.push_str(&fmt_expr_indented(cur, eb, base));
                    }
                    ExprKind::Block(b) => {
                        s.push_str(" else ");
                        s.push_str(&fmt_block(cur, b, base));
                    }
                    _ => {
                        // Un else con una expresión no-bloque: envolver en bloque canónico.
                        s.push_str(&format!(" else {{\n{}{}\n{}}}",
                            INDENT.repeat(base + 1), fmt_expr(cur, eb, 0), INDENT.repeat(base)));
                    }
                }
            }
            s
        }
        ExprKind::While { cond, body } => {
            let c = fmt_expr(cur, cond, 0);
            format!("while ({}) {}", c, fmt_block(cur, body, base))
        }
        ExprKind::Block(b) => fmt_block(cur, b, base),
        ExprKind::Match { scrutinee, arms } => {
            let inner = INDENT.repeat(base + 1);
            let mut s = format!("match ({}) {{\n", fmt_expr(cur, scrutinee, 0));
            for arm in arms {
                s.push_str(&cur.flush_before(arm.body.line, &inner)); // comentarios encima del brazo
                let body = if is_block_form(&arm.body) {
                    fmt_expr_indented(cur, &arm.body, base + 1)
                } else {
                    fmt_expr(cur, &arm.body, 0)
                };
                // Guarda opcional (M40.1a): `patrón if <cond> => …`.
                let guarda = match &arm.guard {
                    Some(g) => format!(" if {}", fmt_expr(cur, g, 0)),
                    None => String::new(),
                };
                let linea = format!("{}{}{} => {},", inner, fmt_pattern(&arm.pattern), guarda, body);
                s.push_str(&linea);
                if !body.contains('\n') {
                    s.push_str(&cur.trailing_on(arm.body.line));
                }
                s.push('\n');
            }
            s.push_str(&INDENT.repeat(base));
            s.push('}');
            s
        }
        _ => fmt_expr(cur, e, 0),
    }
}

fn fmt_pattern(p: &Pattern) -> String {
    match &p.kind {
        PatternKind::Wildcard => "_".to_string(),
        PatternKind::Binding(n) => n.clone(),
        PatternKind::Variant { enum_name, variant, subpatterns } => {
            if subpatterns.is_empty() {
                format!("{}.{}", enum_name, variant)
            } else {
                let bs: Vec<String> = subpatterns.iter().map(fmt_pattern).collect(); // recursivo (M40.1c)
                format!("{}.{}({})", enum_name, variant, bs.join(", "))
            }
        }
        PatternKind::Struct { name, fields } => {
            // M40.1d: `Nombre { campo: sub, … }` (la forma corta `campo` = `campo: campo` se reimprime
            // larga, sin ambigüedad).
            let fs: Vec<String> = fields.iter()
                .map(|(f, p)| format!("{}: {}", f, fmt_pattern(p)))
                .collect();
            format!("{} {{ {} }}", name, fs.join(", "))
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

/// Un literal de bytes `b"…"`. Los bytes altos/no imprimibles se emiten como **`\xNN`** (el lexer los
/// acepta, M16.1a) en vez de `byte as char` —que producía un carácter Latin-1 multibyte en UTF-8 y
/// **rompía el round-trip** (el re-lexado leía otros bytes)—.
fn fmt_bytes_lit(b: &[u8]) -> String {
    let mut out = String::from("b\"");
    for &byte in b {
        match byte {
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            0x20..=0x7e => out.push(byte as char), // ASCII imprimible → tal cual
            _ => out.push_str(&format!("\\x{:02x}", byte)), // resto → escape hex
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(src: &str) -> String {
        format_source(src).expect("formatea")
    }

    #[test]
    fn preserva_doc_trailing_y_sueltos() {
        let src = "/// Documenta.\nfn f(x: int) -> int {\n  let y = x * 2;   // el doble\n  // resultado\n  y\n}\n";
        let out = fmt(src);
        assert!(out.contains("/// Documenta."), "doc comment: {out}");
        assert!(out.contains("let y = x * 2;  // el doble"), "trailing: {out}");
        assert!(out.contains("    // resultado"), "suelto en el cuerpo: {out}");
    }

    #[test]
    fn preserva_comentarios_entre_variantes() {
        let src = "enum Color {\n  Rojo,   // primario\n  // el ultimo\n  Azul,\n}\nfn main() -> int { 0 }\n";
        let out = fmt(src);
        assert!(out.contains("Rojo,  // primario"), "trailing de variante: {out}");
        assert!(out.contains("    // el ultimo"), "suelto entre variantes: {out}");
    }

    #[test]
    fn preserva_comentario_en_brazo_de_match() {
        let src = "enum E { A, B }\nfn f(e: E) -> int {\n  match (e) {\n    E.A => 1,   // caso A\n    E.B => 2,\n  }\n}\nfn main() -> int { 0 }\n";
        let out = fmt(src);
        assert!(out.contains("caso A"), "comentario en brazo: {out}");
    }

    #[test]
    fn ningun_comentario_se_pierde() {
        // Cuenta las líneas de comentario de entrada y de salida: no debe faltar ninguna.
        let src = "// a\n// b\nfn f() -> int {\n  // c\n  let x = 1;  // d\n  x\n}\n// e\n";
        let out = fmt(src);
        let cuenta = |s: &str| s.lines().filter(|l| l.trim_start().starts_with("//")).count();
        // 'd' es trailing (no cuenta como línea de solo comentario en la salida) → se cuenta aparte.
        assert!(out.contains("// d"), "trailing preservado");
        assert_eq!(cuenta(src), cuenta(&out) + /* d es trailing en ambos */ 0 + 1 - 1,
            "in={} out={}\n{out}", cuenta(src), cuenta(&out));
    }

    #[test]
    fn idempotente_con_comentarios() {
        let src = "// cabecera\n\n/// doc\nfn f(x: int) -> int {\n  let y = x;  // t\n  // suelto\n  y\n}\n";
        let a = fmt(src);
        assert_eq!(a, fmt(&a), "fmt(fmt(x)) == fmt(x)");
    }

    #[test]
    fn bytes_literal_alto_round_trip() {
        // Un byte alto se emite como \xNN y round-trippea (idempotente).
        let src = "fn main() -> int {\n  let b = b\"\\x8b\\xff\\x00A\";\n  len(b)\n}\n";
        let a = fmt(src);
        assert!(a.contains("\\x8b") && a.contains("\\xff") && a.contains("\\x00"), "{a}");
        assert!(a.contains('A'), "ASCII imprimible tal cual: {a}");
        assert_eq!(a, fmt(&a), "idempotente");
    }
}
