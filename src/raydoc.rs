//! **raydoc** (M40.4): generador de documentación en Markdown a partir de fuente raylang.
//!
//! Cliente del front-end (como `fmt`/`lsp`): usa el lexer+parser para la lista de ítems y sus
//! **firmas**, y un escaneo del fuente para los **comentarios de documentación** `///` que preceden a
//! cada ítem. No toca el núcleo. Documenta un archivo: si tiene ítems `pub` muestra solo esos (la
//! superficie pública del módulo); si no (un programa suelto), muestra todos.

use crate::ast::*;

/// Genera la documentación Markdown de `src`. `titulo` encabeza el documento (p. ej. el nombre del
/// archivo). Error si el fuente no lexea/parsea.
/// M376 (ray-ds #126): la documentación DE MÓDULO — las líneas `//!` con que empieza el archivo
/// (tras líneas en blanco), como en Rust. Es doc pública (inglés, como `///`): la muestran
/// `ray doc`, `ray_doc <módulo>` del MCP y el listado de un módulo del proyecto. Un comentario
/// `//` ordinario la termina. `None` si el archivo no empieza por `//!`.
pub fn module_doc(src: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        if t.is_empty() && out.is_empty() {
            continue;
        }
        match t.strip_prefix("//!") {
            Some(rest) => out.push(rest.strip_prefix(' ').unwrap_or(rest).trim_end().to_string()),
            None => break,
        }
    }
    let text = out.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Un nombre que una fachada re-exporta con `pub from M import a as b;` (rayhotel #485): el nombre
/// visible (`b`), el original (`a`), el módulo tal como se escribió y el archivo —con su fuente—
/// donde el ítem se DEFINE (siguiendo las cadenas de re-export).
pub struct Reexported {
    /// Reexportado con `opaque`: su forma no es parte del contrato.
    pub opaque: bool,
    pub visible: String,
    pub original: String,
    pub module: String,
    pub file: std::path::PathBuf,
    pub source: String,
}

/// El archivo de un módulo importado desde `from_file`: `<base>/<módulo>.ray` o
/// `<base>/<módulo>/mod.ray` con `base` cada directorio hacia arriba hasta el del `ray.toml`; ahí
/// se prueba también sin el primer segmento (dentro de un paquete sus módulos se importan con el
/// nombre del paquete delante). Es para DOCUMENTAR: no aplica la regla de cápsula ni las
/// dependencias — lo que no encuentra, se omite.
pub fn locate_module(from_file: &std::path::Path, module: &str) -> Option<std::path::PathBuf> {
    let try_at = |base: &std::path::Path, m: &str| {
        let file = base.join(format!("{m}.ray"));
        if file.is_file() {
            return Some(file);
        }
        let facade = base.join(m).join("mod.ray");
        facade.is_file().then_some(facade)
    };
    for base in from_file.parent()?.ancestors() {
        if let Some(found) = try_at(base, module) {
            return Some(found);
        }
        if base.join("ray.toml").is_file() {
            return module.split_once('/').and_then(|(_, rest)| try_at(base, rest));
        }
    }
    None
}

/// Lo que el fuente `src` (del archivo `file`) re-exporta con `pub from`, resuelto hasta su
/// definición. Vacío si no re-exporta nada o no parsea.
pub fn reexports(src: &str, file: &std::path::Path) -> Vec<Reexported> {
    fn defines(prog: &crate::ast::Program, name: &str) -> bool {
        prog.functions.iter().any(|f| f.name == name)
            || prog.structs.iter().any(|s| s.name == name)
            || prog.enums.iter().any(|e| e.name == name)
            || prog.traits.iter().any(|t| t.name == name)
            || prog.consts.iter().any(|c| c.name == name)
            || prog.type_aliases.iter().any(|a| a.name == name)
    }
    fn parse(src: &str) -> Option<crate::ast::Program> {
        crate::parser::parse(crate::lexer::lex(src).ok()?).ok()
    }
    // Sigue `name` desde `file` hasta el archivo que lo define (tope de saltos: sin ciclos).
    fn definition(file: &std::path::Path, module: &str, name: &str, hops: usize) -> Option<(std::path::PathBuf, String, String)> {
        let target = locate_module(file, module)?;
        let source = std::fs::read_to_string(&target).ok()?;
        let prog = parse(&source)?;
        if defines(&prog, name) {
            return Some((target, source, name.to_string()));
        }
        if hops == 0 {
            return None;
        }
        prog.from_imports.iter().filter(|fi| fi.is_pub).find_map(|fi| {
            let n = fi.names.iter().find(|n| n.local() == name)?;
            definition(&target, &fi.module, &n.name, hops - 1)
        })
    }
    let Some(prog) = parse(src) else { return Vec::new() };
    let mut out = Vec::new();
    for fi in prog.from_imports.iter().filter(|fi| fi.is_pub) {
        for n in &fi.names {
            if let Some((def_file, source, original)) = definition(file, &fi.module, &n.name, 4) {
                out.push(Reexported { opaque: n.opaque, visible: n.local().to_string(), original, module: fi.module.clone(), file: def_file, source });
            }
        }
    }
    out
}

/// La cara de un struct reexportado como `opaque`: su nombre y sus parámetros de tipo, sin campos.
pub fn opaque_struct(s: &StructDef) -> String {
    format!("struct {}{} (opaque)", s.name, generics(&s.type_params, &[], &Vec::new()))
}

/// La documentación Markdown de un ARCHIVO: la de su fuente (`generate`) más lo que re-exporta con
/// `pub from` — que en una fachada `dir/mod.ray` es el contrato entero y antes se omitía. Una
/// fachada se titula con el nombre de su cápsula (el directorio), no `mod.ray`.
pub fn generate_file(path: &std::path::Path) -> Result<String, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("could not read '{}': {e}", path.display()))?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("module");
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let title = match (file_name, abs.parent().and_then(|d| d.file_name()).and_then(|n| n.to_str())) {
        ("mod.ray", Some(dir)) => dir.to_string(),
        _ => file_name.to_string(),
    };
    let mut out = generate(&src, &title)?;
    let items = reexports(&src, &abs);
    if items.is_empty() {
        return Ok(out);
    }
    out.push_str("## Re-exports\n\n");
    for item in items {
        let Ok(tokens) = crate::lexer::lex(&item.source) else { continue };
        let Ok(prog) = crate::parser::parse(tokens) else { continue };
        let lines: Vec<&str> = item.source.lines().collect();
        let name = item.original.as_str();
        let (sig, line) = if let Some(f) = prog.functions.iter().find(|f| f.name == name) {
            (sig_function(f), f.line)
        } else if let Some(s) = prog.structs.iter().find(|s| s.name == name) {
            // Un tipo opaco se documenta SIN sus campos: no son parte del contrato.
            if item.opaque { (opaque_struct(s), s.line) } else { (sig_struct(s), s.line) }
        } else if let Some(e) = prog.enums.iter().find(|e| e.name == name) {
            (sig_enum(e), e.line)
        } else if let Some(t) = prog.traits.iter().find(|t| t.name == name) {
            (sig_trait(t), t.line)
        } else if let Some(c) = prog.consts.iter().find(|c| c.name == name) {
            (const_signature(c), c.line)
        } else if let Some(a) = prog.type_aliases.iter().find(|a| a.name == name) {
            (format!("type {}{} = {}", a.name, generics(&a.type_params, &[], &Vec::new()), a.target), a.line)
        } else {
            continue;
        };
        out.push_str(&format!("### `{sig}`\n\n"));
        if item.visible != item.original {
            out.push_str(&format!("Exported as `{}`. ", item.visible));
        }
        out.push_str(&format!("From `{}`.\n\n", item.module));
        emit_doc(&mut out, &lines, line);
    }
    Ok(out)
}

pub fn generate(src: &str, title: &str) -> Result<String, String> {
    let tokens = crate::lexer::lex(src).map_err(|e| format!("{e}"))?;
    let program = crate::parser::parse(tokens).map_err(|e| format!("{e}"))?;
    let lines: Vec<&str> = src.lines().collect();

    // ¿Hay ítems públicos? Si los hay, se documenta solo la superficie pública.
    let has_pub = program.functions.iter().any(|f| f.is_pub)
        || program.structs.iter().any(|s| s.is_pub)
        || program.enums.iter().any(|e| e.is_pub)
        || program.traits.iter().any(|t| t.is_pub)
        || program.type_aliases.iter().any(|a| a.is_pub);
    let include = |is_pub: bool| is_pub || !has_pub;

    let mut out = String::new();
    out.push_str(&format!("# {title}\n\n"));
    if let Some(doc) = module_doc(src) {
        out.push_str(&doc);
        out.push_str("\n\n");
    }

    // --- Traits ---
    let traits: Vec<&TraitDef> = program.traits.iter().filter(|t| include(t.is_pub)).collect();
    if !traits.is_empty() {
        out.push_str("## Traits\n\n");
        for t in traits {
            out.push_str(&format!("### `{}`\n\n", sig_trait(t)));
            emit_doc(&mut out, &lines, t.line);
            for m in &t.methods {
                out.push_str(&format!("- `{}`\n", sig_method(m)));
            }
            out.push('\n');
        }
    }

    // --- Structs ---
    let structs: Vec<&StructDef> = program.structs.iter().filter(|s| include(s.is_pub)).collect();
    if !structs.is_empty() {
        out.push_str("## Structs\n\n");
        for s in structs {
            out.push_str(&format!("### `{}`\n\n", sig_struct(s)));
            emit_doc(&mut out, &lines, s.line);
            let fields = field_docs(s, &lines);
            if fields.iter().any(|(_, _, d)| d.is_some()) {
                for (name, ty, doc) in fields {
                    match doc {
                        Some(d) => out.push_str(&format!("- `{name}: {ty}` — {d}\n")),
                        None => out.push_str(&format!("- `{name}: {ty}`\n")),
                    }
                }
                out.push('\n');
            }
        }
    }

    // --- Alias de tipo (M311) ---
    let aliases: Vec<&TypeAliasDef> = program.type_aliases.iter().filter(|a| include(a.is_pub)).collect();
    if !aliases.is_empty() {
        out.push_str("## Type aliases\n\n");
        for a in aliases {
            out.push_str(&format!("### `type {}{} = {}`\n\n", a.name, generics(&a.type_params, &[], &Vec::new()), a.target));
            emit_doc(&mut out, &lines, a.line);
        }
    }

    // --- Enums ---
    let enums: Vec<&EnumDef> = program.enums.iter().filter(|e| include(e.is_pub)).collect();
    if !enums.is_empty() {
        out.push_str("## Enums\n\n");
        for e in enums {
            out.push_str(&format!("### `{}`\n\n", sig_enum(e)));
            emit_doc(&mut out, &lines, e.line);
        }
    }

    // --- Constantes (M326, findings #101: el modo símbolo ya las daba desde M305; el Markdown no) ---
    let consts: Vec<&ConstDef> = program.consts.iter().filter(|c| include(c.is_pub)).collect();
    if !consts.is_empty() {
        out.push_str("## Constants\n\n");
        for c in consts {
            out.push_str(&format!("### `{}`\n\n", const_signature(c)));
            emit_doc(&mut out, &lines, c.line);
        }
    }

    // --- Funciones ---
    let funcs: Vec<&Function> = program.functions.iter().filter(|f| include(f.is_pub)).collect();
    if !funcs.is_empty() {
        out.push_str("## Functions\n\n");
        for f in funcs {
            out.push_str(&format!("### `{}`\n\n", sig_function(f)));
            emit_doc(&mut out, &lines, f.line);
        }
    }

    Ok(out)
}

/// La firma de una constante (M305): `const NAME: T = valor` cuando el valor es un literal simple
/// (entero, flotante, bool o cadena); si no, la firma sin valor. Compartida por raydoc y el MCP.
pub fn const_signature(c: &ConstDef) -> String {
    let value = match &c.value.kind {
        ExprKind::Int(v, _) => Some(v.to_string()),
        ExprKind::Float(f) => Some(f.to_string()),
        ExprKind::Bool(b) => Some(b.to_string()),
        ExprKind::Str(t) => Some(format!("{t:?}")),
        _ => None,
    };
    match value {
        Some(v) => format!("const {}: {} = {v}", c.name, c.ty),
        None => format!("const {}: {}", c.name, c.ty),
    }
}

/// Añade el comentario de documentación de un ítem (las líneas `///` justo encima de `linea`, 1-basada)
/// como un párrafo. Si no hay, no añade nada.
fn emit_doc(out: &mut String, lines: &[&str], line: usize) {
    if let Some(doc) = doc_above(lines, line) {
        out.push_str(&doc);
        out.push_str("\n\n");
    }
}

/// Reúne las líneas `///` **contiguas** inmediatamente encima de `linea` (1-basada), de arriba abajo.
/// Devuelve el texto de cada línea de doc (sin el `///`), en orden. `None` si no hay ninguna.
/// Compartido con el LSP (hover): raydoc las une con espacios (resumen de una línea); el hover con
/// saltos de línea (Markdown).
pub fn doc_lines_above(lines: &[&str], line: usize) -> Option<Vec<String>> {
    let mut docs: Vec<String> = Vec::new();
    let mut i = line as isize - 2; // la línea justo encima, 0-basada
    // Cota superior: `linea` puede caer FUERA de `lineas` (el LSP la usa con la posición de declaración
    // de un símbolo que en realidad vive en OTRA fuente —una función del prelude/wrapper inyectada, cuya
    // posición no está en el archivo abierto—). Sin esta cota, `lineas[i]` desbordaría (ICE en el LSP).
    while i >= 0 && (i as usize) < lines.len() {
        let l = lines[i as usize].trim_start();
        if let Some(rest) = l.strip_prefix("///") {
            docs.push(rest.trim().to_string());
            i -= 1;
        } else if l.starts_with('@') && docs.is_empty() {
            // M435: un atributo (`@derive(Eq, Show)`) entre la doc y la declaración no la esconde —
            // `ray fmt` deja la doc ENCIMA del atributo, así que es la forma canónica.
            i -= 1;
        } else {
            break;
        }
    }
    if docs.is_empty() {
        return None;
    }
    docs.reverse();
    Some(docs)
}

/// Cada campo de un struct con su `///` (rayzip #199: la doc de `Stat.mode` decía que vale 0 en
/// Windows, pero ni `ray doc` ni raydoc mostraban la doc de los campos). `None` si el campo no
/// tiene; sin `field_lines` (struct sintético) ninguno la tiene. Compartido con el MCP.
pub fn field_docs(s: &StructDef, lines: &[&str]) -> Vec<(String, String, Option<String>)> {
    s.fields
        .iter()
        .enumerate()
        .map(|(i, (name, ty))| {
            let doc = s.field_lines.get(i).and_then(|&l| doc_above(lines, l));
            (name.clone(), ty.to_string(), doc)
        })
        .collect()
}

/// El doc-comment de `linea` como una sola línea (para el resumen de raydoc).
fn doc_above(lines: &[&str], line: usize) -> Option<String> {
    doc_lines_above(lines, line).map(|ls| ls.join(" "))
}

/// Renderiza `<T, U: Bound>` a partir de los parámetros de tipo y sus bounds. Vacío si no hay.
fn generics(tps: &[String], bounds: &[(String, String)], bound_args: &crate::ast::BoundArgs) -> String {
    if tps.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = tps.iter().map(|tp| {
        let bs = crate::ast::bound_texts(tp, bounds, bound_args, &|t| t.to_string());
        if bs.is_empty() { tp.clone() } else { format!("{tp}: {}", bs.join(" + ")) }
    }).collect();
    format!("<{}>", parts.join(", "))
}

fn sig_params(params: &[Param]) -> String {
    // El receptor `self` de un método se escribe sin tipo (como en la fuente).
    params.iter().map(|p| {
        if p.name == "self" { "self".to_string() } else { format!("{}: {}", p.name, p.ty) }
    }).collect::<Vec<_>>().join(", ")
}

fn sig_return(ret: &Type) -> String {
    match ret {
        Type::Unit => String::new(),
        other => format!(" -> {other}"),
    }
}

fn sig_function(f: &Function) -> String {
    format!("fn {}{}({}){}", f.name, generics(&f.type_params, &f.bounds, &f.bound_args), sig_params(&f.params), sig_return(&f.return_type))
}

fn sig_method(m: &MethodSig) -> String {
    format!("fn {}{}({}){}", m.name, generics(&m.type_params, &m.bounds, &m.bound_args), sig_params(&m.params), sig_return(&m.return_type))
}

fn sig_struct(s: &StructDef) -> String {
    let fields = s.fields.iter().map(|(n, t)| format!("{n}: {t}")).collect::<Vec<_>>().join(", ");
    if fields.is_empty() {
        format!("struct {}{}", s.name, generics(&s.type_params, &s.bounds, &s.bound_args))
    } else {
        format!("struct {}{} {{ {fields} }}", s.name, generics(&s.type_params, &s.bounds, &s.bound_args))
    }
}

fn sig_enum(e: &EnumDef) -> String {
    let variants = e.variants.iter().map(|v| {
        if v.payload.is_empty() {
            v.name.clone()
        } else {
            let tys = v.payload.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ");
            format!("{}({})", v.name, tys)
        }
    }).collect::<Vec<_>>().join(", ");
    format!("enum {}{} {{ {variants} }}", e.name, generics(&e.type_params, &e.bounds, &e.bound_args))
}

fn sig_trait(t: &TraitDef) -> String {
    let supers = if t.supertraits.is_empty() { String::new() } else { format!(": {}", t.supertraits.join(" + ")) };
    format!("trait {}{}{supers}", t.name, generics(&t.type_params, &[], &Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M376: `//!` al principio del archivo = doc de módulo; un `//` corriente la cierra; sin `//!`, nada.
    #[test]
    fn module_doc_is_the_leading_bang_comment_block() {
        let src = "\n//! A module.\n//!\n//!   Indented line.\n// interno\n//! no cuenta\npub fn f() {}\n";
        assert_eq!(module_doc(src).as_deref(), Some("A module.\n\n  Indented line."));
        assert_eq!(module_doc("// solo interno\npub fn f() {}\n"), None);
        let md = generate(src, "m").unwrap();
        assert!(md.starts_with("# m\n\nA module.\n\n  Indented line.\n\n"), "{md}");
    }

    #[test]
    fn doc_lines_above_skips_an_attribute_between_the_doc_and_the_declaration() {
        // M435 (net/dns_message): `/// doc` + `@derive(Eq, Show)` + `pub struct` — la doc es la de
        // la línea 1 aunque el atributo vaya en medio (la forma canónica de `ray fmt`).
        let lines = vec!["/// The question.", "/// Two lines.", "@derive(Eq, Show)", "pub struct Q {}"];
        assert_eq!(doc_lines_above(&lines, 4), Some(vec!["The question.".to_string(), "Two lines.".to_string()]));
        // Un atributo sin doc encima sigue siendo «sin doc».
        let bare = vec!["fn other() {}", "@derive(Eq)", "pub struct R {}"];
        assert_eq!(doc_lines_above(&bare, 3), None);
    }

    #[test]
    fn doc_lines_above_does_not_overflow_with_a_line_outside_of_range() {
        // Regresión (crash del LSP): `linea` puede caer fuera de `lineas` (símbolo cuya declaración
        // vive en otra fuente —prelude/wrapper inyectado—). No debe desbordar; devuelve `None`.
        let lines = vec!["/// doc", "fn f() {}"];
        assert_eq!(doc_lines_above(&lines, 726), None, "línea >> nº de líneas → None, sin panic");
        assert_eq!(doc_lines_above(&lines, 3), None, "one más allá del final → None");
        // El caso normal sigue funcionando.
        assert_eq!(doc_lines_above(&lines, 2), Some(vec!["doc".to_string()]), "doc encima de la línea 2");
    }

    #[test]
    fn documents_the_public_surface() {
        let src = "\
/// Un punto.
/// En 2D.
pub struct Point { x: int, y: int }

/// Suma dos.
pub fn sum(a: int, b: int) -> int { a + b }

// no-doc
fn privada() -> int { 0 }
";
        let md = generate(src, "m.ray").unwrap();
        assert!(md.contains("# m.ray"));
        assert!(md.contains("### `struct Point { x: int, y: int }`"));
        // M311: los alias públicos se documentan con su expansión.
        let md2 = generate("/// A point as a pair.\npub type Pt<T> = (T, T);\ntype Hidden = int;\npub fn f() -> Pt<int> { (0, 0) }\n", "g.ray").unwrap();
        assert!(md2.contains("## Type aliases"), "{md2}");
        assert!(md2.contains("### `type Pt<T> = (T, T)`\n\nA point as a pair."), "{md2}");
        assert!(!md2.contains("Hidden"), "{md2}");
        assert!(md.contains("Un punto. En 2D.")); // multi-línea unida
        assert!(md.contains("### `fn sum(a: int, b: int) -> int`"));
        assert!(md.contains("Suma dos."));
        // Con ítems pub, los privados no aparecen.
        assert!(!md.contains("privada"));
        // M326 (findings #101): las constantes públicas también, con su valor literal.
        let md3 = generate("/// Retries.\npub const MAX_RETRIES: int = 3;\npub const NAMES: [string] = [\"a\"];\nconst HIDDEN: int = 1;\npub fn f() -> int { MAX_RETRIES }\n", "cfg.ray").unwrap();
        assert!(md3.contains("## Constants\n\n### `const MAX_RETRIES: int = 3`\n\nRetries.\n\n### `const NAMES: [string]`"), "{md3}");
        assert!(!md3.contains("HIDDEN"), "{md3}");
        let pos_c = md3.find("## Constants").unwrap();
        assert!(pos_c < md3.find("## Functions").unwrap(), "las constantes van antes que las funciones");
    }

    #[test]
    fn a_struct_lists_the_doc_of_its_fields() {
        let src = "/// A point.\npub struct P {\n    /// Horizontal.\n    x: int,\n    y: int,\n}\npub fn f() -> int { 0 }\n";
        let md = generate(src, "p.ray").unwrap();
        assert!(md.contains("- `x: int` — Horizontal.\n"), "{md}");
        assert!(md.contains("- `y: int`\n"), "{md}");
        let bare = generate("pub struct Q { x: int }\npub fn f() -> int { 0 }\n", "q.ray").unwrap();
        assert!(!bare.contains("- `x: int`"), "sin doc de campos no se listan:\n{bare}");
    }

    #[test]
    fn generics_y_bounds_y_self() {
        let src = "\
pub trait Med { fn measure(self) -> int; }
pub fn maxi<T: Ord>(xs: [T]) -> T { xs[0] }
pub enum Box<T> { Llena(T), Vacia }
";
        let md = generate(src, "g.ray").unwrap();
        assert!(md.contains("`fn measure(self) -> int`"), "self sin type:\n{md}");
        assert!(md.contains("`fn maxi<T: Ord>(xs: [T]) -> T`"), "generics+bound:\n{md}");
        assert!(md.contains("`enum Box<T> { Llena(T), Vacia }`"), "enum:\n{md}");
    }

    #[test]
    fn without_pub_documents_everything() {
        // Un programa suelto (sin `pub`) documenta todos sus ítems.
        let src = "/// La entry.\nfn main() -> int { 0 }\n";
        let md = generate(src, "p.ray").unwrap();
        assert!(md.contains("`fn main() -> int`"));
        assert!(md.contains("La entry."));
    }
}
