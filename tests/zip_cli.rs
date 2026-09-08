//! M218/M219 (plan de ray-sublime, lote J): `json.parse_relaxed` y `std/zip` (lectura) — idénticos
//! en ambos motores. El ZIP de prueba (`tests/fixtures/sample.zip`, hecho con `zipfile` de Python)
//! trae una entrada STORED, una DEFLATED de 2 280 bytes y una vacía.

use std::process::Command;

const PROGRAM: &str = r#"import std/json;
import std/zip;
import std/fs;

fn main() -> int {
    let src = "{ // comment\n  \"a\": [1, 2, /* c */ 3,],\n  \"b\": true, }";
    match (json.parse_relaxed(src)) {
        Result.Ok(j) => print(json.stringify(j)),
        Result.Err(e) => print("ERR " + e),
    }
    match (json.parse(src)) {
        Result.Ok(_) => print("strict accepted?!"),
        Result.Err(e) => print("strict: " + e),
    }
    match (json.parse_relaxed("[1, 2,, 3]")) {
        Result.Ok(_) => print("double comma accepted?!"),
        Result.Err(e) => print("relaxed still rejects: " + e),
    }
    let data = match (fs.read_file_bytes(args()[0])) {
        Result.Ok(b) => b,
        Result.Err(e) => { print(e); return 1; },
    };
    let a = match (zip.open(data)) {
        Result.Ok(a) => a,
        Result.Err(e) => { print(e); return 1; },
    };
    let es = zip.entries(a);
    var i = 0;
    while (i < es.len()) {
        print("${es[i].name} m=${es[i].method} size=${es[i].size}");
        i = i + 1;
    }
    match (zip.read(a, "a.txt")) { Result.Ok(b) => print(from_utf8(b).unwrap()), Result.Err(e) => print(e) }
    match (zip.read(a, "dir/b.txt")) { Result.Ok(b) => print(b.len()), Result.Err(e) => print(e) }
    match (zip.read(a, "empty.txt")) { Result.Ok(b) => print(b.len()), Result.Err(e) => print(e) }
    match (zip.read(a, "nope")) { Result.Ok(_) => print("?"), Result.Err(e) => print(e) }
    match (zip.open(b"not a zip at all, really not")) { Result.Ok(_) => print("?"), Result.Err(e) => print(e) }
    0
}
"#;

const WANT: &str = "{\"a\":[1,2,3],\"b\":true}\nstrict: expected a string key\nrelaxed still rejects: expected a number\na.txt m=0 size=13\ndir/b.txt m=8 size=2280\nempty.txt m=8 size=0\nstored alpha\n\n2280\n0\nzip: no entry named 'nope'\nzip: end of central directory not found (not a ZIP file?)\n";

#[test]
fn relaxed_json_and_zip_reading_both_engines() {
    let path = std::env::temp_dir().join("ray_zip_cli.ray");
    std::fs::write(&path, PROGRAM).unwrap();
    let fixture = format!("{}/tests/fixtures/sample.zip", env!("CARGO_MANIFEST_DIR"));
    for flags in [&[][..], &["--vm"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_raylang")).args(flags).arg(&path).arg(&fixture).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{flags:?}");
    }
}
