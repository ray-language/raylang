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
fn escribe_tmp(name: &str, content: &str) -> String {
    let mut p = std::env::temp_dir();
    p.push(format!("rayfmt_{}_{}", std::process::id(), name));
    let mut f = std::fs::File::create(&p).expect("crea temporal");
    f.write_all(content.as_bytes()).expect("escribe temporal");
    p.to_str().unwrap().to_string()
}

/// La lista de ejemplos sobre los que se exige idempotencia. Cubre control de flujo, datos, genéricos,
/// traits/impls, match, closures y la propia librería de regex (single-file, sin imports).
const EJEMPLOS: &[&str] = &[
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
fn formatea_sin_error() {
    for rel in EJEMPLOS {
        let (_, ok) = fmt(&repo(rel));
        assert!(ok, "--fmt falló en {}", rel);
    }
}

#[test]
fn es_idempotente() {
    for rel in EJEMPLOS {
        let (once, ok1) = fmt(&repo(rel));
        assert!(ok1, "--fmt falló en {}", rel);
        let tmp = escribe_tmp(&rel.replace('/', "_"), &once);
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
fn preserves_el_comportamiento() {
    for rel in ["examples/basics/fib.ray", "examples/basics/fizzbuzz.ray", "examples/basics/gcd.ray"] {
        let orig = repo(rel);
        let (formateado, ok) = fmt(&orig);
        assert!(ok, "--fmt falló en {}", rel);
        let tmp = escribe_tmp(&format!("run_{}", rel.replace('/', "_")), &formateado);

        for vm in [false, true] {
            let (o_out, o_code) = run(&orig, vm);
            let (f_out, f_code) = run(&tmp, vm);
            assert_eq!(o_out, f_out, "{} (vm={}): la output cambió al formatear", rel, vm);
            assert_eq!(o_code, f_code, "{} (vm={}): el código de output cambió al formatear", rel, vm);
        }
    }
}
