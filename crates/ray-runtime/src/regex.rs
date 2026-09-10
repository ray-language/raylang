//! R5 (bench políglota, jul 2026) — motor de regex ACELERADO para el binario transpilado.
//!
//! El backend nativo intercepta las funciones internas `run_*` de `std/regex` y las ejecuta aquí
//! con el crate `regex` de Rust (la liga de los ~27 ms del bench). El reparto de responsabilidades
//! preserva la paridad con la VM:
//!
//! - El **parseo y la validación** del patrón siguen siendo los de `std/regex` (raylang,
//!   transpilado): los errores de compilación son BYTE-IDÉNTICOS a la VM, y aquí solo llegan
//!   patrones del subconjunto ya validado.
//! - La **ejecución** traduce el dialecto de `std/regex` al del crate (`translate`) y corre el
//!   motor nativo. Diferencias de dialecto cubiertas: `\d\w\s` son clases ASCII FIJAS de
//!   `std/regex` (`[0-9]`, `[0-9A-Za-z_]`, `[ \t\n\r]` — NO las Unicode/POSIX del crate);
//!   cualquier otro escape `\X` (salvo `\n`/`\t`/`\r`) es el carácter LITERAL `X` (`\b` es la
//!   letra b, no un word-boundary); y `.` casa CUALQUIER carácter, incluido `\n`
//!   (`dot_matches_new_line`).
//! - Los índices del API de `std/regex` son por CARÁCTER; el crate da bytes → `char_at` convierte
//!   (fast-path ASCII).
//! - `replace_all` usa el reemplazo LITERAL: `std/regex` no interpreta `$1`.
//!
//! Sin la feature (`--without regex`), el transpilador NO intercepta y la Pike VM de raylang se
//! transpila tal cual: el fallback es la implementación real, no un stub.

#[cfg(feature = "regex")]
mod imp {
    use regex::{Regex, RegexBuilder};
    use std::cell::RefCell;
    use std::collections::HashMap;

    // Caché por hilo de patrones compilados (clave: patrón del DIALECTO ray + modo full). Los
    // hilos del runtime nativo son hilos de SO → thread_local evita un Mutex en el camino caliente.
    thread_local! {
        static CACHE: RefCell<HashMap<(String, bool), Regex>> = RefCell::new(HashMap::new());
    }

    /// Traduce un patrón del dialecto de `std/regex` (YA validado por su parser) al del crate.
    fn translate(pat: &str) -> String {
        let mut out = String::with_capacity(pat.len() + 8);
        let mut chars = pat.chars().peekable();
        let mut in_class = false;
        while let Some(c) = chars.next() {
            if c == '\\' {
                let Some(e) = chars.next() else { break }; // el parser ya rechazó la '\' colgante
                match e {
                    // Clases ASCII fijas de std/regex.
                    'd' if !in_class => out.push_str("[0-9]"),
                    'd' => out.push_str("0-9"),
                    'D' if !in_class => out.push_str("[^0-9]"),
                    'w' if !in_class => out.push_str("[0-9A-Za-z_]"),
                    'w' => out.push_str("0-9A-Za-z_"),
                    'W' if !in_class => out.push_str("[^0-9A-Za-z_]"),
                    's' if !in_class => out.push_str("[ \\t\\n\\r]"),
                    's' => out.push_str(" \\t\\n\\r"),
                    'S' if !in_class => out.push_str("[^ \\t\\n\\r]"),
                    // Escapes de control compartidos.
                    'n' => out.push_str("\\n"),
                    't' => out.push_str("\\t"),
                    'r' => out.push_str("\\r"),
                    // Cualquier otro `\X` es el carácter LITERAL X en std/regex (incl. `\b`, `\D`
                    // dentro de clase, `\q`…). Se emite escapado si es metacarácter del crate.
                    other => push_literal(&mut out, other, in_class),
                }
            } else {
                if c == '[' && !in_class {
                    in_class = true;
                } else if c == ']' && in_class {
                    in_class = false;
                }
                out.push(c);
            }
        }
        out
    }

    /// Empuja `c` como carácter LITERAL del patrón del crate (escapado si hace falta).
    fn push_literal(out: &mut String, c: char, in_class: bool) {
        let meta = if in_class { "\\^]-&~[" } else { "\\.+*?()|[]{}^$#" };
        if meta.contains(c) {
            out.push('\\');
        }
        out.push(c);
    }

    /// Compila (con caché) el patrón traducido. `full` envuelve en `^(?:…)$` (full_match).
    /// Compila (una vez) el patrón TRADUCIDO al dialecto del crate.
    fn build(pat: &str, full: bool) -> Regex {
        let t = translate(pat);
        let t = if full { format!("^(?:{t})$") } else { t };
        RegexBuilder::new(&t)
            .dot_matches_new_line(true) // el `.` de std/regex casa TAMBIÉN '\n'
            .build()
            // El parser de std/regex ya validó el patrón; si la traducción no compila,
            // es un bug NUESTRO de traducción, no del usuario.
            .unwrap_or_else(|e| panic!("ray-runtime regex: internal translation error for validated pattern {pat:?}: {e}"))
    }

    fn compiled<R>(pat: &str, full: bool, f: impl FnOnce(&Regex) -> R) -> R {
        CACHE.with(|c| {
            let mut map = c.borrow_mut();
            let key = (pat.to_string(), full);
            let rx = map.entry(key).or_insert_with(|| build(pat, full));
            f(rx)
        })
    }

    /// Índice de CARÁCTER del offset de byte `b` en `text` (fast-path ASCII: byte == carácter).
    fn char_at(text: &str, b: usize) -> i64 {
        if text.is_ascii() { b as i64 } else { text[..b].chars().count() as i64 }
    }

    pub fn full_match(pat: &str, text: &str) -> bool {
        compiled(pat, true, |rx| rx.is_match(text))
    }

    pub fn search(pat: &str, text: &str) -> bool {
        compiled(pat, false, |rx| rx.is_match(text))
    }

    pub fn find(pat: &str, text: &str) -> Option<(i64, i64)> {
        compiled(pat, false, |rx| {
            rx.find(text).map(|m| (char_at(text, m.start()), char_at(text, m.end())))
        })
    }

    pub fn find_str(pat: &str, text: &str) -> Option<String> {
        compiled(pat, false, |rx| rx.find(text).map(|m| m.as_str().to_string()))
    }

    /// La siguiente frontera de carácter tras `i` (para avanzar UNO en match vacío, como std/regex).
    fn next_boundary(text: &str, i: usize) -> usize {
        if i >= text.len() {
            i + 1
        } else {
            i + text[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1)
        }
    }

    // find_all/replace_all NO usan `find_iter`: el iterador del crate OMITE un match vacío adyacente
    // al final del match anterior, y `std/regex` sí lo reporta (p. ej. `a*` sobre "baa" da
    // ["", "aa", ""]). Se replica el bucle exacto de std/regex con `find_at` (búsqueda desde
    // offset, sin la supresión del iterador): no-vacío → seguir en `end`; vacío → avanzar un carácter.
    pub fn find_all(pat: &str, text: &str) -> Vec<String> {
        compiled(pat, false, |rx| {
            let mut out = Vec::new();
            let n = text.len();
            let mut s = 0usize;
            while s <= n {
                match rx.find_at(text, s) {
                    Some(m) => {
                        out.push(m.as_str().to_string());
                        s = if m.end() > m.start() { m.end() } else { next_boundary(text, m.start()) };
                    }
                    None => break,
                }
            }
            out
        })
    }

    pub fn replace_all(pat: &str, text: &str, repl: &str) -> String {
        // Reemplazo LITERAL (std/regex no interpreta `$1`), con el MISMO bucle que std/regex
        // (ver la nota de find_all: un match vacío copia el carácter actual y avanza uno).
        compiled(pat, false, |rx| {
            let mut out = String::with_capacity(text.len());
            let n = text.len();
            let mut s = 0usize;
            while s <= n {
                match rx.find_at(text, s) {
                    Some(m) => {
                        out.push_str(&text[s..m.start()]);
                        out.push_str(repl);
                        if m.end() > m.start() {
                            s = m.end();
                        } else {
                            let nb = next_boundary(text, m.start());
                            if m.start() < n {
                                out.push_str(&text[m.start()..nb.min(n)]);
                            }
                            s = nb;
                        }
                    }
                    None => {
                        out.push_str(&text[s..n]);
                        break;
                    }
                }
            }
            out
        })
    }

    pub fn captures(pat: &str, text: &str) -> Option<Vec<Option<(i64, i64)>>> {
        compiled(pat, false, |rx| {
            rx.captures(text).map(|caps| {
                (0..caps.len())
                    .map(|i| caps.get(i).map(|m| (char_at(text, m.start()), char_at(text, m.end()))))
                    .collect()
            })
        })
    }

    pub fn captures_str(pat: &str, text: &str) -> Option<Vec<Option<String>>> {
        compiled(pat, false, |rx| {
            rx.captures(text).map(|caps| {
                (0..caps.len()).map(|i| caps.get(i).map(|m| m.as_str().to_string())).collect()
            })
        })
    }

    /// R6 (bench regex): los rangos de **bytes** de cada captura. El borde nativo construye los
    /// `Rc<str>` directamente del texto con ellos (un alloc por grupo), sin pasar por el
    /// `Vec<Option<String>>` intermedio de `captures_str` (String + recopia a Rc por grupo).
    /// Los límites de un `Match` caen siempre en fronteras de carácter → el slice es seguro.
    /// (Se probó además reusar `CaptureLocations` por patrón —R6b—: midió PEOR que el
    /// `captures` normal (+3 ms en 200k matches, intercalado) y se descartó.)
    pub fn captures_byte_ranges(pat: &str, text: &str) -> Option<Vec<Option<(usize, usize)>>> {
        compiled(pat, false, |rx| {
            rx.captures(text).map(|caps| {
                (0..caps.len()).map(|i| caps.get(i).map(|m| (m.start(), m.end()))).collect()
            })
        })
    }
}

#[cfg(feature = "regex")]
pub use imp::*;

/// M232 — `regex.onig`: el dialecto **Oniguruma** (el de los `.sublime-syntax`, Ruby, TextMate)
/// sobre `fancy-regex`. A diferencia del resto de este módulo, aquí NO hay implementación de
/// referencia en raylang: look-around y backreferences exigen backtracking y los tres motores
/// (intérprete, VM, nativo) llaman a ESTE código, que es la fuente de verdad.
///
/// - Los patrones compilados viven en una tabla global por proceso, indexada por **handle**
///   (`i64`) y deduplicada por patrón: compilar dos veces el mismo texto devuelve el mismo
///   handle, así que la tabla crece con los patrones DISTINTOS del programa, no con las llamadas.
///   Los actores (hilos distintos) comparten la tabla → `RwLock`, lectura en el camino caliente.
/// - Semántica fija (Oniguruma): `^`/`$` son anclas de LÍNEA, `.` no casa `\n` salvo `(?m)`/`(?s)`,
///   `\h` es dígito hexadecimal, `\G` es la posición `from` de `search_from`/`at` de `match_at`.
/// - `match_at` es `\G(?:pat)` compilado aparte (perezoso): el motor no tiene búsqueda anclada
///   a una posición, y envolver el patrón no altera la numeración de grupos.
/// - Índices por CARÁCTER (como todo `std/regex`); el motor trabaja en bytes → conversión con
///   fast-path ASCII.
/// - `backtrack_limit`: un patrón catastrófico no cuelga al llamador; `search` devuelve `Err` y
///   `std/regex` lo convierte en pánico con nombre (idéntico en los tres motores).
#[cfg(feature = "regex")]
pub mod onig {
    use fancy_regex::{Regex, RegexBuilder};
    use std::collections::HashMap;
    use std::sync::{Arc, OnceLock, RwLock};

    /// Pasos de backtracking por búsqueda antes de abandonar (el orden del `DEFAULT_STEP_LIMIT`
    /// del motor de ray-sublime, que nació con el mismo propósito).
    pub const BACKTRACK_LIMIT: usize = 1_000_000;

    struct Entry {
        pattern: String,
        rx: Arc<Regex>,
        /// `\G(?:pat)` para `match_at`; se compila la primera vez que hace falta.
        anchored: OnceLock<Arc<Regex>>,
        names: Vec<String>,
    }

    #[derive(Default)]
    struct Table {
        by_pattern: HashMap<String, usize>,
        entries: Vec<Arc<Entry>>,
    }

    fn table() -> &'static RwLock<Table> {
        static TABLE: OnceLock<RwLock<Table>> = OnceLock::new();
        TABLE.get_or_init(|| RwLock::new(Table::default()))
    }

    fn build(pat: &str) -> Result<Regex, String> {
        RegexBuilder::new(pat)
            .oniguruma_mode(true)
            .multi_line(true)
            .backtrack_limit(BACKTRACK_LIMIT)
            .build()
            .map_err(|e| format!("regex: {}", e.to_string().trim_start_matches("Error compiling regex: ")))
    }

    /// Compila `pat` (o reutiliza el handle si ya estaba). Devuelve `(handle, nombres)`: un
    /// nombre por grupo, `[0]` = `""` (el match entero) y `""` para los grupos sin nombre.
    pub fn compile(pat: &str) -> Result<(i64, Vec<String>), String> {
        {
            let t = table().read().unwrap();
            if let Some(&i) = t.by_pattern.get(pat) {
                return Ok((i as i64, t.entries[i].names.clone()));
            }
        }
        let rx = build(pat)?;
        let mut names = vec![String::new(); rx.captures_len()];
        for (name, idx) in rx.capture_names().enumerate().filter_map(|(i, n)| n.map(|n| (n, i))) {
            if idx < names.len() {
                names[idx] = name.to_string();
            }
        }
        let mut t = table().write().unwrap();
        // Carrera benigna: otro hilo pudo insertarlo entre la lectura y la escritura.
        if let Some(&i) = t.by_pattern.get(pat) {
            return Ok((i as i64, t.entries[i].names.clone()));
        }
        let i = t.entries.len();
        t.entries.push(Arc::new(Entry {
            pattern: pat.to_string(),
            rx: Arc::new(rx),
            anchored: OnceLock::new(),
            names: names.clone(),
        }));
        t.by_pattern.insert(pat.to_string(), i);
        Ok((i as i64, names))
    }

    fn entry(id: i64) -> Option<Arc<Entry>> {
        usize::try_from(id).ok().and_then(|i| table().read().unwrap().entries.get(i).cloned())
    }

    /// Offset de byte del índice de carácter `ci` (`None` si `ci` pasa del final).
    fn byte_at(text: &str, ci: usize) -> Option<usize> {
        if text.is_ascii() {
            return (ci <= text.len()).then_some(ci);
        }
        if ci == 0 {
            return Some(0);
        }
        text.char_indices().nth(ci).map(|(b, _)| b).or_else(|| (text.chars().count() == ci).then_some(text.len()))
    }

    /// Busca desde el carácter `from` (`anchored`: el match debe EMPEZAR en `from`). Devuelve los
    /// spans `[s0, e0, s1, e1, …]` por carácter (`-1, -1` para un grupo que no participó), o
    /// `None` sin match. `Err`: handle desconocido o límite de backtracking superado.
    pub fn search(id: i64, text: &str, from: i64, anchored: bool) -> Result<Option<Vec<i64>>, String> {
        let e = entry(id).ok_or_else(|| format!("regex: unknown onig handle {id}"))?;
        let Some(from_b) = usize::try_from(from).ok().and_then(|f| byte_at(text, f)) else {
            return Ok(None);
        };
        let rx = if anchored {
            e.anchored
                .get_or_init(|| Arc::new(build(&format!("\\G(?:{})", e.pattern)).expect("the plain pattern already compiled")))
                .clone()
        } else {
            e.rx.clone()
        };
        let caps = match rx.captures_from_pos(text, from_b) {
            Ok(Some(c)) => c,
            Ok(None) => return Ok(None),
            Err(fancy_regex::Error::RuntimeError(fancy_regex::RuntimeError::BacktrackLimitExceeded)) => {
                return Err(format!("regex: backtrack limit exceeded ({BACKTRACK_LIMIT} steps) for pattern {:?}", e.pattern))
            }
            Err(err) => return Err(format!("regex: {err}")),
        };
        let ascii = text.is_ascii();
        let mut out = Vec::with_capacity(caps.len() * 2);
        // Los offsets crecen por grupo raramente en orden → contar desde 0 cada vez es O(n·g);
        // con g pequeño y líneas cortas es más barato que una tabla. Fast-path ASCII: byte = char.
        let ci = |b: usize| if ascii { b as i64 } else { text[..b].chars().count() as i64 };
        for i in 0..caps.len() {
            match caps.get(i) {
                Some(m) => {
                    out.push(ci(m.start()));
                    out.push(ci(m.end()));
                }
                None => {
                    out.push(-1);
                    out.push(-1);
                }
            }
        }
        Ok(Some(out))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn spans(pat: &str, text: &str, from: i64, anchored: bool) -> Option<Vec<i64>> {
            let (id, _) = compile(pat).unwrap();
            search(id, text, from, anchored).unwrap()
        }

        #[test]
        fn lookaround_backrefs_and_named_groups() {
            assert_eq!(spans(r"(?<=\$)\w+(?=\b)", "pay $amount now", 0, false), Some(vec![5, 11]));
            assert_eq!(spans(r"(\w)\1", "abccd", 0, false), Some(vec![2, 4, 2, 3]));
            let (_, names) = compile(r"(?<key>\w+)=(\d+)").unwrap();
            assert_eq!(names, vec!["", "key", ""]);
            assert_eq!(spans(r"(a)|(b)", "b", 0, false), Some(vec![0, 1, -1, -1, 0, 1]));
        }

        #[test]
        fn g_anchor_and_match_at() {
            assert_eq!(spans(r"\Gab", "xxab ab", 2, false), Some(vec![2, 4]));
            assert_eq!(spans(r"\Gab", "xxab ab", 0, false), None);
            assert_eq!(spans(r"ab", "xxab ab", 0, true), None);
            assert_eq!(spans(r"ab", "xxab ab", 2, true), Some(vec![2, 4]));
            assert_eq!(spans(r"ab", "xxab ab", 3, false), Some(vec![5, 7]));
        }

        #[test]
        fn line_anchors_hex_and_possessive() {
            assert_eq!(spans(r"^b$", "a\nb\nc", 0, false), Some(vec![2, 3]));
            assert_eq!(spans(r"\h+", "zz1fG", 0, false), Some(vec![2, 4]));
            assert_eq!(spans(r"a++a", "aaa", 0, false), None);
            assert_eq!(spans(r"(?i:ab)c", "ABc", 0, false), Some(vec![0, 3]));
        }

        #[test]
        fn char_indices_not_bytes() {
            assert_eq!(spans(r"ñ+", "añññb", 0, false), Some(vec![1, 4]));
            assert_eq!(spans(r"b", "añññb", 4, true), Some(vec![4, 5]));
            assert_eq!(spans(r"b", "añññb", 9, false), None);
        }

        #[test]
        fn errors_are_values_and_handles_dedupe() {
            let err = compile(r"(?<=a+").unwrap_err();
            assert!(err.starts_with("regex: "), "{err}");
            assert!(compile(r"\1").is_err());
            let (a, _) = compile("dedupe").unwrap();
            let (b, _) = compile("dedupe").unwrap();
            assert_eq!(a, b);
            assert!(search(999_999, "x", 0, false).is_err());
        }
    }
}
