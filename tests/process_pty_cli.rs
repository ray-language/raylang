//! M237 (ray-sublime, terminal del editor): `Cmd.pty(cols, rows)` lanza el hijo bajo un
//! pseudo-terminal — misma salida en la VM y en el binario nativo (el intérprete no tiene
//! streaming). En Windows el `Err` honesto hasta que llegue ConPTY.

use std::process::Command;

const PROG: &str = r#"import std/process;

fn collect(p: process.Proc) -> string {
    var acc = "";
    var going = true;
    while (going) {
        match (p.out.recv()) {
            Option.Some(chunk) => { acc = acc + from_utf8(chunk).unwrap_or("?"); },
            Option.None => { going = false; },
        }
    }
    acc
}

fn main() -> int {
    let p = process.cmd("sh", ["-c", "stty size; tty; printf hola"]).pty(80, 24).stream().unwrap();
    let text = collect(p);
    print(text.contains("24 80"));
    print(text.contains("/dev/"));
    print(text.contains("hola"));
    print(p.err.recv().is_none());
    match (p.wait()) { process.Exit.Code(c) => print("code ${c}"), process.Exit.Signal(s) => print("signal ${s}") }
    let q = process.cmd("sh", ["-c", "sleep 0.3; stty size"]).pty(80, 24).stream().unwrap();
    print(q.resize(120, 40).is_ok());
    print(collect(q).contains("40 120"));
    let _ = q.wait();
    let r = process.cmd("cat", []).pty(80, 24).stream().unwrap();
    let _ = r.write(b"eco\n");
    let _ = r.write(b"\x04");
    print(collect(r).contains("eco"));
    let _ = r.wait();
    let plain = process.cmd("sh", ["-c", "echo x"]).stream().unwrap();
    print(plain.resize(1, 1).is_err());
    let _ = collect(plain);
    let _ = plain.wait();
    print(process.cmd("sh", ["-c", "true"]).pty(0, 24).stream().is_err());
    0
}
"#;

const WANT: &str = "true\ntrue\ntrue\ntrue\ncode 0\ntrue\ntrue\ntrue\ntrue\ntrue\n";

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_pty_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("prog.ray"), PROG).unwrap();
    d
}

#[cfg(unix)]
#[test]
fn pty_gives_the_child_a_terminal_on_the_vm() {
    let d = dir("vm");
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).current_dir(&d).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), WANT);
}

#[cfg(unix)]
#[test]
fn pty_gives_the_child_a_terminal_natively() {
    if !Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("(sin rustc: se omite el nativo)");
        return;
    }
    let d = dir("native");
    let bin = d.join("prog_bin");
    let st = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
        .current_dir(&d)
        .output()
        .expect("build nativo");
    assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
    let out = Command::new(&bin).current_dir(&d).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo");
}

/// M238: ConPTY — el hijo ve una consola (`echo` y `mode con` responden), el resize llega y
/// `out` cierra al terminar el hijo (el vigía cierra la pseudoconsola). VM y nativo.
#[cfg(windows)]
const PROG_WIN: &str = r#"import std/process;

fn collect(p: process.Proc) -> string {
    var acc = "";
    var going = true;
    while (going) {
        match (p.out.recv()) {
            Option.Some(chunk) => { acc = acc + from_utf8(chunk).unwrap_or("?"); },
            Option.None => { going = false; },
        }
    }
    acc
}

fn main() -> int {
    let p = process.cmd("cmd", ["/c", "echo hola-pty"]).pty(80, 24).stream().unwrap();
    print(p.resize(100, 30).is_ok());
    let text = collect(p);
    print("OUT=" + to_string(text.len()) + ":" + text.replace("\r", "<CR>").replace("\n", "<LF>").replace("\x1b", "<ESC>"));
    print(text.contains("hola-pty"));
    print(p.err.recv().is_none());
    match (p.wait()) { process.Exit.Code(c) => print("code ${c}"), process.Exit.Signal(s) => print("signal ${s}") }
    let plain = process.cmd("cmd", ["/c", "echo x"]).stream().unwrap();
    print(plain.resize(1, 1).is_err());
    let _ = collect(plain);
    let _ = plain.wait();
    print(process.cmd("cmd", ["/c", "echo x"]).pty(0, 24).stream().is_err());
    0
}
"#;

#[cfg(windows)]
const WANT_WIN: &str = "true\ntrue\ntrue\ncode 0\ntrue\ntrue\n";

#[cfg(windows)]
fn strip_out(s: &str) -> String {
    s.lines().filter(|l| !l.starts_with("OUT=")).map(|l| format!("{l}\n")).collect()
}

#[cfg(windows)]
#[test]
fn conpty_gives_the_child_a_console_on_the_vm_and_natively() {
    let d = dir("win");
    std::fs::write(d.join("prog.ray"), PROG_WIN).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).current_dir(&d).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let raw = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(strip_out(&raw), WANT_WIN, "vm\n{raw}");
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = d.join("prog_bin.exe");
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&d)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin).current_dir(&d).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let raw = String::from_utf8_lossy(&out.stdout).into_owned();
        assert_eq!(strip_out(&raw), WANT_WIN, "nativo\n{raw}");
    }
}
