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
