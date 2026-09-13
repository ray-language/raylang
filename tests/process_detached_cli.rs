//! M246 (IDEAS §89): `Cmd.spawn_detached()` — el hijo sobrevive al padre — y `process.self_command()`
//! — cómo relanzarse, honesto con el motor (`[ray, "run", entrada]` bajo la VM/intérprete, `[exe]`
//! en el binario nativo). Se prueba en los tres motores: el programa lanza un hijo que escribe un
//! marcador DESPUÉS de que el padre haya terminado, y el test espera y lo lee.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[cfg(unix)]
const SPAWN: &str = r#"process.cmd("sh", ["-c", "sleep 0.4; echo alive > " + marker]).spawn_detached()"#;
#[cfg(windows)]
const SPAWN: &str = r#"process.cmd("cmd", ["/c", "ping -n 2 127.0.0.1 > nul & echo alive > " + marker]).spawn_detached()"#;

#[cfg(unix)]
const MISSING: &str = "/definitely/not/here";
#[cfg(windows)]
const MISSING: &str = "C:/definitely/not/here.exe";

fn program() -> String {
    format!(
        r#"import std/process;
import std/fs;
fn main() -> int {{
    let c = process.self_command();
    print(c.len());
    print(c[c.len() - 1].ends_with("prog.ray"));
    let marker = args()[0];
    match ({SPAWN}) {{
        Result.Ok(pid) => print(pid > 0),
        Result.Err(e) => print("err: " + e),
    }}
    match (process.cmd("{MISSING}", []).spawn_detached()) {{
        Result.Ok(_) => print("bad"),
        Result.Err(e) => print(e.contains("not")),
    }}
    print(fs.exists(marker));
    0
}}
"#
    )
}

fn project(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("raylang_test_detached_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("prog.ray"), program()).unwrap();
    base
}

/// Espera a que el hijo desacoplado escriba el marcador (hasta 5 s) y devuelve su contenido.
fn wait_marker(path: &Path) -> String {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(5) {
        if let Ok(s) = std::fs::read_to_string(path)
            && s.contains("alive")
        {
            return s.trim().to_string();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    String::new()
}

fn check(base: &Path, argv: &[&str], want_self: &str) {
    let marker = base.join("marker.txt");
    let _ = std::fs::remove_file(&marker);
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(argv)
        .arg(marker.to_str().unwrap())
        .current_dir(base)
        .output()
        .unwrap();
    assert!(out.status.success(), "{argv:?}: {}", String::from_utf8_lossy(&out.stderr));
    // El marcador NO existe cuando el padre termina (el hijo duerme antes de escribirlo)…
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{want_self}\ntrue\ntrue\nfalse\n"), "{argv:?}");
    // …y aparece después: el hijo sobrevivió al padre.
    assert_eq!(wait_marker(&marker), "alive", "{argv:?}: el hijo desacoplado no escribió el marcador");
}

#[test]
fn detached_child_outlives_the_vm_and_the_interpreter() {
    let base = project("engines");
    // Bajo la toolchain: [ray, "run", entrada] / [ray, "run", "--interp", entrada].
    check(&base, &["run", "prog.ray"], "3\ntrue");
    check(&base, &["run", "--interp", "prog.ray"], "4\ntrue");
}

#[test]
fn detached_child_outlives_the_native_binary() {
    let base = project("native");
    let bin = base.join(if cfg!(windows) { "prog_bin.exe" } else { "prog_bin" });
    let build = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "--native", "prog.ray", "-o", bin.to_str().unwrap(), "--without", "mimalloc,ahash,fibers"])
        .current_dir(&base)
        .output()
        .unwrap();
    assert!(build.status.success(), "build --native: {}", String::from_utf8_lossy(&build.stderr));
    let marker = base.join("marker.txt");
    let out = Command::new(&bin).arg(marker.to_str().unwrap()).current_dir(&base).output().unwrap();
    assert!(out.status.success(), "native: {}", String::from_utf8_lossy(&out.stderr));
    // Nativo: self_command = [exe] (un elemento, que no es prog.ray).
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\nfalse\ntrue\ntrue\nfalse\n");
    assert_eq!(wait_marker(&marker), "alive", "nativo: el hijo desacoplado no escribió el marcador");
}
