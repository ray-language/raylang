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
fn check(name: &str, driver: &str, expected: &str) {
    for vm in [false, true] {
        let (out, code) = run_with_lib(name, driver, vm);
        assert_eq!(out.trim(), expected, "driver '{name}' (vm={vm})");
        assert_eq!(code, 0, "output 0 (vm={vm})");
    }
}

#[test]
fn round_trip_canonical_con_keys_ordenadas() {
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
    let s = "{\"neg\": -3.5, \"exp\": 1e3, \"text\": \"a\\tb\\n\"}";
    match (parse(s)) {
        Result.Ok(j) => { print(stringify(j)); 0 },
        Result.Err(e) => { eprint(e); 1 },
    }
}
"#;
    // -3.5 se mantiene; 1e3 -> 1000; el tab/newline se re-escapan en la salida.
    check("numeros", driver, r#"{"exp":1000,"neg":-3.5,"text":"a\tb\n"}"#);
}

#[test]
fn escapes_unicode() {
    // Diferido JSON-1: \uXXXX — BMP de 1 y 2 octetos UTF-8, par surrogate (astral), y los
    // errores como valores: surrogate suelto, par incompleto, dígito no hex.
    let driver = r#"
from json import parse, stringify, Json;
fn reports(s: string) {
    match (parse(s)) {
        Result.Ok(j) => { print(stringify(j)); },
        Result.Err(e) => { print("err: " + e); },
    }
}
fn main() -> int {
    reports("\"caf\\u00e9\"");
    reports("\"\\u0041\\u2764\"");
    reports("\"\\ud83d\\ude00\"");
    reports("\"\\udc00\"");
    reports("\"\\ud83dx\"");
    reports("\"\\u12g4\"");
    0
}
"#;
    let expected = "\"café\"\n\"A❤\"\n\"😀\"\nerr: escape \\u con surrogate suelto\nerr: par surrogate incompleto en \\u\nerr: escape \\u con dígito no hexadecimal";
    check("unicode", driver, expected);
}

#[test]
fn escapes_de_control_rfc8259() {
    // M59.1 — conformidad con la RFC 8259 §7: (1) \b y \f son escapes LEGALES y el parse los
    // acepta (antes: "secuencia de escape no soportada"); (2) quote los re-emite cortos; (3) un
    // control < 0x20 sin escape corto sale como \u00XX (antes: crudo → JSON inválido).
    let driver = r#"
from json import parse, stringify;
fn reports(s: string) {
    match (parse(s)) {
        Result.Ok(j) => { print(stringify(j)); },
        Result.Err(e) => { print("err: " + e); },
    }
}
fn main() -> int {
    reports("\"a\\bb\\fc\"");
    reports("\"\\u0008\\u000c\"");
    reports("\"\\u0001\\u001f\"");
    0
}
"#;
    // Los / se canonicalizan a \b/\f; los controles sin escape corto, a \u00XX.
    let expected = "\"a\\bb\\fc\"\n\"\\b\\f\"\n\"\\u0001\\u001f\"";
    check("control", driver, expected);
}

#[test]
fn vacios_y_nesting() {
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
fn errors_como_values() {
    let driver = r#"
from json import parse, stringify;
fn reports(s: string) {
    match (parse(s)) {
        Result.Ok(j) => print("ok: " + stringify(j)),
        Result.Err(e) => print("err: " + e),
    }
}
fn main() -> int {
    reports("{\"a\": 1} basura");      // texto sobrante
    reports("[1, 2");                   // arreglo sin cerrar
    reports("\"sin cierre");            // string sin cerrar
    0
}
"#;
    let expected = "err: text sobrante after el JSON\nerr: array sin close\nerr: string sin close";
    check("errors", driver, expected);
}

// ── Helpers de acceso + pretty-print (M90.3) ─────────────────────────────────────────

#[test]
fn helpers_de_access() {
    let driver = r#"
from json import parse, get_string, get_int, get_float, get_bool, get_array, member, at, as_int, as_string, is_null;

fn shows(o: Option<string>) -> string {
    match (o) {
        Option.Some(s) => s,
        Option.None => "None",
    }
}

fn main() -> int {
    let s = "{\"name\": \"ada\", \"edad\": 36, \"pi\": 3.5, \"viva\": false, \"nums\": [10, 20], \"nada\": null}";
    match (parse(s)) {
        Result.Err(e) => { eprint(e); 1 },
        Result.Ok(j) => {
            print(shows(j.get_string("name")));           // ada
            print(shows(j.get_string("edad")));             // None (no es string)
            match (j.get_int("edad")) {
                Option.Some(n) => print(to_string(n)),        // 36
                Option.None => print("None"),
            };
            match (j.get_int("pi")) {
                Option.Some(n) => print(to_string(n)),
                Option.None => print("None"),                 // None (3.5 no es integral)
            };
            match (j.get_float("pi")) {
                Option.Some(f) => print(to_string(f)),        // 3.5
                Option.None => print("None"),
            };
            match (j.get_bool("viva")) {
                Option.Some(b) => print(to_string(b)),        // false
                Option.None => print("None"),
            };
            // member + at + as_int componen para navegar anidado.
            match (member(j, "nums")) {
                Option.Some(arr) => {
                    match (at(arr, 1)) {
                        Option.Some(v) => {
                            match (as_int(v)) {
                                Option.Some(n) => print(to_string(n)),   // 20
                                Option.None => print("None"),
                            }
                        },
                        Option.None => print("None"),
                    }
                },
                Option.None => print("None"),
            };
            match (member(j, "nada")) {
                Option.Some(v) => print(to_string(is_null(v))),          // true
                Option.None => print("None"),
            };
            print(shows(j.get_string("no_existe")));        // None
            match (at(j, 0)) {
                Option.Some(_) => print("algo"),
                Option.None => print("None"),                 // None (un objeto no se indexa)
            };
            0
        },
    }
}
"#;
    check("helpers", driver, "ada\nNone\n36\nNone\n3.5\nfalse\n20\ntrue\nNone\nNone");
}

#[test]
fn pretty_print_con_sangria() {
    let driver = r#"
from json import parse, stringify_pretty;
fn main() -> int {
    let s = "{\"b\": [1, 2], \"a\": \"x\", \"empty\": {}, \"list_empty\": []}";
    match (parse(s)) {
        Result.Ok(j) => { print(stringify_pretty(j, 2)); 0 },
        Result.Err(e) => { eprint(e); 1 },
    }
}
"#;
    let expected = "{\n  \"a\": \"x\",\n  \"b\": [\n    1,\n    2\n  ],\n  \"list_empty\": [],\n  \"empty\": {}\n}";
    check("pretty", driver, expected);
}
