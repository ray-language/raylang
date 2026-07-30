//! Templates compilados (M55) — un archivo `.ray.html` se compila a una FUNCIÓN raylang tipada, en
//! la línea de `templ` (Go) o `askama` (Rust). Vía normal (M102): el LOADER lo compila EN MEMORIA
//! al resolver su import; `ray build --templates-only` (era el subcomando `ray templ` hasta M99)
//! materializa el generado para inspección. Es la versión "limpia" de la
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
//! `views/list.ray.html` ES el módulo `views/list` con `pub fn render(…) -> string` (M103: el
//! nombre es fijo — el módulo ya namespacea), que se importa como cualquier módulo
//! (`import views/list;` → `list.render(…)`). El escape reusa el `escape_html` de
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
#[derive(Clone)]
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

    // Reubica el token en otra línea (los tokens del layout se atribuyen al `{% extends %}` del hijo).
    fn at_line(self, l: usize) -> Tok {
        match self {
            Tok::Text(s, _) => Tok::Text(s, l),
            Tok::Var(s, _) => Tok::Var(s, l),
            Tok::Raw(s, _) => Tok::Raw(s, l),
            Tok::Tag(s, _) => Tok::Tag(s, l),
        }
    }
}

/// Compila `input` (`*.ray.html`) **en memoria** y devuelve el fuente raylang del módulo generado,
/// sin tocar el disco. Es la vía del loader (M102): al resolver un import a un `.ray.html`, el
/// template es la única fuente de verdad — no hay `.ray` generado en el proyecto.
pub fn generate_module_source(input: &Path) -> Result<String, String> {
    generate_module_with_map(input).map(|(code, _, _)| code)
}

/// Como `generate_module_source`, devolviendo además el fuente del TEMPLATE y su line map
/// (línea-generada → línea-del-template, 1-basadas): lo que el loader guarda para traducir los
/// diagnósticos posteriores del módulo generado de vuelta al `.ray.html` (M102-A2).
pub fn generate_module_with_map(input: &Path) -> Result<(String, String, Vec<usize>), String> {
    let src = std::fs::read_to_string(input)
        .map_err(|e| format!("could not read '{}': {e}", input.display()))?;
    let name = fn_suffix_of(input)?;
    let (code, map) = generate_with_map_at(&src, &name, input.parent())
        .map_err(|e| format!("{}: line {}: {}", input.display(), e.line, e.msg))?;
    Ok((code, src, map))
}

/// Compila `input` (`*.ray.html`) y escribe el módulo generado al lado (`*.ray`). Devuelve la ruta
/// generada. `Err` con el archivo, la línea y el motivo si el template está mal formado. Es la vía
/// **explícita** (`ray build --templates-only`): materializa el generado para inspección; la vía
/// normal (run/build/test) compila en memoria vía `generate_module_source`.
pub fn generate_file(input: &Path) -> Result<PathBuf, String> {
    let code = generate_module_source(input)?;
    let out_path = output_path(input)?;
    std::fs::write(&out_path, &code)
        .map_err(|e| format!("could not write '{}': {e}", out_path.display()))?;
    // Validación temprana: el generado debe parsear. Un error aquí es un error DEL TEMPLATE
    // (expresión empalmada mal formada); el archivo queda escrito para inspección.
    let tokens = crate::lexer::lex(&code)
        .map_err(|e| format!("{}: the generated code does not lex (line {}): {e}", input.display(), e.line))?;
    crate::parser::parse(tokens)
        .map_err(|e| format!("{}: the generated code does not parse ({}:{}): {e}", input.display(), out_path.display(), e.line))?;
    Ok(out_path)
}

// `views/list.ray.html` → `views/list.ray`.
fn output_path(input: &Path) -> Result<PathBuf, String> {
    let s = input.to_string_lossy();
    let Some(base) = s.strip_suffix(".ray.html") else {
        return Err(format!("'{}' does not end in .ray.html", input.display()));
    };
    Ok(PathBuf::from(format!("{base}.ray")))
}

/// El **nombre del template**: el *stem* del archivo, saneado y validado como identificador
/// (`lista-de-usuarios.ray.html` → `lista_de_usuarios`). Desde M103 la función generada se llama
/// siempre `render` (el módulo namespacea); el nombre queda para el doc comment del generado y
/// para validar que el archivo puede ser un módulo.
pub fn fn_suffix_of(input: &Path) -> Result<String, String> {
    let s = input
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".ray.html"))
        .ok_or_else(|| format!("'{}' does not end in .ray.html", input.display()))?;
    let name: String = s.chars().map(|c| if c == '-' { '_' } else { c }).collect();
    let valid = !name.is_empty()
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return Err(format!("'{s}' is not a valid template name (identifier: letters/digits/_)"));
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
            let is_tag = cs[i + 1] == '%';
            let close = if is_tag { '%' } else { '}' };
            let ini = i + 2;
            let mut fin = None;
            let mut j = ini;
            while fin.is_none() && j + 1 < n {
                if cs[j] == close && cs[j + 1] == '}' {
                    fin = Some(j);
                } else {
                    j += 1;
                }
            }
            let Some(fin) = fin else {
                let what = if is_tag { "'{%' without close" } else { "'{{' without close" };
                return Err(TplError { line: tok_line, msg: what.into() });
            };
            let inner: String = cs[ini..fin].iter().collect();
            line += inner.matches('\n').count();
            let inner = inner.trim().to_string();
            i = fin + 2;
            start = i;
            start_line = line;
            if is_tag {
                toks.push(Tok::Tag(inner, tok_line));
            } else if let Some(rest) = inner.strip_prefix('&') {
                toks.push(Tok::Raw(rest.trim().to_string(), tok_line));
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

/// Formatea un template `.ray.html` (M55): cada etiqueta **`{% %}` en su propia línea**, con
/// indentación por la profundidad de bloques del template (`for`/`if`/`block` sangran su
/// contenido; `elif`/`else` al nivel del abridor); las interpolaciones `{{ }}` quedan **inline**
/// con su HTML; el espaciado de los delimitadores se normaliza (`{%for x%}` → `{% for x %}`,
/// `{{titulo}}` → `{{ titulo }}`) sin tocar el interior de la expresión (un string literal con
/// espacios se respeta). Cada línea se re-indenta al nivel de bloque del template (no se
/// re-indenta por etiquetas HTML: diferido); las líneas en blanco se conservan. `None` si el
/// template no tokeniza (no se formatea un buffer roto). El whitespace ENTRE nodos cambia — en
/// HTML es inocuo (y es el punto de un formateador).
pub fn format_template(tpl: &str, unit: &str) -> Option<String> {
    let toks = tokenize(tpl).ok()?;
    let mut out = String::new();
    let mut depth = 0usize;
    let mut buf = String::new(); // la línea de texto/interpolaciones en curso
    // Tras emitir una línea (de texto o de etiqueta), el siguiente `\n` de la fuente es su
    // TERMINADOR (se consume en silencio); un `\n` extra sí es una línea en blanco a conservar.
    let mut pending_terminator = false;

    fn emit(out: &mut String, depth: usize, unit: &str, s: &str) {
        for _ in 0..depth {
            out.push_str(unit);
        }
        out.push_str(s);
        out.push('\n');
    }

    for tok in toks {
        match tok {
            Tok::Text(t, _) => {
                for c in t.chars() {
                    if c != '\n' {
                        buf.push(c);
                        continue;
                    }
                    if !buf.trim().is_empty() {
                        emit(&mut out, depth, unit, buf.trim());
                        pending_terminator = false;
                    } else if pending_terminator {
                        pending_terminator = false; // el fin de línea de lo ya emitido
                    } else {
                        out.push('\n'); // línea en blanco real: se conserva
                    }
                    buf.clear();
                }
            }
            Tok::Var(e, _) => buf.push_str(&format!("{{{{ {e} }}}}")),
            Tok::Raw(e, _) => buf.push_str(&format!("{{{{& {e} }}}}")),
            Tok::Tag(t, _) => {
                // El texto pendiente de la línea va antes (si es solo espacio, se descarta).
                if !buf.trim().is_empty() {
                    emit(&mut out, depth, unit, buf.trim());
                }
                buf.clear();
                let kw = t.split_whitespace().next().unwrap_or("");
                let at = match kw {
                    // El close y los intermedios se alinean con su abridor.
                    "endfor" | "endif" | "endblock" => {
                        depth = depth.saturating_sub(1);
                        depth
                    }
                    "elif" | "else" => depth.saturating_sub(1),
                    _ => depth,
                };
                emit(&mut out, at, unit, &format!("{{% {t} %}}"));
                if matches!(kw, "for" | "if" | "block") {
                    depth += 1;
                }
                pending_terminator = true;
            }
        }
    }
    if !buf.trim().is_empty() {
        emit(&mut out, depth, unit, buf.trim());
    }
    Some(out)
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

/// ¿El argumento de un `{% include %}` es una **referencia a otro template** `ruta(args)`?
/// La ruta son segmentos identificador separados por `/` (sin puntos: `m.f(x)` es una expresión
/// ordinaria y se empalma cruda). Devuelve `(ruta, args)` con `args` = el interior de los
/// paréntesis EXTERIORES (`a/b(f(x), y)` → `("a/b", "f(x), y")`).
pub fn template_ref(s: &str) -> Option<(&str, &str)> {
    let s = s.trim();
    let opens = s.find('(')?;
    if !s.ends_with(')') {
        return None;
    }
    let path = s[..opens].trim_end();
    let seg_ok = |x: &str| {
        !x.is_empty()
            && x.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && x.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    if path.is_empty() || !path.split('/').all(seg_ok) {
        return None;
    }
    Some((path, s[opens + 1..s.len() - 1].trim()))
}

// Qué opens/cierra cada etiqueta (para validar el anidamiento y cuadrar las llaves).
enum Marco {
    If,
    For,
}

/// Genera el fuente raylang del template junto con el **line map**: `map[i]` es la línea del
/// template (1-basada) de la que proviene la línea `i + 1` del generado. Con él, el LSP traduce
/// los diagnósticos del módulo generado de vuelta al `.ray.html`. Sin directorio: un
/// `{% extends %}` (que necesita leer el layout del disco) da error.
pub fn generate_with_map(tpl: &str, name: &str) -> Result<(String, Vec<usize>), TplError> {
    generate_with_map_at(tpl, name, None)
}

/// Como `generate_with_map`, con el **directorio del template** para resolver `{% extends %}`
/// (el layout se lee de `<dir>/<ruta>.ray.html`).
pub fn generate_with_map_at(tpl: &str, name: &str, dir: Option<&Path>) -> Result<(String, Vec<usize>), TplError> {
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
            other => {
                let line = other.map(|t| t.line()).unwrap_or(1);
                return Err(TplError {
                    line,
                    msg: "the first directive must be '{% params name: type, … %}' (the function signature)".into(),
                });
            }
        }
    };
    for p in split_params(&params) {
        let Some((name, ty)) = p.split_once(':') else {
            return Err(TplError { line: params_line, msg: format!("malformed parameter in params: '{p}' (expected 'name: type')") });
        };
        let name = name.trim();
        if name.is_empty() || ty.trim().is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(TplError { line: params_line, msg: format!("malformed parameter in params: '{p}'") });
        }
    }
    // Si el template continúa con un salto de línea inmediato tras `%}`, se recorta (estética del
    // HTML generado; el resto del espaciado se respeta tal cual).
    let mut toks: Vec<Tok> = Vec::new();
    if let Some(Tok::Text(t, l)) = it.peek()
        && let Some(rest) = t.strip_prefix('\n')
    {
        let (rest, l) = (rest.to_string(), *l);
        it.next();
        if !rest.is_empty() {
            toks.push(Tok::Text(rest, l + 1));
        }
    }
    toks.extend(it);
    // Herencia de layout: fusiona `{% extends %}` + `{% block %}`s antes de generar (o, sin
    // extends, hace transparentes los marcadores de bloque: renderizan su contenido por defecto).
    let toks = resolve_extends(toks, dir)?;
    generate_body(name, &params, params_line, toks)
}

/// Herencia de layout (estilo Jinja, resuelta EN COMPILACIÓN). El hijo declara
/// `{% extends ruta %}` como primera etiqueta tras `{% params %}` y solo aporta
/// `{% block nombre %}…{% endblock %}` (más `{% import %}`s) — el layout es otro template cuyo
/// `{% block nombre %}defecto{% endblock %}` marca los huecos. La fusión: se tokeniza el layout
/// (su `{% params %}` se descarta: la firma manda la del HIJO — las variables que el layout use
/// deben estar entre los params del hijo, y el checker lo exige), cada bloque se sustituye por el
/// del hijo (o queda su defecto) y el stream fusionado se genera normal. Las líneas de los tokens
/// del layout se atribuyen al `{% extends %}` del hijo (el line map apunta al archivo del hijo;
/// degradación honesta) — las de los bloques del hijo son suyas y mapean exactas. Sin
/// `{% extends %}`, un `{% block %}` es transparente: renderiza su defecto en el sitio (así un
/// layout también compila standalone).
fn resolve_extends(toks: Vec<Tok>, dir: Option<&Path>) -> Result<Vec<Tok>, TplError> {
    let ident_ok = |s: &str| {
        !s.is_empty()
            && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    let kw_of = |t: &str| -> (String, String) {
        match t.split_once(char::is_whitespace) {
            Some((k, r)) => (k.to_string(), r.trim().to_string()),
            None => (t.to_string(), String::new()),
        }
    };
    // ¿Modo herencia? La primera etiqueta debe ser `{% extends %}` (como en Jinja).
    let inherits = toks.iter()
        .find_map(|t| match t {
            Tok::Tag(s, _) => Some(s.starts_with("extends")),
            Tok::Text(s, _) if s.trim().is_empty() => None,
            _ => Some(false),
        })
        .unwrap_or(false);
    if !inherits {
        // Sin herencia: quitar los marcadores de bloque (validando el anidamiento); el contenido
        // por defecto queda en su sitio. Un `{% extends %}` tardío es error (debe ir primero).
        let mut out = Vec::new();
        let mut open: Option<usize> = None;
        for tok in toks {
            if let Tok::Tag(t, l) = &tok {
                let (kw, rest) = kw_of(t);
                match kw.as_str() {
                    "extends" => return Err(TplError { line: *l, msg: "'{% extends %}' must be the first tag after '{% params %}'".into() }),
                    "block" => {
                        if open.is_some() {
                            return Err(TplError { line: *l, msg: "'{% block %}' nested".into() });
                        }
                        if !ident_ok(&rest) {
                            return Err(TplError { line: *l, msg: format!("malformed '{{% block %}}': '{rest}' (a name is expected)") });
                        }
                        open = Some(*l);
                        continue;
                    }
                    "endblock" => {
                        if open.is_none() {
                            return Err(TplError { line: *l, msg: "'{% endblock %}' without a '{% block %}' to close".into() });
                        }
                        open = None;
                        continue;
                    }
                    _ => {}
                }
            }
            out.push(tok);
        }
        if let Some(l) = open {
            return Err(TplError { line: l, msg: "'{% block %}' without '{% endblock %}'".into() });
        }
        return Ok(out);
    }

    // Herencia: recoger del hijo el extends, sus imports y sus bloques (nada más fuera de bloques).
    let mut layout_ref: Option<(String, usize)> = None;
    let mut imports: Vec<Tok> = Vec::new();
    let mut blocks: Vec<(String, usize, Vec<Tok>)> = Vec::new();
    let mut cur: Option<(String, usize)> = None;
    for tok in toks {
        if let Tok::Tag(t, l) = &tok {
            let (kw, rest) = kw_of(t);
            match kw.as_str() {
                "extends" if cur.is_none() => {
                    if layout_ref.is_some() {
                        return Err(TplError { line: *l, msg: "'{% extends %}' repetido".into() });
                    }
                    if !valid_import(&rest) || rest.contains(" as ") {
                        return Err(TplError { line: *l, msg: format!("malformed '{{% extends %}}': '{rest}' (expected 'path/to/layout')") });
                    }
                    layout_ref = Some((rest, *l));
                    continue;
                }
                "import" if cur.is_none() => {
                    imports.push(tok.clone());
                    continue;
                }
                "block" => {
                    if cur.is_some() {
                        return Err(TplError { line: *l, msg: "'{% block %}' nested".into() });
                    }
                    if !ident_ok(&rest) {
                        return Err(TplError { line: *l, msg: format!("malformed '{{% block %}}': '{rest}' (a name is expected)") });
                    }
                    if blocks.iter().any(|(n, _, _)| *n == rest) {
                        return Err(TplError { line: *l, msg: format!("'{{% block {rest} %}}' repetido") });
                    }
                    cur = Some((rest.clone(), *l));
                    blocks.push((rest, *l, Vec::new()));
                    continue;
                }
                "endblock" => {
                    if cur.is_none() {
                        return Err(TplError { line: *l, msg: "'{% endblock %}' without a '{% block %}' to close".into() });
                    }
                    cur = None;
                    continue;
                }
                _ => {}
            }
        }
        match (&cur, &tok) {
            (Some(_), _) => blocks.last_mut().expect("open block").2.push(tok),
            (None, Tok::Text(s, _)) if s.trim().is_empty() => {}
            (None, t) => {
                return Err(TplError { line: t.line(), msg: "a template with '{% extends %}' can only have '{% block %}'s (and '{% import %}'s) outside the blocks".into() });
            }
        }
    }
    if let Some((n, l)) = cur {
        return Err(TplError { line: l, msg: format!("'{{% block {n} %}}' without '{{% endblock %}}'") });
    }
    let (lpath, eline) = layout_ref.expect("inherits");
    let Some(dir) = dir else {
        return Err(TplError { line: eline, msg: "'{% extends %}' requires generating from a file (the layout path is resolved from the project root)".into() });
    };
    let file = resolve_layout_path(&lpath, dir);
    let lsrc = std::fs::read_to_string(&file)
        .map_err(|e| TplError { line: eline, msg: format!("could not read layout '{}': {e}", file.display()) })?;
    let ltoks = tokenize(&lsrc)
        .map_err(|e| TplError { line: eline, msg: format!("in layout '{}' (line {}): {}", file.display(), e.line, e.msg) })?;
    // Saltar el `{% params %}` del layout (+ su salto de cortesía): la firma la pone el hijo.
    let mut it = ltoks.into_iter().peekable();
    loop {
        match it.peek() {
            Some(Tok::Text(t, _)) if t.trim().is_empty() => { it.next(); }
            Some(Tok::Tag(t, _)) if t.starts_with("params") => { it.next(); break; }
            _ => break, // un layout sin params también vale como destino
        }
    }
    if let Some(Tok::Text(t, l)) = it.peek()
        && let Some(rest) = t.strip_prefix('\n')
    {
        let (rest, l) = (rest.to_string(), *l);
        it.next();
        if !rest.is_empty() {
            // Se reintroduce sin el salto (la línea real es la siguiente, pero todo el layout se
            // atribuye al extends de todos modos).
            let tok = Tok::Text(rest, l + 1);
            return merge_layout(imports, blocks, std::iter::once(tok).chain(it).collect(), &lpath, eline);
        }
    }
    merge_layout(imports, blocks, it.collect(), &lpath, eline)
}

// La ruta del layout se resuelve COMO LOS IMPORTS/INCLUDES: desde la **raíz del proyecto**
// (el directorio con `ray.toml` más cercano por encima del template) — una sola convención
// de rutas en los templates. Fallback: relativa al directorio del template (proyectos sin
// manifiesto, o un layout hermano).
fn resolve_layout_path(lpath: &str, dir: &Path) -> PathBuf {
    let from_root = project_root_of(dir).map(|r| r.join(format!("{lpath}.ray.html")));
    match from_root {
        Some(p) if p.is_file() => p,
        _ => dir.join(format!("{lpath}.ray.html")),
    }
}

/// The layout template this template inherits from (`{% extends path %}`), resolved like at
/// generation time (project root, falling back to the template's directory). `None` if the
/// template does not inherit or does not tokenize (generation will report that). Used by the
/// stale-check of the auto-regeneration: `{% extends %}` fuses the layout at COMPILE time, so a
/// child's generated `.ray` also goes stale when the layout changes.
pub fn extends_target(tpl: &str, dir: &Path) -> Option<PathBuf> {
    let toks = tokenize(tpl).ok()?;
    for t in &toks {
        if let Tok::Tag(s, _) = t {
            let mut it = s.split_whitespace();
            let kw = it.next()?;
            if kw == "params" {
                continue; // la firma va primero; el extends (si lo hay) es la siguiente etiqueta
            }
            if kw == "extends" {
                let lpath = it.next()?;
                return Some(resolve_layout_path(lpath, dir));
            }
            return None; // cualquier otra etiqueta primero → no inherits (extends debe ir primero)
        }
    }
    None
}

// La raíz del proyecto: el directorio con `ray.toml` más cercano por encima de `dir` (inclusive).
fn project_root_of(dir: &Path) -> Option<PathBuf> {
    let mut d = dir.to_path_buf();
    loop {
        if d.join("ray.toml").is_file() {
            return Some(d);
        }
        if !d.pop() {
            return None;
        }
    }
}

// Sustituye cada `{% block %}` del layout por el bloque del hijo (o deja su defecto) y atribuye
// las líneas del layout al `{% extends %}` del hijo. Un bloque del hijo que el layout no declara
// es error (typo del nombre); la herencia encadenada (layout con extends) queda diferida.
fn merge_layout(
    imports: Vec<Tok>,
    blocks: Vec<(String, usize, Vec<Tok>)>,
    ltoks: Vec<Tok>,
    lpath: &str,
    eline: usize,
) -> Result<Vec<Tok>, TplError> {
    let mut out = imports; // los del hijo (se hoistean en la cabecera igualmente)
    let mut used: Vec<&str> = Vec::new();
    let mut in_block = false;
    let mut skip_default = false;
    for tok in ltoks {
        if let Tok::Tag(t, _) = &tok {
            let (kw, rest) = match t.split_once(char::is_whitespace) {
                Some((k, r)) => (k, r.trim()),
                None => (t.as_str(), ""),
            };
            match kw {
                "extends" => {
                    return Err(TplError { line: eline, msg: format!("the layout '{lpath}' also uses '{{% extends %}}' (chained inheritance: deferred)") });
                }
                "block" => {
                    if in_block {
                        return Err(TplError { line: eline, msg: format!("in layout '{lpath}': nested '{{% block %}}'") });
                    }
                    in_block = true;
                    if let Some((n, _, body)) = blocks.iter().find(|(n, _, _)| n == rest) {
                        out.extend(body.iter().cloned()); // líneas del HIJO: mapean exactas
                        used.push(n);
                        skip_default = true;
                    } else {
                        skip_default = false; // queda el contenido por defecto del layout
                    }
                    continue;
                }
                "endblock" => {
                    in_block = false;
                    skip_default = false;
                    continue;
                }
                _ => {}
            }
        }
        if skip_default {
            continue;
        }
        out.push(tok.at_line(eline));
    }
    for (n, l, _) in &blocks {
        if !used.contains(&n.as_str()) {
            return Err(TplError { line: *l, msg: format!("the layout '{lpath}' does not declare a '{{% block {n} %}}'") });
        }
    }
    Ok(out)
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
    let emit_line = |body: &mut Vec<(usize, String)>, depth: usize, tpl_line: usize, s: String| {
        body.push((tpl_line, format!("{}{s}", "    ".repeat(depth))));
    };

    for tok in toks {
        last_line = tok.line();
        match tok {
            Tok::Text(t, l) => {
                if !t.is_empty() {
                    emit_line(&mut body, depth, l, format!("out.push(\"{}\");", lit(&t)));
                }
            }
            Tok::Var(e, l) => {
                if e.is_empty() {
                    return Err(TplError { line: l, msg: "empty '{{ }}'".into() });
                }
                emit_line(&mut body, depth, l, format!("out.push(escape_html(to_string({e})));"));
            }
            Tok::Raw(e, l) => {
                if e.is_empty() {
                    return Err(TplError { line: l, msg: "empty '{{& }}'".into() });
                }
                emit_line(&mut body, depth, l, format!("out.push(to_string({e}));"));
            }
            // (Los casos `import`/`include` de composición van en el match de etiquetas, abajo.)
            Tok::Tag(t, l) => {
                let (kw, rest) = match t.split_once(char::is_whitespace) {
                    Some((k, r)) => (k, r.trim()),
                    None => (t.as_str(), ""),
                };
                match kw {
                    "if" => {
                        if rest.is_empty() {
                            return Err(TplError { line: l, msg: "'{% if %}' without condition".into() });
                        }
                        emit_line(&mut body, depth, l, format!("if ({rest}) {{"));
                        depth += 1;
                        stack.push(Marco::If);
                    }
                    "elif" => {
                        if !matches!(stack.last(), Some(Marco::If)) {
                            return Err(TplError { line: l, msg: "'{% elif %}' outside an '{% if %}'".into() });
                        }
                        if rest.is_empty() {
                            return Err(TplError { line: l, msg: "'{% elif %}' without condition".into() });
                        }
                        emit_line(&mut body, depth - 1, l, format!("}} else if ({rest}) {{"));
                    }
                    "else" => {
                        if !matches!(stack.last(), Some(Marco::If)) {
                            return Err(TplError { line: l, msg: "'{% else %}' outside an '{% if %}'".into() });
                        }
                        emit_line(&mut body, depth - 1, l, "} else {".to_string());
                    }
                    "endif" => {
                        if !matches!(stack.last(), Some(Marco::If)) {
                            return Err(TplError { line: l, msg: "'{% endif %}' without an '{% if %}' to close".into() });
                        }
                        stack.pop();
                        depth -= 1;
                        emit_line(&mut body, depth, l, "}".to_string());
                    }
                    "for" => {
                        // `for <patrón> in <expr>`: el patrón puede ser `x` o `(k, v)`.
                        let Some(pos) = rest.find(" in ") else {
                            return Err(TplError { line: l, msg: "malformed '{% for %}' (expected 'for x in expr')".into() });
                        };
                        let patron = rest[..pos].trim();
                        let expr = rest[pos + 4..].trim();
                        if patron.is_empty() || expr.is_empty() {
                            return Err(TplError { line: l, msg: "malformed '{% for %}' (expected 'for x in expr')".into() });
                        }
                        emit_line(&mut body, depth, l, format!("for {patron} in {expr} {{"));
                        depth += 1;
                        stack.push(Marco::For);
                    }
                    "endfor" => {
                        if !matches!(stack.last(), Some(Marco::For)) {
                            return Err(TplError { line: l, msg: "'{% endfor %}' without a '{% for %}' to close".into() });
                        }
                        stack.pop();
                        depth -= 1;
                        emit_line(&mut body, depth, l, "}".to_string());
                    }
                    // `{% let name = expr %}`: una local inmutable del template. Alcance = el
                    // bloque raylang generado (dentro de un for/if vive hasta su endfor/endif).
                    "let" => {
                        let bien = rest.split_once('=').is_some_and(|(lhs, rhs)| {
                            let lhs = lhs.trim();
                            !rhs.trim().is_empty()
                                && !lhs.is_empty()
                                && lhs.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
                                && lhs.chars().all(|c| c.is_alphanumeric() || c == '_')
                        });
                        if !bien {
                            return Err(TplError { line: l, msg: format!("malformed '{{% let %}}': '{rest}' (expected 'let name = expr')") });
                        }
                        // Se empalma tal cual (espaciado incluido): el LSP localiza el fragmento
                        // del template como subcadena de la línea generada.
                        emit_line(&mut body, depth, l, format!("let {rest};"));
                    }
                    // Composición de templates: `{% import vistas/tarjeta [as t] %}` trae otro
                    // módulo (otro template compilado, o cualquier módulo del proyecto) al ámbito
                    // del generado. Se HOISTEA a la cabecera (los imports van al frente del módulo),
                    // esté donde esté en el template.
                    "import" => {
                        if !valid_import(rest) {
                            return Err(TplError { line: l, msg: format!("malformed '{{% import %}}': '{rest}' (expected 'path/to/module [as alias]')") });
                        }
                        imports.push((rest.to_string(), l));
                    }
                    // `{% include ruta/al/template(args) %}`: incluye OTRO template. Quien escribe
                    // el template no tiene por qué conocer el nombre de la función generada: el
                    // generador importa el módulo (si no está ya) y llama a su `render` (M103).
                    // La otra forma, `{% include expr %}` (sin la forma `ruta(args)`), empalma el
                    // string de `expr` SIN escapar — HTML ya renderizado (p. ej. el `contenido`
                    // de un layout). Para una expresión arbitraria inline está `{{& expr }}`.
                    "include" => {
                        if rest.is_empty() {
                            return Err(TplError { line: l, msg: "'{% include %}' without argument (expected 'include path/to/template(args)' or 'include expr')".into() });
                        }
                        if let Some((path, args)) = template_ref(rest) {
                            let leaf = path.rsplit('/').next().unwrap_or(path);
                            if !imports.iter().any(|(p, _)| p == path) {
                                imports.push((path.to_string(), l));
                            }
                            emit_line(&mut body, depth, l, format!("out.push(to_string({leaf}.render({args})));"));
                        } else {
                            emit_line(&mut body, depth, l, format!("out.push(to_string({rest}));"));
                        }
                    }
                    "params" => {
                        return Err(TplError { line: l, msg: "'{% params %}' repeated (it can appear only once, at the start)".into() });
                    }
                    other => {
                        return Err(TplError { line: l, msg: format!("unknown tag: '{other}'") });
                    }
                }
            }
        }
    }
    if !stack.is_empty() {
        let missing = match stack.last() {
            Some(Marco::If) => "endif",
            _ => "endfor",
        };
        return Err(TplError { line: last_line, msg: format!("missing a '{{% {missing} %}}' at the end of the template") });
    }

    // Ensamblado + line map. La cabecera son 3 líneas fijas + un `import` por cada `{% import %}`
    // (mapean a SU línea del template) + 4 fijas más (todas las fijas mapean a la línea de
    // `params`, donde vive la firma); el cierre, a la última línea del template.
    let mut header = format!(
        "// GENERATED by `ray build --templates-only` from {name}.ray.html — do NOT edit by hand;\n\
         // regenerate with `ray build --templates-only <path>`. The template is the source of truth.\n\
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
         pub fn render({params}) -> string {{\n\
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
    fn generates_typed_function() {
        let tpl = "{% params title: string, n: int %}\n<h1>{{ title }}</h1>{% if n > 0 %}<p>{{ n }}</p>{% endif %}";
        let code = generate(tpl, "vista").unwrap();
        assert!(code.contains("pub fn render(title: string, n: int) -> string {"), "{code}");
        assert!(code.contains("out.push(escape_html(to_string(title)));"), "{code}");
        assert!(code.contains("if (n > 0) {"), "{code}");
        // Y el generado parsea como raylang válido.
        let tokens = crate::lexer::lex(&code).unwrap();
        assert!(crate::parser::parse(tokens).is_ok());
    }

    #[test]
    fn escapes_html_literals() {
        // Un `${` o una comilla del HTML no deben romper (ni interpolar) el string generado.
        let tpl = "{% params x: int %}precio: \"${simbolo}\" y \\ raro {{ x }}";
        let code = generate(tpl, "t").unwrap();
        assert!(code.contains("\\$"), "el $ va escapado:\n{code}");
        assert!(code.contains("\\\""), "las comillas van escapadas:\n{code}");
        let tokens = crate::lexer::lex(&code).unwrap();
        assert!(crate::parser::parse(tokens).is_ok());
    }

    #[test]
    fn template_errors_with_line() {
        assert!(generate("<html>", "t").unwrap_err().msg.contains("params"));
        let e = generate("{% params x: int %}\n\n{% if x %}", "t").unwrap_err();
        assert!(e.msg.contains("endif"));
        assert_eq!(e.line, 3, "el error señala la línea del if sin close");
        assert!(generate("{% params x: int %}{% endfor %}", "t").unwrap_err().msg.contains("endfor"));
        // Etiqueta INVENTADA (no `block`, que es real desde M55): debe dar "etiqueta desconocida".
        assert!(generate("{% params x: int %}{% zzz %}", "t").unwrap_err().msg.contains("unknown"));
        assert!(generate("{% params x %}hello", "t").unwrap_err().msg.contains("malformed"));
    }

    #[test]
    fn split_params_respects_nested_vars() {
        let ps = split_params("m: Map<string, int>, xs: [string], f: fn(int) -> int");
        assert_eq!(ps, vec!["m: Map<string, int>", "xs: [string]", "f: fn(int) -> int"]);
    }

    #[test]
    fn include_by_path_does_not_expose_generated_name() {
        // `{% include ruta/al/template(args) %}`: quien escribe el template NO conoce el
        // `render` generado (M103) — el generador importa el módulo solo (dedup con un import
        // explícito) y llama a la función por él.
        let tpl = "{% params p: string %}\n<div>{% include vistas/tarjeta(p) %}</div>\n{% include vistas/tarjeta(p + \"!\") %}\n";
        let (code, _) = generate_with_map(tpl, "pagina").unwrap();
        assert_eq!(code.matches("import vistas/tarjeta;\n").count(), 1, "auto-import, sin duplicate\n{code}");
        assert!(code.contains("out.push(to_string(tarjeta.render(p)));"), "{code}");
        assert!(code.contains("out.push(to_string(tarjeta.render(p + \"!\")));"), "{code}");
        let tokens = crate::lexer::lex(&code).unwrap();
        assert!(crate::parser::parse(tokens).is_ok());
        // template_ref: la forma con `.` NO es referencia (expresión ordinaria, cruda).
        assert_eq!(template_ref("vistas/tarjeta(p)"), Some(("vistas/tarjeta", "p")));
        assert_eq!(template_ref("a/b(f(x), y)"), Some(("a/b", "f(x), y")));
        assert_eq!(template_ref("m.f(x)"), None);
        assert_eq!(template_ref("contenido"), None);
    }

    #[test]
    fn import_and_include_compose_templates() {
        // `{% import %}` se hoistea a la cabecera (con su línea en el map) y el `{% include %}`
        // de EXPRESIÓN empalma sin escapar (HTML ya renderizado, p. ej. el `contenido` de un
        // layout o una llamada explícita).
        let tpl = "{% params p: string %}\n{% import vistas/tarjeta %}\n{% import util/fmt as f %}\n<div>{% include tarjeta.render(p) %}</div>\n";
        let (code, map) = generate_with_map(tpl, "pagina").unwrap();
        assert!(code.contains("import vistas/tarjeta;\n"), "{code}");
        assert!(code.contains("import util/fmt as f;\n"), "{code}");
        assert!(code.contains("out.push(to_string(tarjeta.render(p)));"), "{code}");
        // Los imports van ANTES de la función y su línea del map es la del template.
        let lines: Vec<&str> = code.lines().collect();
        let (i, _) = lines.iter().enumerate().find(|(_, l)| l.contains("import vistas/tarjeta")).unwrap();
        assert!(i < lines.iter().position(|l| l.contains("pub fn")).unwrap());
        assert_eq!(map[i], 2, "{code}");
        assert_eq!(lines.len(), map.len());
        let tokens = crate::lexer::lex(&code).unwrap();
        assert!(crate::parser::parse(tokens).is_ok());
        // Errores: import mal formado (no se empalma texto arbitrario) e include vacío.
        assert!(generate("{% params x: int %}{% import ../outside %}", "t").unwrap_err().msg.contains("malformed"));
        assert!(generate("{% params x: int %}{% import a; drop %}", "t").unwrap_err().msg.contains("malformed"));
        assert!(generate("{% params x: int %}{% include %}", "t").unwrap_err().msg.contains("include"));
    }

    #[test]
    fn let_declares_locals() {
        let tpl = "{% params precios: [int] %}\n{% let total = precios.len() %}\n<p>{{ total }}</p>\n";
        let code = generate(tpl, "v").unwrap();
        assert!(code.contains("let total = precios.len();"), "{code}");
        let tokens = crate::lexer::lex(&code).unwrap();
        assert!(crate::parser::parse(tokens).is_ok());
        assert!(generate("{% params x: int %}{% let 1a = 2 %}", "t").unwrap_err().msg.contains("malformed"));
        assert!(generate("{% params x: int %}{% let y %}", "t").unwrap_err().msg.contains("malformed"));
    }

    #[test]
    fn extends_and_block_inherit_layout() {
        let base = std::env::temp_dir().join("ray_templ_extends_unit");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("base.ray.html"),
            "{% params title: string %}\n<html><title>{{ title }}</title><body>\n{% block body %}<p>default</p>{% endblock %}\n<footer>{% block pie %}pie común{% endblock %}</footer>\n</body></html>\n").unwrap();
        // El hijo solo aporta bloques; hereda la estructura (y el bloque `pie` queda con su defecto).
        let child = "{% params title: string, n: int %}\n{% extends base %}\n{% block body %}<b>{{ n }}</b>{% endblock %}\n";
        let (code, map) = generate_with_map_at(child, "pagina", Some(&base)).unwrap();
        assert!(code.contains("pub fn render(title: string, n: int) -> string"), "la signature es la del HIJO\n{code}");
        assert!(code.contains("out.push(escape_html(to_string(n)));"), "el block del child\n{code}");
        assert!(code.contains("pie com"), "el default del layout queda\n{code}");
        assert!(!code.contains("default"), "el block sobreescrito NO deja su default\n{code}");
        let tokens = crate::lexer::lex(&code).unwrap();
        assert!(crate::parser::parse(tokens).is_ok());
        // El line map: la línea del bloque del hijo apunta a SU línea (3); las del layout, al extends (2).
        let lines: Vec<&str> = code.lines().collect();
        let (i, _) = lines.iter().enumerate().find(|(_, l)| l.contains("to_string(n)")).unwrap();
        assert_eq!(map[i], 3, "{code}");
        let (j, _) = lines.iter().enumerate().find(|(_, l)| l.contains("title")).unwrap();
        let _ = j; // la firma mapea a params; el <title> del layout:
        let (k, _) = lines.iter().enumerate().find(|(_, l)| l.contains("<title>")).unwrap();
        assert_eq!(map[k], 2, "las líneas del layout se atribuyen al extends\n{code}");

        // El layout compila STANDALONE: los marcadores de bloque son transparentes.
        let lsrc = std::fs::read_to_string(base.join("base.ray.html")).unwrap();
        let solo = generate(&lsrc, "base").unwrap();
        assert!(solo.contains("default") && solo.contains("pie com"), "{solo}");
        assert!(!solo.contains("block"), "{solo}");

        // Errores: bloque que el layout no declara; extends tardío; contenido suelto; sin endblock.
        let e = generate_with_map_at("{% params t: string %}\n{% extends base %}\n{% block noexiste %}x{% endblock %}\n", "p", Some(&base)).unwrap_err();
        assert!(e.msg.contains("does not declare"), "{}", e.msg);
        assert_eq!(e.line, 3);
        let e = generate("{% params t: string %}\nhola\n{% extends base %}\n", "p").unwrap_err();
        assert!(e.msg.contains("first tag"), "{}", e.msg);
        let e = generate_with_map_at("{% params t: string %}\n{% extends base %}\nsuelto\n", "p", Some(&base)).unwrap_err();
        assert!(e.msg.contains("can only have"), "{}", e.msg);
        let e = generate("{% params t: string %}\n{% block a %}x\n", "p").unwrap_err();
        assert!(e.msg.contains("endblock"), "{}", e.msg);

        // Resolución desde la RAÍZ del proyecto (donde está `ray.toml`), como los imports: un
        // template en `sub/` referencia el layout por su ruta completa `sub/base2`.
        std::fs::write(base.join("ray.toml"), "[package]\nname = \"t\"\nversion = \"0.1.0\"\n").unwrap();
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::write(base.join("sub/base2.ray.html"),
            "{% params t: string %}<b>{% block body %}{% endblock %}</b>\n").unwrap();
        let (code, _) = generate_with_map_at(
            "{% params t: string %}\n{% extends sub/base2 %}\n{% block body %}{{ t }}{% endblock %}\n",
            "p", Some(&base.join("sub"))).unwrap();
        assert!(code.contains("out.push(escape_html(to_string(t)));"), "{code}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn formats_the_template() {
        // Cada `{% %}` en su línea, indentación por bloques, `{{ }}` inline, delimitadores
        // normalizados, blancos conservados.
        let tpl = "{% params xs: [string], ok: bool %}\n\
                   <ul>{% for lang in xs %}  {%include tarjeta.render(lang)%}\n\
                   {% endfor %}</ul>\n\
                   \n\
                   {%if ok%}<p>{{titulo}} y {{& crudo }}</p>{%else%}<i>no</i>{%endif%}\n";
        let out = format_template(tpl, "    ").unwrap();
        let expected = "{% params xs: [string], ok: bool %}\n\
                        <ul>\n\
                        {% for lang in xs %}\n\
                        \x20   {% include tarjeta.render(lang) %}\n\
                        {% endfor %}\n\
                        </ul>\n\
                        \n\
                        {% if ok %}\n\
                        \x20   <p>{{ titulo }} y {{& crudo }}</p>\n\
                        {% else %}\n\
                        \x20   <i>no</i>\n\
                        {% endif %}\n";
        assert_eq!(out, expected);
        // Idempotente: formatear lo formateado no cambia nada.
        assert_eq!(format_template(&out, "    ").unwrap(), out);
        // Los bloques anidan; el interior de una expresión NO se toca (string con espacios).
        let tpl = "{% params xs: [[int]] %}{% for row in xs %}{% for c in row %}{{ \"dos  espacios\" }}{% endfor %}{% endfor %}\n";
        let out = format_template(tpl, "  ").unwrap();
        assert!(out.contains("\n  {% for c in row %}\n    {{ \"dos  espacios\" }}\n  {% endfor %}\n"), "{out}");
        // Un buffer roto (delimitador sin cerrar) no se formatea.
        assert!(format_template("{% params x: int %}\n<p>{{ x </p>\n", "    ").is_none());
    }

    #[test]
    fn line_map_translates_to_template() {
        let tpl = "{% params t: string %}\n<h1>{{ t }}</h1>\n{% if t != \"\" %}\n<p>{{ t }}</p>\n{% endif %}\n";
        let (code, map) = generate_with_map(tpl, "v").unwrap();
        let lines: Vec<&str> = code.lines().collect();
        assert_eq!(lines.len(), map.len(), "one entry del mapa por línea generada");
        // La línea generada del `if` mapea a la línea 3 del template.
        let (i, _) = lines.iter().enumerate().find(|(_, l)| l.contains("if (t !=")).unwrap();
        assert_eq!(map[i], 3, "{code}");
        // Y la del `<p>{{ t }}</p>` a la 4.
        let (k, _) = lines.iter().enumerate().skip(i).find(|(_, l)| l.contains("escape_html")).unwrap();
        assert_eq!(map[k], 4, "{code}");
    }
}
