//! M126 — hasher INCREMENTAL de std/crypto (`sha256_init`/`sha512_init` + `hash_update` +
//! `hash_final`): el patrón que tres apps copiaban a mano (takeit → raysync → raypass) para
//! hashear archivos grandes por trozos. Se asevera el vector NIST FIPS 180-4 ("abc" troceado),
//! incremental ≡ una-pasada, y los caminos de error (final consume el handle). El estado vive en
//! ray_runtime::crypto — el MISMO código en los tres motores → digests byte-idénticos.

use std::io::Write;
use std::process::Command;

const SRC: &str = include_str!("fixtures/crypto_hasher.ray");

const EXPECTED: &[&str] = &[
    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", // NIST sha256("abc")
    "true",                                                             // incremental == una pasada
    "invalid hasher handle: 2",                                         // segundo final
    "invalid hasher handle: 2",                                         // update tras final
    "true",                                                             // sha512 troceado == una pasada
];

fn run(flags: &[&str]) -> Vec<String> {
    let mut path = std::env::temp_dir();
    path.push(format!("crypto_hasher_{}.ray", flags.join("_")));
    std::fs::File::create(&path).expect("crea").write_all(SRC.as_bytes()).expect("escribe");
    let out = Command::new(env!("CARGO_BIN_EXE_raylang")).args(flags).arg(&path).output().expect("lanza raylang");
    assert!(out.status.success(), "falló: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect()
}

#[test]
fn incremental_hasher_interpreter() {
    assert_eq!(run(&[]), EXPECTED);
}

#[test]
fn incremental_hasher_vm() {
    assert_eq!(run(&["--vm"]), EXPECTED);
}

/// El binario NATIVO (sabor default): mismo programa, misma salida byte a byte.
#[test]
fn incremental_hasher_native() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        assert!(std::env::var_os("CI").is_none(), "rustc no disponible bajo CI: falso verde");
        eprintln!("saltando incremental_hasher_native: rustc no disponible");
        return;
    }
    let mut src = std::env::temp_dir();
    src.push("crypto_hasher_native.ray");
    std::fs::write(&src, SRC).expect("escribe");
    let bin = std::env::temp_dir().join(format!("ray_hasher_{}", std::process::id()));
    let build = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["build", src.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza build --native");
    assert!(build.status.success(), "build --native falló: {}", String::from_utf8_lossy(&build.stderr));
    let out = Command::new(&bin).output().expect("corre el binario");
    let _ = std::fs::remove_file(&bin);
    assert!(out.status.success(), "el binario falló: {}", String::from_utf8_lossy(&out.stderr));
    let lines: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    assert_eq!(lines, EXPECTED, "el nativo diverge en el hasher incremental");
}
