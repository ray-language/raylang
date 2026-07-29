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
