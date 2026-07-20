//! Lector mínimo de **`.editorconfig`** para `ray fmt` (solo indentación).
//!
//! `.editorconfig` es el estándar de facto entre editores/herramientas para fijar el estilo de
//! indentación por proyecto. Aquí se lee lo justo para el formateador: `indent_style` (`space`/`tab`)
//! e `indent_size`. Se sube por los directorios ancestros del archivo juntando lo que casa, hasta un
//! `.editorconfig` con `root = true` (que corta la búsqueda hacia arriba). Los ajustes **más cercanos**
//! ganan; dentro de un archivo, la **última** sección que casa gana (semántica de `.editorconfig`).
//!
//! Globs soportados (subconjunto suficiente): `*` (todo), `*.ray`, `*.{ray,otro}` (lista de
//! extensiones) y un nombre exacto (`main.ray`). Cero dependencias.

use std::path::Path;

/// La indentación efectiva para `file` según los `.editorconfig` ancestros: `(style, size)` con
/// `style` = `"space"`/`"tab"`. Cualquiera puede faltar (`None`) si ningún `.editorconfig` lo fija.
pub fn indent_for(file: &Path) -> (Option<String>, Option<usize>) {
    let name = file.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let (mut style, mut size) = (None, None);
    let mut dir = file.parent();
    while let Some(d) = dir {
        if let Ok(src) = std::fs::read_to_string(d.join(".editorconfig")) {
            let (s, z, is_root) = parse(&src, name);
            // Más cercano gana: solo rellenamos lo que aún no tengamos.
            if style.is_none() {
                style = s;
            }
            if size.is_none() {
                size = z;
            }
            if is_root {
                break; // `root = true` corta el ascenso
            }
        }
        dir = d.parent();
    }
    (style, size)
}

/// Parsea un `.editorconfig` para `filename`: devuelve `(indent_style, indent_size, root)` de la
/// **última** sección que casa (semántica del formato).
fn parse(src: &str, filename: &str) -> (Option<String>, Option<usize>, bool) {
    let (mut style, mut size, mut root) = (None, None, false);
    let mut seen_section = false;
    let mut matched = false;
    for line in src.lines() {
        // Comentarios: `#` o `;` a inicio de token.
        let l = line.split(['#', ';']).next().unwrap_or("").trim();
        if l.is_empty() {
            continue;
        }
        if let Some(rest) = l.strip_prefix('[') {
            if let Some(glob) = rest.strip_suffix(']') {
                seen_section = true;
                matched = glob_matches(glob.trim(), filename);
            }
            continue;
        }
        let Some((key, value)) = l.split_once('=') else { continue };
        let (key, value) = (key.trim().to_lowercase(), value.trim());
        // `root = true` solo cuenta en el preámbulo (antes de cualquier sección).
        if !seen_section && key == "root" {
            root = value.eq_ignore_ascii_case("true");
            continue;
        }
        if matched {
            match key.as_str() {
                "indent_style" => style = Some(value.to_lowercase()),
                "indent_size" => size = value.parse::<usize>().ok(),
                _ => {}
            }
        }
    }
    (style, size, root)
}

/// ¿El glob de una sección de `.editorconfig` casa con `filename`? Subconjunto: `*`, `*.ext`,
/// `*.{a,b,c}` y nombre exacto.
fn glob_matches(glob: &str, filename: &str) -> bool {
    if glob == "*" {
        return true;
    }
    if let Some(rest) = glob.strip_prefix("*.") {
        // `*.{ray,foo}` (lista) o `*.ray` (una extensión).
        if let Some(list) = rest.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            return list.split(',').any(|ext| filename.ends_with(&format!(".{}", ext.trim())));
        }
        return filename.ends_with(&format!(".{}", rest));
    }
    glob == filename
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_glob() {
        assert!(glob_matches("*", "main.ray"));
        assert!(glob_matches("*.ray", "main.ray"));
        assert!(!glob_matches("*.rs", "main.ray"));
        assert!(glob_matches("*.{ray,toml}", "ray.toml"));
        assert!(glob_matches("main.ray", "main.ray"));
        assert!(!glob_matches("other.ray", "main.ray"));
    }

    #[test]
    fn parses_sections_and_root() {
        let src = "root = true\n\n[*]\nindent_style = space\nindent_size = 2\n\n[*.ray]\nindent_size = 4\n";
        // Para un .ray, la última sección que casa (`[*.ray]`) fija size=4; `[*]` fija style=space.
        let (style, size, root) = parse(src, "main.ray");
        assert_eq!(style.as_deref(), Some("space"));
        assert_eq!(size, Some(4));
        assert!(root);
        // Para un archivo que no es .ray, solo casa `[*]`.
        let (_, size2, _) = parse(src, "notas.txt");
        assert_eq!(size2, Some(2));
    }

    #[test]
    fn tab_style() {
        let (style, _, _) = parse("[*]\nindent_style = tab\n", "x.ray");
        assert_eq!(style.as_deref(), Some("tab"));
    }
}
