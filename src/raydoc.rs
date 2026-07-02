//! **raydoc** (M40.4): generador de documentación en Markdown a partir de fuente raylang.
//!
//! Cliente del front-end (como `fmt`/`lsp`): usa el lexer+parser para la lista de ítems y sus
//! **firmas**, y un escaneo del fuente para los **comentarios de documentación** `///` que preceden a
//! cada ítem. No toca el núcleo. Documenta un archivo: si tiene ítems `pub` muestra solo esos (la
//! superficie pública del módulo); si no (un programa suelto), muestra todos.

use crate::ast::*;

/// Genera la documentación Markdown de `src`. `titulo` encabeza el documento (p. ej. el nombre del
/// archivo). Error si el fuente no lexea/parsea.
pub fn generate(src: &str, titulo: &str) -> Result<String, String> {
    let tokens = crate::lexer::lex(src).map_err(|e| format!("{e}"))?;
    let program = crate::parser::parse(tokens).map_err(|e| format!("{e}"))?;
    let lineas: Vec<&str> = src.lines().collect();

    // ¿Hay ítems públicos? Si los hay, se documenta solo la superficie pública.
    let hay_pub = program.functions.iter().any(|f| f.is_pub)
        || program.structs.iter().any(|s| s.is_pub)
        || program.enums.iter().any(|e| e.is_pub)
        || program.traits.iter().any(|t| t.is_pub);
    let incluir = |es_pub: bool| es_pub || !hay_pub;

    let mut out = String::new();
    out.push_str(&format!("# {titulo}\n\n"));

    // --- Traits ---
    let traits: Vec<&TraitDef> = program.traits.iter().filter(|t| incluir(t.is_pub)).collect();
    if !traits.is_empty() {
        out.push_str("## Traits\n\n");
        for t in traits {
            out.push_str(&format!("### `{}`\n\n", firma_trait(t)));
            emitir_doc(&mut out, &lineas, t.line);
            for m in &t.methods {
                out.push_str(&format!("- `{}`\n", firma_metodo(m)));
            }
            out.push('\n');
        }
    }

    // --- Structs ---
    let structs: Vec<&StructDef> = program.structs.iter().filter(|s| incluir(s.is_pub)).collect();
    if !structs.is_empty() {
        out.push_str("## Structs\n\n");
        for s in structs {
            out.push_str(&format!("### `{}`\n\n", firma_struct(s)));
            emitir_doc(&mut out, &lineas, s.line);
        }
    }

    // --- Enums ---
    let enums: Vec<&EnumDef> = program.enums.iter().filter(|e| incluir(e.is_pub)).collect();
    if !enums.is_empty() {
        out.push_str("## Enums\n\n");
        for e in enums {
            out.push_str(&format!("### `{}`\n\n", firma_enum(e)));
            emitir_doc(&mut out, &lineas, e.line);
        }
    }

    // --- Funciones ---
    let funcs: Vec<&Function> = program.functions.iter().filter(|f| incluir(f.is_pub)).collect();
    if !funcs.is_empty() {
        out.push_str("## Funciones\n\n");
        for f in funcs {
            out.push_str(&format!("### `{}`\n\n", firma_funcion(f)));
            emitir_doc(&mut out, &lineas, f.line);
        }
    }

    Ok(out)
}

/// Añade el comentario de documentación de un ítem (las líneas `///` justo encima de `linea`, 1-basada)
/// como un párrafo. Si no hay, no añade nada.
fn emitir_doc(out: &mut String, lineas: &[&str], linea: usize) {
    if let Some(doc) = doc_arriba(lineas, linea) {
        out.push_str(&doc);
        out.push_str("\n\n");
    }
}

/// Reúne las líneas `///` **contiguas** inmediatamente encima de `linea` (1-basada), de arriba abajo.
fn doc_arriba(lineas: &[&str], linea: usize) -> Option<String> {
    let mut docs: Vec<String> = Vec::new();
    let mut i = linea as isize - 2; // la línea justo encima, 0-basada
    while i >= 0 {
        let l = lineas[i as usize].trim_start();
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
    Some(docs.join(" "))
}

/// Renderiza `<T, U: Bound>` a partir de los parámetros de tipo y sus bounds. Vacío si no hay.
fn generics(tps: &[String], bounds: &[(String, String)]) -> String {
    if tps.is_empty() {
        return String::new();
    }
    let partes: Vec<String> = tps.iter().map(|tp| {
        let bs: Vec<&str> = bounds.iter().filter(|(p, _)| p == tp).map(|(_, t)| t.as_str()).collect();
        if bs.is_empty() { tp.clone() } else { format!("{tp}: {}", bs.join(" + ")) }
    }).collect();
    format!("<{}>", partes.join(", "))
}

fn firma_params(params: &[Param]) -> String {
    // El receptor `self` de un método se escribe sin tipo (como en la fuente).
    params.iter().map(|p| {
        if p.name == "self" { "self".to_string() } else { format!("{}: {}", p.name, p.ty) }
    }).collect::<Vec<_>>().join(", ")
}

fn firma_retorno(ret: &Type) -> String {
    match ret {
        Type::Unit => String::new(),
        otro => format!(" -> {otro}"),
    }
}

fn firma_funcion(f: &Function) -> String {
    format!("fn {}{}({}){}", f.name, generics(&f.type_params, &f.bounds), firma_params(&f.params), firma_retorno(&f.return_type))
}

fn firma_metodo(m: &MethodSig) -> String {
    format!("fn {}{}({}){}", m.name, generics(&m.type_params, &m.bounds), firma_params(&m.params), firma_retorno(&m.return_type))
}

fn firma_struct(s: &StructDef) -> String {
    let campos = s.fields.iter().map(|(n, t)| format!("{n}: {t}")).collect::<Vec<_>>().join(", ");
    if campos.is_empty() {
        format!("struct {}{}", s.name, generics(&s.type_params, &s.bounds))
    } else {
        format!("struct {}{} {{ {campos} }}", s.name, generics(&s.type_params, &s.bounds))
    }
}

fn firma_enum(e: &EnumDef) -> String {
    let variantes = e.variants.iter().map(|v| {
        if v.payload.is_empty() {
            v.name.clone()
        } else {
            let tys = v.payload.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ");
            format!("{}({})", v.name, tys)
        }
    }).collect::<Vec<_>>().join(", ");
    format!("enum {}{} {{ {variantes} }}", e.name, generics(&e.type_params, &e.bounds))
}

fn firma_trait(t: &TraitDef) -> String {
    format!("trait {}{}", t.name, generics(&t.type_params, &[]))
}

#[cfg(test)]
mod tests {
    use super::*;

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
