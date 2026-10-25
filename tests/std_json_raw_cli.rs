//! rayreq #257: `std/json` no devuelve OTRO entero por encima de 2^53 − 1 (`as_int`/`get_int`
//! → None), `raw_at` da el texto de un subvalor por JSON Pointer (los dígitos de un id de 64 bits
//! intactos), y `reindent(text, 0)`/`compact` compactan sin reordenar. VM e intérprete.

use std::process::Command;

const PROG: &str = r##"import std/json;
fn show(o: Option<string>) -> string {
    match (o) { Option.Some(s) => s, Option.None => "None" }
}
fn showi(o: Option<int>) -> string {
    match (o) { Option.Some(n) => to_string(n), Option.None => "None" }
}
fn main() {
    let text = "{\"id\": 9007199254740993, \"big\": 12345678901234567890, \"ok\": 42, \"neg\": -9007199254740991, \"data\": [ {\"x\": 1.50, \"s\": \"a\\\"b\"}, [true, null] ], \"a/b\": 7, \"t~\": 8}";
    let j = json.parse(text).unwrap();
    print("get_int id: " + showi(json.get_int(j, "id")));
    print("get_int big: " + showi(json.get_int(j, "big")));
    print("get_int ok: " + showi(json.get_int(j, "ok")));
    print("get_int neg: " + showi(json.get_int(j, "neg")));
    print("raw id: " + show(json.raw_at(text, "/id")));
    print("raw big: " + show(json.raw_at(text, "/big")));
    print("raw x: " + show(json.raw_at(text, "/data/0/x")));
    print("raw s: " + show(json.raw_at(text, "/data/0/s")));
    print("raw null: " + show(json.raw_at(text, "/data/1/1")));
    print("raw arr: " + show(json.raw_at(text, "/data/1")));
    print("raw esc: " + show(json.raw_at(text, "/a~1b")) + " " + show(json.raw_at(text, "/t~0")));
    print("raw missing: " + show(json.raw_at(text, "/data/5")) + " " + show(json.raw_at(text, "/nope")) + " " + show(json.raw_at(text, "/data/01")) + " " + show(json.raw_at(text, "bad")));
    print("raw whole: " + show(json.raw_at("  [1, 2] ", "")));
    print("compact: " + json.compact("{ \"a\" : [ 1 , 2 ],\n  \"b\": \"x y\" }"));
    print("reindent0: " + json.reindent("{\"a\": [1, 2]}", 0));
    print("reindent2: " + json.reindent("{\"a\": [1, 2]}", 2).replace("\n", "|"));
}
"##;

const WANT: &str = r##"get_int id: None
get_int big: None
get_int ok: 42
get_int neg: -9007199254740991
raw id: 9007199254740993
raw big: 12345678901234567890
raw x: 1.50
raw s: "a\"b"
raw null: null
raw arr: [true, null]
raw esc: 7 8
raw missing: None None None None
raw whole: [1, 2]
compact: {"a":[1,2],"b":"x y"}
reindent0: {"a":[1,2]}
reindent2: {|  "a": [|    1,|    2|  ]|}
"##;

#[test]
fn std_json_keeps_big_integers_exact_or_refuses_them() {
    let dir = std::env::temp_dir().join(format!("ray_std_json_raw_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.ray"), PROG).unwrap();
    for flags in [&[][..], &["--interp"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray")).arg("run").args(flags).arg("a.ray").current_dir(&dir).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{flags:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}
