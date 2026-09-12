//! M243: las aserciones de la gramática de Sublime (`editors/sublime/tests/syntax_test_raylang.ray`,
//! formato `// ^^^ scope` de Sublime Text) se **ejecutan** en Sublime (Build sobre el archivo) y en
//! el tokenizador de ray-sublime, que las suma a su corpus. Aquí no hay motor de Sublime, así que
//! el CI sostiene lo que sí puede: el archivo es un programa raylang válido (compila y corre con
//! salida 0: las aserciones son comentarios), su cabecera apunta a la sintaxis real, cada scope
//! que asevera existe en LAS DOS gramáticas (Sublime y VSCode prometen scopes idénticos), y toda
//! palabra clave del lexer está en la gramática.

use std::path::Path;
use std::process::Command;

const TEST_FILE: &str = "editors/sublime/tests/syntax_test_raylang.ray";
const SUBLIME: &str = "editors/sublime/raylang.sublime-syntax";
const VSCODE: &str = "editors/vscode/syntaxes/raylang.tmLanguage.json";

fn read(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// Los scopes aseverados: la última palabra de cada línea `// ^^^ scope` o `//<- scope`.
fn asserted_scopes(text: &str) -> Vec<String> {
    let mut scopes: Vec<String> = text
        .lines()
        .filter_map(|l| {
            let t = l.trim_start();
            let rest = t.strip_prefix("//")?;
            let rest = rest.trim_start();
            if rest.starts_with('^') || rest.starts_with("<-") {
                rest.split_whitespace().last().map(str::to_string)
            } else {
                None
            }
        })
        .collect();
    scopes.sort();
    scopes.dedup();
    scopes
}

#[test]
fn syntax_test_file_is_a_valid_program_with_the_right_header() {
    let text = read(TEST_FILE);
    let first = text.lines().next().unwrap_or("");
    assert_eq!(first, "// SYNTAX TEST \"Packages/raylang/raylang.sublime-syntax\"", "cabecera de Sublime");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", TEST_FILE]).current_dir(root).output().unwrap();
    assert!(
        out.status.success(),
        "el archivo de aserciones debe compilar y correr:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn every_asserted_scope_exists_in_both_grammars() {
    let scopes = asserted_scopes(&read(TEST_FILE));
    assert!(scopes.len() >= 20, "pocas aserciones distintas: {scopes:?}");
    let sublime = read(SUBLIME);
    let vscode = read(VSCODE);
    let missing: Vec<&String> = scopes.iter().filter(|s| !sublime.contains(s.as_str()) || !vscode.contains(s.as_str())).collect();
    assert!(missing.is_empty(), "scopes aseverados que no están en las dos gramáticas: {missing:?}");
}

#[test]
fn every_lexer_keyword_is_in_the_sublime_grammar() {
    let sublime = read(SUBLIME);
    // Las palabras clave del lexer (src/lexer.rs) y los nombres de tipo primitivo.
    let lexer = read("src/lexer.rs");
    let mut keywords: Vec<&str> = lexer
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            let (word, rest) = t.split_once(" => TokenKind::")?;
            let word = word.strip_prefix('"')?.strip_suffix('"')?;
            // Solo el mapa de palabras reservadas: `"let" => TokenKind::Let`, no los caracteres.
            if rest.chars().next().is_some_and(|c| c.is_ascii_uppercase()) { Some(word) } else { None }
        })
        .collect();
    keywords.sort();
    keywords.dedup();
    assert!(keywords.len() >= 30, "el mapa de palabras clave del lexer cambió de forma: {keywords:?}");
    let missing: Vec<&&str> = keywords.iter().filter(|k| !sublime.contains(&format!("{k}|")) && !sublime.contains(&format!("|{k})")) && !sublime.contains(&format!("({k})")) && !sublime.contains(&format!("({k}|"))).collect();
    assert!(missing.is_empty(), "palabras clave del lexer sin regla en la gramática de Sublime: {missing:?}");
}

/// M244 (ray-sublime #75): la lista `builtins` de las gramáticas es UNA y contiene todos los
/// builtins libres que documenta `llms.txt` (sección "Free global builtins" + concurrencia);
/// la regla de pipeline de VSCode reusa la misma alternancia.
#[test]
fn the_builtin_list_matches_llms_txt_in_both_grammars() {
    let llms = read("llms.txt");
    let start = llms.find("**Free global builtins**").expect("sección de builtins libres en llms.txt");
    let section = &llms[start..];
    let end = section.find("\n2. ").expect("fin de la sección");
    let section = &section[..end];
    let mut free: Vec<&str> = section
        .split('`')
        .skip(1)
        .step_by(2)
        .flat_map(|chunk| chunk.split_whitespace())
        .filter(|w| w.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        .collect();
    free.sort();
    free.dedup();
    assert!(free.len() >= 25, "llms.txt cambió de forma: {free:?}");
    let sublime = read(SUBLIME);
    let sublime_list = sublime
        .lines()
        .find_map(|l| l.trim().strip_prefix("builtins: '").map(|r| r.trim_end_matches('\'').to_string()))
        .expect("variable builtins en la gramática de Sublime");
    let vscode = read(VSCODE);
    let vscode_lists = vscode.matches(sublime_list.as_str()).count();
    assert_eq!(vscode_lists, 2, "VSCode debe usar la MISMA alternancia en builtins y pipeline-target");
    let names: Vec<&str> = sublime_list.split('|').collect();
    let missing: Vec<&&str> = free.iter().filter(|f| !names.contains(f)).collect();
    assert!(missing.is_empty(), "builtins libres de llms.txt ausentes en la gramática: {missing:?}");
}
