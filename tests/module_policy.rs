//! M134 — la BATERÍA DE ADMISIÓN de módulos: lo que un PR con un módulo nuevo (de `std/` o de
//! `packages/`) debe pasar para ser considerado para revisión (además de fmt_policy y
//! naming_policy, que ya vigilan forma canónica e identificadores en inglés). Corre SIEMPRE en
//! CI: también evita que la superficie existente degrade.
//!
//! Reglas (el porqué de cada una está en CONTRIBUTING.md):
//! 1. Toda la superficie pública (`pub fn/struct/enum/const`) lleva doc `///` (visible en
//!    LSP/raydoc; en INGLÉS por la política de superficie — el idioma lo revisa el humano).
//! 2. Todo módulo embebido de `std/` tiene su fila en REFERENCE.md (el catálogo es contrato).
//! 3. Todo módulo de `packages/` está referenciado por algún test de `tests/` (sin dogfood no
//!    hay admisión) y su paquete tiene `README.md` y `ray.toml` con nombre/versión.
//! 4. Los módulos de `std/` también están ejercitados por algún test.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Los `.ray` bajo `dir`, recursivo (sin fixtures ni ocultos).
fn ray_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "fixtures" {
            continue;
        }
        if p.is_dir() {
            ray_files(&p, out);
        } else if name.ends_with(".ray") {
            out.push(p);
        }
    }
}

/// Regla 1: cada `pub fn/struct/enum/const` precedido (saltando `//` y blancos) por un `///`.
#[test]
fn public_surface_is_documented() {
    let mut files = Vec::new();
    ray_files(&repo().join("std"), &mut files);
    ray_files(&repo().join("packages"), &mut files);
    let mut missing = Vec::new();
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        let lines: Vec<&str> = src.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            let t = l.trim_start();
            if !(t.starts_with("pub fn ")
                || t.starts_with("pub struct ")
                || t.starts_with("pub enum ")
                || t.starts_with("pub const "))
            {
                continue;
            }
            let mut j = i;
            let mut documented = false;
            while j > 0 {
                j -= 1;
                let prev = lines[j].trim_start();
                if prev.starts_with("///") {
                    documented = true;
                    break;
                }
                if prev.starts_with("//") || prev.is_empty() {
                    continue;
                }
                break;
            }
            if !documented {
                missing.push(format!("{}:{}: {}", f.strip_prefix(repo()).unwrap().display(), i + 1, t));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "superficie pública SIN doc `///` (regla 1 de admisión; la doc es la cara en LSP/raydoc):\n{}",
        missing.join("\n")
    );
}

/// Los módulos `std/*` embebidos de verdad (los declara `src/stdlib.rs`).
fn embedded_std_modules() -> Vec<String> {
    let src = std::fs::read_to_string(repo().join("src/stdlib.rs")).unwrap();
    let mut out = Vec::new();
    for l in src.lines() {
        // ("std/<nombre>", include_str!(...)) — solo las filas de la tabla real.
        if let Some(rest) = l.trim_start().strip_prefix("(\"std/")
            && let Some(name) = rest.split('"').next()
            && l.contains("include_str!")
        {
            out.push(name.to_string());
        }
    }
    assert!(out.len() > 20, "la tabla de stdlib.rs no parsea: {out:?}");
    out
}

/// Regla 2: todo módulo embebido tiene su fila en REFERENCE.md.
#[test]
fn every_std_module_is_in_reference() {
    let reference = std::fs::read_to_string(repo().join("REFERENCE.md")).unwrap();
    let missing: Vec<String> = embedded_std_modules()
        .into_iter()
        .filter(|m| !reference.contains(&format!("std/{m}")))
        .collect();
    assert!(
        missing.is_empty(),
        "módulos de std SIN fila en REFERENCE.md (regla 2: el catálogo es contrato): {missing:?}"
    );
}

/// El texto de todos los tests (para las reglas de dogfood 3/4).
fn all_tests_text() -> String {
    let mut out = String::new();
    for e in std::fs::read_dir(repo().join("tests")).unwrap().flatten() {
        if e.path().extension().is_some_and(|x| x == "rs") {
            out.push_str(&std::fs::read_to_string(e.path()).unwrap());
        }
    }
    out
}

/// Regla 3: cada módulo de `packages/` aparece en algún test, y su paquete tiene README + manifest.
#[test]
fn every_package_module_is_tested_and_packaged() {
    let tests = all_tests_text();
    let mut untested = Vec::new();
    for pkg in std::fs::read_dir(repo().join("packages")).unwrap().flatten() {
        let pdir = pkg.path();
        if !pdir.is_dir() {
            continue;
        }
        let pname = pkg.file_name().to_string_lossy().into_owned();
        assert!(pdir.join("README.md").is_file(), "packages/{pname} sin README.md (regla 3)");
        let manifest = std::fs::read_to_string(pdir.join("ray.toml"))
            .unwrap_or_else(|_| panic!("packages/{pname} sin ray.toml (regla 3)"));
        assert!(
            manifest.contains("name") && manifest.contains("version"),
            "packages/{pname}/ray.toml sin name/version"
        );
        let mut files = Vec::new();
        ray_files(&pdir, &mut files);
        for f in files {
            let leaf = f.file_stem().unwrap().to_string_lossy().into_owned();
            if !tests.contains(&leaf) {
                untested.push(format!("packages/{pname}/{leaf}.ray"));
            }
        }
    }
    assert!(
        untested.is_empty(),
        "módulos de packages sin NINGÚN test que los mencione (regla 3: sin dogfood no hay admisión):\n{}",
        untested.join("\n")
    );
}

/// Regla 4: cada módulo embebido de std está ejercitado por algún test.
#[test]
fn every_std_module_is_tested() {
    let tests = all_tests_text();
    let missing: Vec<String> = embedded_std_modules()
        .into_iter()
        .filter(|m| {
            let leaf = m.rsplit('/').next().unwrap();
            !tests.contains(leaf)
        })
        .collect();
    assert!(missing.is_empty(), "módulos de std sin test que los mencione (regla 4): {missing:?}");
}
