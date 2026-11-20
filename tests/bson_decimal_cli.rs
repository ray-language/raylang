//! db 0.9.0 (raycdc #402): `Bson.Decimal128` y `Bson.BinSub` en `packages/db/bson.ray`. Vectores
//! deterministas en la VM y el intérprete, sin servidor: la codificación de `NumberDecimal` es la
//! de la especificación (BID, little-endian) y el texto es el que imprime MongoDB.
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

const MAIN: &str = r#"import db/bson;

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

fn show(text: string) {
    match (bson.decimal128_from_string(text)) {
        Result.Ok(b) => print(text + " => " + to_hex(b) + " => " + bson.decimal128_to_string(b)),
        Result.Err(e) => print(text + " => ERR " + e),
    }
}

fn main() -> int {
    for t in ["1", "0.1", "12.50", "-0.001", "1E+3", "0", "0.000001", "1E-7", "1.5e-7", "100",
        "1234567890123456789012345678901234", "9.999999999999999999999999999999999E+6144", "1E-6176",
        "Infinity", "-Infinity", "NaN", "1E+6112", "10E-6177", "+7", "007.50"] {
        show(t);
    }
    for t in ["12345678901234567890123456789012345", "1E+6145", "1E-6177", "abc", "1.2.3", "1e"] {
        show(t);
    }
    // Un documento con un decimal y binarios de subtipo 4 y 0: ida y vuelta exacta.
    let uuid = bytes_of([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
    let doc = [
        bson.field("price", bson.Bson.Decimal128(bson.decimal128_from_string("12.50").unwrap())),
        bson.field("u", bson.Bson.BinSub(4, uuid)),
        bson.field("b", bson.Bson.Bin(bytes_of([1, 2, 3]))),
    ];
    let enc = bson.encode(doc);
    print("enc: " + to_hex(enc));
    match (bson.decode(enc)) {
        Result.Ok(fields) => {
            print("dec: " + bson.dump_doc(fields));
            print("rt: " + to_string(bson.encode(fields) == enc));
            match (bson.get(fields, "u")) {
                Option.Some(bson.Bson.BinSub(subtype, raw)) => print("subtype ${subtype} len ${raw.len()}"),
                _ => print("u is not BinSub"),
            }
            match (bson.get(fields, "b")) {
                Option.Some(bson.Bson.Bin(raw)) => print("generic len ${raw.len()}"),
                _ => print("b is not Bin"),
            }
        },
        Result.Err(e) => print("dec ERR " + e),
    }
    0
}
"#;

const EXPECTED: &str = "1 => 01000000000000000000000000004030 => 1\n\
0.1 => 01000000000000000000000000003e30 => 0.1\n\
12.50 => e2040000000000000000000000003c30 => 12.50\n\
-0.001 => 01000000000000000000000000003ab0 => -0.001\n\
1E+3 => 01000000000000000000000000004630 => 1E+3\n\
0 => 00000000000000000000000000004030 => 0\n\
0.000001 => 01000000000000000000000000003430 => 0.000001\n\
1E-7 => 01000000000000000000000000003230 => 1E-7\n\
1.5e-7 => 0f000000000000000000000000003030 => 1.5E-7\n\
100 => 64000000000000000000000000004030 => 100\n\
1234567890123456789012345678901234 => f2af967ed05c82de3297ff6fde3c4030 => 1234567890123456789012345678901234\n\
9.999999999999999999999999999999999E+6144 => ffffffff638e8d37c087adbe09edff5f => 9.999999999999999999999999999999999E+6144\n\
1E-6176 => 01000000000000000000000000000000 => 1E-6176\n\
Infinity => 00000000000000000000000000000078 => Infinity\n\
-Infinity => 000000000000000000000000000000f8 => -Infinity\n\
NaN => 0000000000000000000000000000007c => NaN\n\
1E+6112 => 0a00000000000000000000000000fe5f => 1.0E+6112\n\
10E-6177 => 01000000000000000000000000000000 => 1E-6176\n\
+7 => 07000000000000000000000000004030 => 7\n\
007.50 => ee020000000000000000000000003c30 => 7.50\n\
12345678901234567890123456789012345 => ERR decimal128 holds 34 significant digits, '12345678901234567890123456789012345' has 35\n\
1E+6145 => ERR decimal128 cannot hold '1E+6145' exactly: its exponent is out of range\n\
1E-6177 => ERR decimal128 cannot hold '1E-6177' exactly: its exponent is out of range\n\
abc => ERR invalid decimal128: 'abc'\n\
1.2.3 => ERR invalid decimal128: '1.2.3'\n\
1e => ERR invalid decimal128: '1e'\n\
enc: 3f00000013707269636500e2040000000000000000000000003c300575001000000004000102030405060708090a0b0c0d0e0f056200030000000001020300\n\
dec: {price: decimal(12.50), u: bin(4:000102030405060708090a0b0c0d0e0f), b: bin(010203)}\n\
rt: true\n\
subtype 4 len 16\n\
generic len 3\n";

fn project(name: &str) -> std::path::PathBuf {
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/db");
    let app = std::env::temp_dir().join(format!("ray_bson_decimal_{name}"));
    let _ = std::fs::remove_dir_all(&app);
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ndb = \"path:{}\"\n", db.display()),
    )
    .unwrap();
    std::fs::write(app.join("src/main.ray"), MAIN).unwrap();
    app
}

fn run(flags: &[&str], name: &str) -> String {
    let app = project(name);
    let mut args = vec!["run"];
    args.extend_from_slice(flags);
    let out = Command::new(BIN).args(&args).current_dir(&app).output().unwrap();
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn decimal128_and_binary_subtypes_round_trip_on_both_engines() {
    let vm = run(&[], "vm");
    let interp = run(&["--interp"], "interp");
    assert_eq!(vm, interp, "la VM y el intérprete divergen");
    assert_eq!(vm, EXPECTED);
}
