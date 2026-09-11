//! M235 (ray-sublime #69): `fs.copy_all` (árbol completo, espejo de `remove_all`) y el builtin
//! `platform()` del prelude — idénticos en intérprete, VM y binario nativo.

use std::process::Command;

fn project(name: &str) -> std::path::PathBuf {
    // Un directorio por test: los dos corren en paralelo y comparten binario, no árbol.
    let base = std::env::temp_dir().join(format!("raylang_test_fs_copy_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("tree/sub/deep")).unwrap();
    std::fs::write(base.join("tree/a.txt"), "a").unwrap();
    std::fs::write(base.join("tree/sub/b.txt"), "bb").unwrap();
    std::fs::write(base.join("tree/sub/deep/c.txt"), "ccc").unwrap();
    std::fs::write(
        base.join("prog.ray"),
        "import std/fs;\nfn main() -> int {\n    print(platform());\n    let _ = fs.remove_all(\"out\");\n    match (fs.copy_all(\"tree\", \"out\")) { Result.Ok(n) => print(\"copied ${n}\"), Result.Err(e) => print(e) }\n    print(fs.read_file(\"out/sub/deep/c.txt\").unwrap_or(\"?\"));\n    print(fs.is_dir(\"out/sub\"));\n    match (fs.copy_all(\"tree\", \"out\")) { Result.Ok(n) => print(\"again ${n}\"), Result.Err(e) => print(e) }\n    match (fs.copy_all(\"tree/a.txt\", \"out2\")) { Result.Ok(_) => print(\"bad\"), Result.Err(e) => print(e) }\n    match (fs.copy_all(\"nope\", \"out3\")) { Result.Ok(_) => print(\"bad\"), Result.Err(e) => print(e) }\n    0\n}\n",
    )
    .unwrap();
    base
}

fn want() -> String {
    format!(
        "{}\ncopied 3\nccc\ntrue\nagain 3\nfs: copy_all: 'tree/a.txt' is not a directory\nfs: copy_all: 'nope' is not a directory\n",
        std::env::consts::OS
    )
}

#[test]
fn copy_all_and_platform_on_interpreter_and_vm() {
    let base = project("engines");
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(engine).current_dir(&base).output().unwrap();
        assert!(out.status.success(), "{engine:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), want(), "{engine:?}");
    }
}

#[test]
fn copy_all_and_platform_natively() {
    if !Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("(sin rustc: se omite el nativo)");
        return;
    }
    let base = project("native");
    let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
    let st = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
        .current_dir(&base)
        .output()
        .expect("build nativo");
    assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
    let out = Command::new(&bin).current_dir(&base).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), want(), "nativo");
}
