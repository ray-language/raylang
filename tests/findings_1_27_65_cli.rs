//! M411 — los hallazgos de lenguaje de rayzip, raycron, raydata, raystat, rayffi y raymcp
//! (ray-apps a 1.27.65, #188–#221), cada uno con su programa mínimo en los motores que aplican.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_findings_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Corre `prog.ray` de `dir` en VM, intérprete y (si hay rustc) nativo; los tres deben dar `want`.
fn three_engines(dir: &PathBuf, want: &str) {
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(dir, engine);
        assert_eq!(code, 0, "{engine:?}: {err}");
        assert_eq!(out, want, "{engine:?}");
    }
    if has_rustc() {
        let bin = dir.join("prog_bin");
        let (_o, err, code) = ray(dir, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        let out = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "nativo");
    }
}

/// Como `three_engines`, para programas con concurrencia (el intérprete no la ejecuta).
fn vm_and_native(dir: &PathBuf, want: &str) {
    let (out, err, code) = ray(dir, &["run", "prog.ray"]);
    assert_eq!(code, 0, "vm: {err}");
    assert_eq!(out, want, "vm");
    if has_rustc() {
        let bin = dir.join("prog_bin");
        let (_o, err, code) = ray(dir, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        let out = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "nativo");
    }
}

fn write_project(dir: &PathBuf, files: &[(&str, &str)]) {
    std::fs::write(dir.join("ray.toml"), "[package]\nname = \"p\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    for (name, src) in files {
        std::fs::write(dir.join("src").join(name), src).unwrap();
    }
}

/// rayzip #188/#191/#192/#190: gzip de varios miembros, la entrada consumida, la CRC encadenable y
/// los trozos de un mismo stream DEFLATE.
#[test]
fn compression_additions_agree_on_all_engines() {
    let d = tmp("compression");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/deflate;
import std/inflate;
fn main() -> int {
    let a = deflate.gzip_compress(b"hello ");
    let b = deflate.gzip_compress(b"world");
    print("${inflate.gunzip(a + b).unwrap().len()}");
    let r = deflate.deflate_raw(b"abc");
    match (inflate.inflate_raw_consumed(r + b"TRAILING")) {
        Result.Ok(t) => print("${t.1 == r.len()}"),
        Result.Err(e) => print(e),
    }
    let s = inflate.inflate_stream();
    let _ = inflate.stream_push(s, r + b"XY");
    print("${inflate.stream_remaining(s).len()}");
    let whole = deflate.deflate_raw_chunk(b"one one one ", 6) + deflate.deflate_raw_chunk(b"two ", 1) + deflate.deflate_raw_level(b"three", 9);
    print("${inflate.inflate_raw(whole).unwrap().len()}");
    print("${inflate.crc32_update(inflate.crc32(b"hello "), b"world") == inflate.crc32(b"hello world")}");
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "11\ntrue\n2\n21\ntrue\n");
}

/// rayzip #195/#194: `fs.read_link` y `fs.set_mtime`.
#[cfg(unix)]
#[test]
fn read_link_and_set_mtime_on_all_engines() {
    let d = tmp("fs_link_mtime");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/fs;
fn main() -> int {
    let d = fs.make_temp_dir("rlm").unwrap();
    let _ = fs.write_file(d + "/a.txt", "hi");
    let _ = fs.symlink("a.txt", d + "/l");
    print(fs.read_link(d + "/l").unwrap());
    print("${fs.read_link(d + "/a.txt").is_err()}");
    let _ = fs.set_mtime(d + "/a.txt", 946684800000);
    print("${fs.stat(d + "/a.txt").unwrap().mtime_ms}");
    let _ = fs.remove_all(d);
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "a.txt\ntrue\n946684800000\n");
}

/// raydata #204: un patrón de tupla con variantes del enum del propio módulo, importado.
#[test]
fn a_tuple_pattern_with_own_enum_variants_survives_an_import() {
    let d = tmp("tuple_pattern_import");
    write_project(
        &d,
        &[
            ("geo.ray", "pub enum Shape { Dot, Line(int), }\npub fn first(a: Shape, b: int) -> int {\n    match ((a, b)) {\n        (Shape.Line(x), _) => x + b,\n        _ => 0,\n    }\n}\n"),
            ("main.ray", "import geo;\nfn main() -> int { print(\"${geo.first(geo.Shape.Line(1), 2)}\"); 0 }\n"),
        ],
    );
    let (out, err, code) = ray(&d, &["run", "src/main.ray"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "3\n");
}

/// raydata #203: `@derive(ToJson)` sin el import señala el `@derive` y dice qué importar.
#[test]
fn derive_tojson_without_the_import_points_at_the_annotation() {
    let d = tmp("derive_tojson");
    std::fs::write(d.join("prog.ray"), "import std/json;\n\n@derive(ToJson)\nstruct S { n: int }\n\nfn main() { print(S { n: 1 }.to_json()); }\n").unwrap();
    let (_out, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!(code, 65);
    assert!(err.contains("at 3:1: trait 'ToJson' not declared: it lives in std/json, import it with `from std/json import ToJson;`"), "{err}");
}

/// raydata #206: `.eq()` y `assert_eq` usan el `impl Eq` en los tres motores — y desde M418 también
/// `==` (antes era estructural: `false true`).
#[test]
fn assert_eq_uses_the_user_impl_eq_on_all_engines() {
    let d = tmp("impl_eq");
    std::fs::write(
        d.join("prog.ray"),
        r#"enum Word { W(string), }
impl Eq for Word {
    fn eq(self, other: Word) -> bool {
        match (self) { Word.W(a) => match (other) { Word.W(b) => a.to_lower() == b.to_lower() } }
    }
}
impl Show for Word { fn show(self) -> string { match (self) { Word.W(a) => "W<" + a + ">" } } }
fn main() -> int {
    let a = Word.W("Hola");
    let b = Word.W("HOLA");
    print("${a == b} ${a.eq(b)}");
    assert_eq(a, b);
    print("ok");
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "true true\nok\n");
}

/// raydata #206: `==` sobre un `T: Eq` despacha al bound (M418); sin el bound, el error lo dice.
#[test]
fn eq_operator_on_a_type_parameter_uses_the_bound_or_asks_for_it() {
    let d = tmp("eq_tparam");
    std::fs::write(d.join("prog.ray"), "fn same<T: Eq>(a: T, b: T) -> bool { a == b }\nfn main() { print(same(1, 1)); print(same(\"a\", \"b\")); }\n").unwrap();
    let (out, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "true\nfalse\n");
    std::fs::write(d.join("prog.ray"), "fn same<T>(a: T, b: T) -> bool { a == b }\nfn main() { print(same(1, 1)); }\n").unwrap();
    let (_out, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!(code, 65);
    assert!(err.contains("a type parameter compares with == only under an Eq bound (fn f<T: Eq>)"), "{err}");
}

/// rayffi #212: dos módulos declaran la misma función C (una `blocking`); se llaman calificadas.
#[cfg(unix)]
#[test]
fn two_modules_declare_the_same_extern_fn() {
    let d = tmp("extern_per_module");
    write_project(
        &d,
        &[
            ("a.ray", "extern \"c\" { fn abs(n: int) -> int; }\npub fn f(n: int) -> int { abs(n) }\n"),
            ("b.ray", "extern \"c\" blocking { fn abs(n: int) -> int; }\npub fn f(n: int) -> int { abs(n) + 1 }\n"),
            ("main.ray", "import a;\nimport b;\nfn main() -> int { print(\"${a.f(0 - 3)} ${b.f(0 - 3)} ${b.abs(0 - 7)}\"); 0 }\n"),
        ],
    );
    let (out, err, code) = ray(&d, &["run", "src/main.ray"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "3 4 7\n");
    if has_rustc() {
        let bin = d.join("prog_bin");
        let (_o, err, code) = ray(&d, &["build", "src/main.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        assert_eq!(String::from_utf8_lossy(&Command::new(&bin).output().unwrap().stdout), "3 4 7\n");
    }
}

/// rayffi #209/#210/#213/#214 y raymcp #218: lo que la VM ya hacía y el nativo no.
#[cfg(unix)]
#[test]
fn ffi_and_values_that_native_did_not_cover() {
    let d = tmp("native_gaps");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/ffi;
extern "c" { fn labs(n: u64) -> u64; fn malloc(n: int) -> ptr; fn free(p: ptr) -> unit; }
struct H { p: ptr, name: string }
fn f(x: int) -> Option<(int, string, string, string)> { if (x > 0) { Option.Some((x, "a", "b", "c")) } else { Option.None } }
fn main() -> int {
    print("${labs(5 as u64)}");
    print("${ffi.errno() >= 0}");
    print("${["1", "x", "3"].map(parse_int).len()}");
    print("${f(1).is_none()}");
    let ch: Channel<H> = Channel.new();
    let t = spawn(fn() -> string { let h = ch.recv().unwrap(); free(h.p); h.name });
    ch.send(H { p: malloc(16), name: "sent" });
    print(t.join());
    print("${ffi.buffer(4).len()}");
    0
}
"#,
    )
    .unwrap();
    vm_and_native(&d, "5\ntrue\n3\nfalse\nsent\n4\n");
}

/// rayffi #211: cada evaluación de un literal `b"…"` es un búfer propio.
#[cfg(unix)]
#[test]
fn a_bytes_literal_is_a_fresh_buffer_each_evaluation() {
    let d = tmp("bytes_literal_ffi");
    std::fs::write(
        d.join("prog.ray"),
        r#"extern "c" { fn memset(p: bytes, c: int, n: int) -> ptr; }
fn main() -> int {
    for i in 0..2 {
        let lit = b"\x00\x00";
        print("${lit[0]}");
        let _ = memset(lit, 120, 2);
    }
    0
}
"#,
    )
    .unwrap();
    vm_and_native(&d, "0\n0\n");
}
