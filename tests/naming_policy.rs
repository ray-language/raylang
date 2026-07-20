//! La convención de nombres (CLAUDE.md §Convenciones) se auto-defiende: los
//! IDENTIFICADORES van en inglés en `src/`, `selfhost/`, `packages/` y
//! `benchmarks/` (incluidos los nombres de test dentro de `#[cfg(test)] mod
//! tests` y los snippets raylang embebidos ahí). **`tests/` (integración) queda
//! FUERA de la política** (decisión 20 jul 2026, ver `docs/arqueo-spanglish.md`):
//! sus nombres de test pueden estar en español. `examples/` y `book/` también
//! quedan fuera (código de usuario / material didáctico, más flexible).
//!
//! Detección pragmática: se extraen los identificadores DECLARADOS (`fn`,
//! `let [mut]`, `var`) de los `.rs`/`.ray`, se parten por `_` y se comparan
//! contra una wordlist curada de tokens en español
//! (`tests/naming_policy_es.txt`). Si aparece un token español nuevo que la
//! lista no cubre, añádelo a la lista. Una excepción deliberada se marca con
//! `// es-ok` en la misma línea.

use std::collections::HashSet;
use std::path::Path;

/// Directorios bajo la política. `tests/` queda excluido deliberadamente.
const EN_POLITICA: &[&str] = &["src", "selfhost", "packages", "benchmarks"];

/// Palabras clave que introducen una declaración con nombre.
const DECLARADORES: &[&str] = &["fn ", "let mut ", "let ", "var "];

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

/// Extrae el identificador que sigue a `pos` (letras minúsculas, dígitos, `_`).
fn ident_after(line: &str, pos: usize) -> &str {
    let rest = &line[pos..];
    let end = rest
        .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
        .unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn identifiers_are_english() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let wordlist = std::fs::read_to_string(root.join("tests/naming_policy_es.txt"))
        .expect("wordlist tests/naming_policy_es.txt");
    let spanish: HashSet<&str> = wordlist.lines().map(str::trim).filter(|l| !l.is_empty()).collect();

    let mut files = Vec::new();
    for dir in EN_POLITICA {
        collect_files(&root.join(dir), &mut files);
    }
    files.sort();

    let mut violations = Vec::new();
    for file in &files {
        let Ok(source) = std::fs::read_to_string(file) else { continue };
        let rel = file.strip_prefix(root).unwrap_or(file).display().to_string();
        for (i, line) in source.lines().enumerate() {
            if line.contains("es-ok") {
                continue; // excepción deliberada, documentada en el propio sitio
            }
            for kw in DECLARADORES {
                let mut from = 0;
                while let Some(off) = line[from..].find(kw) {
                    let at = from + off;
                    from = at + kw.len();
                    // Debe ser palabra completa (no `often `, `si let `→ok).
                    if at > 0 {
                        let prev = line.as_bytes()[at - 1];
                        if prev.is_ascii_alphanumeric() || prev == b'_' {
                            continue;
                        }
                    }
                    let name = ident_after(line, from);
                    if name.is_empty() {
                        continue;
                    }
                    if name.split('_').any(|tok| spanish.contains(tok)) {
                        violations.push(format!("{rel}:{}: {kw}{name}", i + 1));
                    }
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "los identificadores van en inglés (CLAUDE.md §Convenciones); tokens en español \
         detectados ({} sitios):\n{}",
        violations.len(),
        violations.join("\n")
    );
}
