//! Pruebas del formateador `rayfmt` (M29.2, `raylang --fmt`). Dos garantías:
//! (1) **idempotencia** — `fmt(fmt(x)) == fmt(x)` sobre una batería de ejemplos reales;
//! (2) **preservación de comportamiento** — un programa formateado produce la MISMA salida que el
//!     original (en ambos motores), es decir el formateo no cambia la semántica.

use std::io::Write;
use std::process::Command;

fn repo(rel: &str) -> String {
    format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel)
}

/// Ejecuta `raylang --fmt <path>` y devuelve (stdout, ok).
fn fmt(path: &str) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--fmt")
        .arg(path)
        .output()
        .expect("ejecuta --fmt");
    (String::from_utf8_lossy(&out.stdout).to_string(), out.status.success())
}

/// Escribe `content` a un temporal con nombre único y devuelve su ruta.
fn write_tmp(name: &str, content: &str) -> String {
    let mut p = std::env::temp_dir();
    p.push(format!("rayfmt_{}_{}", std::process::id(), name));
    let mut f = std::fs::File::create(&p).expect("crea temporal");
    f.write_all(content.as_bytes()).expect("escribe temporal");
    p.to_str().unwrap().to_string()
}

/// La lista de ejemplos sobre los que se exige idempotencia. Cubre control de flujo, datos, genéricos,
/// traits/impls, match, closures y la propia librería de regex (single-file, sin imports).
const EXAMPLES: &[&str] = &[
    "examples/basics/fib.ray",
    "examples/basics/fizzbuzz.ray",
    "examples/basics/gcd.ray",
    "examples/data/enums.ray",
    "examples/data/match_figuras.ray",
    "examples/data/arrays.ray",
    "examples/types/genericos.ray",
    "examples/types/traits.ray",
    "examples/types/impls_genericos.ray",
    "examples/types/trait_objects.ray",
    "examples/stdlib/closures.ray",
    "examples/stdlib/ufcs.ray",
    "examples/stdlib/stdlib.ray",
    "examples/stdlib/regex.ray",
];

#[test]
fn formats_without_error() {
    for rel in EXAMPLES {
        let (_, ok) = fmt(&repo(rel));
        assert!(ok, "--fmt falló en {}", rel);
    }
}

#[test]
fn is_idempotent() {
    for rel in EXAMPLES {
        let (once, ok1) = fmt(&repo(rel));
        assert!(ok1, "--fmt falló en {}", rel);
        let tmp = write_tmp(&rel.replace('/', "_"), &once);
        let (twice, ok2) = fmt(&tmp);
        assert!(ok2, "--fmt (2a pasada) falló en {}", rel);
        assert_eq!(once, twice, "el formateo no es idempotente en {}", rel);
    }
}

/// Ejecuta un `.ray` y devuelve (stdout, código de salida) con el motor indicado.
fn run(path: &str, vm: bool) -> (String, i32) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    if vm {
        cmd.arg("--vm");
    }
    let out = cmd.arg(path).output().expect("ejecuta program");
    (String::from_utf8_lossy(&out.stdout).to_string(), out.status.code().unwrap_or(-1))
}

/// El formateo preserva el comportamiento: original y formateado dan la misma salida y código de salida,
/// en ambos motores. Se prueba con programas autocontenidos (sin imports).
#[test]
fn preserves_behavior() {
    for rel in ["examples/basics/fib.ray", "examples/basics/fizzbuzz.ray", "examples/basics/gcd.ray"] {
        let orig = repo(rel);
        let (formateado, ok) = fmt(&orig);
        assert!(ok, "--fmt falló en {}", rel);
        let tmp = write_tmp(&format!("run_{}", rel.replace('/', "_")), &formateado);

        for vm in [false, true] {
            let (o_out, o_code) = run(&orig, vm);
            let (f_out, f_code) = run(&tmp, vm);
            assert_eq!(o_out, f_out, "{} (vm={}): la output cambió al formatear", rel, vm);
            assert_eq!(o_code, f_code, "{} (vm={}): el código de output cambió al formatear", rel, vm);
        }
    }
}

/// M105 — `ray fmt --write` reescribe EN EL SITIO. Tres garantías: reescribe lo que no es canónico,
/// **no toca** lo que ya lo es (ni el mtime), y admite varios archivos en una invocación.
#[test]
fn write_rewrites_in_place_and_leaves_canonical_files_alone() {
    let messy = write_tmp("write_messy.ray", "fn main(){print(1);}\n");
    let canonical = write_tmp("write_canonical.ray", "fn main() {\n    print(2);\n}\n");
    let before = std::fs::metadata(&canonical).unwrap().modified().unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["fmt", "--write", &messy, &canonical])
        .output()
        .expect("ejecuta fmt --write");
    assert!(out.status.success(), "fmt --write sale 0");
    let report = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(report.contains("write_messy.ray"), "reporta el reescrito: {report}");
    assert!(!report.contains("write_canonical.ray"), "no reporta el que ya era canónico: {report}");

    assert_eq!(
        std::fs::read_to_string(&messy).unwrap(),
        "fn main() {\n    print(1);\n}\n",
        "el archivo quedó formateado en el sitio"
    );
    assert_eq!(
        std::fs::metadata(&canonical).unwrap().modified().unwrap(),
        before,
        "el canónico no se reescribe (mtime intacto)"
    );

    // Segunda pasada: ya no hay nada que hacer.
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["fmt", "--write", &messy])
        .output()
        .expect("ejecuta fmt --write");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("already formatted"),
        "la segunda pasada no cambia nada"
    );
}

/// Sin `--write`, varios archivos es un error de USO (la salida a stdout de varios se solaparía).
#[test]
fn several_files_without_write_is_a_usage_error() {
    let a = write_tmp("several_a.ray", "fn main() { print(1); }\n");
    let b = write_tmp("several_b.ray", "fn main() { print(2); }\n");
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["fmt", &a, &b])
        .output()
        .expect("ejecuta fmt");
    assert_eq!(out.status.code(), Some(64), "código de error de uso");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--write"),
        "el mensaje señala --write: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
