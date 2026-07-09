//! `ray templ` (M55): **templates compilados** — un archivo `.ray.html` se compila a una FUNCIÓN
//! raylang tipada, en la línea de `templ` (Go) o `askama` (Rust). Es la versión "limpia" de la
//! localidad de PHP: el archivo es la página, pero el código incrustado se limita a la sintaxis
//! restringida del template y las variables son PARÁMETROS TIPADOS (un typo en `{{ var }}` o un
//! tipo equivocado es error de compilación, no un "" silencioso como en el motor runtime).
//!
//! Sintaxis del template (la de `std/template`, más la directiva de firma):
//!   - `{% params nombre: tipo, … %}` — obligatoria, primera directiva: la firma de la función.
//!   - `{{ expr }}` — interpola con autoescape HTML. `expr` es una EXPRESIÓN raylang arbitraria
//!     (se empalma verbatim): `{{ titulo }}`, `{{ p.nombre }}`, `{{ n + 1 }}`.
//!   - `{{& expr }}` — interpola cruda (sin escapar).
//!   - `{% if expr %} … {% elif expr %} … {% else %} … {% endif %}` — condición raylang verbatim.
//!   - `{% for x in expr %} … {% endfor %}` — cualquier iterable del `for` de raylang
//!     (arreglos, rangos `a..b`, `Map` con `(k, v)`, iteradores).
//!
//! `vistas/lista.ray.html` genera `vistas/lista.ray` con `pub fn render_lista(…) -> string`, que se
//! importa como cualquier módulo (`import vistas/lista;` → `lista.render_lista(…)`). El archivo
//! generado se commitea (inspeccionable, cero magia); el escape reusa el `escape_html` de
//! `std/template` (pub). Es un cliente del front-end (como `ray fmt`/`ray doc`): el `.ray` generado
//! lo valida el pipeline entero al compilarse; aquí solo se comprueba que **parsea** para dar el
//! error temprano contra el template.
//!
//! El motor runtime (`std/template`: `compile`/`render` con contexto `TVal`) sigue siendo la opción
//! para plantillas dinámicas (cargadas de disco/BD en caliente).

use std::path::{Path, PathBuf};

// Un token del template (espejo del tokenizador de std/template, en Rust).
enum Tok {
    Text(String),
    Var(String),
    Raw(String),
    Tag(String),
}

/// Compila `input` (`*.ray.html`) y escribe el módulo generado al lado (`*.ray`). Devuelve la ruta
/// generada. `Err` con el archivo y el motivo si el template está mal formado.
pub fn generate_file(input: &Path) -> Result<PathBuf, String> {
    let src = std::fs::read_to_string(input)
        .map_err(|e| format!("no se pudo leer '{}': {e}", input.display()))?;
    let name = fn_suffix_of(input)?;
    let code = generate(&src, &name).map_err(|e| format!("{}: {e}", input.display()))?;
    let out_path = output_path(input)?;
    std::fs::write(&out_path, &code)
        .map_err(|e| format!("no se pudo escribir '{}': {e}", out_path.display()))?;
    // Validación temprana: el generado debe parsear. Un error aquí es un error DEL TEMPLATE
    // (expresión empalmada mal formada); el archivo queda escrito para inspección.
    let tokens = crate::lexer::lex(&code)
        .map_err(|e| format!("{}: el código generado no lexea (línea {}): {e}", input.display(), e.line))?;
    crate::parser::parse(tokens)
        .map_err(|e| format!("{}: el código generado no parsea ({}:{}): {e}", input.display(), out_path.display(), e.line))?;
    Ok(out_path)
}

// `vistas/lista.ray.html` → `vistas/lista.ray`.
fn output_path(input: &Path) -> Result<PathBuf, String> {
    let s = input.to_string_lossy();
    let Some(base) = s.strip_suffix(".ray.html") else {
        return Err(format!("'{}' no termina en .ray.html", input.display()));
    };
    Ok(PathBuf::from(format!("{base}.ray")))
}

// El sufijo del nombre de la función generada: el *stem* del archivo, saneado a identificador
// (`lista-de-usuarios.ray.html` → `lista_de_usuarios` → `render_lista_de_usuarios`).
fn fn_suffix_of(input: &Path) -> Result<String, String> {
    let s = input
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".ray.html"))
        .ok_or_else(|| format!("'{}' no termina en .ray.html", input.display()))?;
    let name: String = s.chars().map(|c| if c == '-' { '_' } else { c }).collect();
    let valido = !name.is_empty()
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valido {
        return Err(format!("'{s}' no es un nombre de template válido (identificador: letras/dígitos/_)"));
    }
    Ok(name)
}

// Tokeniza el template: texto literal, `{{ expr }}`, `{{& expr }}`, `{% tag %}`.
fn tokenize(tpl: &str) -> Result<Vec<Tok>, String> {
    let cs: Vec<char> = tpl.chars().collect();
    let n = cs.len();
    let mut toks = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < n {
        if i + 1 < n && cs[i] == '{' && (cs[i + 1] == '{' || cs[i + 1] == '%') {
            if i > start {
                toks.push(Tok::Text(cs[start..i].iter().collect()));
            }
            let es_tag = cs[i + 1] == '%';
            let cierre = if es_tag { '%' } else { '}' };
            let ini = i + 2;
            let mut fin = None;
            let mut j = ini;
            while fin.is_none() && j + 1 < n {
                if cs[j] == cierre && cs[j + 1] == '}' {
                    fin = Some(j);
                } else {
                    j += 1;
                }
            }
            let Some(fin) = fin else {
                return Err(if es_tag { "'{%' sin cerrar".into() } else { "'{{' sin cerrar".into() });
            };
            let inner: String = cs[ini..fin].iter().collect();
            let inner = inner.trim().to_string();
            i = fin + 2;
            start = i;
            if es_tag {
                toks.push(Tok::Tag(inner));
            } else if let Some(resto) = inner.strip_prefix('&') {
                toks.push(Tok::Raw(resto.trim().to_string()));
            } else {
                toks.push(Tok::Var(inner));
            }
        } else {
            i += 1;
        }
    }
    if n > start {
        toks.push(Tok::Text(cs[start..n].iter().collect()));
    }
    Ok(toks)
}

// Escapa un texto literal para incrustarlo en un string de raylang: `\ " $` y los controles.
// El `$` SIEMPRE se escapa (`\$`): un `${` del HTML no debe volverse interpolación del generado.
fn lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

// Parte la lista de parámetros por comas de NIVEL 0 (los tipos anidan: `Map<K, V>`, `(A, B)`,
// `[T]`, `fn(A) -> R`). El `>` de `->` no cuenta como cierre de genérico.
fn split_params(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    let mut prev = ' ';
    for c in s.chars() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' if prev != '-' => depth -= 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(cur.trim().to_string());
                cur.clear();
                prev = c;
                continue;
            }
            _ => {}
        }
        cur.push(c);
        prev = c;
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_string());
    }
    parts
}

// Qué abre/cierra cada etiqueta (para validar el anidamiento y cuadrar las llaves).
enum Marco {
    If,
    For,
}

/// Genera el fuente raylang del template. `name` es el sufijo de la función (`render_<name>`).
fn generate(tpl: &str, name: &str) -> Result<String, String> {
    let toks = tokenize(tpl)?;
    let mut it = toks.into_iter().peekable();

    // La primera directiva debe ser `{% params … %}` (se admite texto en blanco antes).
    let params = loop {
        match it.peek() {
            Some(Tok::Text(t)) if t.trim().is_empty() => {
                it.next();
            }
            Some(Tok::Tag(t)) if t.starts_with("params") => {
                let Some(Tok::Tag(t)) = it.next() else { unreachable!() };
                break t["params".len()..].trim().to_string();
            }
            _ => {
                return Err("la primera directiva debe ser '{% params nombre: tipo, … %}' (la firma de la función)".into());
            }
        }
    };
    for p in split_params(&params) {
        let Some((nombre, tipo)) = p.split_once(':') else {
            return Err(format!("parámetro mal formado en params: '{p}' (se espera 'nombre: tipo')"));
        };
        let nombre = nombre.trim();
        if nombre.is_empty() || tipo.trim().is_empty() || !nombre.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("parámetro mal formado en params: '{p}'"));
        }
    }
    // Si el template continúa con un salto de línea inmediato tras `%}`, se recorta (estética del
    // HTML generado; el resto del espaciado se respeta tal cual).
    if let Some(Tok::Text(t)) = it.peek()
        && let Some(resto) = t.strip_prefix('\n')
    {
        let resto = resto.to_string();
        it.next();
        if !resto.is_empty() {
            // reinyectar el texto sin el primer salto
            return generate_body(name, &params, std::iter::once(Tok::Text(resto)).chain(it).collect());
        }
        return generate_body(name, &params, it.collect());
    }
    generate_body(name, &params, it.collect())
}

fn generate_body(name: &str, params: &str, toks: Vec<Tok>) -> Result<String, String> {
    let mut body = String::new();
    let mut depth = 1usize; // dentro de la función
    let mut stack: Vec<Marco> = Vec::new();
    let linea = |body: &mut String, depth: usize, s: &str| {
        for _ in 0..depth {
            body.push_str("    ");
        }
        body.push_str(s);
        body.push('\n');
    };

    for tok in toks {
        match tok {
            Tok::Text(t) => {
                if !t.is_empty() {
                    linea(&mut body, depth, &format!("out.push(\"{}\");", lit(&t)));
                }
            }
            Tok::Var(e) => {
                if e.is_empty() {
                    return Err("'{{ }}' vacío".into());
                }
                linea(&mut body, depth, &format!("out.push(escape_html(to_string({e})));"));
            }
            Tok::Raw(e) => {
                if e.is_empty() {
                    return Err("'{{& }}' vacío".into());
                }
                linea(&mut body, depth, &format!("out.push(to_string({e}));"));
            }
            Tok::Tag(t) => {
                let (kw, resto) = match t.split_once(char::is_whitespace) {
                    Some((k, r)) => (k, r.trim()),
                    None => (t.as_str(), ""),
                };
                match kw {
                    "if" => {
                        if resto.is_empty() {
                            return Err("'{% if %}' sin condición".into());
                        }
                        linea(&mut body, depth, &format!("if ({resto}) {{"));
                        depth += 1;
                        stack.push(Marco::If);
                    }
                    "elif" => {
                        if !matches!(stack.last(), Some(Marco::If)) {
                            return Err("'{% elif %}' fuera de un '{% if %}'".into());
                        }
                        if resto.is_empty() {
                            return Err("'{% elif %}' sin condición".into());
                        }
                        linea(&mut body, depth - 1, &format!("}} else if ({resto}) {{"));
                    }
                    "else" => {
                        if !matches!(stack.last(), Some(Marco::If)) {
                            return Err("'{% else %}' fuera de un '{% if %}'".into());
                        }
                        linea(&mut body, depth - 1, "} else {");
                    }
                    "endif" => {
                        if !matches!(stack.last(), Some(Marco::If)) {
                            return Err("'{% endif %}' sin '{% if %}' que cerrar".into());
                        }
                        stack.pop();
                        depth -= 1;
                        linea(&mut body, depth, "}");
                    }
                    "for" => {
                        // `for <patrón> in <expr>`: el patrón puede ser `x` o `(k, v)`.
                        let Some(pos) = resto.find(" in ") else {
                            return Err("'{% for %}' mal formado (se espera 'for x in expr')".into());
                        };
                        let patron = resto[..pos].trim();
                        let expr = resto[pos + 4..].trim();
                        if patron.is_empty() || expr.is_empty() {
                            return Err("'{% for %}' mal formado (se espera 'for x in expr')".into());
                        }
                        linea(&mut body, depth, &format!("for {patron} in {expr} {{"));
                        depth += 1;
                        stack.push(Marco::For);
                    }
                    "endfor" => {
                        if !matches!(stack.last(), Some(Marco::For)) {
                            return Err("'{% endfor %}' sin '{% for %}' que cerrar".into());
                        }
                        stack.pop();
                        depth -= 1;
                        linea(&mut body, depth, "}");
                    }
                    "params" => {
                        return Err("'{% params %}' repetido (solo puede ir una vez, al principio)".into());
                    }
                    otro => return Err(format!("etiqueta desconocida: '{otro}'")),
                }
            }
        }
    }
    if !stack.is_empty() {
        let falta = match stack.last() {
            Some(Marco::If) => "endif",
            _ => "endfor",
        };
        return Err(format!("falta un '{{% {falta} %}}' al final del template"));
    }

    Ok(format!(
        "// GENERADO por `ray templ` desde {name}.ray.html — NO editar a mano; regenera con\n\
         // `ray templ <ruta>`. El template es la fuente de verdad.\n\
         from std/template import escape_html;\n\
         \n\
         /// Renders the `{name}` template (generated; the `.ray.html` file is the source of truth).\n\
         pub fn render_{name}({params}) -> string {{\n\
         \x20   var out: [string] = [];\n\
         {body}\
         \x20   out.join(\"\")\n\
         }}\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genera_una_funcion_tipada() {
        let tpl = "{% params titulo: string, n: int %}\n<h1>{{ titulo }}</h1>{% if n > 0 %}<p>{{ n }}</p>{% endif %}";
        let code = generate(tpl, "vista").unwrap();
        assert!(code.contains("pub fn render_vista(titulo: string, n: int) -> string {"), "{code}");
        assert!(code.contains("out.push(escape_html(to_string(titulo)));"), "{code}");
        assert!(code.contains("if (n > 0) {"), "{code}");
        // Y el generado parsea como raylang válido.
        let tokens = crate::lexer::lex(&code).unwrap();
        assert!(crate::parser::parse(tokens).is_ok());
    }

    #[test]
    fn escapa_los_literales_del_html() {
        // Un `${` o una comilla del HTML no deben romper (ni interpolar) el string generado.
        let tpl = "{% params x: int %}precio: \"${simbolo}\" y \\ raro {{ x }}";
        let code = generate(tpl, "t").unwrap();
        assert!(code.contains("\\$"), "el $ va escapado:\n{code}");
        assert!(code.contains("\\\""), "las comillas van escapadas:\n{code}");
        let tokens = crate::lexer::lex(&code).unwrap();
        assert!(crate::parser::parse(tokens).is_ok());
    }

    #[test]
    fn errores_del_template() {
        assert!(generate("<html>", "t").unwrap_err().contains("params"));
        assert!(generate("{% params x: int %}{% if x %}", "t").unwrap_err().contains("endif"));
        assert!(generate("{% params x: int %}{% endfor %}", "t").unwrap_err().contains("endfor"));
        assert!(generate("{% params x: int %}{% bloque %}", "t").unwrap_err().contains("desconocida"));
        assert!(generate("{% params x %}hola", "t").unwrap_err().contains("mal formado"));
    }

    #[test]
    fn split_params_respeta_los_anidados() {
        let ps = split_params("m: Map<string, int>, xs: [string], f: fn(int) -> int");
        assert_eq!(ps, vec!["m: Map<string, int>", "xs: [string]", "f: fn(int) -> int"]);
    }
}
