//! M419 (rayzip #193) — `packages/zip`: archivos ZIP sobre un handle de archivo. Ida y vuelta
//! (escribir con el paquete, leer con el paquete y con `std/zip`, el lector independiente), fechas
//! y modos, lectura por trozos, `unzip -t` del sistema cuando lo hay, y ZIP64 (70 000 entradas).

use std::path::PathBuf;
use std::process::Command;

fn project(name: &str) -> PathBuf {
    let zip = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/zip");
    let base = std::env::temp_dir().join(format!("ray_zip_cli_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let app = base.join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\nentry = \"main.ray\"\n\n[dependencies]\nzip = \"path:{}\"\n", zip.display()),
    )
    .unwrap();
    app
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

const ROUNDTRIP: &str = r#"import std/fs;
import std/zip as stdzip;
import zip/zip;
import zip/format;

fn text_block(n: int) -> string {
    var s = "";
    var i = 0;
    while (s.len() < n) {
        s = s + "line ${i}: the quick brown fox jumps over the lazy dog\n";
        i = i + 1;
    }
    s
}

fn noise(n: int, seed: int) -> bytes {
    let xs: [int] = [];
    var x = seed;
    for _ in 0..n {
        x = (x * 1103515245 + 12345) & 0x7FFFFFFF;
        xs.push((x >> 16) & 0xFF);
    }
    bytes_of(xs)
}

fn main() -> int {
    let root = args()[0];
    let text = text_block(50000).to_bytes();
    let rnd = noise(70000, 7);
    fs.write_file_bytes("${root}/big.bin", rnd).unwrap();
    fs.set_mtime("${root}/big.bin", 1760000000000).unwrap();
    let w = zip.create("${root}/out.zip").unwrap();
    zip.add_dir(w, "docs").unwrap();
    zip.add_bytes_at(w, "docs/a.txt", text, 1760000000000, 0o640).unwrap();
    zip.add_bytes(w, "docs/empty", b"").unwrap();
    zip.add_bytes(w, "./docs/ñandú.txt", "ñandú\n".to_bytes()).unwrap();
    zip.add_file(w, "big.bin", "${root}/big.bin").unwrap();
    let s = zip.finish(w).unwrap();
    print("${s.entries} ${s.bytes_in} ${s.bytes_out < s.bytes_in} ${fs.exists("${root}/out.zip.zip-tmp")}");
    let a = zip.open("${root}/out.zip").unwrap();
    for e in zip.entries(a) {
        print("${e.name} ${e.size} ${e.method} ${e.is_dir}");
    }
    let t = zip.find(a, "docs/a.txt").unwrap();
    print(zip.read(a, t).unwrap() == text);
    print(zip.mtime_ms(t) == 1760000000000);
    print(format.permissions(t));
    let b = zip.find(a, "big.bin").unwrap();
    print(zip.read(a, b).unwrap() == rnd);
    print(zip.mtime_ms(b) == 1760000000000);
    var chunks = 0;
    zip.read_with(a, b, fn(c: bytes) -> Result<int, string> { chunks = chunks + 1; Result.Ok(0) }).unwrap();
    print(chunks);
    print(zip.find(a, "nope").is_none());
    zip.close(a);
    let m = stdzip.open(fs.read_file_bytes("${root}/out.zip").unwrap()).unwrap();
    print(stdzip.read(m, "docs/ñandú.txt").unwrap() == "ñandú\n".to_bytes());
    print(stdzip.read(m, "docs/a.txt").unwrap() == text);
    print(stdzip.entries(m).len());
    0
}
"#;

const WANT: &str = "5 120010 true false\ndocs/ 0 0 true\ndocs/a.txt 50002 8 false\ndocs/empty 0 0 false\ndocs/ñandú.txt 8 0 false\nbig.bin 70000 0 false\ntrue\ntrue\n416\ntrue\ntrue\n1\ntrue\ntrue\ntrue\n5\n";

/// Ida y vuelta en los tres motores; `std/zip` lee lo escrito; `unzip -t` (si existe) lo acepta.
#[test]
fn the_zip_package_writes_and_reads_archives_on_all_engines() {
    let app = project("roundtrip");
    std::fs::write(app.join("main.ray"), ROUNDTRIP).unwrap();
    for (i, flags) in [&[][..], &["--interp"][..]].iter().enumerate() {
        let root = app.join(format!("run{i}"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = vec!["run"];
        a.extend_from_slice(flags);
        a.push("main.ray");
        a.push(root.to_str().unwrap());
        let (out, err, code) = ray(&app, &a);
        assert_eq!(code, 0, "{flags:?}: {out}\n{err}");
        assert_eq!(out, WANT, "{flags:?}");
        if let Ok(o) = Command::new("unzip").arg("-t").arg(root.join("out.zip")).output() {
            let s = String::from_utf8_lossy(&o.stdout);
            assert!(o.status.success() && s.contains("No errors detected"), "unzip -t: {s}");
        }
    }
    if has_rustc() {
        let bin = app.join("app_native");
        let (out, err, code) = ray(&app, &["build", "--native", "main.ray", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build: {out}\n{err}");
        let root = app.join("run_native");
        std::fs::create_dir_all(&root).unwrap();
        let o = Command::new(&bin).arg(&root).current_dir(&app).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), WANT, "nativo: {}", String::from_utf8_lossy(&o.stderr));
    }
}

/// Más de 65 535 entradas: la cuenta solo cabe en el registro ZIP64; `std/zip` lo rechaza y el
/// paquete lo lee. Un `abort` no deja rastro.
#[test]
fn zip64_end_records_for_seventy_thousand_entries() {
    let app = project("zip64");
    std::fs::write(
        app.join("main.ray"),
        r#"import std/fs;
import std/zip as stdzip;
import zip/zip;
fn main() -> int {
    let root = args()[0];
    let w = zip.create_with("${root}/many.zip", zip.Options { level: 0, deflate_limit: 1024, clock: zip.dostime_utc() }).unwrap();
    for i in 0..70000 {
        zip.add_bytes_at(w, "f/${i}", b"1", 1760000000000, 0o644).unwrap();
    }
    let s = zip.finish(w).unwrap();
    print(s.entries);
    let a = zip.open("${root}/many.zip").unwrap();
    print(zip.entries(a).len());
    print(zip.entries(a)[69999].name);
    print(zip.read(a, zip.entries(a)[69999]).unwrap() == b"1");
    zip.close(a);
    print(stdzip.open(fs.read_file_bytes("${root}/many.zip").unwrap()).is_err());
    let w2 = zip.create("${root}/gone.zip").unwrap();
    zip.add_bytes(w2, "x", b"x").unwrap();
    zip.abort(w2);
    print(fs.exists("${root}/gone.zip") || fs.exists("${root}/gone.zip.zip-tmp"));
    0
}
"#,
    )
    .unwrap();
    let root = app.join("run");
    std::fs::create_dir_all(&root).unwrap();
    let (out, err, code) = ray(&app, &["run", "main.ray", root.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(out, "70000\n70000\nf/69999\ntrue\ntrue\nfalse\n");
}
