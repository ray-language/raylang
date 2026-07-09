//! Pruebas de la librería JSON en raylang (M15.4a). JSON es determinista, pero la librería es
//! multi-archivo (se importa `json.ray`), así que en vez del oráculo in-process se prueba por
//! subproceso: se copia `examples/web/json.ray` a un directorio temporal junto a un driver que la
//! importa, y se ejecuta en ambos motores comprobando la salida exacta (golden).

use std::io::Write;
use std::process::Command;

/// Copia `examples/web/json.ray` a un temporal único por `name`, escribe `driver` como `main.ray` a su
/// lado (para que `from json import …` resuelva), ejecuta en el motor dado y devuelve `(stdout, código)`.
fn run_with_lib(name: &str, driver: &str, vm: bool) -> (String, i32) {
    let mut dir = std::env::temp_dir();
    dir.push(format!("ray_json_{name}_{}", if vm { "vm" } else { "interp" }));
    std::fs::create_dir_all(&dir).expect("crea dir");

    let lib_src = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/web/json.ray");
    std::fs::copy(lib_src, dir.join("json.ray")).expect("copia json.ray");

    let driver_path = dir.join("main.ray");
    std::fs::File::create(&driver_path).expect("crea driver").write_all(driver.as_bytes()).expect("escribe");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    if vm {
        cmd.arg("--vm");
    }
    let out = cmd.arg(&driver_path).output().expect("lanza raylang");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

/// Comprueba que `driver` produce `esperado` (stdout, trim) en ambos motores.
fn check(name: &str, driver: &str, esperado: &str) {
    for vm in [false, true] {
        let (out, code) = run_with_lib(name, driver, vm);
        assert_eq!(out.trim(), esperado, "driver '{name}' (vm={vm})");
        assert_eq!(code, 0, "salida 0 (vm={vm})");
    }
}

#[test]
fn round_trip_canonico_con_claves_ordenadas() {
    let driver = r#"
from json import parse, stringify;
fn main() -> int {
    let s = "{\"b\": 2, \"a\": [1, 2, 3], \"c\": {\"x\": true, \"w\": null}}";
    match (parse(s)) {
        Result.Ok(j) => { print(stringify(j)); 0 },
        Result.Err(e) => { eprint(e); 1 },
    }
}
"#;
    // Claves ordenadas en cada nivel: a, b, c y dentro de c: w, x.
    check("roundtrip", driver, r#"{"a":[1,2,3],"b":2,"c":{"w":null,"x":true}}"#);
}

#[test]
fn numeros_y_escapes() {
    let driver = r#"
from json import parse, stringify;
fn main() -> int {
    let s = "{\"neg\": -3.5, \"exp\": 1e3, \"texto\": \"a\\tb\\n\"}";
    match (parse(s)) {
        Result.Ok(j) => { print(stringify(j)); 0 },
        Result.Err(e) => { eprint(e); 1 },
    }
}
"#;
    // -3.5 se mantiene; 1e3 -> 1000; el tab/newline se re-escapan en la salida.
    check("numeros", driver, r#"{"exp":1000,"neg":-3.5,"texto":"a\tb\n"}"#);
}

#[test]
fn escapes_unicode() {
    // Diferido JSON-1: \uXXXX — BMP de 1 y 2 octetos UTF-8, par surrogate (astral), y los
    // errores como valores: surrogate suelto, par incompleto, dígito no hex.
    let driver = r#"
from json import parse, stringify, Json;
fn reporta(s: string) {
    match (parse(s)) {
        Result.Ok(j) => { print(stringify(j)); },
        Result.Err(e) => { print("err: " + e); },
    }
}
fn main() -> int {
    reporta("\"caf\\u00e9\"");
    reporta("\"\\u0041\\u2764\"");
    reporta("\"\\ud83d\\ude00\"");
    reporta("\"\\udc00\"");
    reporta("\"\\ud83dx\"");
    reporta("\"\\u12g4\"");
    0
}
"#;
    let esperado = "\"café\"\n\"A❤\"\n\"😀\"\nerr: escape \\u con surrogate suelto\nerr: par surrogate incompleto en \\u\nerr: escape \\u con dígito no hexadecimal";
    check("unicode", driver, esperado);
}

#[test]
fn vacios_y_anidamiento() {
    let driver = r#"
from json import parse, stringify;
fn main() -> int {
    let s = "[{}, [], {\"k\": []}]";
    match (parse(s)) {
        Result.Ok(j) => { print(stringify(j)); 0 },
        Result.Err(e) => { eprint(e); 1 },
    }
}
"#;
    check("vacios", driver, r#"[{},[],{"k":[]}]"#);
}

#[test]
fn errores_como_valores() {
    let driver = r#"
from json import parse, stringify;
fn reporta(s: string) {
    match (parse(s)) {
        Result.Ok(j) => print("ok: " + stringify(j)),
        Result.Err(e) => print("err: " + e),
    }
}
fn main() -> int {
    reporta("{\"a\": 1} basura");      // texto sobrante
    reporta("[1, 2");                   // arreglo sin cerrar
    reporta("\"sin cierre");            // string sin cerrar
    0
}
"#;
    let esperado = "err: texto sobrante tras el JSON\nerr: arreglo sin cerrar\nerr: string sin cerrar";
    check("errores", driver, esperado);
}
