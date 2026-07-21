//! La convención de nombres (CLAUDE.md §Convenciones) se auto-defiende: los
//! IDENTIFICADORES van en inglés en `src/`, `selfhost/`, `packages/`,
//! `benchmarks/`, `tools/` y `tests/` (integración) — incluidos los nombres de
//! test dentro de `#[cfg(test)] mod tests` y los snippets raylang embebidos ahí
//! (decisión 21 jul 2026, ver `docs/arqueo-spanglish.md`). `examples/` y `book/`
//! quedan fuera (código de usuario / material didáctico, más flexible).
//!
//! Detección pragmática (alineada con `tools/arqueo_spanglish.py`, el script que
//! hizo el arqueo completo — mismos declaradores, mismo enfoque): se extraen los
//! identificadores DECLARADOS (`fn`, `let [mut]`, `var`, `struct`, `enum`,
//! `trait`, `const`, `static`, parámetros y campos de struct) de los `.rs`/`.ray`,
//! se parten por `_`/CamelCase y se comparan contra una wordlist curada de
//! tokens en español (`tests/naming_policy_es.txt`). Antes de buscar, se
//! **enmascaran** comentarios de línea y el CONTENIDO de strings/raw-strings —
//! si no, un fixture raylang embebido como `r#"fn valor() {...}"#` o un mensaje
//! de assert como `"idempotente: {:?}"` se leerían como declaraciones reales.
//!
//! Si aparece un token español nuevo que la lista no cubre, añádelo a la lista.
//! Una excepción deliberada se marca con `// es-ok` en la misma línea (para
//! casos aislados); un **falso amigo** recurrente (palabra igual en inglés y
//! español, como "variable") va en `FALSOS_AMIGOS`.

use std::collections::HashSet;
use std::path::Path;

/// Directorios bajo la política.
const EN_POLITICA: &[&str] =
    &["src", "selfhost", "packages", "benchmarks", "std", "tools", "tests"];

/// Palabras clave simples que introducen una declaración con nombre justo detrás.
const DECLARADORES: &[&str] = &[
    "fn ", "let mut ", "let ", "var ", "struct ", "enum ", "trait ", "const ", "static ",
];

/// Falsos amigos: palabras que están en la wordlist española (porque lo son en general) pero
/// que en este repo aparecen casi siempre como la palabra INGLESA real (mismo deletreo o jerga
/// técnica aceptada) — `variable`/`indices`/`regen`… Residuo documentado en
/// `docs/arqueo-spanglish.md` §3; si un futuro identificador realmente necesita esa palabra en
/// español, usa `// es-ok` en su línea en vez de tocar esta lista.
const FALSOS_AMIGOS: &[&str] = &[
    "variable", "variables", "indices", "regen", "configurable", "bitops", "ancount", "saslname",
    "operators",
    // Falsos amigos ya excluidos por `tools/arqueo_spanglish.py` (mismo deletreo en inglés):
    "error", "errores", "total", "totals", "final", "temporal", "temporals", "color", "colors",
    "animal", "animales", "division", "modulo", "persona", "personas", "auxiliar", "auxiliares",
    "subtotal", "normal", "base",
];

fn collect_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_files(&p, out);
        } else if matches!(p.extension().and_then(|x| x.to_str()), Some("rs" | "ray")) {
            out.push(p);
        }
    }
}

/// Enmascara (reemplaza por espacios, preservando saltos de línea) los comentarios de línea y el
/// contenido de strings/raw-strings de `source`, para que el detector de declaraciones no lea
/// texto entre comillas como código real. Reconoce raw strings de Rust (`r#"…"#`, con cualquier
/// número de `#`) solo si `is_rust`; los `.ray` no los tienen.
fn mask(source: &str, is_rust: bool) -> String {
    let b = source.as_bytes();
    let n = b.len();
    let mut out = vec![b' '; n];
    for (i, &c) in b.iter().enumerate() {
        if c == b'\n' {
            out[i] = b'\n';
        }
    }
    let mut i = 0;
    while i < n {
        if b[i] == b'/' && i + 1 < n && b[i + 1] == b'/' {
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if is_rust && b[i] == b'r' {
            let mut j = i + 1;
            let mut hashes = 0usize;
            while j < n && b[j] == b'#' {
                hashes += 1;
                j += 1;
            }
            if j < n && b[j] == b'"' {
                j += 1;
                let close: Vec<u8> = std::iter::once(b'"').chain(std::iter::repeat(b'#').take(hashes)).collect();
                while j < n && !b[j..].starts_with(close.as_slice()) {
                    j += 1;
                }
                j = (j + close.len()).min(n);
                i = j;
                continue;
            }
        }
        if b[i] == b'"' {
            let mut j = i + 1;
            while j < n && b[j] != b'"' {
                if b[j] == b'\\' && j + 1 < n {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            j = (j + 1).min(n);
            i = j;
            continue;
        }
        out[i] = b[i];
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Extrae el identificador (letras, dígitos, `_`) que sigue a `pos`. Admite may/minúsculas para
/// cubrir tanto `fn snake_case` como `struct CamelCase`/`const ALL_CAPS`.
fn ident_after(line: &str, pos: usize) -> &str {
    let rest = &line[pos..];
    let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
    &rest[..end]
}

/// ¿Es `line[at]` un byte precedido por un carácter de identificador? (evita que `fn ` case
/// dentro de `often `, o que `let ` case dentro de `si let `→ok en realidad si let SÍ es válido,
/// pero evita falsos "medio-palabra" como `reflect `).
fn word_boundary_before(line: &str, at: usize) -> bool {
    at == 0 || {
        let prev = line.as_bytes()[at - 1];
        !(prev.is_ascii_alphanumeric() || prev == b'_')
    }
}

/// Sitios de declaración simple (`fn`/`let`/`var`/`struct`/`enum`/`trait`/`const`/`static`).
fn find_simple(line: &str, out: &mut Vec<String>) {
    for kw in DECLARADORES {
        let mut from = 0;
        while let Some(off) = line[from..].find(kw) {
            let at = from + off;
            from = at + kw.len();
            if !word_boundary_before(line, at) {
                continue;
            }
            let name = ident_after(line, from);
            if !name.is_empty() {
                out.push(name.to_string());
            }
        }
    }
}

/// Parámetros: `(nombre: Tipo` o `, nombre: Tipo` (con `mut` opcional). Heurística de posición,
/// igual que `tools/arqueo_spanglish.py`.
fn find_params(line: &str, out: &mut Vec<String>) {
    let bytes = line.as_bytes();
    for (i, &c) in bytes.iter().enumerate() {
        if c != b'(' && c != b',' {
            continue;
        }
        let mut p = i + 1;
        while p < bytes.len() && bytes[p] == b' ' {
            p += 1;
        }
        if line[p..].starts_with("mut ") {
            p += 4;
            while p < bytes.len() && bytes[p] == b' ' {
                p += 1;
            }
        }
        let name = ident_after(line, p);
        if name.is_empty() || !name.as_bytes()[0].is_ascii_lowercase() {
            continue;
        }
        let mut q = p + name.len();
        while q < bytes.len() && bytes[q] == b' ' {
            q += 1;
        }
        if q < bytes.len() && bytes[q] == b':' {
            out.push(name.to_string());
        }
    }
}

/// Campos de struct: la línea (tras recortar espacio inicial e ignorar un `pub ` opcional)
/// empieza con `nombre:`. Requiere indentación (para no casar con anotaciones de tipo sueltas).
fn find_fields(line: &str, out: &mut Vec<String>) {
    let trimmed = line.trim_start();
    if trimmed.len() == line.len() {
        return; // sin indentación → no es campo
    }
    let rest = trimmed.strip_prefix("pub ").unwrap_or(trimmed);
    let name = ident_after(rest, 0);
    if name.is_empty() || !name.as_bytes()[0].is_ascii_lowercase() {
        return;
    }
    let after = &rest[name.len()..];
    if after.trim_start().starts_with(':') {
        out.push(name.to_string());
    }
}

#[test]
fn identifiers_are_english() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let wordlist = std::fs::read_to_string(root.join("tests/naming_policy_es.txt"))
        .expect("wordlist tests/naming_policy_es.txt");
    let falsos_amigos: HashSet<&str> = FALSOS_AMIGOS.iter().copied().collect();
    let spanish: HashSet<&str> = wordlist
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !falsos_amigos.contains(l))
        .collect();

    let mut files = Vec::new();
    for dir in EN_POLITICA {
        collect_files(&root.join(dir), &mut files);
    }
    files.sort();

    let mut violations = Vec::new();
    for file in &files {
        let Ok(source) = std::fs::read_to_string(file) else { continue };
        let is_rust = file.extension().and_then(|x| x.to_str()) == Some("rs");
        let masked = mask(&source, is_rust);
        let rel = file.strip_prefix(root).unwrap_or(file).display().to_string();
        for (i, (raw_line, masked_line)) in source.lines().zip(masked.lines()).enumerate() {
            if raw_line.contains("es-ok") {
                continue; // excepción deliberada, documentada en el propio sitio
            }
            let mut names = Vec::new();
            find_simple(masked_line, &mut names);
            find_params(masked_line, &mut names);
            find_fields(masked_line, &mut names);
            for name in names {
                if name == "self" {
                    continue;
                }
                if name.split('_').any(|tok| spanish.contains(tok)) {
                    violations.push(format!("{rel}:{}: {name}", i + 1));
                }
            }
        }
    }
    violations.sort();
    violations.dedup();
    assert!(
        violations.is_empty(),
        "los identificadores van en inglés (CLAUDE.md §Convenciones); tokens en español \
         detectados ({} sitios):\n{}",
        violations.len(),
        violations.join("\n")
    );
}
