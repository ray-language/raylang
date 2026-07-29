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
