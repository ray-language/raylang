//! M100 fase 1d (IDEAS §53.8) — golden intérprete≡VM de `std/process` vía el CLI, sobre el
//! ejemplo determinista `examples/stdlib/process_run.ray` (el MISMO archivo que recoge el corpus
//! nativo → los tres motores quedan clavados al mismo texto). Cubre los invariantes del contrato:
//! exit≠0 es Ok, builder completo (dir/env/stdin/merge), muerte por señal como `Signal(15)`,
//! ENOENT como `Err`, timeout con Output PARCIAL, tope con `truncated`.
#![cfg(unix)]

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// La salida esperada, LITERAL (el `y\n`×5 es el prefijo de 10 octetos que deja `max_output(10)`).
const EXPECTED: &str = "code:3 [hi]\n\
    code:0 [start:IN:v1:/X]\n\
    signal:15 []\n\
    no se pudo lanzar\n\
    signal:15 timed_out [partial]\n\
    code:0 truncated [y\ny\ny\ny\ny\n]\n";

fn run(flags: &[&str]) -> String {
    let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/stdlib/process_run.ray");
    let mut args = vec!["run"];
    args.extend_from_slice(flags);
    args.push(example.to_str().unwrap());
    let out = Command::new(BIN).args(&args).output().expect("lanza el binario");
    assert!(
        out.status.success(),
        "corre sin error\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn process_example_is_golden_on_vm_and_interpreter() {
    assert_eq!(run(&[]), EXPECTED, "VM");
    assert_eq!(run(&["--interp"]), EXPECTED, "intérprete");
}

/// M100 v2 fase 2b: `stream()` — trozos por canal acotado, merge con `err` cerrado, volumen
/// (1 MB entero a través del canal), kill al grupo y ENOENT. Solo VM (las bombas usan
/// spawn/canales); el intérprete debe dar su error limpio de concurrencia, no uno confuso.
const STREAM_SRC: &str = r#"import std/process;

fn text(b: bytes) -> string {
    match (from_utf8(b)) { Result.Ok(s) => s, Result.Err(e) => "?", }
}

fn drain_text(ch: Channel<bytes>) -> string {
    var acc = "";
    var going = true;
    while (going) {
        match (recv(ch)) {
            Option.Some(b) => { acc = acc + text(b); },
            Option.None => { going = false; },
        }
    }
    acc
}

fn drain_len(ch: Channel<bytes>) -> int {
    var n = 0;
    var going = true;
    while (going) {
        match (recv(ch)) {
            Option.Some(b) => { n = n + b.len(); },
            Option.None => { going = false; },
        }
    }
    n
}

fn show_exit(e: process.Exit) -> string {
    match (e) {
        process.Exit.Code(c) => "code:" + c.to_string(),
        process.Exit.Signal(s) => "signal:" + s.to_string(),
    }
}

fn main() -> int {
    let p = match (process.cmd("sh", ["-c", "printf uno; printf dos >&2; printf tres; exit 4"]).stream()) {
        Result.Ok(p) => p,
        Result.Err(e) => { print("launch error: " + e); return 1; },
    };
    print("out=[" + drain_text(p.out) + "] err=[" + drain_text(p.err) + "]");
    print(show_exit(p.wait()));

    let m = match (process.cmd("sh", ["-c", "echo a; echo b >&2; echo c"]).merge_output().stream()) {
        Result.Ok(p) => p,
        Result.Err(e) => { print(e); return 1; },
    };
    match (recv(m.err)) {
        Option.Some(b) => print("unexpected err data"),
        Option.None => print("err closed (merge)"),
    }
    print("merged chars: " + drain_text(m.out).len().to_string());
    print(show_exit(m.wait()));

    // Volumen: 1 MB entero cruza el canal acotado (contrapresión incluida), byte a byte contado.
    let v = match (process.cmd("sh", ["-c", "yes | head -c 1000000"]).stream()) {
        Result.Ok(p) => p,
        Result.Err(e) => { print(e); return 1; },
    };
    print("bytes: " + drain_len(v.out).to_string());
    let _ = drain_len(v.err);
    print(show_exit(v.wait()));

    let k = match (process.cmd("sh", ["-c", "sleep 30 & wait"]).stream()) {
        Result.Ok(p) => p,
        Result.Err(e) => { print(e); return 1; },
    };
    k.kill(false);
    print(show_exit(k.wait()));

    match (process.cmd("no-such-binary-v2", []).stream()) {
        Result.Ok(p) => print("unexpected ok"),
        Result.Err(e) => print("stream launch failed as expected"),
    }
    0
}
"#;

const STREAM_EXPECTED: &str = "out=[unotres] err=[dos]\n\
    code:4\n\
    err closed (merge)\n\
    merged chars: 6\n\
    code:0\n\
    bytes: 1000000\n\
    code:0\n\
    signal:15\n\
    stream launch failed as expected\n";

#[test]
fn process_stream_is_golden_on_vm_and_interp_rejects_cleanly() {
    let base = std::env::temp_dir().join("ray_process_stream_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let prog = base.join("stream.ray");
    std::fs::write(&prog, STREAM_SRC).unwrap();

    let out = Command::new(BIN).args(["run", prog.to_str().unwrap()]).output().expect("lanza el binario");
    assert!(out.status.success(), "VM ok\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), STREAM_EXPECTED, "VM");

    // El intérprete rechaza el streaming con su error de concurrencia, no con uno confuso.
    let out = Command::new(BIN).args(["run", "--interp", prog.to_str().unwrap()]).output().expect("lanza el binario");
    assert!(!out.status.success(), "el intérprete debe rechazarlo");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("requires the VM"), "mensaje de concurrencia esperado, got: {err}");
}
