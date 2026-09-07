//! Política de las traducciones (M197): cada `X.en.md` (raíz y `docs/`) tiene su original `X.md`,
//! ambos enlazan al otro en su cabecera (selector de idioma), y la traducción lleva al pie un
//! marcador `<!-- sync: sha256:… -->` con el hash del original del que se tradujo. Si el original
//! cambia sin revisar la traducción, este test lo dice — las traducciones no se pudren en silencio.
//! Refrescar el marcador tras revisar: `python3 tools/docs_sync.py --update <archivo.en.md>`.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn translations() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in [repo_root(), repo_root().join("docs")] {
        for entry in std::fs::read_dir(&dir).expect("lee el directorio") {
            let p = entry.expect("entrada").path();
            if p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".en.md")) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn short_sha256(path: &Path) -> String {
    // SHA-256 del propio proyecto (el gestor de paquetes ya lo trae): sin dependencia nueva.
    let bytes = std::fs::read(path).expect("lee el original");
    let hex = raylang::sha256::sha256_hex(&bytes);
    hex[..12].to_string()
}

#[test]
fn every_translation_has_its_original_cross_linked_and_in_sync() {
    let mut problems = Vec::new();
    let list = translations();
    assert!(!list.is_empty(), "hay al menos README.en.md");
    for en in list {
        let rel = en.strip_prefix(repo_root()).unwrap().display().to_string();
        let original = en.with_file_name(en.file_name().unwrap().to_str().unwrap().replace(".en.md", ".md"));
        if !original.exists() {
            problems.push(format!("{rel}: no existe el original {}", original.file_name().unwrap().to_str().unwrap()));
            continue;
        }
        let en_text = std::fs::read_to_string(&en).unwrap();
        let original_text = std::fs::read_to_string(&original).unwrap();
        let en_name = en.file_name().unwrap().to_str().unwrap();
        let original_name = original.file_name().unwrap().to_str().unwrap();
        if !en_text.contains(&format!("]({original_name})")) {
            problems.push(format!("{rel}: falta el enlace de idioma al original ({original_name})"));
        }
        if !original_text.contains(&format!("]({en_name})")) {
            problems.push(format!("{}: falta el enlace de idioma a la traducción ({en_name})", original_name));
        }
        let want = short_sha256(&original);
        match en_text.find("<!-- sync: sha256:") {
            Some(i) => {
                let have = &en_text[i + "<!-- sync: sha256:".len()..i + "<!-- sync: sha256:".len() + 12];
                if have != want {
                    problems.push(format!(
                        "{rel}: DESFASE — el original cambió (marcador {have}, actual {want}); revisa la traducción y corre `python3 tools/docs_sync.py --update {rel}`"
                    ));
                }
            }
            None => problems.push(format!("{rel}: falta el marcador `<!-- sync: sha256:… -->` (corre `python3 tools/docs_sync.py --update {rel}`)")),
        }
    }
    assert!(problems.is_empty(), "traducciones fuera de política:\n{}", problems.join("\n"));
}
