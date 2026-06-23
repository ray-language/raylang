//! Pruebas de módulos (M11.3a) sobre el binario: escriben varios archivos `.ray` en un
//! directorio temporal, ejecutan `raylang <entrada>` y comprueban la salida / código de salida.
//! Los módulos viven en archivos separados, así que se prueban de verdad por subproceso.

use std::io::Write;
use std::process::Command;

/// Crea un directorio temporal único, escribe los `(nombre, fuente)` dados, ejecuta
/// `raylang [--vm] <dir>/<entry>.ray` y devuelve `(stdout, código)`.
fn run_modules(dir: &str, entry: &str, files: &[(&str, &str)], vm: bool) -> (String, i32) {
    let mut base = std::env::temp_dir();
    base.push(dir);
    std::fs::create_dir_all(&base).expect("crea el dir temporal");
    for (name, src) in files {
        let mut path = base.clone();
        path.push(format!("{name}.ray"));
        let mut f = std::fs::File::create(&path).expect("crea el módulo");
        f.write_all(src.as_bytes()).expect("escribe el módulo");
    }
    let mut entry_path = base.clone();
    entry_path.push(format!("{entry}.ray"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    if vm {
        cmd.arg("--vm");
    }
    let out = cmd.arg(&entry_path).output().expect("ejecuta el binario");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn import_calificado_en_ambos_motores() {
    let files = &[
        ("mates", "pub fn doble(n: int) -> int { n + n }\nfn secreto() -> int { 99 }\n"),
        ("app", "import mates;\nfn doble(x: int) -> int { x + 1 }\nfn main() -> int { print(mates.doble(10)); print(doble(10)); mates.doble(21) }\n"),
    ];
    for vm in [false, true] {
        let (out, code) = run_modules("ray_mod_basico", "app", files, vm);
        assert!(out.contains("20"), "mates.doble(10)=20 (vm={vm})\n{out}");
        assert!(out.contains("11"), "la local doble(10)=11 no colisiona (vm={vm})\n{out}");
        assert_eq!(code, 42, "exit = mates.doble(21) (vm={vm})");
    }
}

#[test]
fn import_transitivo() {
    let files = &[
        ("base", "pub fn uno() -> int { 1 }\n"),
        ("mid", "import base;\nfn interno(n: int) -> int { n + base.uno() }\npub fn cinco() -> int { interno(base.uno()) + interno(2) }\n"),
        ("top", "import mid;\nfn main() -> int { mid.cinco() + 37 }\n"),
    ];
    let (_, code) = run_modules("ray_mod_trans", "top", files, false);
    assert_eq!(code, 42, "5 + 37 = 42 (mid usa base, transitivo)");
}

#[test]
fn from_import_con_alias_en_ambos_motores() {
    // `from M import a as b` trae funciones `pub` al ámbito (sin calificar), con alias para
    // evitar colisiones con una función propia homónima (M11.3b).
    let files = &[
        ("mates", "pub fn doble(n: int) -> int { n + n }\npub fn triple(n: int) -> int { n + n + n }\n"),
        ("app", "from mates import doble as md, triple as tri;\nfn doble(x: int) -> int { x + 1 }\nfn main() -> int { print(doble(10)); print(md(10)); print(tri(10)); md(10) + tri(10) }\n"),
    ];
    for vm in [false, true] {
        let (out, code) = run_modules("ray_from_alias", "app", files, vm);
        assert!(out.contains("11"), "la local doble(10)=11 (vm={vm})\n{out}");
        assert!(out.contains("20"), "md = mates.doble, md(10)=20 (vm={vm})\n{out}");
        assert!(out.contains("30"), "tri = mates.triple, tri(10)=30 (vm={vm})\n{out}");
        assert_eq!(code, 50, "exit = md(10) + tri(10) = 50 (vm={vm})");
    }
}

#[test]
fn from_import_colision_sin_alias_es_error() {
    // Importar un nombre que ya existe en el módulo (sin `as`) es error: pide renombrar.
    let files = &[
        ("mates", "pub fn doble(n: int) -> int { n + n }\n"),
        ("app", "from mates import doble;\nfn doble(x: int) -> int { x + 1 }\nfn main() -> int { doble(1) }\n"),
    ];
    let (_, code) = run_modules("ray_from_colision", "app", files, false);
    assert_ne!(code, 0, "una colisión de nombre importado sin alias debe fallar");
}

#[test]
fn from_import_de_tipo_es_error_diferido() {
    // Importar un tipo con `from` está diferido (los tipos son globales); error claro.
    let files = &[
        ("lib", "pub fn f() -> int { 1 }\nstruct Punto { x: int, y: int }\n"),
        ("app", "from lib import Punto;\nfn main() -> int { 0 }\n"),
    ];
    let (_, code) = run_modules("ray_from_tipo", "app", files, false);
    assert_ne!(code, 0, "importar un tipo con 'from' debe fallar (diferido)");
}

#[test]
fn llamar_funcion_privada_es_error() {
    let files = &[
        ("lib", "pub fn publica() -> int { 1 }\nfn privada() -> int { 2 }\n"),
        ("app", "import lib;\nfn main() -> int { lib.privada() }\n"),
    ];
    let (out, code) = run_modules("ray_mod_priv", "app", files, false);
    assert_ne!(code, 0, "una llamada a función privada debe fallar");
    let _ = out;
}
