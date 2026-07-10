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

/// Compila `input` (`*.ray.html`) y escribe el módulo generado al lado (`*.ray`). Devuelve la ruta
/// generada. `Err` con el archivo, la línea y el motivo si el template está mal formado.
pub fn generate_file(input: &Path) -> Result<PathBuf, String> {
    let src = std::fs::read_to_string(input)
        .map_err(|e| format!("no se pudo leer '{}': {e}", input.display()))?;
    let name = fn_suffix_of(input)?;
    let (code, _map) = generate_with_map_at(&src, &name, input.parent())
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
    let hereda = toks.iter()
        .find_map(|t| match t {
            Tok::Tag(s, _) => Some(s.starts_with("extends")),
            Tok::Text(s, _) if s.trim().is_empty() => None,
            _ => Some(false),
        })
        .unwrap_or(false);
    if !hereda {
        // Sin herencia: quitar los marcadores de bloque (validando el anidamiento); el contenido
        // por defecto queda en su sitio. Un `{% extends %}` tardío es error (debe ir primero).
        let mut out = Vec::new();
        let mut open: Option<usize> = None;
        for tok in toks {
            if let Tok::Tag(t, l) = &tok {
                let (kw, resto) = kw_of(t);
                match kw.as_str() {
                    "extends" => return Err(TplError { line: *l, msg: "'{% extends %}' debe ser la primera etiqueta tras '{% params %}'".into() }),
                    "block" => {
                        if open.is_some() {
                            return Err(TplError { line: *l, msg: "'{% block %}' anidado".into() });
                        }
                        if !ident_ok(&resto) {
                            return Err(TplError { line: *l, msg: format!("'{{% block %}}' mal formado: '{resto}' (se espera un nombre)") });
                        }
                        open = Some(*l);
                        continue;
                    }
                    "endblock" => {
                        if open.is_none() {
                            return Err(TplError { line: *l, msg: "'{% endblock %}' sin '{% block %}' que cerrar".into() });
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
            return Err(TplError { line: l, msg: "'{% block %}' sin '{% endblock %}'".into() });
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
            let (kw, resto) = kw_of(t);
            match kw.as_str() {
                "extends" if cur.is_none() => {
                    if layout_ref.is_some() {
                        return Err(TplError { line: *l, msg: "'{% extends %}' repetido".into() });
                    }
                    if !valid_import(&resto) || resto.contains(" as ") {
                        return Err(TplError { line: *l, msg: format!("'{{% extends %}}' mal formado: '{resto}' (se espera 'ruta/al/layout')") });
                    }
                    layout_ref = Some((resto, *l));
                    continue;
                }
                "import" if cur.is_none() => {
                    imports.push(tok.clone());
                    continue;
                }
                "block" => {
                    if cur.is_some() {
                        return Err(TplError { line: *l, msg: "'{% block %}' anidado".into() });
                    }
                    if !ident_ok(&resto) {
                        return Err(TplError { line: *l, msg: format!("'{{% block %}}' mal formado: '{resto}' (se espera un nombre)") });
                    }
                    if blocks.iter().any(|(n, _, _)| *n == resto) {
                        return Err(TplError { line: *l, msg: format!("'{{% block {resto} %}}' repetido") });
                    }
                    cur = Some((resto.clone(), *l));
                    blocks.push((resto, *l, Vec::new()));
                    continue;
                }
                "endblock" => {
                    if cur.is_none() {
                        return Err(TplError { line: *l, msg: "'{% endblock %}' sin '{% block %}' que cerrar".into() });
                    }
                    cur = None;
                    continue;
                }
                _ => {}
            }
        }
        match (&cur, &tok) {
            (Some(_), _) => blocks.last_mut().expect("bloque abierto").2.push(tok),
            (None, Tok::Text(s, _)) if s.trim().is_empty() => {}
            (None, t) => {
                return Err(TplError { line: t.line(), msg: "un template con '{% extends %}' solo puede tener '{% block %}'s (e '{% import %}'s) fuera de los bloques".into() });
            }
        }
    }
    if let Some((n, l)) = cur {
        return Err(TplError { line: l, msg: format!("'{{% block {n} %}}' sin '{{% endblock %}}'") });
    }
    let (lpath, eline) = layout_ref.expect("hereda");
    let Some(dir) = dir else {
        return Err(TplError { line: eline, msg: "'{% extends %}' requiere generar desde un archivo (la ruta del layout se resuelve relativa al template)".into() });
    };
    let file = dir.join(format!("{lpath}.ray.html"));
    let lsrc = std::fs::read_to_string(&file)
        .map_err(|e| TplError { line: eline, msg: format!("no se pudo leer el layout '{}': {e}", file.display()) })?;
    let ltoks = tokenize(&lsrc)
        .map_err(|e| TplError { line: eline, msg: format!("en el layout '{}' (línea {}): {}", file.display(), e.line, e.msg) })?;
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
        && let Some(resto) = t.strip_prefix('\n')
    {
        let (resto, l) = (resto.to_string(), *l);
        it.next();
        if !resto.is_empty() {
            // Se reintroduce sin el salto (la línea real es la siguiente, pero todo el layout se
            // atribuye al extends de todos modos).
            let tok = Tok::Text(resto, l + 1);
            return merge_layout(imports, blocks, std::iter::once(tok).chain(it).collect(), &lpath, eline);
        }
    }
    merge_layout(imports, blocks, it.collect(), &lpath, eline)
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
    let mut usados: Vec<&str> = Vec::new();
    let mut in_block = false;
    let mut skip_default = false;
    for tok in ltoks {
        if let Tok::Tag(t, _) = &tok {
            let (kw, resto) = match t.split_once(char::is_whitespace) {
                Some((k, r)) => (k, r.trim()),
                None => (t.as_str(), ""),
            };
            match kw {
                "extends" => {
                    return Err(TplError { line: eline, msg: format!("el layout '{lpath}' también usa '{{% extends %}}' (herencia encadenada: diferida)") });
                }
                "block" => {
                    if in_block {
                        return Err(TplError { line: eline, msg: format!("en el layout '{lpath}': '{{% block %}}' anidado") });
                    }
                    in_block = true;
                    if let Some((n, _, body)) = blocks.iter().find(|(n, _, _)| n == resto) {
                        out.extend(body.iter().cloned()); // líneas del HIJO: mapean exactas
                        usados.push(n);
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
        if !usados.contains(&n.as_str()) {
            return Err(TplError { line: *l, msg: format!("el layout '{lpath}' no declara un '{{% block {n} %}}'") });
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
                    // `{% let nombre = expr %}`: una local inmutable del template. Alcance = el
                    // bloque raylang generado (dentro de un for/if vive hasta su endfor/endif).
                    "let" => {
                        let bien = resto.split_once('=').is_some_and(|(lhs, rhs)| {
                            let lhs = lhs.trim();
                            !rhs.trim().is_empty()
                                && !lhs.is_empty()
                                && lhs.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
                                && lhs.chars().all(|c| c.is_alphanumeric() || c == '_')
                        });
                        if !bien {
                            return Err(TplError { line: l, msg: format!("'{{% let %}}' mal formado: '{resto}' (se espera 'let nombre = expr')") });
                        }
                        // Se empalma tal cual (espaciado incluido): el LSP localiza el fragmento
                        // del template como subcadena de la línea generada.
                        linea(&mut body, depth, l, format!("let {resto};"));
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
    fn let_declara_locales() {
        let tpl = "{% params precios: [int] %}\n{% let total = precios.len() %}\n<p>{{ total }}</p>\n";
        let code = generate(tpl, "v").unwrap();
        assert!(code.contains("let total = precios.len();"), "{code}");
        let tokens = crate::lexer::lex(&code).unwrap();
        assert!(crate::parser::parse(tokens).is_ok());
        assert!(generate("{% params x: int %}{% let 1a = 2 %}", "t").unwrap_err().msg.contains("mal formado"));
        assert!(generate("{% params x: int %}{% let y %}", "t").unwrap_err().msg.contains("mal formado"));
    }

    #[test]
    fn extends_y_block_heredan_el_layout() {
        let base = std::env::temp_dir().join("ray_templ_extends_unit");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("base.ray.html"),
            "{% params titulo: string %}\n<html><title>{{ titulo }}</title><body>\n{% block cuerpo %}<p>defecto</p>{% endblock %}\n<footer>{% block pie %}pie común{% endblock %}</footer>\n</body></html>\n").unwrap();
        // El hijo solo aporta bloques; hereda la estructura (y el bloque `pie` queda con su defecto).
        let hijo = "{% params titulo: string, n: int %}\n{% extends base %}\n{% block cuerpo %}<b>{{ n }}</b>{% endblock %}\n";
        let (code, map) = generate_with_map_at(hijo, "pagina", Some(&base)).unwrap();
        assert!(code.contains("pub fn render_pagina(titulo: string, n: int) -> string"), "la firma es la del HIJO\n{code}");
        assert!(code.contains("out.push(escape_html(to_string(n)));"), "el bloque del hijo\n{code}");
        assert!(code.contains("pie com"), "el defecto del layout queda\n{code}");
        assert!(!code.contains("defecto"), "el bloque sobreescrito NO deja su defecto\n{code}");
        let tokens = crate::lexer::lex(&code).unwrap();
        assert!(crate::parser::parse(tokens).is_ok());
        // El line map: la línea del bloque del hijo apunta a SU línea (3); las del layout, al extends (2).
        let lines: Vec<&str> = code.lines().collect();
        let (i, _) = lines.iter().enumerate().find(|(_, l)| l.contains("to_string(n)")).unwrap();
        assert_eq!(map[i], 3, "{code}");
        let (j, _) = lines.iter().enumerate().find(|(_, l)| l.contains("titulo")).unwrap();
        let _ = j; // la firma mapea a params; el <title> del layout:
        let (k, _) = lines.iter().enumerate().find(|(_, l)| l.contains("<title>")).unwrap();
        assert_eq!(map[k], 2, "las líneas del layout se atribuyen al extends\n{code}");

        // El layout compila STANDALONE: los marcadores de bloque son transparentes.
        let lsrc = std::fs::read_to_string(base.join("base.ray.html")).unwrap();
        let solo = generate(&lsrc, "base").unwrap();
        assert!(solo.contains("defecto") && solo.contains("pie com"), "{solo}");
        assert!(!solo.contains("block"), "{solo}");

        // Errores: bloque que el layout no declara; extends tardío; contenido suelto; sin endblock.
        let e = generate_with_map_at("{% params t: string %}\n{% extends base %}\n{% block noexiste %}x{% endblock %}\n", "p", Some(&base)).unwrap_err();
        assert!(e.msg.contains("no declara"), "{}", e.msg);
        assert_eq!(e.line, 3);
        let e = generate("{% params t: string %}\nhola\n{% extends base %}\n", "p").unwrap_err();
        assert!(e.msg.contains("primera etiqueta"), "{}", e.msg);
        let e = generate_with_map_at("{% params t: string %}\n{% extends base %}\nsuelto\n", "p", Some(&base)).unwrap_err();
        assert!(e.msg.contains("solo puede tener"), "{}", e.msg);
        let e = generate("{% params t: string %}\n{% block a %}x\n", "p").unwrap_err();
        assert!(e.msg.contains("endblock"), "{}", e.msg);
        let _ = std::fs::remove_dir_all(&base);
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
