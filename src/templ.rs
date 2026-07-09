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
//! **Posiciones y line map** (soporte del LSP): los errores del template llevan su LÍNEA, y
//! `generate_with_map` devuelve, junto al fuente generado, un mapa línea-generada → línea-del-
//! template. Con él, el LSP analiza el módulo generado y **traduce los errores de tipos de vuelta
//! al template** (el typo en `{{ titluo }}` se subraya en el `.ray.html`).
//!
//! El motor runtime (`std/template`: `compile`/`render` con contexto `TVal`) sigue siendo la opción
//! para plantillas dinámicas (cargadas de disco/BD en caliente).

use std::path::{Path, PathBuf};

/// Un error del template, con la línea (1-basada) del `.ray.html` donde ocurre.
#[derive(Debug)]
pub struct TplError {
    pub line: usize,
    pub msg: String,
}

// Un token del template (espejo del tokenizador de std/template, en Rust), con la línea del
// template donde empieza.
enum Tok {
    Text(String, usize),
    Var(String, usize),
    Raw(String, usize),
    Tag(String, usize),
}

impl Tok {
    fn line(&self) -> usize {
        match self {
            Tok::Text(_, l) | Tok::Var(_, l) | Tok::Raw(_, l) | Tok::Tag(_, l) => *l,
        }
    }
}

/// Compila `input` (`*.ray.html`) y escribe el módulo generado al lado (`*.ray`). Devuelve la ruta
/// generada. `Err` con el archivo, la línea y el motivo si el template está mal formado.
pub fn generate_file(input: &Path) -> Result<PathBuf, String> {
    let src = std::fs::read_to_string(input)
        .map_err(|e| format!("no se pudo leer '{}': {e}", input.display()))?;
    let name = fn_suffix_of(input)?;
    let (code, _map) = generate_with_map(&src, &name)
        .map_err(|e| format!("{}: línea {}: {}", input.display(), e.line, e.msg))?;
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

/// El sufijo del nombre de la función generada: el *stem* del archivo, saneado a identificador
/// (`lista-de-usuarios.ray.html` → `lista_de_usuarios` → `render_lista_de_usuarios`). Lo usa
/// también el LSP para nombrar la función al analizar un buffer `.ray.html`.
pub fn fn_suffix_of(input: &Path) -> Result<String, String> {
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

/// Los parámetros de la cabecera `{% params nombre: tipo, … %}` como pares `(nombre, tipo)`.
/// **Tolerante** a un template a medio escribir (no tokeniza entero, solo escanea la cabecera):
/// es para la completion del LSP, no para validar — con cabecera ausente o rota devuelve `[]`.
pub fn header_params(tpl: &str) -> Vec<(String, String)> {
    let Some(i) = tpl.find("{%") else { return vec![] };
    let rest = tpl[i + 2..].trim_start();
    let Some(body) = rest.strip_prefix("params") else { return vec![] };
    let Some(j) = body.find("%}") else { return vec![] };
    split_params(&body[..j])
        .into_iter()
        .filter_map(|p| p.split_once(':').map(|(n, t)| (n.trim().to_string(), t.trim().to_string())))
        .filter(|(n, t)| !n.is_empty() && !t.is_empty())
        .collect()
}

// Tokeniza el template: texto literal, `{{ expr }}`, `{{& expr }}`, `{% tag %}`. Cada token lleva
// la línea (1-basada) del template donde empieza.
fn tokenize(tpl: &str) -> Result<Vec<Tok>, TplError> {
    let cs: Vec<char> = tpl.chars().collect();
    let n = cs.len();
    let mut toks = Vec::new();
    let mut start = 0;
    let mut start_line = 1usize;
    let mut line = 1usize;
    let mut i = 0;
    while i < n {
        if i + 1 < n && cs[i] == '{' && (cs[i + 1] == '{' || cs[i + 1] == '%') {
            if i > start {
                toks.push(Tok::Text(cs[start..i].iter().collect(), start_line));
            }
            let tok_line = line;
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
                let que = if es_tag { "'{%' sin cerrar" } else { "'{{' sin cerrar" };
                return Err(TplError { line: tok_line, msg: que.into() });
            };
            let inner: String = cs[ini..fin].iter().collect();
            line += inner.matches('\n').count();
            let inner = inner.trim().to_string();
            i = fin + 2;
            start = i;
            start_line = line;
            if es_tag {
                toks.push(Tok::Tag(inner, tok_line));
            } else if let Some(resto) = inner.strip_prefix('&') {
                toks.push(Tok::Raw(resto.trim().to_string(), tok_line));
            } else {
                toks.push(Tok::Var(inner, tok_line));
            }
        } else {
            if cs[i] == '\n' {
                line += 1;
            }
            i += 1;
        }
    }
    if n > start {
        toks.push(Tok::Text(cs[start..n].iter().collect(), start_line));
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

/// Valida el argumento de un `{% import %}`: `ruta/al/modulo [as alias]` — segmentos que sean
/// identificadores separados por `/`. Evita empalmar texto arbitrario en el `import …;` generado.
fn valid_import(s: &str) -> bool {
    let (path, alias) = match s.split_once(" as ") {
        Some((p, a)) => (p.trim(), Some(a.trim())),
        None => (s, None),
    };
    let seg_ok = |x: &str| {
        !x.is_empty()
            && x.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && x.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    !path.is_empty() && path.split('/').all(seg_ok) && alias.map(seg_ok).unwrap_or(true)
}

// Qué abre/cierra cada etiqueta (para validar el anidamiento y cuadrar las llaves).
enum Marco {
    If,
    For,
}

/// Genera el fuente raylang del template junto con el **line map**: `map[i]` es la línea del
/// template (1-basada) de la que proviene la línea `i + 1` del generado. Con él, el LSP traduce
/// los diagnósticos del módulo generado de vuelta al `.ray.html`.
pub fn generate_with_map(tpl: &str, name: &str) -> Result<(String, Vec<usize>), TplError> {
    let toks = tokenize(tpl)?;
    let mut it = toks.into_iter().peekable();

    // La primera directiva debe ser `{% params … %}` (se admite texto en blanco antes).
    let (params, params_line) = loop {
        match it.peek() {
            Some(Tok::Text(t, _)) if t.trim().is_empty() => {
                it.next();
            }
            Some(Tok::Tag(t, _)) if t.starts_with("params") => {
                let Some(Tok::Tag(t, l)) = it.next() else { unreachable!() };
                break (t["params".len()..].trim().to_string(), l);
            }
            otro => {
                let line = otro.map(|t| t.line()).unwrap_or(1);
                return Err(TplError {
                    line,
                    msg: "la primera directiva debe ser '{% params nombre: tipo, … %}' (la firma de la función)".into(),
                });
            }
        }
    };
    for p in split_params(&params) {
        let Some((nombre, tipo)) = p.split_once(':') else {
            return Err(TplError { line: params_line, msg: format!("parámetro mal formado en params: '{p}' (se espera 'nombre: tipo')") });
        };
        let nombre = nombre.trim();
        if nombre.is_empty() || tipo.trim().is_empty() || !nombre.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(TplError { line: params_line, msg: format!("parámetro mal formado en params: '{p}'") });
        }
    }
    // Si el template continúa con un salto de línea inmediato tras `%}`, se recorta (estética del
    // HTML generado; el resto del espaciado se respeta tal cual).
    let mut toks: Vec<Tok> = Vec::new();
    if let Some(Tok::Text(t, l)) = it.peek()
        && let Some(resto) = t.strip_prefix('\n')
    {
        let (resto, l) = (resto.to_string(), *l);
        it.next();
        if !resto.is_empty() {
            toks.push(Tok::Text(resto, l + 1));
        }
    }
    toks.extend(it);
    generate_body(name, &params, params_line, toks)
}

fn generate_body(
    name: &str,
    params: &str,
    params_line: usize,
    toks: Vec<Tok>,
) -> Result<(String, Vec<usize>), TplError> {
    // Cada línea del cuerpo con la línea del template de la que proviene.
    let mut body: Vec<(usize, String)> = Vec::new();
    let mut imports: Vec<(String, usize)> = Vec::new(); // `{% import %}` hoisteados a la cabecera
    let mut depth = 1usize; // dentro de la función
    let mut stack: Vec<Marco> = Vec::new();
    let mut last_line = params_line;
    let linea = |body: &mut Vec<(usize, String)>, depth: usize, tpl_line: usize, s: String| {
        body.push((tpl_line, format!("{}{s}", "    ".repeat(depth))));
    };

    for tok in toks {
        last_line = tok.line();
        match tok {
            Tok::Text(t, l) => {
                if !t.is_empty() {
                    linea(&mut body, depth, l, format!("out.push(\"{}\");", lit(&t)));
                }
            }
            Tok::Var(e, l) => {
                if e.is_empty() {
                    return Err(TplError { line: l, msg: "'{{ }}' vacío".into() });
                }
                linea(&mut body, depth, l, format!("out.push(escape_html(to_string({e})));"));
            }
            Tok::Raw(e, l) => {
                if e.is_empty() {
                    return Err(TplError { line: l, msg: "'{{& }}' vacío".into() });
                }
                linea(&mut body, depth, l, format!("out.push(to_string({e}));"));
            }
            // (Los casos `import`/`include` de composición van en el match de etiquetas, abajo.)
            Tok::Tag(t, l) => {
                let (kw, resto) = match t.split_once(char::is_whitespace) {
                    Some((k, r)) => (k, r.trim()),
                    None => (t.as_str(), ""),
                };
                match kw {
                    "if" => {
                        if resto.is_empty() {
                            return Err(TplError { line: l, msg: "'{% if %}' sin condición".into() });
                        }
                        linea(&mut body, depth, l, format!("if ({resto}) {{"));
                        depth += 1;
                        stack.push(Marco::If);
                    }
                    "elif" => {
                        if !matches!(stack.last(), Some(Marco::If)) {
                            return Err(TplError { line: l, msg: "'{% elif %}' fuera de un '{% if %}'".into() });
                        }
                        if resto.is_empty() {
                            return Err(TplError { line: l, msg: "'{% elif %}' sin condición".into() });
                        }
                        linea(&mut body, depth - 1, l, format!("}} else if ({resto}) {{"));
                    }
                    "else" => {
                        if !matches!(stack.last(), Some(Marco::If)) {
                            return Err(TplError { line: l, msg: "'{% else %}' fuera de un '{% if %}'".into() });
                        }
                        linea(&mut body, depth - 1, l, "} else {".to_string());
                    }
                    "endif" => {
                        if !matches!(stack.last(), Some(Marco::If)) {
                            return Err(TplError { line: l, msg: "'{% endif %}' sin '{% if %}' que cerrar".into() });
                        }
                        stack.pop();
                        depth -= 1;
                        linea(&mut body, depth, l, "}".to_string());
                    }
                    "for" => {
                        // `for <patrón> in <expr>`: el patrón puede ser `x` o `(k, v)`.
                        let Some(pos) = resto.find(" in ") else {
                            return Err(TplError { line: l, msg: "'{% for %}' mal formado (se espera 'for x in expr')".into() });
                        };
                        let patron = resto[..pos].trim();
                        let expr = resto[pos + 4..].trim();
                        if patron.is_empty() || expr.is_empty() {
                            return Err(TplError { line: l, msg: "'{% for %}' mal formado (se espera 'for x in expr')".into() });
                        }
                        linea(&mut body, depth, l, format!("for {patron} in {expr} {{"));
                        depth += 1;
                        stack.push(Marco::For);
                    }
                    "endfor" => {
                        if !matches!(stack.last(), Some(Marco::For)) {
                            return Err(TplError { line: l, msg: "'{% endfor %}' sin '{% for %}' que cerrar".into() });
                        }
                        stack.pop();
                        depth -= 1;
                        linea(&mut body, depth, l, "}".to_string());
                    }
                    // Composición de templates: `{% import vistas/tarjeta [as t] %}` trae otro
                    // módulo (otro template compilado, o cualquier módulo del proyecto) al ámbito
                    // del generado. Se HOISTEA a la cabecera (los imports van al frente del módulo),
                    // esté donde esté en el template.
                    "import" => {
                        if !valid_import(resto) {
                            return Err(TplError { line: l, msg: format!("'{{% import %}}' mal formado: '{resto}' (se espera 'ruta/al/modulo [as alias]')") });
                        }
                        imports.push((resto.to_string(), l));
                    }
                    // `{% include expr %}`: empalma el string de `expr` SIN escapar — es HTML ya
                    // renderizado (normalmente el `render_<x>(…)` de otro template). Equivale a
                    // `{{& expr }}`, con la intención declarada.
                    "include" => {
                        if resto.is_empty() {
                            return Err(TplError { line: l, msg: "'{% include %}' sin expresión (se espera 'include modulo.render_x(args)')".into() });
                        }
                        linea(&mut body, depth, l, format!("out.push(to_string({resto}));"));
                    }
                    "params" => {
                        return Err(TplError { line: l, msg: "'{% params %}' repetido (solo puede ir una vez, al principio)".into() });
                    }
                    otro => {
                        return Err(TplError { line: l, msg: format!("etiqueta desconocida: '{otro}'") });
                    }
                }
            }
        }
    }
    if !stack.is_empty() {
        let falta = match stack.last() {
            Some(Marco::If) => "endif",
            _ => "endfor",
        };
        return Err(TplError { line: last_line, msg: format!("falta un '{{% {falta} %}}' al final del template") });
    }

    // Ensamblado + line map. La cabecera son 3 líneas fijas + un `import` por cada `{% import %}`
    // (mapean a SU línea del template) + 4 fijas más (todas las fijas mapean a la línea de
    // `params`, donde vive la firma); el cierre, a la última línea del template.
    let mut header = format!(
        "// GENERADO por `ray templ` desde {name}.ray.html — NO editar a mano; regenera con\n\
         // `ray templ <ruta>`. El template es la fuente de verdad.\n\
         from std/template import escape_html;\n"
    );
    let mut map: Vec<usize> = vec![params_line; 3];
    for (p, l) in &imports {
        header.push_str(&format!("import {p};\n"));
        map.push(*l);
    }
    header.push_str(&format!(
        "\n\
         /// Renders the `{name}` template (generated; the `.ray.html` file is the source of truth).\n\
         pub fn render_{name}({params}) -> string {{\n\
         \x20   var out: [string] = [];\n"
    ));
    map.extend([params_line; 4]);
    let mut code = header;
    for (l, linea_src) in &body {
        map.push(*l);
        code.push_str(linea_src);
        code.push('\n');
    }
    code.push_str("    out.join(\"\")\n}\n");
    map.push(last_line);
    map.push(last_line);
    Ok((code, map))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate(tpl: &str, name: &str) -> Result<String, TplError> {
        generate_with_map(tpl, name).map(|(c, _)| c)
    }

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
    fn errores_del_template_con_linea() {
        assert!(generate("<html>", "t").unwrap_err().msg.contains("params"));
        let e = generate("{% params x: int %}\n\n{% if x %}", "t").unwrap_err();
        assert!(e.msg.contains("endif"));
        assert_eq!(e.line, 3, "el error señala la línea del if sin cerrar");
        assert!(generate("{% params x: int %}{% endfor %}", "t").unwrap_err().msg.contains("endfor"));
        assert!(generate("{% params x: int %}{% bloque %}", "t").unwrap_err().msg.contains("desconocida"));
        assert!(generate("{% params x %}hola", "t").unwrap_err().msg.contains("mal formado"));
    }

    #[test]
    fn split_params_respeta_los_anidados() {
        let ps = split_params("m: Map<string, int>, xs: [string], f: fn(int) -> int");
        assert_eq!(ps, vec!["m: Map<string, int>", "xs: [string]", "f: fn(int) -> int"]);
    }

    #[test]
    fn import_e_include_componen_templates() {
        // `{% import %}` se hoistea a la cabecera (con su línea en el map) y `{% include %}`
        // empalma sin escapar (HTML ya renderizado por otro template).
        let tpl = "{% params p: string %}\n{% import vistas/tarjeta %}\n{% import util/fmt as f %}\n<div>{% include tarjeta.render_tarjeta(p) %}</div>\n";
        let (code, map) = generate_with_map(tpl, "pagina").unwrap();
        assert!(code.contains("import vistas/tarjeta;\n"), "{code}");
        assert!(code.contains("import util/fmt as f;\n"), "{code}");
        assert!(code.contains("out.push(to_string(tarjeta.render_tarjeta(p)));"), "{code}");
        // Los imports van ANTES de la función y su línea del map es la del template.
        let lines: Vec<&str> = code.lines().collect();
        let (i, _) = lines.iter().enumerate().find(|(_, l)| l.contains("import vistas/tarjeta")).unwrap();
        assert!(i < lines.iter().position(|l| l.contains("pub fn")).unwrap());
        assert_eq!(map[i], 2, "{code}");
        assert_eq!(lines.len(), map.len());
        let tokens = crate::lexer::lex(&code).unwrap();
        assert!(crate::parser::parse(tokens).is_ok());
        // Errores: import mal formado (no se empalma texto arbitrario) e include vacío.
        assert!(generate("{% params x: int %}{% import ../fuera %}", "t").unwrap_err().msg.contains("mal formado"));
        assert!(generate("{% params x: int %}{% import a; drop %}", "t").unwrap_err().msg.contains("mal formado"));
        assert!(generate("{% params x: int %}{% include %}", "t").unwrap_err().msg.contains("include"));
    }

    #[test]
    fn el_line_map_traduce_al_template() {
        let tpl = "{% params t: string %}\n<h1>{{ t }}</h1>\n{% if t != \"\" %}\n<p>{{ t }}</p>\n{% endif %}\n";
        let (code, map) = generate_with_map(tpl, "v").unwrap();
        let lines: Vec<&str> = code.lines().collect();
        assert_eq!(lines.len(), map.len(), "una entrada del mapa por línea generada");
        // La línea generada del `if` mapea a la línea 3 del template.
        let (i, _) = lines.iter().enumerate().find(|(_, l)| l.contains("if (t !=")).unwrap();
        assert_eq!(map[i], 3, "{code}");
        // Y la del `<p>{{ t }}</p>` a la 4.
        let (k, _) = lines.iter().enumerate().skip(i).find(|(_, l)| l.contains("escape_html")).unwrap();
        assert_eq!(map[k], 4, "{code}");
    }
}
