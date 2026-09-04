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

/// M100 v2 (fases 2b-2d): `stream()` — trozos por canal acotado (ACUMULADOS: el troceo del pipe
/// no es determinista, el contenido sí), merge con `err` cerrado, 1 MB entero cruzando el canal,
/// kill al grupo y ENOENT. Corre el MISMO ejemplo que recoge el corpus nativo
/// (examples/stdlib/process_stream.ray) → VM y nativo quedan clavados al mismo texto. Solo VM
/// (las bombas usan spawn/canales); el intérprete debe dar su error limpio de concurrencia.
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
    let prog = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/stdlib/process_stream.ray");

    let out = Command::new(BIN).args(["run", prog.to_str().unwrap()]).output().expect("lanza el binario");
    assert!(out.status.success(), "VM ok\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), STREAM_EXPECTED, "VM");

    // El intérprete rechaza el streaming con su error de concurrencia, no con uno confuso.
    let out = Command::new(BIN).args(["run", "--interp", prog.to_str().unwrap()]).output().expect("lanza el binario");
    assert!(!out.status.success(), "el intérprete debe rechazarlo");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("requires the VM"), "mensaje de concurrencia esperado, got: {err}");
}

/// M100 fase 2e: la cosecha ESTRUCTURAL. (1) Una hermana falla → el scope cancela y MATA al GRUPO
/// del hijo (el marker de 1 s nunca se escribe). (2) Un hijo "demonizado" (cierra sus pipes: las
/// bombas acaban y el scope cierra con éxito) tampoco sobrevive al scope sin `wait()`. Se asevera
/// en la VM y, si hay rustc, en el binario nativo (con fibras, el default).
const CANCEL_SRC_TEMPLATE: &str = r#"import std/process;
import std/time;
import std/fs;

fn main() -> int {
    let r = try_call(fn() -> int {
        scope(fn() -> int {
            let p = match (process.cmd("sh", ["-c", "sleep 1; echo alive > @M1@"]).stream()) {
                Result.Ok(p) => p,
                Result.Err(e) => { panic("launch: " + e); },
            };
            spawn(fn() { panic("boom"); });
            0
        })
    });
    match (r) {
        Result.Ok(v) => print("unexpected ok"),
        Result.Err(e) => print("scope failed as expected"),
    }
    let s = scope(fn() -> int {
        let p = match (process.cmd("sh", ["-c", "exec >/dev/null 2>&1; sleep 1; echo alive > @M2@"]).stream()) {
            Result.Ok(p) => p,
            Result.Err(e) => { panic("launch: " + e); },
        };
        var going = true;
        while (going) {
            match (recv(p.out)) {
                Option.Some(b) => { },
                Option.None => { going = false; },
            }
        }
        7
    });
    print("scope ok: " + s.to_string());
    time.sleep(1500);
    print("marker1: " + fs.exists("@M1@").to_string());
    print("marker2: " + fs.exists("@M2@").to_string());
    0
}
"#;

const CANCEL_EXPECTED: &str =
    "scope failed as expected\nscope ok: 7\nmarker1: false\nmarker2: false\n";

#[test]
fn scope_cancellation_kills_and_reaps_the_child_group() {
    let base = std::env::temp_dir().join("ray_process_cancel_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let src = CANCEL_SRC_TEMPLATE
        .replace("@M1@", base.join("marker1").to_str().unwrap())
        .replace("@M2@", base.join("marker2").to_str().unwrap());
    let prog = base.join("cancel.ray");
    std::fs::write(&prog, &src).unwrap();

    let out = Command::new(BIN).args(["run", prog.to_str().unwrap()]).output().expect("lanza el binario");
    assert!(out.status.success(), "VM ok\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), CANCEL_EXPECTED, "VM");

    // Nativo (fibras por defecto), si hay rustc en la máquina.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando la parte nativa: rustc no disponible");
        return;
    }
    let _ = std::fs::remove_file(base.join("marker1"));
    let _ = std::fs::remove_file(base.join("marker2"));
    let bin = base.join(format!("cancel_native{}", std::env::consts::EXE_SUFFIX));
    let out = Command::new(BIN)
        .args(["build", "--native", prog.to_str().unwrap(), "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza el build");
    assert!(out.status.success(), "build nativo ok\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(&bin).output().expect("lanza el nativo");
    assert!(out.status.success(), "nativo ok\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), CANCEL_EXPECTED, "nativo");
}

/// M100 v3: **sesión persistente** sobre un hijo VIVO (`stdin_pipe` + `write` + `close_stdin`) —
/// el esqueleto de un cliente MCP/LSP: se escribe una petición, se lee SU respuesta, y el hijo
/// sigue vivo para la siguiente (con el stdin de la v2 —escribir-y-cerrar— la segunda vuelta
/// sería imposible). Cubre además el EOF explícito y que un `write` tras la muerte del hijo sea
/// `Err` (EPIPE visible), no un silencio.
const SESSION_SRC: &str = r#"import std/process;

fn main() -> int {
    match (process.cmd("cat", []).stdin_pipe().stream()) {
        Result.Err(e) => { print("spawn: " + e); 1 },
        Result.Ok(p) => {
            var i = 1;
            while (i <= 3) {
                match (p.write("req ${i}\n".to_bytes())) {
                    Result.Err(e) => { print("write: " + e); },
                    Result.Ok(n) => { print("-> ${n}"); },
                }
                match (recv(p.out)) {
                    Option.Some(chunk) => { print("<- " + from_utf8(chunk).unwrap_or("?").trim()); },
                    Option.None => { print("<- (closed)"); },
                }
                i = i + 1;
            }
            p.close_stdin();
            match (p.wait()) {
                process.Exit.Code(c) => { print("exit ${c}"); },
                process.Exit.Signal(s) => { print("signal ${s}"); },
            }
            // El hijo ya murió: escribir es un Err visible (EPIPE), nunca un silencio.
            match (p.write(b"tarde")) {
                Result.Err(_) => print("write-after-exit: err"),
                Result.Ok(_) => print("write-after-exit: OK (INESPERADO)"),
            }
            0
        },
    }
}
"#;

const SESSION_EXPECTED: &str = "-> 6\n<- req 1\n-> 6\n<- req 2\n-> 6\n<- req 3\nexit 0\n\
                                write-after-exit: err\n";

#[test]
fn writable_stdin_keeps_a_session_alive_on_vm_and_native() {
    let base = std::env::temp_dir().join("ray_process_session_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let prog = base.join("session.ray");
    std::fs::write(&prog, SESSION_SRC).unwrap();

    let out = Command::new(BIN).args(["run", prog.to_str().unwrap()]).output().expect("lanza el binario");
    assert!(out.status.success(), "VM ok\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), SESSION_EXPECTED, "VM");

    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando la parte nativa: rustc no disponible");
        return;
    }
    // Los DOS modelos de concurrencia nativos: fibras (default) e hilo-por-tarea.
    for flags in [vec![], vec!["--without".to_string(), "fibers".to_string()]] {
        let stem = if flags.is_empty() { "session_native" } else { "session_native_nf" };
        let bin = base.join(format!("{stem}{}", std::env::consts::EXE_SUFFIX));
        let mut args: Vec<String> = vec![
            "build".into(), "--native".into(), prog.to_str().unwrap().into(),
            "-o".into(), bin.to_str().unwrap().into(),
        ];
        args.extend(flags.clone());
        let out = Command::new(BIN).args(&args).output().expect("lanza el build");
        assert!(out.status.success(), "build nativo ok\nstderr: {}", String::from_utf8_lossy(&out.stderr));
        let out = Command::new(&bin).output().expect("lanza el nativo");
        assert_eq!(String::from_utf8_lossy(&out.stdout), SESSION_EXPECTED, "nativo {flags:?}");
    }
}
