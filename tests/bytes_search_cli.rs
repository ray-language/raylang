//! M245 (ray-sublime #71): `b.index_of(needle)` / `b.starts_with(prefix)` sobre `bytes` y
//! `time.monotonic_millis()` — idénticos en intérprete, VM y binario nativo.

use std::process::Command;

const PROGRAM: &str = r#"import std/time;
fn show(o: Option<int>) -> string {
    match (o) { Option.Some(i) => to_string(i), Option.None => "none" }
}
fn main() -> int {
    let buf = "Content-Length: 5\r\n\r\nhello".to_bytes();
    let sep = "\r\n\r\n".to_bytes();
    print(show(buf.index_of(sep)));
    print(show(buf.index_of("hello".to_bytes())));
    print(show(buf.index_of("nope".to_bytes())));
    print(show(buf.index_of("".to_bytes())));
    print(show("".to_bytes().index_of("a".to_bytes())));
    print(show(bytes_of([1, 2, 1, 2, 3]).index_of(bytes_of([1, 2, 3]))));
    print(buf.starts_with("Content".to_bytes()));
    print(buf.starts_with("content".to_bytes()));
    print(buf.starts_with("".to_bytes()));
    print("".to_bytes().starts_with("x".to_bytes()));
    let t0 = time.monotonic_millis();
    print(time.monotonic_millis() - t0 < 1000);
    print(time.monotonic_millis() - time.monotonic() < 5);
    0
}
"#;

const WANT: &str = "17\n21\nnone\n0\nnone\n2\ntrue\nfalse\ntrue\nfalse\ntrue\ntrue\n";

fn project(name: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("raylang_test_bytes_search_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("prog.ray"), PROGRAM).unwrap();
    base
}

#[test]
fn bytes_search_on_interpreter_and_vm() {
    let base = project("engines");
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(engine).current_dir(&base).output().unwrap();
        assert!(out.status.success(), "{engine:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{engine:?}");
    }
}

#[test]
fn bytes_search_natively() {
    let base = project("native");
    let build = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "--native", "prog.ray", "-o", "prog_bin", "--without", "mimalloc,ahash,fibers"])
        .current_dir(&base)
        .output()
        .unwrap();
    assert!(build.status.success(), "build --native: {}", String::from_utf8_lossy(&build.stderr));
    let out = Command::new(base.join("prog_bin")).current_dir(&base).output().unwrap();
    assert!(out.status.success(), "native: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "native");
}
