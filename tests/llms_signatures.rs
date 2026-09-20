//! M275 (raystream [17]): las firmas que `llms.txt` cita con tipo de retorno (`name(args) -> T`)
//! deben coincidir con `ray doc name` — la lista "Return types that surprise" existía justo para
//! evitar adivinar, y decía `char_from_code(n) -> char` cuando es `Option<char>`. Guarda de CI:
//! cada snippet con `->` en `llms.txt` se coteja contra la firma real.

use std::process::Command;

#[test]
fn every_signature_quoted_in_llms_txt_matches_ray_doc() {
    let text = std::fs::read_to_string(format!("{}/llms.txt", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let re = regex_lite(&text);
    assert!(!re.is_empty(), "llms.txt cita firmas con '->'");
    let mut bad = Vec::new();
    for (snippet, name, ret) in &re {
        let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["doc", name]).output().unwrap();
        let doc = String::from_utf8_lossy(&out.stdout).into_owned();
        let first = doc.lines().next().unwrap_or("").split("  [").next().unwrap_or("").to_string();
        // Un nombre SIN calificar que `ray doc` resuelve en un módulo (`std/json: parse(…)`) es
        // ambiguo (el snippet puede hablar de otro módulo): no se compara.
        if !snippet.contains('.') && first.starts_with("std/") {
            continue;
        }
        let Some(pos) = first.rfind(" -> ") else {
            bad.push(format!("{snippet}: `ray doc {name}` no da una firma ({first})"));
            continue;
        };
        let real = first[pos + 4..].trim().trim_end_matches(';').to_string();
        if normalize(&real) != normalize(ret) {
            bad.push(format!("{snippet}: llms.txt dice `{ret}`, ray doc dice `{real}`"));
        }
    }
    assert!(bad.is_empty(), "firmas de llms.txt fuera de sincronía con ray doc:\n  {}", bad.join("\n  "));
}

/// Compara el tipo citado con el real. `T`/`_` en llms.txt son comodines (se compara solo el
/// constructor); un prefijo de módulo (`process.Exit`) se ignora; los espacios no cuentan.
fn normalize(t: &str) -> String {
    let flat: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    let flat = strip_module_prefixes(&flat);
    match flat.find('<') {
        Some(i) if flat.contains('T') || flat.contains('_') => flat[..i].to_string(),
        _ => flat,
    }
}

fn strip_module_prefixes(t: &str) -> String {
    let mut out = String::new();
    let mut word = String::new();
    for c in t.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
            word.push(c);
        } else {
            out.push_str(word.rsplit('.').next().unwrap_or(""));
            word.clear();
            out.push(c);
        }
    }
    out.push_str(word.rsplit('.').next().unwrap_or(""));
    out
}

/// Extrae (snippet, nombre pelado, tipo de retorno) de cada firma `callee(args) -> T` dentro de los
/// fragmentos entre backticks de `llms.txt`. Un fragmento puede traer varias firmas seguidas
/// (`b.len() b.sub_bytes(i, j) -> bytes b.index_of(needle) -> Option<int>`): el tipo termina donde
/// empieza la siguiente llamada. El callee es el ÚLTIMO método antes del `->`
/// (`process.cmd(p, args).spawn_detached() -> …` → `spawn_detached`).
fn regex_lite(text: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    // Solo los snippets EN LÍNEA: los bloques ``` … ``` son código de ejemplo (definiciones `fn`).
    let inline: String = text.split("```").step_by(2).collect::<Vec<_>>().join(" ");
    for raw in inline.split('`').skip(1).step_by(2) {
        if raw.trim_start().starts_with("fn ") || raw.contains('{') {
            continue;
        }
        let mut rest = raw;
        while let Some(arrow) = rest.find(" -> ") {
            let head = &rest[..arrow];
            let after = &rest[arrow + 4..];
            // El tipo acaba en el próximo ` ident(` (otra firma) o al final del fragmento.
            let mut end = after.len();
            let bytes = after.as_bytes();
            let mut j = 0;
            while j < bytes.len() {
                if bytes[j] == b'(' {
                    let mut k = j;
                    while k > 0 && (bytes[k - 1].is_ascii_alphanumeric() || bytes[k - 1] == b'_' || bytes[k - 1] == b'.') { k -= 1; }
                    if k < j && k > 0 && bytes[k - 1].is_ascii_whitespace() { end = k; break; }
                }
                j += 1;
            }
            let ret = after[..end].trim().trim_end_matches(';').to_string();
            // callee: el identificador justo antes del último `(` del head.
            if let Some(paren) = head.rfind('(') {
                let ident_end = head[..paren].trim_end().len();
                let ident_start = head[..ident_end].rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map_or(0, |p| p + 1);
                let name = head[ident_start..ident_end].to_string();
                if !name.is_empty() && !name.chars().next().unwrap().is_ascii_uppercase() && head[..paren].contains(&name) {
                    let qualified = head[..ident_end].rfind(|c: char| c == '.').is_some();
                    out.push((format!("`{}`", head[ident_start..].trim().to_string() + " -> " + &ret), if qualified { name.clone() } else { name.clone() }, ret));
                    let _ = qualified;
                }
            }
            rest = &after[end..];
        }
    }
    out
}
