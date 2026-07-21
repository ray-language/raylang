//! Prueba de SCRAM-SHA-256 (`examples/web/scram.ray`, M32.1a — el mecanismo de autenticación de
//! PostgreSQL). Apila PBKDF2 + HMAC-SHA256 + SHA-256 + base64. El demo ejecuta el ejemplo COMPLETO del
//! RFC 7677 §3 (usuario "user", contraseña "pencil", nonce y server-first fijos); el test exige que el
//! `client-final` (con la prueba) sea byte-idéntico al del RFC y que la firma del servidor se verifique.

use std::process::Command;

const EXPECTED: &[&str] = &[
    "n,,n=user,r=rOprNGfwEbeRWgbNEkqO",
    "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=",
    "server verificado",
];

fn run(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/web/scram_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta scram_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    (lines, out.status.success())
}

const REJECTIONS: &[&str] = &[
    "n,,n=ad=2Ca=3Dx,r=clientnonce", // M75: el usuario se escapa (',' → =2C, '=' → =3D)
    "nonce rechazado",              // el nonce del servidor debe extender el del cliente (RFC 5802 §5.1)
    "iter rechazado",               // iteraciones absurdas = Err (bomba de CPU)
    "verify rechazado",             // scram_verify con server_sig vacío = false
];

fn run_reject(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/web/scram_reject_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta scram_reject_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    (lines, out.status.success())
}

/// M75 — salvaguardas de la revisión en frío. Rápido (todos los casos retornan antes del PBKDF2), va
/// en la suite por defecto por ambos motores.
#[test]
fn scram_rejections_interpreter() {
    let (lines, ok) = run_reject(&[]);
    assert!(ok, "scram_reject_demo falló en el intérprete");
    assert_eq!(lines, REJECTIONS);
}

#[test]
fn scram_rejections_vm() {
    let (lines, ok) = run_reject(&["--vm"]);
    assert!(ok, "scram_reject_demo falló en la VM");
    assert_eq!(lines, REJECTIONS);
}

/// Ambos van `#[ignore]`: este test valida SCRAM contra el vector del RFC 7677 con i=4096 (PBKDF2 lento,
/// ~8–26 s por motor). La cobertura del CÓDIGO SCRAM en la suite por defecto la da `postgres_cli.rs`
/// (e2e a i=64, rápido); la validación contra el RFC se corre a demanda con `-- --ignored`.
#[test]
#[ignore]
fn scram_interpreter() {
    let (lines, ok) = run(&[]);
    assert!(ok, "scram_demo falló en el intérprete");
    assert_eq!(lines, EXPECTED);
}

#[test]
#[ignore]
fn scram_vm() {
    let (lines, ok) = run(&["--vm"]);
    assert!(ok, "scram_demo falló en la VM");
    assert_eq!(lines, EXPECTED);
}
