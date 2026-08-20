//! Formateador canónico (`rayfmt`, M29.2): reescribe un `.ray` en un estilo único e **idempotente**,
//! sin configuración (estilo `gofmt`). **Cliente externo**: reusa `lexer`+`parser` y hace *pretty-print*
//! del AST; no toca el núcleo (checker/motores). `ray fmt <archivo>` imprime la versión formateada.
//!
//! Como trabaja sobre el **AST**, el formateador **normaliza** el estilo, pero **preserva las features
//! de superficie** que el parser desazucara: la **interpolación** `"…${x}…"` y los **pipelines** `x |>
//! f()`. El parser las baja al AST (concatenación / llamada) para el checker y los motores, pero guarda
//! la forma original en `Program::interp_sites`/`pipe_sites` (indexada por posición); aquí se reemiten
//! desde ahí (`fmt_expr` → `fmt_interp`/`fmt_pipe`). El resultado siempre es válido y `fmt(fmt(x)) == fmt(x)`.
//!
//! **Comentarios.** El lexer los descarta, así que se recolectan aparte (`collect_comments`, respetando
//! cadenas/chars) y se **re-insertan** durante la emisión (`Cur`): antes de cada ítem/sentencia/miembro
//! se vuelcan los comentarios de las líneas anteriores (con su sangría), y un comentario al final de una
//! línea de código (*trailing*) se re-pega a esa línea. Un comentario tras la última sentencia, antes del
//! `}`, se acota al bloque usando `Block.end_line` (la línea del cierre). Invariante fuerte: **ningún
//! comentario se pierde** y cada uno queda en su sitio.
//!
//! También se **preserva la separación en blanco** entre sentencias de un bloque (una línea en blanco
//! entre grupos se mantiene; 2+ se colapsan a una; ninguna al abrir el bloque) — es agrupación visual
//! intencional. Los ítems de nivel superior siempre van separados por un blanco (canónico).

use crate::ast::*;

const INDENT: &str = "    "; // 4 espacios

/// Ancho máximo de una línea emitida. Hoy solo lo consulta `from … import` (M104): el resto del
/// formateador es de **una construcción, una línea** y no reparte expresiones por el margen. 100 es el
/// ancho de facto del código raylang del repo (p95 = 99 columnas) y el `max_width` de rustfmt.
const MAX_WIDTH: usize = 100;

/// Formatea el código fuente con la indentación **canónica** (4 espacios, estilo gofmt). Es lo que usa
/// `ray fmt`. Devuelve el texto formateado, o un error de lexer/parser (ya formateado).
pub fn format_source(src: &str) -> Result<String, String> {
    let tokens = crate::lexer::lex(src).map_err(|e| e.to_string())?;
    let program = crate::parser::parse(tokens).map_err(|e| e.to_string())?;
    let mut cur = Cur::new(src, &program);
    Ok(format_program(&program, &mut cur))
}

/// Como [`format_source`], pero con la **unidad de indentación** dada (`"  "` para 2 espacios, `"\t"`
/// para tabuladores, etc.). Lo usa el **LSP** para honrar la preferencia del editor (`tabSize`/
/// `insertSpaces` del request de formateo). Se formatea canónico (4 espacios) y luego se **reajusta**
/// la sangría: como el canónico indenta en múltiplos de 4, cada nivel se reescribe con `unit`.
pub fn format_source_with_indent(src: &str, unit: &str) -> Result<String, String> {
    let canonical = format_source(src)?;
    Ok(reindent(&canonical, unit))
}

/// Reescribe la sangría de `text` (canónico: múltiplos de 4 espacios) con la unidad `unit` por nivel.
/// Solo toca los espacios **iniciales** de cada línea (que en el canónico son sangría pura); el resto
/// de la línea —código, comentarios, literales— no se toca. Idempotente para `unit == "    "`.
fn reindent(text: &str, unit: &str) -> String {
    if unit == INDENT {
        return text.to_string();
    }
    let mut out = String::new();
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let n_spaces = line.chars().take_while(|&c| c == ' ').count();
        let rest = &line[n_spaces..];
        if rest.is_empty() {
            continue; // línea en blanco: sin sangría
        }
        out.push_str(&unit.repeat(n_spaces / 4));
        for _ in 0..(n_spaces % 4) {
            out.push(' '); // resto defensivo (el canónico nunca lo produce)
        }
        out.push_str(rest);
    }
    out
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
    let mut line_has_code = false;
    let (mut in_str, mut in_char) = (false, false);
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            line_has_code = false;
            in_str = false;
            in_char = false;
            i += 1;
            continue;
        }
        if in_str || in_char {
            if c == '\\' {
                i += 2; // salta el carácter escapado
                continue;
            }
            if (in_str && c == '"') || (in_char && c == '\'') {
                in_str = false;
                in_char = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                line_has_code = true;
                i += 1;
            }
            '\'' => {
                in_char = true;
                line_has_code = true;
                i += 1;
            }
            '/' if i + 1 < chars.len() && chars[i + 1] == '/' => {
                let mut j = i;
                while j < chars.len() && chars[j] != '\n' {
                    j += 1;
                }
                let text: String = chars[i..j].iter().collect::<String>().trim_end().to_string();
                out.push(Comment { line, text, trailing: line_has_code });
                i = j; // al `\n` (o EOF)
            }
            _ => {
                if !c.is_whitespace() {
                    line_has_code = true;
                }
                i += 1;
            }
        }
    }
    out
}

/// Cursor sobre los comentarios recolectados, consumidos **en orden de fuente** conforme el formateador
/// emite los constructos (que se recorren en ese mismo orden). Ver el `//!` del módulo. También conoce
/// las **líneas en blanco** de la fuente, para preservar la separación visual entre sentencias.
struct Cur {
    items: Vec<Comment>,
    /// Las líneas del fuente como chars (M95): para saber si un literal de string era un
    /// BACKTICK (el token no guarda el delimitador; se mira el carácter en su posición).
    srclines: Vec<Vec<char>>,
    i: usize,
    /// Líneas (1-basadas) que en la fuente están **en blanco** (vacías o solo espacios). Una línea que
    /// es solo un comentario NO cuenta como blanco (tiene contenido).
    blanks: std::collections::HashSet<usize>,
    /// Azúcar preservado (M29.3): la forma de superficie de interpolación/pipelines por posición del
    /// nodo desazucarado raíz. El formateador la consulta en `fmt_expr` para reemitir `"…${e}…"` / `x |> f`.
    interp: std::collections::HashMap<(usize, usize), Vec<InterpSeg>>,
    pipe: std::collections::HashMap<(usize, usize), (Expr, Expr)>,
    /// Indentación (nivel, no columnas) del contexto en curso. `fmt_expr`/`fmt_expr_raw` no llevan la
    /// indentación como parámetro (son muchísimos sitios de llamada); en su lugar, las funciones que sí la
    /// conocen (`fmt_stmt`/`fmt_value`/`fmt_expr_indented`) la depositan aquí y la rama block-form de
    /// `fmt_expr_raw` la lee, para que un `match`/bloque **como sub-expresión** (argumento de llamada,
    /// operando, elemento de arreglo…) se indente relativo a su línea y no a la columna 0. Se guarda y
    /// restaura en cada mutación para no contaminar el formateo de expresiones hermanas.
    base: usize,
    /// ¿Segunda pasada, con envuelto de cadenas de métodos? La primera pasada emite todo en una línea;
    /// si la sentencia no cabe en [`MAX_WIDTH`] se reemite con esto activo (M105).
    wrap: bool,
}

impl Cur {
    fn new(src: &str, program: &Program) -> Self {
        let blanks = src.lines().enumerate()
            .filter(|(_, l)| l.trim().is_empty())
            .map(|(i, _)| i + 1)
            .collect();
        Cur {
            items: collect_comments(src),
            srclines: src.lines().map(|l| l.chars().collect()).collect(),
            i: 0,
            blanks,
            interp: program.interp_sites.clone(),
            pipe: program.pipe_sites.clone(),
            base: 0,
            wrap: false,
        }
    }

    /// ¿Era un BACKTICK el literal que empieza en `(line, col)`? (M95: se mira el fuente.)
    fn is_template(&self, line: usize, col: usize) -> bool {
        self.srclines.get(line.wrapping_sub(1)).and_then(|l| l.get(col.wrapping_sub(1))) == Some(&'`')
    }

    /// ¿Emitir una línea en blanco antes del constructo que empieza en `line` (con sus comentarios
    /// previos)? Sí si la línea de fuente **justo encima** del grupo (los comentarios sueltos que van a
    /// volcarse + la propia línea) está en blanco. Colapsa 2+ blancos a uno (se emite a lo sumo uno).
    fn blank_before(&self, line: usize) -> bool {
        let group_start = match self.items.get(self.i) {
            Some(c) if c.line < line => c.line, // el primer comentario suelto de encima
            _ => line,
        };
        group_start > 1 && self.blanks.contains(&(group_start - 1))
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
    Extern(String, bool), // librería + blocking (bloque `extern "lib" [blocking] { … }`)
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
    // M41: los `extern "lib" { … }` se reagrupan por (librería, blocking) en orden de primera
    // aparición (un bloque `blocking` no se fusiona con uno normal de la misma librería).
    for (lib, blocking, line) in extern_libs_in_order(&p.externs) {
        tops.push((line, Top::Extern(lib, blocking)));
    }
    tops.sort_by_key(|(line, _)| *line);

    // 2. Emitir en orden de fuente, volcando los comentarios de encima de cada ítem (y el trailing si el
    //    ítem cabe en una línea). El cursor avanza monótonamente porque recorremos por línea creciente.
    let is_import = |t: &Top| matches!(t, Top::Import(_) | Top::FromImport(_));
    let mut out = String::new();
    for (idx, (line, top)) in tops.iter().enumerate() {
        let comments = cur.flush_before(*line, "");
        if idx > 0 {
            // Línea en blanco entre ítems de nivel superior, EXCEPTO entre dos imports consecutivos
            // (se agrupan sin separación, como gofmt/rustfmt).
            let between_imports = is_import(&tops[idx - 1].1) && is_import(top);
            if !between_imports {
                out.push('\n');
            }
        }
        out.push_str(&comments);
        let text = match top {
            Top::Import(it) => fmt_import(it),
            Top::FromImport(it) => fmt_from_import(it),
            Top::Const(it) => fmt_const(cur, it),
            Top::Struct(it) => fmt_struct(cur, it),
            Top::Enum(it) => fmt_enum(cur, it),
            Top::Trait(it) => fmt_trait(cur, it),
            Top::Impl(it) => fmt_impl(cur, it),
            Top::Fn(it) => fmt_function(cur, it),
            Top::Extern(lib, blocking) => fmt_extern_block(lib, *blocking, &p.externs),
        };
        // El trailing de un `from … import` se emite también cuando el ítem se ENVUELVE (M104), y va
        // tras `import`, en la PRIMERA línea: así al re-formatear sigue siendo el trailing de
        // `it.line` y el formateador es idempotente (al final de la lista no lo sería).
        let trail = if !text.contains('\n') || matches!(top, Top::FromImport(_)) {
            cur.trailing_on(*line)
        } else {
            String::new()
        };
        match text.split_once('\n') {
            Some((head, rest)) if !trail.is_empty() => {
                out.push_str(head);
                out.push_str(&trail);
                out.push('\n');
                out.push_str(rest);
            }
            _ => {
                out.push_str(&text);
                out.push_str(&trail);
            }
        }
        out.push('\n');
    }
    // 3. Comentarios sueltos al final del archivo (tras el último ítem).
    if tops.is_empty() {
        out.push_str(&cur.flush_rest(""));
    } else {
        let tail = cur.flush_rest("");
        if !tail.is_empty() {
            out.push('\n');
            out.push_str(&tail);
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

/// `from M import a, b, c;` en UNA línea si cabe en [`MAX_WIDTH`]; si no, **un nombre por línea**
/// (M104). Envolver es todo-o-nada, nunca relleno hasta el margen: así añadir o quitar un nombre es
/// una línea del diff. La lista envuelta NO lleva coma final —sin llaves que cierren, el `;` quedaría
/// colgando en su propia línea— aunque el parser sí la tolera para quien la escriba a mano.
fn fmt_from_import(it: &FromImport) -> String {
    let names: Vec<String> = it.names.iter().map(|n| match &n.alias {
        Some(a) => format!("{} as {}", n.name, a),
        None => n.name.clone(),
    }).collect();
    let pref = if it.is_pub { "pub " } else { "" };
    let one_line = format!("{}from {} import {};", pref, it.module, names.join(", "));
    // Se mide la línea RENDERIDA completa (`pub`, módulo, alias y `;`), no el nº de nombres: lo que
    // molesta es el ancho. Con un solo nombre no hay nada que repartir, así que nunca se envuelve.
    if names.len() < 2 || one_line.chars().count() <= MAX_WIDTH {
        return one_line;
    }
    let mut s = format!("{}from {} import\n", pref, it.module);
    for (i, n) in names.iter().enumerate() {
        let sep = if i + 1 == names.len() { ";" } else { "," };
        s.push_str(&format!("{}{}{}", INDENT, n, sep));
        if i + 1 < names.len() {
            s.push('\n');
        }
    }
    s
}

fn fmt_const(cur: &mut Cur, it: &ConstDef) -> String {
    let pref = if it.is_pub { "pub " } else { "" };
    format!("{}const {}: {} = {};", pref, it.name, fmt_type(&it.ty), fmt_expr(cur, &it.value, 0))
}

/// Los pares (librería, blocking) de los bloques `extern` en orden de primera aparición, con la
/// línea de esa primera firma (para ordenar el bloque entre los ítems de nivel superior). (M41)
fn extern_libs_in_order(externs: &[ExternFn]) -> Vec<(String, bool, usize)> {
    let mut seen: Vec<(String, bool, usize)> = Vec::new();
    for e in externs {
        if !seen.iter().any(|(l, b, _)| *l == e.lib && *b == e.blocking) {
            seen.push((e.lib.clone(), e.blocking, e.line));
        }
    }
    seen
}

/// Formatea un bloque `extern "lib" [blocking] { fn …; … }` con todas las firmas de esa librería
/// y esa marca. (M41)
fn fmt_extern_block(lib: &str, blocking: bool, externs: &[ExternFn]) -> String {
    let mut s = format!("extern {:?}{} {{\n", lib, if blocking { " blocking" } else { "" });
    for e in externs.iter().filter(|e| e.lib == lib && e.blocking == blocking) {
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

/// Renderiza un tipo en sintaxis de fuente. Casi todo coincide con el `Display` de `Type`, salvo un
/// caso: un tipo **función que retorna unit** — su `Display` es `fn(...) -> unit`, pero `unit` **no es
/// escribible** en raylang (el retorno unit se OMITE, como en las firmas). Por eso se recorre a mano:
/// en un `Type::Fn` el retorno se emite con `fmt_return` (que omite `-> unit`), y se recursa en los
/// contenedores (`[T]`, `(T,…)`, `Map`/`Channel`/`Task`, args genéricos) para arreglar tipos función
/// **anidados** (p. ej. `[fn(Ctx, Res)]`). Los tipos hoja delegan en su `Display`.
fn fmt_type(t: &Type) -> String {
    match t {
        Type::Fn(params, ret) => {
            let ps: Vec<String> = params.iter().map(fmt_type).collect();
            format!("fn({}){}", ps.join(", "), fmt_return(ret))
        }
        Type::Array(elem) => format!("[{}]", fmt_type(elem)),
        Type::Tuple(ts) => format!("({})", ts.iter().map(fmt_type).collect::<Vec<_>>().join(", ")),
        Type::Map(k, v) => format!("Map<{}, {}>", fmt_type(k), fmt_type(v)),
        Type::Channel(inner) => format!("Channel<{}>", fmt_type(inner)),
        Type::Task(inner) => format!("Task<{}>", fmt_type(inner)),
        Type::Struct(name, args) | Type::Enum(name, args) if !args.is_empty() => {
            format!("{}<{}>", name, args.iter().map(fmt_type).collect::<Vec<_>>().join(", "))
        }
        // Hojas (primitivos, Var, Self, Dyn, struct/enum sin args): su Display ya es sintaxis válida.
        _ => t.to_string(),
    }
}

/// `<T, U>` o `<T: A + B, U>` a partir de los parámetros de tipo y sus bounds.
fn fmt_generics(type_params: &[String], bounds: &[(String, String)]) -> String {
    if type_params.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = type_params.iter().map(|tp| {
        let bs: Vec<&str> = bounds.iter().filter(|(p, _)| p == tp).map(|(_, t)| t.as_str()).collect();
        if bs.is_empty() {
            tp.clone()
        } else {
            format!("{}: {}", tp, bs.join(" + "))
        }
    }).collect();
    format!("<{}>", parts.join(", "))
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
    let parts: Vec<String> = params.iter().map(|p| {
        // El receptor `self` de un método se imprime sin tipo.
        if p.name == "self" && matches!(p.ty, Type::SelfType) {
            "self".to_string()
        } else {
            format!("{}: {}", p.name, fmt_type(&p.ty))
        }
    }).collect();
    parts.join(", ")
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
        let comments = cur.flush_before(m.line, INDENT); // comentarios encima del método (1 nivel)
        if i > 0 {
            s.push('\n');
        }
        s.push_str(&comments);
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
    let inner = INDENT.repeat(base + 1);
    if b.statements.is_empty() && b.tail.is_none() {
        // Bloque vacío. Si es MULTILÍNEA y encierra comentarios (línea < `}`), se preservan; si no, `{ }`.
        if b.end_line > b.line {
            let tail_comments = cur.flush_before(b.end_line, &inner);
            if !tail_comments.is_empty() {
                return format!("{{\n{}{}}}", tail_comments, INDENT.repeat(base));
            }
        }
        return "{ }".to_string();
    }
    // Preserva un bloque **inline**: un cuerpo de solo un tail (sin sentencias) que en la FUENTE cabía
    // ENTERO en una línea (`{`, tail y `}` en `b.line`) y no es una forma con bloque se mantiene inline
    // (`{ expr }`). raylang tiene muchas funciones de una línea (`fn square(n) { n * n }`); expandirlas
    // todas sería anti-idiomático. Al ser de una sola línea, no hay comentarios dentro.
    if b.statements.is_empty()
        && let Some(tail) = &b.tail
        && tail.line == b.line
        && b.end_line == b.line
        && !is_block_form(tail)
    {
        return format!("{{ {} }}", fmt_value(cur, tail, base));
    }
    let mut s = String::from("{\n");
    for (idx, st) in b.statements.iter().enumerate() {
        // Preserva una línea en blanco entre sentencias (agrupación visual), salvo antes de la primera.
        if idx > 0 && cur.blank_before(st.line) {
            s.push('\n');
        }
        s.push_str(&cur.flush_before(st.line, &inner));
        let text = fmt_stmt(cur, st, base + 1);
        s.push_str(&inner);
        s.push_str(&text);
        // Una forma con bloque (if/while/match/bloque) como sentencia-expresión normalmente NO lleva `;`.
        // PERO si es la ÚLTIMA sentencia y el bloque no tiene tail, omitir el `;` la promovería, al
        // re-parsear, a **tail** del bloque (un block-form final sin `;` es el tail) — cambiando el valor
        // del bloque de `unit` al del block-form. Ahí el `;` es semánticamente necesario: se preserva.
        if idx + 1 == b.statements.len()
            && b.tail.is_none()
            && matches!(&st.kind, StmtKind::Expr(e) if is_block_form(e))
        {
            s.push(';');
        }
        if !text.contains('\n') {
            s.push_str(&cur.trailing_on(st.line));
        }
        s.push('\n');
    }
    if let Some(tail) = &b.tail {
        if !b.statements.is_empty() && cur.blank_before(tail.line) {
            s.push('\n');
        }
        s.push_str(&cur.flush_before(tail.line, &inner));
        let text = fmt_value(cur, tail, base + 1);
        s.push_str(&inner);
        s.push_str(&text);
        if !text.contains('\n') {
            s.push_str(&cur.trailing_on(tail.line));
        }
        s.push('\n');
    }
    // Comentarios que quedan DENTRO del bloque, tras la última sentencia/tail y antes del `}` (línea <
    // `end_line`). Antes se reubicaban tras el `}` (el AST no tenía la posición de cierre); ahora se
    // acotan al bloque. Se respeta también un blanco de separación previo.
    let blank_tail = cur.blank_before(b.end_line);
    let tail_comments = cur.flush_before(b.end_line, &inner);
    if !tail_comments.is_empty() {
        if blank_tail {
            s.push('\n');
        }
        s.push_str(&tail_comments);
    }
    s.push_str(&INDENT.repeat(base));
    s.push('}');
    s
}

fn fmt_stmt(cur: &mut Cur, st: &Stmt, indent: usize) -> String {
    // La sentencia vive en `indent`: cualquier forma con bloque anidada en una sub-expresión no-valor
    // (p. ej. `print(match …)`) debe indentarse relativa a aquí. `fmt_value` refina el valor por caso.
    let saved = cur.base;
    cur.base = indent;
    let r = retry_wrapped(cur, indent, |c| fmt_stmt_inner(c, st, indent));
    cur.base = saved;
    r
}

/// Emite `render` y, si alguna línea del resultado **no cabe** en [`MAX_WIDTH`], lo reemite con el
/// envuelto de cadenas activo (M105). El cursor de comentarios se restaura entre pasadas: renderizar
/// los CONSUME, y sin restaurarlo la segunda pasada los perdería.
fn retry_wrapped(cur: &mut Cur, indent: usize, render: impl Fn(&mut Cur) -> String) -> String {
    let save = cur.i;
    let first = render(cur);
    if cur.wrap || fits_width(&first, indent) {
        return first;
    }
    cur.i = save;
    cur.wrap = true;
    let wrapped = render(cur);
    cur.wrap = false;
    wrapped
}

/// ¿Cabe el texto emitido? La primera línea lleva la sangría de la sentencia (el bloque la antepone);
/// las siguientes ya la traen incorporada.
fn fits_width(text: &str, indent: usize) -> bool {
    let pad = INDENT.chars().count() * indent;
    text.split('\n').enumerate().all(|(i, l)| {
        let w = if i == 0 { pad + l.chars().count() } else { l.chars().count() };
        w <= MAX_WIDTH
    })
}

fn fmt_stmt_inner(cur: &mut Cur, st: &Stmt, indent: usize) -> String {
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
                // Deja la indentación del contexto en `cur.base` para las formas con bloque anidadas en
                // la expresión (una función anónima con cuerpo `spawn(fn() { … })`, o un `match` dentro
                // de un argumento), igual que `fmt_value`.
                let saved = cur.base;
                cur.base = indent;
                let s = format!("{};", fmt_expr(cur, e, 0));
                cur.base = saved;
                s
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
        // No es una forma con bloque en la raíz, pero puede contenerla anidada (`print(match …)`): deja la
        // indentación del contexto en `cur.base` para que la rama block-form de `fmt_expr_raw` la use.
        let saved = cur.base;
        cur.base = ind;
        let s = retry_wrapped(cur, ind, |c| fmt_expr(c, e, 0));
        cur.base = saved;
        s
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
    // M29.3: azúcar preservado. Si esta posición es la raíz de una interpolación o un pipeline
    // desazucarado, se reemite la forma de superficie. Se **quita** la entrada de la tabla mientras se
    // formatea y se restaura: el nodo raíz desazucarado y una de sus sub-expresiones comparten posición
    // (p. ej. `"${x}"` → `to_string(x)`, ambos en la misma `(línea, col)`), así que sin quitarla la
    // recursión reentraría infinitamente; quitándola solo LA raíz, el azúcar anidado (en otras
    // posiciones) se sigue preservando.
    let pos = (e.line, e.col);
    if let Some(segs) = cur.interp.remove(&pos) {
        let template = cur.is_template(e.line, e.col);
        let s = fmt_interp(cur, &segs, template); // una cadena interpolada es primaria → sin paréntesis
        cur.interp.insert(pos, segs);
        return s;
    }
    if let Some((recv, rhs)) = cur.pipe.remove(&pos) {
        let s = fmt_pipe(cur, &recv, &rhs);
        cur.pipe.insert(pos, (recv, rhs));
        // El pipeline `|>` tiene la precedencia MÍNIMA: se parentiza en cualquier operando más fuerte.
        return if min_prec > PIPE_PREC { format!("({})", s) } else { s };
    }
    // M105: en la pasada de envuelto, una cadena de 2+ eslabones que no cabe se reparte. El aplanado
    // se renderiza igual para medirlo, restaurando el cursor de comentarios (renderizar los consume).
    if cur.wrap && let Some((recv, links)) = chain_links(e) {
        let save = cur.i;
        let flat = fmt_expr_raw(cur, e);
        let fits = indent_width(cur) + flat.chars().count() <= MAX_WIDTH;
        let s = if fits {
            flat
        } else {
            cur.i = save;
            fmt_chain_wrapped(cur, recv, &links)
        };
        return if expr_prec(e) < min_prec { format!("({})", s) } else { s };
    }
    let s = fmt_expr_raw(cur, e);
    if expr_prec(e) < min_prec {
        format!("({})", s)
    } else {
        s
    }
}

/// Precedencia del pipeline `|>` (la más baja del lenguaje; menor que cualquier binario).
const PIPE_PREC: u8 = 0;

/// Reemite una cadena interpolada desde sus segmentos de superficie (M29.3): `"a${x}b"`. Las partes
/// literales se re-escapan como un string; cada `${e}` formatea su expresión (recursivo → interpolación
/// anidada funciona). Un `${` literal en el texto se re-escapa como `\${`.
fn fmt_interp(cur: &mut Cur, segs: &[InterpSeg], template: bool) -> String {
    let delim = if template { '`' } else { '"' };
    let mut out = String::from(delim);
    for seg in segs {
        match seg {
            InterpSeg::Lit(s) => {
                // Igual que `fmt_string_lit`, salvo que un `$` seguido de `{` en el TEXTO literal se
                // escapa `\${` para que no reabra una interpolación al re-lexar (`"\${x}"` es un `${x}`
                // literal). Un `$` que no precede a `{` es literal por sí solo → no se escapa.
                // M95 (backticks): la `"` es literal, el `` ` `` se escapa y los saltos de línea
                // se reemiten LITERALES (multilínea).
                let chars: Vec<char> = s.chars().collect();
                for (i, &c) in chars.iter().enumerate() {
                    match c {
                        '\\' => out.push_str("\\\\"),
                        '\n' if template => out.push('\n'),
                        '\n' => out.push_str("\\n"),
                        '\t' => out.push_str("\\t"),
                        '\r' => out.push_str("\\r"),
                        '"' if !template => out.push_str("\\\""),
                        '`' if template => out.push_str("\\`"),
                        '$' if chars.get(i + 1) == Some(&'{') => out.push_str("\\$"),
                        other => out.push(other),
                    }
                }
            }
            InterpSeg::Expr(e) => {
                out.push_str("${");
                out.push_str(&fmt_expr(cur, e, 0));
                out.push('}');
            }
        }
    }
    out.push(delim);
    out
}

/// Reemite un pipeline desde su forma de superficie (M29.3): `recv |> rhs`. El receptor puede ser a su
/// vez un pipeline (encadenado `a |> b |> c`, asociativo a la izquierda → sin paréntesis).
fn fmt_pipe(cur: &mut Cur, recv: &Expr, rhs: &Expr) -> String {
    // El receptor liga a nivel `logic_or` (más fuerte que `|>`) o es otro pipe (izq-asociativo): en
    // ambos casos no necesita paréntesis. El rhs es un objetivo de llamada (`f`, `f(a)`, `m.f(a)`).
    format!("{} |> {}", fmt_expr(cur, recv, PIPE_PREC), fmt_expr(cur, rhs, 13))
}

/// Los eslabones de una **cadena de métodos** `recv.a(…).b(…)`: el receptor y los `(nombre, args)` en
/// orden de escritura. `None` si no hay al menos DOS eslabones (con uno no hay nada que repartir).
/// UFCS llega como `Call(Field(obj, m), args)`, así que la espina se recorre hacia el receptor.
fn chain_links<'a>(e: &'a Expr) -> Option<(&'a Expr, Vec<(&'a str, &'a [Expr])>)> {
    let mut links: Vec<(&str, &[Expr])> = Vec::new();
    let mut node = e;
    while let ExprKind::Call { callee, args } = &node.kind {
        let ExprKind::Field { object, name } = &callee.kind else { break };
        links.push((name.as_str(), args.as_slice()));
        node = object;
    }
    if links.len() >= 2 {
        links.reverse();
        Some((node, links))
    } else {
        None
    }
}

/// Emite una cadena repartida: el receptor se queda donde está y **cada eslabón baja una línea**, a un
/// nivel de sangría por debajo de la sentencia. El cierre de lo que envuelva la cadena se pega al
/// último eslabón (forma canónica elegida: la más compacta).
fn fmt_chain_wrapped(cur: &mut Cur, recv: &Expr, links: &[(&str, &[Expr])]) -> String {
    let pad = INDENT.repeat(cur.base + 1);
    let mut s = fmt_expr(cur, recv, 13); // el receptor liga a nivel de llamada/campo
    for (name, args) in links {
        s.push('\n');
        s.push_str(&pad);
        s.push('.');
        s.push_str(name);
        s.push('(');
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            let a = fmt_expr(cur, a, 0);
            s.push_str(&a);
        }
        s.push(')');
    }
    s
}

/// La anchura que ya consume la sangría de la sentencia en curso.
fn indent_width(cur: &Cur) -> usize {
    INDENT.chars().count() * cur.base
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
        ExprKind::Str(s) => {
            if cur.is_template(e.line, e.col) { fmt_template_lit(s) } else { fmt_string_lit(s) }
        }
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
        // M48.2: literal de Map. `[:]` vacío; `[k: v, …]` poblado.
        ExprKind::MapLit(pairs) => {
            if pairs.is_empty() {
                "[:]".to_string()
            } else {
                let a: Vec<String> = pairs.iter()
                    .map(|(k, v)| format!("{}: {}", fmt_expr(cur, k, 0), fmt_expr(cur, v, 0)))
                    .collect();
                format!("[{}]", a.join(", "))
            }
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
            // El cuerpo se indenta relativo a la línea donde aparece el `fn(...)` (`cur.base`), no a 0:
            // una función anónima como argumento de llamada (`spawn(fn() { … })`) o inicializador vive
            // dentro de un contexto ya indentado. `cur.base` lo dejan `fmt_value`/`fmt_stmt`.
            format!("fn({}){} {}", fmt_params(&fe.params), fmt_return(&fe.return_type), fmt_block(cur, &fe.body, cur.base))
        }
        ExprKind::Try(inner) => format!("{}?", fmt_expr(cur, inner, 13)),
        ExprKind::Match { .. } | ExprKind::If { .. } | ExprKind::While { .. } | ExprKind::Block(_) => {
            // Forma multilínea como SUB-expresión (argumento de llamada, operando, elemento…): se indenta
            // relativa a la línea del contexto, que `fmt_stmt`/`fmt_value` dejaron en `cur.base`.
            fmt_expr_indented(cur, e, cur.base)
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
                // El cuerpo del brazo vive en `base + 1`: una forma con bloque anidada en una sub-expresión
                // no-valor del cuerpo (`A => print(match …)`) debe indentarse desde aquí.
                let saved = cur.base;
                cur.base = base + 1;
                let body = if is_block_form(&arm.body) {
                    fmt_expr_indented(cur, &arm.body, base + 1)
                } else {
                    fmt_expr(cur, &arm.body, 0)
                };
                cur.base = saved;
                // Guarda opcional (M40.1a): `patrón if <cond> => …`.
                let guard = match &arm.guard {
                    Some(g) => format!(" if {}", fmt_expr(cur, g, 0)),
                    None => String::new(),
                };
                let line = format!("{}{}{} => {},", inner, fmt_pattern(&arm.pattern), guard, body);
                s.push_str(&line);
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

fn escape_char(c: char, out: &mut String, in_double_quotes: bool) {
    match c {
        '\\' => out.push_str("\\\\"),
        '\n' => out.push_str("\\n"),
        '\t' => out.push_str("\\t"),
        '\r' => out.push_str("\\r"),
        '"' if in_double_quotes => out.push_str("\\\""),
        '\'' if !in_double_quotes => out.push_str("\\'"),
        other => out.push(other),
    }
}

fn fmt_string_lit(s: &str) -> String {
    let mut out = String::from("\"");
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        // Un `$` seguido de `{` en el VALOR debe re-escaparse `\$` — si no, el roundtrip del
        // formateador convertiría un `\${x}` literal en una interpolación real (fix M95).
        if c == '$' && chars.get(i + 1) == Some(&'{') {
            out.push_str("\\$");
        } else {
            escape_char(c, &mut out, true);
        }
    }
    out.push('"');
    out
}

/// Reemite un string cuyo literal original era un BACKTICK (M95): la `"` es literal, el
/// `` ` `` se escapa y los saltos de línea se conservan literales (multilínea).
fn fmt_template_lit(s: &str) -> String {
    let mut out = String::from("`");
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '\\' => out.push_str("\\\\"),
            '`' => out.push_str("\\`"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push('\n'),
            '$' if chars.get(i + 1) == Some(&'{') => out.push_str("\\$"),
            other => out.push(other),
        }
    }
    out.push('`');
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
    fn extern_blocking_roundtrips_and_does_not_merge_with_plain_blocks() {
        // Un bloque `extern "c" blocking { … }` conserva su marca al reformatear, y NO se fusiona
        // con un bloque normal de la MISMA librería (la reagrupación es por (lib, blocking)).
        let src = "extern \"c\" blocking {\n    fn sleep(s: int) -> int;\n}\n\nextern \"c\" {\n    fn abs(x: int) -> int;\n}\n\nfn main() -> int {\n    0\n}\n";
        assert_eq!(fmt(src), src);
        assert_eq!(fmt(&fmt(src)), fmt(src), "idempotente");
    }

    #[test]
    fn backticks_are_preserved() {
        // M95: el fmt reemite un backtick como backtick (comillas literales, multilínea, `${}`).
        let src = "fn main() -> int {\n    print(`{\"id\": ${1 + 2}}`);\n    let d = `a\nb`;\n    print(d);\n    0\n}\n";
        assert_eq!(fmt(src), src);
        // Y un `\${` literal en un string PLANO sobrevive al roundtrip (fix M95).
        let plain = "fn main() -> int {\n    print(\"precio \\${USD}\");\n    0\n}\n";
        assert_eq!(fmt(plain), plain);
    }

    #[test]
    fn preserves_doc_trailing_and_standalone() {
        let src = "/// Documenta.\nfn f(x: int) -> int {\n  let y = x * 2;   // el double\n  // resultado\n  y\n}\n";
        let out = fmt(src);
        assert!(out.contains("/// Documenta."), "doc comment: {out}");
        assert!(out.contains("let y = x * 2;  // el double"), "trailing: {out}");
        assert!(out.contains("    // resultado"), "suelto en el body: {out}");
    }

    #[test]
    fn fn_anonymous_as_argument_indents_body() {
        // Regresión: `spawn(fn() { … })` como sentencia — el cuerpo va un nivel más adentro que la
        // llamada y el `});` alinea con ella (antes el cuerpo se quedaba al nivel de la llamada y el
        // cierre en la columna 0).
        let src = "fn main() -> int {\n    spawn(fn() {\n        work(1);\n    });\n    0\n}\n";
        let out = fmt(src);
        assert!(out.contains("    spawn(fn() {\n        work(1);\n    });"), "body a +1, cierre alineado: {out:?}");
        assert_eq!(fmt(&out), out, "idempotente");
        // Anidado: `scope(fn() { spawn(fn() { … }) })`.
        let nested = "fn main() -> int {\n    scope(fn() {\n        spawn(fn() {\n            send(ch, 7);\n        });\n    });\n    0\n}\n";
        assert_eq!(fmt(nested), nested, "función anónima anidada, idempotente: {:?}", fmt(nested));
    }

    /// M104 — un `from … import` que pasa de MAX_WIDTH se envuelve a un nombre por línea; el que cabe
    /// se queda en una. Se mide la línea renderizada completa, no el nº de nombres.
    #[test]
    fn wraps_a_long_from_import_one_name_per_line() {
        let long = "from web/framework import new_app, GET, POST, listen_graceful, static_files_cached, \
                    not_found, log_requests, after, cors, html;\nfn main() -> int { 0 }\n";
        let out = fmt(long);
        assert!(out.starts_with("from web/framework import\n    new_app,\n    GET,\n"), "envuelto: {out:?}");
        assert!(out.contains("    html;\n"), "el ultimo nombre cierra con ';' y SIN coma final: {out:?}");
        assert!(!out.contains(",\n;"), "sin coma final colgando el ';': {out:?}");
        assert_eq!(fmt(&out), out, "idempotente");
        // Ninguna línea del resultado pasa del ancho máximo.
        for l in out.lines() {
            assert!(l.chars().count() <= MAX_WIDTH, "linea de {} cols: {l:?}", l.chars().count());
        }
    }

    /// La vía del LSP (`format_source_with_indent`) reajusta la sangría del envuelto a la unidad del
    /// editor: el canónico indenta con 4 espacios, así que el reindent lo reescribe con `unit`.
    #[test]
    fn reindents_a_wrapped_import_for_the_editor_unit() {
        let src = "from web/framework import new_app, GET, POST, listen_graceful, static_files_cached, \
                   not_found, log_requests, after, cors, html;\nfn main() -> int { 0 }\n";
        let out = super::format_source_with_indent(src, "  ").expect("formatea");
        assert!(out.contains("from web/framework import\n  new_app,\n  GET,\n"), "sangria de 2: {out:?}");
    }

    /// M105 — una cadena de métodos que no cabe en MAX_WIDTH se reparte a un eslabón por línea, un nivel
    /// por debajo de la sentencia; el receptor se queda donde está y el cierre se pega al último eslabón.
    #[test]
    fn wraps_a_long_method_chain_one_link_per_line() {
        let src = "fn f(o: string, k: string, v: string) -> string { o }\n\
                   fn main() {\n\
                   \x20   let x = \"\".f(\"aaaaaaaaaaaa\", \"bbbbbbbbbbbb\").f(\"cccccccccccc\", \"dddddddddddd\").f(\"eeeeeeeeeeee\", \"ffffffff\");\n\
                   \x20   print(x);\n\
                   }\n";
        let out = fmt(src);
        assert!(out.contains("let x = \"\"\n        .f(\"aaaaaaaaaaaa\""), "receptor + eslabon a +1: {out}");
        assert!(out.contains(".f(\"eeeeeeeeeeee\", \"ffffffff\");"), "el cierre se pega al ultimo: {out}");
        assert_eq!(fmt(&out), out, "idempotente");
        for l in out.lines() {
            assert!(l.chars().count() <= MAX_WIDTH, "linea de {} cols: {l:?}", l.chars().count());
        }
    }

    /// Una cadena que CABE se queda en una línea, y una de un solo eslabón nunca se reparte (no hay nada
    /// que repartir) aunque la línea sea larga.
    #[test]
    fn keeps_a_short_chain_inline() {
        let src = "fn main() { let s = \"  x  \"; print(s.trim().to_lower()); }\n";
        assert!(fmt(src).contains("print(s.trim().to_lower());"), "{:?}", fmt(src));
    }

    /// El envuelto NO toca el azúcar: un pipeline `|>` se reemite como pipeline, no como cadena repartida
    /// (se comprueba antes, en `fmt_expr`).
    #[test]
    fn does_not_wrap_a_pipeline_as_a_chain() {
        let src = "fn a(x: int) -> int { x }\nfn b(x: int) -> int { x }\n\
                   fn main() { let averyveryverylongname = 1; print(averyveryverylongname |> a |> b |> a |> b |> a |> b |> a); }\n";
        let out = fmt(src);
        assert!(out.contains("|>"), "el pipeline se preserva: {out}");
        assert!(!out.contains("\n        .a("), "no se reparte como cadena: {out}");
    }

    #[test]
    fn keeps_a_short_from_import_on_one_line() {
        let src = "from std/json import obj, field, list;\nfn main() -> int { 0 }\n";
        assert!(fmt(src).starts_with("from std/json import obj, field, list;\n"), "{:?}", fmt(src));
        // También se colapsa el que el autor escribió en varias líneas pero cabe en una (canónico).
        let multi = "from std/json import\n    obj,\n    field,\n    list;\nfn main() -> int { 0 }\n";
        assert!(fmt(multi).starts_with("from std/json import obj, field, list;\n"), "{:?}", fmt(multi));
    }

    /// La coma final la ACEPTA el parser (para quien la escriba a mano), pero el formateador no la
    /// emite: sin llaves que cierren, el `;` quedaría colgando en su propia línea.
    #[test]
    fn accepts_but_does_not_emit_a_trailing_comma() {
        let src = "from std/json import\n    obj,\n    field,\n;\nfn main() -> int { 0 }\n";
        assert!(fmt(src).starts_with("from std/json import obj, field;\n"), "{:?}", fmt(src));
    }

    /// El comentario trailing de un import ENVUELTO va tras `import`, en la primera línea: al
    /// re-formatear vuelve a ser el trailing de esa misma línea (si fuera al final, se relocalizaría).
    #[test]
    fn keeps_the_trailing_comment_of_a_wrapped_import() {
        let src = "from web/framework import new_app, GET, POST, listen_graceful, static_files_cached, \
                   not_found, log_requests, after, cors, html;  // el router\nfn main() -> int { 0 }\n";
        let out = fmt(src);
        assert!(out.starts_with("from web/framework import  // el router\n"), "trailing en la cabeza: {out:?}");
        assert_eq!(fmt(&out), out, "idempotente");
    }

    #[test]
    fn preserves_comments_between_variantes() {
        let src = "enum Color {\n  Rojo,   // primario\n  // el ultimo\n  Azul,\n}\nfn main() -> int { 0 }\n";
        let out = fmt(src);
        assert!(out.contains("Rojo,  // primario"), "trailing de variant: {out}");
        assert!(out.contains("    // el ultimo"), "suelto between variantes: {out}");
    }

    #[test]
    fn preserves_comment_in_match_branch() {
        let src = "enum E { A, B }\nfn f(e: E) -> int {\n  match (e) {\n    E.A => 1,   // case A\n    E.B => 2,\n  }\n}\nfn main() -> int { 0 }\n";
        let out = fmt(src);
        assert!(out.contains("case A"), "comment en branch: {out}");
    }

    #[test]
    fn no_comment_is_lost() {
        // Cuenta las líneas de comentario de entrada y de salida: no debe faltar ninguna.
        let src = "// a\n// b\nfn f() -> int {\n  // c\n  let x = 1;  // d\n  x\n}\n// e\n";
        let out = fmt(src);
        let count = |s: &str| s.lines().filter(|l| l.trim_start().starts_with("//")).count();
        // 'd' es trailing (no cuenta como línea de solo comentario en la salida) → se cuenta aparte.
        assert!(out.contains("// d"), "trailing preservado");
        assert_eq!(count(src), count(&out) + /* d es trailing en ambos */ 0 + 1 - 1,
            "in={} out={}\n{out}", count(src), count(&out));
    }

    #[test]
    fn idempotent_with_comments() {
        let src = "// header\n\n/// doc\nfn f(x: int) -> int {\n  let y = x;  // t\n  // suelto\n  y\n}\n";
        let a = fmt(src);
        assert_eq!(a, fmt(&a), "fmt(fmt(x)) == fmt(x)");
    }

    #[test]
    fn consecutive_imports_without_blank_line() {
        let src = "import a/b;\nimport c/d as x;\nfrom e import f;\nfn main() -> int { 0 }\n";
        let out = fmt(src);
        // Los tres imports quedan agrupados (sin blancos entre ellos)…
        assert!(out.contains("import a/b;\nimport c/d as x;\nfrom e import f;"), "{out:?}");
        // …pero sí hay un blanco antes de la función.
        assert!(out.contains("from e import f;\n\nfn main"), "blanco antes de fn: {out:?}");
    }

    #[test]
    fn comment_before_close_stays_inside() {
        // Un comentario tras la última sentencia, antes del `}`, se acota al bloque (ya no se reubica
        // tras el `}`). Con blanco previo, se conserva. Y un bloque vacío con solo un comentario lo mantiene.
        let src = "fn g() -> int {\n  let z = 1;\n  z\n\n  // final\n}\nfn empty() {\n  // solo comment\n}\nfn main() -> int { 0 }\n";
        let out = fmt(src);
        assert!(out.contains("    z\n\n    // final\n}"), "comment final inside, con blanco: {out:?}");
        assert!(out.contains("fn empty() {\n    // solo comment\n}"), "block vacío conserva su comment: {out:?}");
        assert_eq!(out, fmt(&out), "idempotente");
    }

    #[test]
    fn preserves_blank_lines_between_statements() {
        // Un blanco entre grupos de sentencias se conserva; 2+ se colapsan a uno; sin blanco al inicio.
        let src = "fn main() -> int {\n  let a = 1;\n  let b = 2;\n\n\n  // grupo 2\n  let c = 3;\n  c\n}\n";
        let out = fmt(src);
        assert!(out.contains("let b = 2;\n\n    // grupo 2"), "un blanco antes del grupo 2: {out:?}");
        assert!(!out.contains("\n\n\n"), "2+ blancos colapsados a one: {out:?}");
        assert!(out.starts_with("fn main() -> int {\n    let a = 1;"), "sin blanco after el {{: {out:?}");
        assert_eq!(out, fmt(&out), "idempotente");
    }

    #[test]
    fn inline_function_is_preserved() {
        // Cuerpo de una línea en la fuente → se mantiene inline; multilínea → se mantiene multilínea.
        let src = "fn square(n: int) -> int { n * n }\nfn largo(x: int) -> int {\n  let y = x;\n  y\n}\n";
        let out = fmt(src);
        assert!(out.contains("fn square(n: int) -> int { n * n }"), "inline conservado: {out:?}");
        assert!(out.contains("fn largo(x: int) -> int {\n    let y = x;"), "multilínea conservado: {out:?}");
        // Un cuerpo multilínea con un solo tail NO se colapsa a inline (respeta la fuente).
        let ml = "fn f() -> int {\n  1 + 2\n}\n";
        assert!(fmt(ml).contains("fn f() -> int {\n    1 + 2\n}"), "no colapsa: {}", fmt(ml));
        // Idempotente en ambos.
        assert_eq!(out, fmt(&out));
    }

    #[test]
    fn configurable_and_canonical_indent() {
        let src = "fn f(x: int) -> int {\n  if (x > 0) {\n    x\n  } else {\n    0\n  }\n}\n";
        // Canónico = 4 espacios.
        assert!(fmt(src).contains("\n    if ("), "canónico 4 espacios");
        // 2 espacios.
        let two_spaces = format_source_with_indent(src, "  ").unwrap();
        assert!(two_spaces.contains("\n  if ("), "2 espacios: {two_spaces:?}");
        assert!(two_spaces.contains("\n    x"), "nivel 2 = 4 espacios en 2-espacios: {two_spaces:?}");
        assert_eq!(two_spaces, format_source_with_indent(&two_spaces, "  ").unwrap(), "idempotente en 2 espacios");
        // Tabuladores.
        let tabs = format_source_with_indent(src, "\t").unwrap();
        assert!(tabs.contains("\n\tif ("), "1 tab por nivel: {tabs:?}");
        assert!(tabs.contains("\n\t\tx"), "nivel 2 = 2 tabs: {tabs:?}");
        // Unidad = 4 espacios ⇒ idéntico al canónico.
        assert_eq!(format_source_with_indent(src, "    ").unwrap(), fmt(src));
    }

    #[test]
    fn preserves_interpolation() {
        // M29.3: `ray fmt` ya NO desazucara la interpolación a `+ to_string(...)`.
        let src = "fn main() -> int {\n  let x = 7;\n  print(\"${x} al square es ${x * x}.\");\n  print(\"lit \\${no} y ${x}\");\n  0\n}\n";
        let out = fmt(src);
        assert!(out.contains("\"${x} al square es ${x * x}.\""), "interpolación conservada: {out:?}");
        assert!(!out.contains("to_string"), "no must aparecer to_string: {out:?}");
        // Un `${` literal (`\${no}`) se conserva escapado (no reabre interpolación).
        assert!(out.contains("\\${no}"), "'${{' literal escapado: {out:?}");
        assert_eq!(out, fmt(&out), "idempotente");
    }

    #[test]
    fn form_with_block_as_subexpression_indents() {
        // Regresión: un `match`/bloque como ARGUMENTO de llamada (u otra sub-expresión no-valor) se
        // indentaba desde la columna 0 (`fmt_expr` no llevaba la indentación). Debe indentarse relativo
        // a su línea: brazos a base+1, cierre a base.
        let src = "fn main() {\n  print(match (x) {\n    A => 1,\n    B => 2,\n  });\n}\n";
        let out = fmt(src);
        assert!(out.contains("    print(match (x) {\n        A => 1,\n        B => 2,\n    });"),
            "match en call bien indentado: {out:?}");
        // Hermanas independientes: `f(match) + g(match)` no se contaminan (base guardada/restaurada).
        let bin = "fn main() {\n  let y = f(match (x) { A => 1 }) + g(match (z) { C => 3 });\n}\n";
        let ob = fmt(bin);
        assert!(ob.contains("    let y = f(match (x) {\n        A => 1,\n    }) + g(match (z) {\n        C => 3,\n    });"),
            "ambos matches en base 1: {ob:?}");
        assert_eq!(out, fmt(&out), "idempotente");
        assert_eq!(ob, fmt(&ob), "idempotente");
    }

    #[test]
    fn preserves_semicolon_in_final_block_form() {
        // Regresión (grave: cambiaba semántica): un `match`/`if`/bloque como sentencia-expresión ÚLTIMA de
        // un bloque sin tail se emitía SIN `;`; al re-parsear, un block-form final sin `;` es el **tail**,
        // así que el bloque pasaba de producir `unit` a producir el valor del block-form. El `;` debe
        // preservarse ahí.
        let src = "enum Op { A, B }\nfn emit(o: Op) -> int { 5 }\nfn f(o: Op) {\n  emit(o);\n  match (o) { Op.A => emit(o), Op.B => emit(o) };\n}\nfn main() -> int { f(Op.A); 0 }\n";
        let out = fmt(src);
        assert!(out.contains("    };\n}"), "match final conserva `;`: {out:?}");
        // En cambio, un block-form seguido de TAIL no lleva `;` (el tail lo mantiene como sentencia).
        let con_tail = "enum Op { A, B }\nfn emit(o: Op) -> int { 5 }\nfn f(o: Op) -> int {\n  match (o) { Op.A => emit(o), Op.B => emit(o) }\n  7\n}\nfn main() -> int { f(Op.A) }\n";
        let ot = fmt(con_tail);
        assert!(ot.contains("    }\n    7\n"), "block-form con tail NO lleva `;`: {ot:?}");
        assert_eq!(out, fmt(&out), "idempotente");
        assert_eq!(ot, fmt(&ot), "idempotente");
    }

    #[test]
    fn preserves_pipelines() {
        let src = "fn dob(n: int) -> int { n + n }\nfn inc(n: int) -> int { n + 1 }\nfn main() -> int {\n  5 |> dob() |> inc()\n}\n";
        let out = fmt(src);
        assert!(out.contains("5 |> dob() |> inc()"), "pipeline conservado (chained): {out:?}");
        assert_eq!(out, fmt(&out), "idempotente");
    }

    #[test]
    fn fn_type_returning_unit_omits_return_val() {
        // Un tipo `fn(...) -> unit` (retorno implícito) NO debe emitirse con `-> unit` (no es escribible).
        // Antes se emitía por el `Display` de Type, corrompiendo el archivo.
        let src = "struct R { h: fn(int, string) }\nfn f(cb: fn(int)) -> int { 0 }\nfn g(xs: [fn(int)]) -> int { 0 }\nfn main() -> int { 0 }\n";
        let out = fmt(src);
        assert!(!out.contains("-> unit"), "no must aparecer '-> unit': {out:?}");
        assert!(out.contains("h: fn(int, string)"), "campo función sin return_val: {out:?}");
        assert!(out.contains("cb: fn(int)"), "param función sin return_val: {out:?}");
        assert!(out.contains("xs: [fn(int)]"), "función anidada en array: {out:?}");
        // Un retorno NO-unit sí se conserva.
        assert!(fmt("fn f(cb: fn(int) -> bool) -> int { 0 }\nfn main() -> int { 0 }\n").contains("fn(int) -> bool"));
        assert_eq!(out, fmt(&out), "idempotente");
    }

    #[test]
    fn bytes_literal_alto_round_trip() {
        // Un byte alto se emite como \xNN y round-trippea (idempotente).
        let src = "fn main() -> int {\n  let b = b\"\\x8b\\xff\\x00A\";\n  b.len()\n}\n";
        let a = fmt(src);
        assert!(a.contains("\\x8b") && a.contains("\\xff") && a.contains("\\x00"), "{a}");
        assert!(a.contains('A'), "ASCII imprimible tal which: {a}");
        assert_eq!(a, fmt(&a), "idempotente");
    }

    // ─── Codemod M48.4e-2 (uso único): builtins de contenedor prefijos → `.metodo()` ───────────────────
    //
    // Reescribe `Call(Ident(builtin), [recv, ...resto])` → `Call(Field(recv, builtin), [...resto])` para
    // los 20 builtins traitificados (M48.4a–d) en todo el AST de cada `.ray` del corpus, y reemite con el
    // formateador. Como el corpus ya es **canónico** (fmt idempotente) y NO hay builtins retirados en el
    // azúcar (pipes/interpolación) ni **shadowing** por locales/params del mismo nombre —ambos verificados
    // antes de correrlo—, el diff resultante toca **solo** los sitios migrados. En e-2 los builtins siguen
    // vivos: `recv.metodo()` resuelve por el trait (coexistencia), así que el corpus corre igual.
    //
    //   cargo test --lib fmt::tests::migrate_prefix_builtins -- --ignored --nocapture

    const RETIRED: &[&str] = &[
        "len", "push", "reverse", "contains", "insert", "contains_key", "keys", "values", "trim", "split",
        "replace", "chars", "starts_with", "ends_with", "to_upper", "to_lower", "substring", "repeat",
        "to_bytes", "sub_bytes",
    ];

    fn cm_expr(e: &mut Expr, n: &mut usize) {
        // Post-orden: transformar los hijos antes que el nodo (así `len(push(a, x))` → `a.push(x).len()`).
        match &mut e.kind {
            ExprKind::Unary { expr, .. } => cm_expr(expr, n),
            ExprKind::Binary { left, right, .. } => {
                cm_expr(left, n);
                cm_expr(right, n);
            }
            ExprKind::Call { callee, args } => {
                cm_expr(callee, n);
                for a in args.iter_mut() {
                    cm_expr(a, n);
                }
            }
            ExprKind::ArrayLit(xs) | ExprKind::TupleLit(xs) => {
                for x in xs {
                    cm_expr(x, n);
                }
            }
            ExprKind::MapLit(ps) => {
                for (k, v) in ps {
                    cm_expr(k, n);
                    cm_expr(v, n);
                }
            }
            ExprKind::Index { array, index } => {
                cm_expr(array, n);
                cm_expr(index, n);
            }
            ExprKind::Cast { expr, .. } => cm_expr(expr, n),
            ExprKind::StructLit { fields, .. } => {
                for (_, v) in fields {
                    cm_expr(v, n);
                }
            }
            ExprKind::Field { object, .. } => cm_expr(object, n),
            ExprKind::EnumLit { args, .. } => {
                for a in args {
                    cm_expr(a, n);
                }
            }
            ExprKind::Func(fe) => cm_block(&mut fe.body, n),
            ExprKind::Match { scrutinee, arms } => {
                cm_expr(scrutinee, n);
                for arm in arms {
                    if let Some(g) = &mut arm.guard {
                        cm_expr(g, n);
                    }
                    cm_expr(&mut arm.body, n);
                }
            }
            ExprKind::Try(inner) => cm_expr(inner, n),
            ExprKind::If { cond, then_branch, else_branch } => {
                cm_expr(cond, n);
                cm_block(then_branch, n);
                if let Some(eb) = else_branch {
                    cm_expr(eb, n);
                }
            }
            ExprKind::While { cond, body } => {
                cm_expr(cond, n);
                cm_block(body, n);
            }
            ExprKind::Block(b) => cm_block(b, n),
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_)
            | ExprKind::Char(_) | ExprKind::Bytes(_) | ExprKind::Ident(_) => {}
        }
        // ¿Este nodo es una llamada prefija a un builtin retirado? → `recv.builtin(resto)`.
        let migrate = matches!(&e.kind, ExprKind::Call { callee, args }
            if !args.is_empty()
                && matches!(&callee.kind, ExprKind::Ident(nm) if RETIRED.contains(&nm.as_str())));
        if migrate {
            if let ExprKind::Call { callee, args } = std::mem::replace(&mut e.kind, ExprKind::Bool(false)) {
                let (name, cl, cc) = match callee.kind {
                    ExprKind::Ident(nm) => (nm, callee.line, callee.col),
                    _ => unreachable!(),
                };
                let mut it = args.into_iter();
                let recv = it.next().expect("args no vacío");
                let rest: Vec<Expr> = it.collect();
                let field = Expr { kind: ExprKind::Field { object: Box::new(recv), name }, line: cl, col: cc };
                e.kind = ExprKind::Call { callee: Box::new(field), args: rest };
                *n += 1;
            }
        }
    }

    fn cm_block(b: &mut Block, n: &mut usize) {
        for st in &mut b.statements {
            cm_stmt(st, n);
        }
        if let Some(t) = &mut b.tail {
            cm_expr(t, n);
        }
    }

    fn cm_stmt(st: &mut Stmt, n: &mut usize) {
        match &mut st.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => cm_expr(value, n),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => {
                        cm_expr(start, n);
                        cm_expr(end, n);
                    }
                    ForIter::In(e) => cm_expr(e, n),
                    ForIter::Iter { expr, .. } => cm_expr(expr, n),
                }
                cm_block(body, n);
            }
            StmtKind::Assign { target, value } => {
                cm_expr(target, n);
                cm_expr(value, n);
            }
            StmtKind::Return { value } => {
                if let Some(e) = value {
                    cm_expr(e, n);
                }
            }
            StmtKind::Expr(e) => cm_expr(e, n),
        }
    }

    fn cm_program(p: &mut Program, n: &mut usize) {
        for f in &mut p.functions {
            cm_block(&mut f.body, n);
        }
        for im in &mut p.impls {
            for m in &mut im.methods {
                cm_block(&mut m.body, n);
            }
        }
        for tr in &mut p.traits {
            for m in &mut tr.methods {
                if let Some(b) = &mut m.default_body {
                    cm_block(b, n);
                }
            }
        }
        for c in &mut p.consts {
            cm_expr(&mut c.value, n);
        }
        // Azúcar (pipes/interpolación): verificado 0 casos de builtin retirado; se dejan intactos (el
        // `rhs` de un pipe es parcial —sin receptor— y transformarlo sería incorrecto).
    }

    fn collect_ray(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                collect_ray(&p, out);
            } else if p.extension().map(|x| x == "ray").unwrap_or(false) {
                out.push(p);
            }
        }
    }

    #[test]
    #[ignore = "codemod auxiliar (M48.4e-3): migra el file en RAY_MIGRATE_FILE (p. ej. el prelude extraído)"]
    fn migrar_file_env() {
        let path = std::env::var("RAY_MIGRATE_FILE").expect("RAY_MIGRATE_FILE");
        let src = std::fs::read_to_string(&path).expect("lee");
        let tokens = crate::lexer::lex(&src).unwrap_or_else(|e| panic!("lex: {e}"));
        let mut program = crate::parser::parse(tokens).unwrap_or_else(|e| panic!("parse: {e}"));
        let mut n = 0;
        cm_program(&mut program, &mut n);
        let mut cur = Cur::new(&src, &program);
        let out = format_program(&program, &mut cur);
        std::fs::write(&path, &out).expect("escribe");
        println!("{n} sitios migrados en {path}");
    }

    #[test]
    #[ignore = "codemod de un solo use (M48.4e-2); runs con --ignored"]
    fn migrate_prefix_builtins() {
        let root = env!("CARGO_MANIFEST_DIR");
        let mut files = Vec::new();
        for d in ["examples", "std", "packages", "selfhost", "benchmarks"] {
            collect_ray(&std::path::Path::new(root).join(d), &mut files);
        }
        files.sort();
        let (mut sites, mut changed) = (0usize, 0usize);
        for f in &files {
            let src = std::fs::read_to_string(f).expect("lee");
            let tokens = crate::lexer::lex(&src).unwrap_or_else(|e| panic!("lex {f:?}: {e}"));
            let mut program = crate::parser::parse(tokens).unwrap_or_else(|e| panic!("parse {f:?}: {e}"));
            let mut n = 0;
            cm_program(&mut program, &mut n);
            if n == 0 {
                continue;
            }
            let mut cur = Cur::new(&src, &program);
            let out = format_program(&program, &mut cur);
            std::fs::write(f, &out).expect("escribe");
            sites += n;
            changed += 1;
            println!("{sites:>5}  (+{n:>3})  {}", f.strip_prefix(root).unwrap().display());
        }
        println!("TOTAL: {sites} sitios migrados en {changed} files");
    }
}
