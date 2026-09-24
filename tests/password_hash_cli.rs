//! M290 — golden de TRES motores de las contraseñas (`examples/stdlib/password_hash.ray`): los
//! vectores oficiales de PBKDF2-HMAC-SHA256 (RFC 7914 §11) y el flujo `password_hash`/
//! `password_verify`. El hash lleva sal aleatoria, así que se comprueba por verificación, no por
//! valor.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

const EXPECTED: &str = "v1 55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783\n\
v2 4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56a1d425a1225833549adb841b51c9b3176a272bdebba1d078478f62b397f33c8d\n\
iteraciones 0: true\n\
len 1025:      true\n\
formato:       true 117\n\
verify ok:     true\n\
verify mal:    false\n\
malformado:    false false false\n\
default:       600000\n";

fn example() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/stdlib/password_hash.ray")
}

fn run(flags: &[&str]) -> String {
    let path = example();
    let mut args = vec!["run"];
    args.extend_from_slice(flags);
    args.push(path.to_str().unwrap());
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
fn password_hashing_matches_the_rfc_vectors_on_vm_and_interpreter() {
    assert_eq!(run(&[]), EXPECTED, "VM");
    assert_eq!(run(&["--interp"]), EXPECTED, "intérprete");
}

/// El binario nativo da el MISMO texto: PBKDF2 va por `ray_runtime::crypto`, igual que la VM.
#[test]
fn password_hashing_is_byte_identical_on_the_native_binary() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando la parte nativa: rustc no disponible");
        return;
    }
    let base = std::env::temp_dir().join("ray_password_hash_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let bin = base.join(format!("password_hash_native{}", std::env::consts::EXE_SUFFIX));
    let path = example();
    let out = Command::new(BIN)
        .args(["build", "--native", path.to_str().unwrap(), "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza el build nativo");
    assert!(
        out.status.success(),
        "compila el nativo\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let out = Command::new(&bin).output().expect("corre el nativo");
    assert!(out.status.success(), "el nativo corre: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), EXPECTED, "nativo");
    let _ = std::fs::remove_dir_all(&base);
}
