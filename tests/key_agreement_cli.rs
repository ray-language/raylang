//! M114 — golden de TRES motores del acuerdo de claves (`examples/stdlib/key_agreement.ray`).
//!
//! La primera mitad del ejemplo son los **vectores oficiales** —RFC 7748 §6.1 (X25519) y RFC 5869 A.1/A.3
//! (HKDF-SHA256)—, así que este test no solo comprueba que los tres motores coinciden entre sí: los clava
//! al valor que dice el estándar. La segunda mitad es la receta de canal seguro (firma de la efímera,
//! clave por sentido, nonce contador), cuyas aserciones cubren las propiedades que la sostienen.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// Salida LITERAL. Las tres primeras líneas hexadecimales son los vectores de RFC 7748 §6.1 y las dos
/// siguientes los OKM de RFC 5869 A.1 y A.3, copiados de los propios RFC.
const EXPECTED: &str = "\
pub A  8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a
pub B  de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f
dh A-B 4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742
iguales: true
okm A1 3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865
okm A3 8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8
clave corta:    true
orden pequeño:  true
hkdf len 0:     true
hkdf len 8161:  true
ct_eq distinto: false
identidad de A verificada: true
misma firma como si fuera de B: false
mismo secreto en ambos lados: true
claves de sentido distintas: true
B descifra: hola desde el otro lado
con la clave del otro sentido: true
";

fn example() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/stdlib/key_agreement.ray")
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
fn key_agreement_matches_the_rfc_vectors_on_vm_and_interpreter() {
    assert_eq!(run(&[]), EXPECTED, "VM");
    assert_eq!(run(&["--interp"]), EXPECTED, "intérprete");
}

/// El binario nativo debe dar el MISMO texto: el acuerdo de claves va por `ray_runtime::crypto`, igual
/// que la VM, así que la byte-identidad es el contrato (PRODUCTION.md), no una coincidencia.
#[test]
fn key_agreement_is_byte_identical_on_the_native_binary() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando la parte nativa: rustc no disponible");
        return;
    }
    let base = std::env::temp_dir().join("ray_key_agreement_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let bin = base.join(format!("key_agreement_native{}", std::env::consts::EXE_SUFFIX));
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
    let out = Command::new(&bin).output().expect("ejecuta el binario nativo");
    assert!(out.status.success(), "el nativo corre\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), EXPECTED, "nativo");
    let _ = std::fs::remove_dir_all(&base);
}
