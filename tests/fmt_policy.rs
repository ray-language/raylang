//! El repo es un **punto fijo** de `ray fmt`: pasar el formateador a cualquier `.ray` versionado no
//! cambia ni un byte. Lo aseveramos aquí para que no vuelva a derivar (M104–M106 dejaron el repo
//! canónico; el arreglo de los comentarios que se movían de sitio cerró los dos últimos archivos).
//!
//! Se comprueban dos cosas distintas, y las dos importan:
//!
//! 1. **Canónico**: `format(fuente) == fuente`. Si falla, el archivo está sin formatear → `ray fmt
//!    --write <archivo>`. (Ese es el fallo esperado y barato: lo arregla el autor del cambio.)
//! 2. **Idempotente**: `format(format(fuente)) == format(fuente)`. Si esto falla y lo anterior no, el
//!    bug NO es del archivo sino del FORMATEADOR: no converge. La distinción se hace explícita en el
//!    mensaje porque son dos acciones muy distintas.
//!
//! El barrido va sobre los archivos **versionados** (`git ls-files`), no sobre el árbol: así los
//! artefactos de una compilación nativa o un `target/` con `.ray` copiados no entran a la prueba.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Los `.ray` versionados del repo, en orden estable.
fn tracked_ray_files(root: &Path) -> Vec<PathBuf> {
    let out = Command::new("git")
        .args(["ls-files", "-z", "*.ray"])
        .current_dir(root)
        .output()
        .expect("git ls-files");
    assert!(out.status.success(), "git ls-files falló: {}", String::from_utf8_lossy(&out.stderr));
    let mut files: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| root.join(s))
        .collect();
    files.sort();
    files
}

#[test]
fn every_tracked_ray_file_is_a_fixed_point_of_the_formatter() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = tracked_ray_files(root);
    assert!(files.len() > 100, "el barrido debería ver los ~270 .ray del repo, vio {}", files.len());

    let mut unformatted: Vec<String> = Vec::new();
    let mut divergent: Vec<String> = Vec::new();
    let mut unparsed: Vec<String> = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(root).unwrap_or(file).display().to_string();
        let source = std::fs::read_to_string(file).expect("lee el archivo");
        // Un `.ray` que no parsea no se puede formatear. No se salta en silencio: se reporta, porque
        // un archivo del repo que no compila es en sí mismo una señal (y si alguno debiera quedar
        // fuera de la prueba a propósito, la exclusión tiene que ser explícita, no un `continue`).
        let Ok(once) = raylang::fmt::format_source(&source) else {
            unparsed.push(rel);
            continue;
        };
        if once != source {
            unformatted.push(rel.clone());
        }
        match raylang::fmt::format_source(&once) {
            Ok(twice) if twice == once => {}
            _ => divergent.push(rel),
        }
    }

    assert!(
        divergent.is_empty(),
        "el FORMATEADOR no converge en {} archivo(s) — esto es un bug de src/fmt.rs, no del \
         archivo (formatear dos veces da algo distinto que formatear una):\n  {}",
        divergent.len(),
        divergent.join("\n  ")
    );
    assert!(
        unparsed.is_empty(),
        "{} archivo(s) .ray versionados no parsean:\n  {}",
        unparsed.len(),
        unparsed.join("\n  ")
    );
    assert!(
        unformatted.is_empty(),
        "{} archivo(s) .ray no están en forma canónica; arréglalo con \
         `ray fmt --write <archivo>`:\n  {}",
        unformatted.len(),
        unformatted.join("\n  ")
    );
}
