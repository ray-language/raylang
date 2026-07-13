//! Pruebas de AWS Signature V4 (`examples/web/sigv4.ray`, M20.9). Cómputo puro determinista → se corre
//! `examples/web/sigv4_demo.ray` por ambos motores y se compara con los vectores de referencia: el caso
//! 1 es `get-vanilla` de la suite oficial de AWS; el 2 (query + cuerpo) lo da la referencia de Python.

use std::process::Command;

const ESPERADO: &[&str] = &[
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
     SignedHeaders=host;x-amz-date, \
     Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31",
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
     SignedHeaders=host;x-amz-date, \
     Signature=9eb84a4204a201b2eee292a6fd7bd72b820d6580ec32367b8ad37f7be64f8aed",
];

fn run(flags: &[&str]) -> Vec<String> {
    let demo = format!("{}/examples/web/sigv4_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta sigv4_demo.ray");
    assert!(
        out.status.success(),
        "sigv4_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn sigv4_interpreter() {
    assert_eq!(run(&[]), ESPERADO);
}

#[test]
fn sigv4_vm() {
    assert_eq!(run(&["--vm"]), ESPERADO);
}
