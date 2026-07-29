//! Guarda de la documentación de la RAÍZ (auditoría de jul 2026): lo que se puede verificar de
//! forma mecánica, se verifica — para que la deriva entre los documentos y el binario se note en
//! CI el día que aparece, no meses después.
//!
//! Tres comprobaciones, deliberadamente conservadoras (ningún falso positivo vale la pena):
//!
//! 1. **Enlaces relativos que existen.** Todo `[texto](ruta)` de un `.md` de la raíz apunta a un
//!    archivo o directorio real. Se ignoran los enlaces externos (`http(s)`, `mailto:`) y los
//!    anclajes puros (`#seccion`); de un enlace con anclaje se comprueba solo el archivo (el
//!    *slug* de un título con acentos depende del renderizador, así que verificarlo aquí daría
//!    falsos negativos).
//! 2. **Ningún documento huérfano.** Todo `.md` de la raíz está enlazado desde el README —
//!    excepto el propio README y `CLAUDE.md` (instrucciones del agente, no documentación de
//!    usuario).
//! 3. **El catálogo del CLI coincide con el binario.** Los subcomandos que lista `REFERENCE.md`
//!    §14 son exactamente los que imprime `ray help`. Es la tabla que más se desactualizó
//!    históricamente (le faltaban `mcp`, `--fast`, `--target`…), y es trivial de comprobar.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// El binario de producto; `ray` es el mismo con otro nombre.
const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// Documentos de la raíz que NO tienen por qué estar enlazados desde el README.
const NOT_LINKED_FROM_README: &[&str] = &["README.md", "CLAUDE.md"];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Los `.md` de la raíz del repositorio, ordenados.
fn root_docs() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(repo_root())
        .expect("could not read the repository root")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".md"))
        .collect();
    out.sort();
    out
}

/// El texto sin código: fuera los bloques cercados por ``` y el contenido de los `spans` de
/// backticks. Sin esto, una firma como `serve_shutdown[_limits](…, stop, …)` se leería como un
/// enlace roto: el `](` de un ejemplo de código no es un enlace.
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
        for ch in line.chars() {
            if ch == '`' {
                in_code = !in_code;
                continue;
            }
            out.push(if in_code { ' ' } else { ch });
        }
        out.push('\n');
    }
    out
}

/// Los destinos de los enlaces Markdown `](destino)` de un texto (ignorando el código).
fn link_targets(text: &str) -> Vec<String> {
    let text = without_code(text);
    let mut out = Vec::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == ']' && bytes[i + 1] == '(' {
            let mut j = i + 2;
            let mut target = String::new();
            // Un destino no cruza líneas ni admite paréntesis anidados: si aparece alguno de los
            // dos, no es un enlace bien formado y se descarta en silencio.
            while j < bytes.len() && bytes[j] != ')' && bytes[j] != '\n' && bytes[j] != '(' {
                target.push(bytes[j]);
                j += 1;
            }
            if j < bytes.len() && bytes[j] == ')' && !target.is_empty() {
                out.push(target);
            }
            i = j;
        }
        i += 1;
    }
    out
}

/// ¿El destino apunta a otro sitio de este repositorio (y por tanto hay que comprobarlo)?
fn is_local_path(target: &str) -> bool {
    let t = target.trim();
    !(t.starts_with("http://")
        || t.starts_with("https://")
        || t.starts_with("mailto:")
        || t.starts_with('#')
        || t.is_empty())
}

/// El destino sin su anclaje (`REFERENCE.md#14-el-cli-ray` → `REFERENCE.md`).
fn path_part(target: &str) -> &str {
    target.split('#').next().unwrap_or(target).trim()
}

#[test]
fn root_doc_links_resolve() {
    let root = repo_root();
    let mut broken: Vec<String> = Vec::new();

    for doc in root_docs() {
        let text = std::fs::read_to_string(root.join(&doc)).expect("could not read the document");
        for target in link_targets(&text) {
            if !is_local_path(&target) {
                continue;
            }
            let path = path_part(&target);
            if path.is_empty() {
                continue; // enlace de solo anclaje escrito como `](#x)`, ya filtrado
            }
            if !root.join(path).exists() {
                broken.push(format!("{doc} → {target}"));
            }
        }
    }

    assert!(
        broken.is_empty(),
        "hay enlaces relativos rotos en la documentación de la raíz:\n  {}",
        broken.join("\n  ")
    );
}

#[test]
fn every_root_doc_is_linked_from_the_readme() {
    let root = repo_root();
    let readme = std::fs::read_to_string(root.join("README.md")).expect("could not read README.md");
    let linked: BTreeSet<String> = link_targets(&readme)
        .iter()
        .filter(|t| is_local_path(t))
        .map(|t| path_part(t).trim_end_matches('/').to_string())
        .collect();

    let orphans: Vec<String> = root_docs()
        .into_iter()
        .filter(|d| !NOT_LINKED_FROM_README.contains(&d.as_str()))
        .filter(|d| !linked.contains(d))
        .collect();

    assert!(
        orphans.is_empty(),
        "estos documentos de la raíz no están enlazados desde el README (añádelos a su tabla de \
         documentación, o a NOT_LINKED_FROM_README si es deliberado): {orphans:?}"
    );
}

/// Los subcomandos que anuncia `ray help`. Del bloque de comandos, cada línea indentada empieza
/// por el nombre; `registry` los agrupa, así que su subcomando cuenta como parte del nombre.
fn subcommands_from_help(help: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in help.lines() {
        let Some(rest) = line.strip_prefix("  ") else { continue };
        if rest.starts_with(' ') || rest.is_empty() {
            continue; // continuación de una descripción, no una entrada
        }
        let mut words = rest.split_whitespace();
        let Some(first) = words.next() else { continue };
        if !first.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
            continue; // "Usage:", "<subcommand>"… no son nombres de comando
        }
        if first == "registry" {
            if let Some(second) = words.next() {
                out.insert(format!("registry {second}"));
            }
            continue;
        }
        out.insert(first.to_string());
    }
    out
}

/// Los subcomandos que documenta la tabla del CLI de `REFERENCE.md` (§14): filas `| \`ray x …\` |`.
fn subcommands_from_reference(reference: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in reference.lines() {
        let Some(rest) = line.strip_prefix("| `ray ") else { continue };
        let Some(cmd) = rest.split('`').next() else { continue };
        let mut words = cmd.split_whitespace();
        let Some(first) = words.next() else { continue };
        if first == "registry" {
            if let Some(second) = words.next() {
                out.insert(format!("registry {second}"));
            }
            continue;
        }
        out.insert(first.to_string());
    }
    out
}

#[test]
fn reference_cli_table_matches_the_binary() {
    let help = std::process::Command::new(BIN)
        .arg("help")
        .output()
        .expect("could not run the binary");
    let help = String::from_utf8_lossy(&help.stdout).into_owned();
    let announced = subcommands_from_help(&help);
    assert!(
        announced.len() > 10,
        "no se pudo leer la ayuda del binario (solo se reconocieron {announced:?})"
    );

    let reference = std::fs::read_to_string(repo_root().join("REFERENCE.md"))
        .expect("could not read REFERENCE.md");
    let documented = subcommands_from_reference(&reference);

    let missing: Vec<&String> = announced.difference(&documented).collect();
    let extra: Vec<&String> = documented.difference(&announced).collect();

    assert!(
        missing.is_empty(),
        "el binario ofrece subcomandos que REFERENCE.md §14 no documenta: {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "REFERENCE.md §14 documenta subcomandos que el binario ya no ofrece: {extra:?}"
    );
}

#[test]
fn the_helpers_read_what_they_should() {
    // Guarda de la propia guarda: si el extractor de enlaces se rompe, los tests de arriba
    // pasarían en vacío sin que nadie se entere.
    let targets = link_targets("ver [la spec](SPEC.md) y [esto](https://x.dev) y [ancla](#s)");
    assert_eq!(targets, vec!["SPEC.md", "https://x.dev", "#s"]);
    // El código no contiene enlaces: ni un span de backticks ni un bloque cercado.
    assert!(link_targets("firma: `serve_shutdown[_limits](…, stop, …)`").is_empty());
    assert!(link_targets("```\nver [x](no-existe.md)\n```\n").is_empty());
    assert!(is_local_path("SPEC.md") && !is_local_path("https://x.dev") && !is_local_path("#s"));
    assert_eq!(path_part("REFERENCE.md#14-el-cli-ray"), "REFERENCE.md");

    let help = "Project:\n  new <name>        create\n  registry yank <n>  yank\n    continuation\n";
    let subs = subcommands_from_help(help);
    assert!(subs.contains("new") && subs.contains("registry yank") && subs.len() == 2);

    let reference = "| `ray doc <archivo>` | doc |\n| `ray registry keygen [--out F]` | k |\n";
    let docd = subcommands_from_reference(reference);
    assert!(docd.contains("doc") && docd.contains("registry keygen") && docd.len() == 2);
}

/// El README es la portada: que no se quede sin las piezas que sí sabemos comprobar.
#[test]
fn readme_documents_the_contract_docs() {
    let readme = std::fs::read_to_string(repo_root().join("README.md")).expect("no README.md");
    for must in ["SPEC.md", "SECURITY.md", "CHANGELOG.md"] {
        assert!(
            readme.contains(must),
            "el README no menciona {must}, que es uno de los documentos-contrato"
        );
    }
}
