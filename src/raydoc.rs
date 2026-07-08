//! **raydoc** (M40.4): generador de documentación en Markdown a partir de fuente raylang.
//!
//! Cliente del front-end (como `fmt`/`lsp`): usa el lexer+parser para la lista de ítems y sus
//! **firmas**, y un escaneo del fuente para los **comentarios de documentación** `///` que preceden a
//! cada ítem. No toca el núcleo. Documenta un archivo: si tiene ítems `pub` muestra solo esos (la
//! superficie pública del módulo); si no (un programa suelto), muestra todos.

use crate::ast::*;

/// Genera la documentación Markdown de `src`. `titulo` encabeza el documento (p. ej. el nombre del
/// archivo). Error si el fuente no lexea/parsea.
pub fn generate(src: &str, title: &str) -> Result<String, String> {
    let tokens = crate::lexer::lex(src).map_err(|e| format!("{e}"))?;
    let program = crate::parser::parse(tokens).map_err(|e| format!("{e}"))?;
    let lines: Vec<&str> = src.lines().collect();

    // ¿Hay ítems públicos? Si los hay, se documenta solo la superficie pública.
    let has_pub = program.functions.iter().any(|f| f.is_pub)
        || program.structs.iter().any(|s| s.is_pub)
        || program.enums.iter().any(|e| e.is_pub)
        || program.traits.iter().any(|t| t.is_pub);
    let include = |is_pub: bool| is_pub || !has_pub;

    let mut out = String::new();
    out.push_str(&format!("# {title}\n\n"));

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

    // --- Funciones ---
    let funcs: Vec<&Function> = program.functions.iter().filter(|f| include(f.is_pub)).collect();
    if !funcs.is_empty() {
        out.push_str("## Funciones\n\n");
        for f in funcs {
            out.push_str(&format!("### `{}`\n\n", sig_function(f)));
            emit_doc(&mut out, &lines, f.line);
        }
    }

    Ok(out)
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

/// El doc-comment de `linea` como una sola línea (para el resumen de raydoc).
fn doc_above(lines: &[&str], line: usize) -> Option<String> {
    doc_lines_above(lines, line).map(|ls| ls.join(" "))
}

/// Renderiza `<T, U: Bound>` a partir de los parámetros de tipo y sus bounds. Vacío si no hay.
fn generics(tps: &[String], bounds: &[(String, String)]) -> String {
    if tps.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = tps.iter().map(|tp| {
        let bs: Vec<&str> = bounds.iter().filter(|(p, _)| p == tp).map(|(_, t)| t.as_str()).collect();
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
    format!("fn {}{}({}){}", f.name, generics(&f.type_params, &f.bounds), sig_params(&f.params), sig_return(&f.return_type))
}

fn sig_method(m: &MethodSig) -> String {
    format!("fn {}{}({}){}", m.name, generics(&m.type_params, &m.bounds), sig_params(&m.params), sig_return(&m.return_type))
}

fn sig_struct(s: &StructDef) -> String {
    let fields = s.fields.iter().map(|(n, t)| format!("{n}: {t}")).collect::<Vec<_>>().join(", ");
    if fields.is_empty() {
        format!("struct {}{}", s.name, generics(&s.type_params, &s.bounds))
    } else {
        format!("struct {}{} {{ {fields} }}", s.name, generics(&s.type_params, &s.bounds))
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
    format!("enum {}{} {{ {variants} }}", e.name, generics(&e.type_params, &e.bounds))
}

fn sig_trait(t: &TraitDef) -> String {
    format!("trait {}{}", t.name, generics(&t.type_params, &[]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_lineas_arriba_no_desborda_con_linea_fuera_de_rango() {
        // Regresión (crash del LSP): `linea` puede caer fuera de `lineas` (símbolo cuya declaración
        // vive en otra fuente —prelude/wrapper inyectado—). No debe desbordar; devuelve `None`.
        let lines = vec!["/// doc", "fn f() {}"];
        assert_eq!(doc_lines_above(&lines, 726), None, "línea >> nº de líneas → None, sin panic");
        assert_eq!(doc_lines_above(&lines, 3), None, "una más allá del final → None");
        // El caso normal sigue funcionando.
        assert_eq!(doc_lines_above(&lines, 2), Some(vec!["doc".to_string()]), "doc encima de la línea 2");
    }

    #[test]
    fn documenta_superficie_publica() {
        let src = "\
/// Un punto.
/// En 2D.
pub struct Punto { x: int, y: int }

/// Suma dos.
pub fn suma(a: int, b: int) -> int { a + b }

// no-doc
fn privada() -> int { 0 }
";
        let md = generate(src, "m.ray").unwrap();
        assert!(md.contains("# m.ray"));
        assert!(md.contains("### `struct Punto { x: int, y: int }`"));
        assert!(md.contains("Un punto. En 2D.")); // multi-línea unida
        assert!(md.contains("### `fn suma(a: int, b: int) -> int`"));
        assert!(md.contains("Suma dos."));
        // Con ítems pub, los privados no aparecen.
        assert!(!md.contains("privada"));
    }

    #[test]
    fn genericos_y_bounds_y_self() {
        let src = "\
pub trait Med { fn medir(self) -> int; }
pub fn maxi<T: Ord>(xs: [T]) -> T { xs[0] }
pub enum Caja<T> { Llena(T), Vacia }
";
        let md = generate(src, "g.ray").unwrap();
        assert!(md.contains("`fn medir(self) -> int`"), "self sin tipo:\n{md}");
        assert!(md.contains("`fn maxi<T: Ord>(xs: [T]) -> T`"), "generics+bound:\n{md}");
        assert!(md.contains("`enum Caja<T> { Llena(T), Vacia }`"), "enum:\n{md}");
    }

    #[test]
    fn sin_pub_documenta_todo() {
        // Un programa suelto (sin `pub`) documenta todos sus ítems.
        let src = "/// La entrada.\nfn main() -> int { 0 }\n";
        let md = generate(src, "p.ray").unwrap();
        assert!(md.contains("`fn main() -> int`"));
        assert!(md.contains("La entrada."));
    }
}
