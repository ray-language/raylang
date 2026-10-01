//! Guarda del HANDBOOK (`handbook/`, docs/plan-handbook.md §B.6). El handbook promete que su
//! código compila y que sus caminos llevan a algún sitio; este test lo cumple:
//!
//! 1. **Cada bloque ```rust compila.** Por convención el código raylang se etiqueta `rust` (para
//!    que GitHub y el sitio lo coloreen). Cada bloque se escribe a un archivo temporal y pasa por
//!    `ray check`. Dos marcas invisibles (comentarios HTML en la línea anterior al bloque) cambian
//!    eso: `<!-- check: skip (motivo) -->` lo salta (fragmentos de varios archivos, código que
//!    necesita dependencias) y `<!-- check: project=examples/apps/x -->` exige que el bloque
//!    aparezca TAL CUAL en un `.ray` de ese proyecto y que el proyecto compile — el ejemplo es la
//!    verdad, y el capítulo lo cita.
//! 2. **Ningún bloque `raylang`.** La etiqueta no colorea en ninguna parte todavía.
//! 3. **Enlaces relativos que existen** (relativos a `handbook/`; el anclaje no se comprueba).
//! 4. **Índice completo y bilingüe.** Todo capítulo está enlazado desde `index.md`, y cada `.md`
//!    tiene su `.en.md` (la sincronía de las traducciones la vigila `docs_i18n`).
//! 5. **Sin historia interna.** Ningún M-número de arco (`M123`) fuera del código: el handbook se
//!    lee en presente; la historia vive en CHANGELOG y DESIGN.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn handbook_dir() -> PathBuf {
    repo_root().join("handbook")
}

/// Los `.md` del handbook (ambos idiomas), ordenados.
fn handbook_docs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(handbook_dir())
        .expect("could not read handbook/")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    out.sort();
    out
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

/// Un bloque de código cercado: su etiqueta, su texto, la línea donde abre y la marca previa.
struct Block {
    lang: String,
    body: String,
    line: usize,
    mark: Option<String>,
}

fn fenced_blocks(text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        if let Some(info) = t.strip_prefix("```") {
            let lang = info.trim().to_string();
            // la marca: la línea no vacía inmediatamente anterior, si es un comentario `check:`
            let mark = lines[..i]
                .iter()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim())
                .filter(|l| l.starts_with("<!-- check:"))
                .map(|l| l.trim_start_matches("<!-- check:").trim_end_matches("-->").trim().to_string());
            let start = i;
            let mut body = String::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                body.push_str(lines[i]);
                body.push('\n');
                i += 1;
            }
            out.push(Block { lang, body, line: start + 1, mark });
        }
        i += 1;
    }
    out
}

/// El texto sin código (bloques cercados y spans de backticks), para buscar enlaces y M-números.
fn without_code(text: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            out.push('\n');
            continue;
        }
        if fenced {
            out.push('\n');
            continue;
        }
        let mut in_code = false;
        for c in line.chars() {
            if c == '`' {
                in_code = !in_code;
            } else if !in_code {
                out.push(c);
            }
        }
        out.push('\n');
    }
    out
}

fn link_targets(text: &str) -> Vec<String> {
    let clean = without_code(text);
    let mut out = Vec::new();
    for part in clean.split("](").skip(1) {
        if let Some(end) = part.find(')') {
            out.push(part[..end].trim().to_string());
        }
    }
    out
}

fn is_local(target: &str) -> bool {
    !(target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.starts_with('#')
        || target.is_empty())
}

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("raylang_handbook_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("could not create the scratch directory");
    dir
}

fn ray_check(path: &Path) -> Result<(), String> {
    let out = Command::new(BIN).arg("check").arg(path).output().expect("could not run ray check");
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// Los `.ray` de un directorio, recursivo (para buscar el bloque citado de un proyecto).
fn ray_sources(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            let name = file_name(&p);
            if name != ".ray-deps" && name != "node_modules" && name != "target" {
                ray_sources(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "ray") {
            out.push(std::fs::read_to_string(&p).unwrap_or_default());
        }
    }
}

#[test]
fn every_rust_block_compiles() {
    let scratch = scratch_dir("snippets");
    let mut failures = Vec::new();
    let mut checked = 0;
    let mut projects: BTreeMap<String, Result<(), String>> = BTreeMap::new();

    for doc in handbook_docs() {
        let name = file_name(&doc);
        let text = std::fs::read_to_string(&doc).expect("could not read the chapter");
        for (n, block) in fenced_blocks(&text).into_iter().enumerate() {
            if block.lang == "raylang" {
                failures.push(format!(
                    "{name}:{}: etiqueta el bloque como ```rust (convención del handbook)",
                    block.line
                ));
                continue;
            }
            if block.lang != "rust" {
                continue;
            }
            match block.mark.as_deref() {
                Some(m) if m.starts_with("skip") => continue,
                Some(m) if m.starts_with("project=") => {
                    let dir = m.trim_start_matches("project=").trim().to_string();
                    let abs = repo_root().join(&dir);
                    let status = projects
                        .entry(dir.clone())
                        .or_insert_with(|| ray_check(&abs))
                        .clone();
                    if let Err(e) = status {
                        failures.push(format!("{name}:{}: el proyecto {dir} no compila:\n{e}", block.line));
                        continue;
                    }
                    let mut sources = Vec::new();
                    ray_sources(&abs, &mut sources);
                    let wanted = block.body.trim();
                    if !sources.iter().any(|s| s.contains(wanted)) {
                        failures.push(format!(
                            "{name}:{}: el bloque no aparece tal cual en ningún .ray de {dir}",
                            block.line
                        ));
                    }
                    checked += 1;
                }
                Some(other) => failures.push(format!(
                    "{name}:{}: marca desconocida `check: {other}` (usa `skip (motivo)` o `project=<dir>`)",
                    block.line
                )),
                None => {
                    let file = scratch.join(format!("{}_{n}.ray", name.replace('.', "_")));
                    std::fs::write(&file, &block.body).expect("could not write the snippet");
                    if let Err(e) = ray_check(&file) {
                        failures.push(format!("{name}:{}: no compila:\n{e}", block.line));
                    }
                    checked += 1;
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    assert!(checked > 0, "el handbook debería tener bloques ```rust que comprobar");
    assert!(failures.is_empty(), "bloques del handbook con problemas:\n  {}", failures.join("\n  "));
}

#[test]
fn handbook_links_resolve() {
    let mut broken = Vec::new();
    for doc in handbook_docs() {
        let text = std::fs::read_to_string(&doc).unwrap();
        for target in link_targets(&text) {
            if !is_local(&target) {
                continue;
            }
            let path = target.split('#').next().unwrap_or("").trim();
            if path.is_empty() {
                continue;
            }
            if !handbook_dir().join(path).exists() {
                broken.push(format!("{} → {target}", file_name(&doc)));
            }
        }
    }
    assert!(broken.is_empty(), "enlaces rotos en el handbook:\n  {}", broken.join("\n  "));
}

#[test]
fn every_chapter_is_indexed_and_translated() {
    let index = std::fs::read_to_string(handbook_dir().join("index.md")).expect("falta handbook/index.md");
    let linked: Vec<String> = link_targets(&index);
    let mut problems = Vec::new();
    for doc in handbook_docs() {
        let name = file_name(&doc);
        if name.ends_with(".en.md") {
            continue;
        }
        let en = name.replace(".md", ".en.md");
        if !handbook_dir().join(&en).exists() {
            problems.push(format!("{name}: falta la traducción {en}"));
        }
        if name != "index.md" && !linked.iter().any(|t| t == &name) {
            problems.push(format!("{name}: no está enlazado desde handbook/index.md"));
        }
    }
    assert!(problems.is_empty(), "índice del handbook incompleto:\n  {}", problems.join("\n  "));
}

#[test]
fn no_internal_milestone_numbers() {
    let mut hits = Vec::new();
    for doc in handbook_docs() {
        let text = without_code(&std::fs::read_to_string(&doc).unwrap());
        for (n, line) in text.lines().enumerate() {
            let chars: Vec<char> = line.chars().collect();
            for i in 0..chars.len() {
                let boundary_before = i == 0 || !chars[i - 1].is_alphanumeric();
                if chars[i] != 'M' || !boundary_before {
                    continue;
                }
                let digits = chars[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
                let after = chars.get(i + 1 + digits);
                let boundary_after = after.is_none_or(|c| !c.is_alphanumeric());
                if (2..=3).contains(&digits) && boundary_after {
                    hits.push(format!("{}:{}: {}", file_name(&doc), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "el handbook no cita arcos internos (M-números); di lo que hace hoy:\n  {}",
        hits.join("\n  ")
    );
}

#[test]
fn the_helpers_read_what_they_should() {
    let blocks = fenced_blocks("x\n<!-- check: skip (dos archivos) -->\n```rust\nfn a() {}\n```\n```sh\nls\n```\n");
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].lang, "rust");
    assert_eq!(blocks[0].mark.as_deref(), Some("skip (dos archivos)"));
    assert_eq!(blocks[0].body, "fn a() {}\n");
    assert_eq!(blocks[1].mark, None);
    assert_eq!(link_targets("ver [a](x.md) y `[b](no.md)`"), vec!["x.md"]);
}
