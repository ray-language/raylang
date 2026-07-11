//! M54.1 — BSON (`packages/db/bson.ray`), el formato de documentos de MongoDB, en raylang puro.
//! Oráculo conductual determinista por ambos motores (sin servidor): (1) la codificación de
//! `{"hello": "world"}` reproduce **byte a byte** el vector canónico de bsonspec.org; (2) el
//! segundo vector del spec (`{"BSON": ["awesome", 5.05, 1986]}`, con double IEEE 754 e int32) se
//! decodifica; (3) round-trip exacto de todos los tipos v1 (doc/array anidados, negativos, UTF-8
//! multi-byte, bin, ObjectId, null, int64 > 2^53); (4) errores como valores (truncado, tipo no
//! soportado) con la posición del octeto.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

fn proyecto(base: &std::path::Path) -> std::path::PathBuf {
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/db");
    let app = base.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ndb = \"path:{}\"\n",
            db.display()
        ),
    )
    .unwrap();
    let main = r#"import db/bson;

// hex local para comparar la codificación contra los vectores del spec.
fn to_hex(b: bytes) -> string {
    let d = "0123456789abcdef";
    var s = "";
    var i = 0;
    while (i < b.len()) {
        s = s + to_string(d[b[i] >> 4]) + to_string(d[b[i] & 15]);
        i = i + 1;
    }
    s
}

fn from_hex(s: string) -> bytes {
    var out: [int] = [];
    var i = 0;
    while (i < s.len()) {
        out.push(hex_digit(s[i]) * 16 + hex_digit(s[i + 1]));
        i = i + 2;
    }
    bytes_of(out)
}

fn hex_digit(c: char) -> int {
    let d = "0123456789abcdef";
    var i = 0;
    while (i < 16) {
        if (d[i] == c) { return i; }
        i = i + 1;
    }
    panic("dígito hex inválido");
    0
}

fn main() -> int {
    // 1. Vector canónico de bsonspec.org: {"hello": "world"}.
    let doc1 = [bson.field("hello", bson.Bson.Str("world"))];
    print("enc1: " + to_hex(bson.encode(doc1)));

    // 2. Decodificar el segundo vector del spec: {"BSON": ["awesome", 5.05, 1986]} (double + int32).
    let v2 = from_hex("310000000442534f4e002600000002300008000000617765736f6d65000131003333333333331440103200c20700000000");
    match (bson.decode(v2)) {
        Result.Ok(fields) => { print("dec2: " + bson.dump_doc(fields)); },
        Result.Err(e) => { print("err2: " + e); },
    }

    // 3. Round-trip con todos los tipos v1 (incl. anidados, negativos y UTF-8 multi-byte).
    var arr: [bson.Bson] = [];
    arr.push(bson.Bson.Int(0 - 42));
    arr.push(bson.Bson.Null);
    let inner = [bson.field("ok", bson.Bson.Double(1.0))];
    let doc3 = [
        bson.field("d", bson.Bson.Double(0.0 - 2.5)),
        bson.field("s", bson.Bson.Str("café")),
        bson.field("sub", bson.Bson.Doc(inner)),
        bson.field("a", bson.Bson.Arr(arr)),
        bson.field("bin", bson.Bson.Bin(bytes_of([1, 2, 255]))),
        bson.field("id", bson.Bson.ObjectId(bytes_of([0,1,2,3,4,5,6,7,8,9,10,11]))),
        bson.field("t", bson.Bson.Bool(true)),
        bson.field("n", bson.Bson.Null),
        bson.field("big", bson.Bson.Int(9007199254740993)),
        bson.field("dt", bson.Bson.Date(1783600496789)),
        bson.field("ts", bson.Bson.Timestamp(7660503669145600007)),
    ];
    let enc3 = bson.encode(doc3);
    match (bson.decode(enc3)) {
        Result.Ok(fields) => {
            print("dec3: " + bson.dump_doc(fields));
            print("rt: " + to_string(to_hex(bson.encode(fields)) == to_hex(enc3)));
        },
        Result.Err(e) => { print("err3: " + e); },
    }

    // 4. Errores como valores: truncado, tipo desconocido.
    match (bson.decode(v2.sub_bytes(0, 10))) {
        Result.Ok(_) => { print("no debería"); },
        Result.Err(e) => { print("trunc: " + e); },
    }
    match (bson.decode(from_hex("0c0000000e7800040000000000"))) {
        Result.Ok(_) => { print("no debería"); },
        Result.Err(e) => { print("tipo: " + e); },
    }
    0
}
"#;
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

fn correr(app: &std::path::Path, flags: &[&str]) -> String {
    let mut args = vec!["run"];
    args.extend_from_slice(flags);
    let out = Command::new(BIN).args(&args).current_dir(app).output().expect("lanza el binario");
    assert!(
        out.status.success(),
        "corre sin error\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const ESPERADO: &str = "enc1: 160000000268656c6c6f0006000000776f726c640000\n\
dec2: {BSON: [\"awesome\", 5.05, 1986]}\n\
dec3: {d: -2.5, s: \"café\", sub: {ok: 1}, a: [-42, null], bin: bin(0102ff), id: oid(000102030405060708090a0b), t: true, n: null, big: 9007199254740993, dt: date(2026-07-09T12:34:56.789Z), ts: timestamp(1783600000,7)}\n\
rt: true\n\
trunc: BSON inválido (octeto 0): longitud de documento inválida: 49\n\
tipo: BSON inválido (octeto 6): tipo BSON no soportado: 14\n";

fn proyecto_puente(base: &std::path::Path) -> std::path::PathBuf {
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/db");
    let app = base.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ndb = \"path:{}\"\n",
            db.display()
        ),
    )
    .unwrap();
    let main = r#"import db/bson;
from std/json import stringify;

fn main() -> int {
    // JSON string → documento BSON (la ruta ergonómica para filtros de mongo).
    match (bson.doc_from_json("{\"nombre\": \"ada\", \"nota\": 36, \"tags\": [\"a\", null, true]}")) {
        Result.Ok(fields) => { print("from: " + bson.dump_doc(fields)); },
        Result.Err(e) => { print("err: " + e); },
    }
    // Un escape \uXXXX del JSON llega como el carácter (diferido JSON-1 compone con el puente).
    match (bson.doc_from_json("{\"s\": \"caf\\u00e9\"}")) {
        Result.Ok(fields) => { print("uni: " + bson.dump_doc(fields)); },
        Result.Err(e) => { print("err: " + e); },
    }
    // No-objeto en el tope y JSON malformado → errores como valores.
    match (bson.doc_from_json("[1, 2]")) {
        Result.Ok(_) => { print("no debería"); },
        Result.Err(e) => { print("tope: " + e); },
    }
    // Bson → Json con degradación documentada: Int → número, ObjectId → hex, claves ordenadas.
    let doc = [
        bson.field("n", bson.Bson.Int(42)),
        bson.field("id", bson.Bson.ObjectId(bytes_of([0,1,2,3,4,5,6,7,8,9,10,11]))),
        bson.field("sub", bson.Bson.Doc([bson.field("ok", bson.Bson.Double(1.0))])),
    ];
    print("to: " + stringify(bson.to_json(bson.Bson.Doc(doc))));
    0
}
"#;
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

const ESPERADO_PUENTE: &str = "from: {nombre: \"ada\", nota: 36, tags: [\"a\", null, true]}\n\
uni: {s: \"café\"}\n\
tope: el JSON de un documento debe ser un objeto\n\
to: {\"id\":\"000102030405060708090a0b\",\"n\":42,\"sub\":{\"ok\":1}}\n";

#[test]
fn bson_puente_json() {
    let base = std::env::temp_dir().join("ray_bson_cli_json");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let app = proyecto_puente(&base);

    assert_eq!(correr(&app, &[]), ESPERADO_PUENTE, "VM");
    assert_eq!(correr(&app, &["--interp"]), ESPERADO_PUENTE, "intérprete");
}

#[test]
fn bson_vectores_del_spec_roundtrip_y_errores() {
    let base = std::env::temp_dir().join("ray_bson_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let app = proyecto(&base);

    // VM (motor de producto) e intérprete (oráculo): mismo stdout exacto.
    assert_eq!(correr(&app, &[]), ESPERADO, "VM");
    assert_eq!(correr(&app, &["--interp"]), ESPERADO, "intérprete");
}

fn proyecto_profundo(base: &std::path::Path) -> std::path::PathBuf {
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/db");
    let app = base.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ndb = \"path:{}\"\n",
            db.display()
        ),
    )
    .unwrap();
    // Construye un BSON de N documentos anidados y comprueba que decode lo rechaza como VALOR
    // (Err), sin agotar la pila. 50 y 200 (dentro del tope) deben decodificar; 600 debe fallar.
    let main = r#"import db/bson;

fn nested(n: int) -> bytes {
    var cur: [int] = [5, 0, 0, 0, 0];
    var k = 0;
    while (k < n) {
        let l = cur.len() + 8;
        var next: [int] = [];
        next.push(l & 255); next.push((l >> 8) & 255);
        next.push((l >> 16) & 255); next.push((l >> 24) & 255);
        next.push(3); next.push(97); next.push(0);
        var i = 0;
        while (i < cur.len()) { next.push(cur[i]); i = i + 1; }
        next.push(0);
        cur = next;
        k = k + 1;
    }
    bytes_of(cur)
}

fn probar(etiqueta: string, n: int) {
    match (bson.decode(nested(n))) {
        Result.Ok(fs) => { print(etiqueta + ": OK"); },
        Result.Err(e) => { print(etiqueta + ": Err"); },
    }
}

fn main() -> int {
    probar("d50", 50);
    probar("d200", 200);
    probar("d600", 600);
    0
}
"#;
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

/// M76 — un BSON con anidamiento excesivo (documentos/arreglos recursivos) ya NO tumba al cliente
/// por desbordamiento de pila: se rechaza como valor (`Err`). Antes, ~4.8 KB (600 niveles) abortaban
/// el proceso con "desbordamiento de pila".
#[test]
fn bson_anidamiento_profundo_es_error() {
    let base = std::env::temp_dir().join("ray_bson_cli_profundo");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let app = proyecto_profundo(&base);

    const ESPERADO_PROFUNDO: &str = "d50: OK\nd200: OK\nd600: Err\n";
    assert_eq!(correr(&app, &[]), ESPERADO_PROFUNDO, "VM");
    assert_eq!(correr(&app, &["--interp"]), ESPERADO_PROFUNDO, "intérprete");
}
