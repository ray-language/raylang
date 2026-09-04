//! M131 — normalización Unicode de `std/text` (NFC/NFD/NFKC/NFKD vía ray-runtime, crate
//! `unicode-normalization`): golden determinista en ambos motores + el binario nativo byte a
//! byte (la feature `unicode` se detecta por USO). El caso de uso que la pidió (raysite,
//! IDEAS §71.5): slug accent-insensitive = NFD + descartar combinantes.

use std::process::Command;

const SRC: &str = r#"
import std/text;

fn main() -> int {
    let precomposed = "café";
    let decomposed = text.nfd(precomposed);
    // NFD separa el acento: 4 chars -> 5; NFC lo recompone al original.
    print(to_string(precomposed.len()) + " " + to_string(decomposed.len()));
    print(to_string(text.nfc(decomposed) == precomposed));
    print(to_string(decomposed == precomposed));
    // Las formas K aplanan variantes de presentación (ligadura fi, superíndice 2).
    print(text.nfkc("ﬁn ²"));
    // Idempotencia: normalizar dos veces = una.
    print(to_string(text.nfkd(text.nfkd("ﬁn ² épico")) == text.nfkd("ﬁn ² épico")));
    // El slug del hallazgo: NFD + descartar marcas combinantes (U+0300..U+036F) + lower.
    var slug = "";
    for c in text.nfd("Canción Épica") .chars() {
        let code = char_code(c);
        if (!(code >= 768 && code <= 879)) {
            slug = slug + to_string(c);
        }
    }
    print(slug.to_lower());
    0
}
"#;

const EXPECTED: &[&str] = &["4 5", "true", "false", "fin 2", "true", "cancion epica"];

fn run(vm: bool) -> (Vec<String>, bool) {
    let mut path = std::env::temp_dir();
    path.push(format!("ray_unicode_{}.ray", if vm { "vm" } else { "interp" }));
    std::fs::write(&path, SRC).expect("escribe el fuente");
    let flag = if vm { "--vm" } else { "--interp" };
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(flag)
        .arg(&path)
        .output()
        .expect("ejecuta");
    let lines = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    (lines, out.status.success())
}

#[test]
fn unicode_normalization_both_engines() {
    for vm in [false, true] {
        let (lines, ok) = run(vm);
        assert!(ok, "falló (vm={vm})");
        assert_eq!(lines, EXPECTED, "vm={vm}");
    }
}

/// El binario NATIVO: mismo programa, misma salida; la feature `unicode` entra por USO.
#[test]
fn unicode_normalization_native() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        assert!(std::env::var_os("CI").is_none(), "rustc no disponible bajo CI: falso verde");
        eprintln!("saltando unicode_normalization_native: rustc no disponible");
        return;
    }
    let mut src_path = std::env::temp_dir();
    src_path.push("ray_unicode_native.ray");
    std::fs::write(&src_path, SRC).expect("escribe el fuente");
    let bin = std::env::temp_dir().join(format!("ray_unicode_{}{}", std::process::id(), std::env::consts::EXE_SUFFIX));
    let build = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["build", src_path.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza build --native");
    let build_out = String::from_utf8_lossy(&build.stdout).into_owned();
    assert!(build.status.success(), "build --native falló: {}", String::from_utf8_lossy(&build.stderr));
    assert!(build_out.contains("unicode"), "la feature `unicode` debe activarse por uso: {build_out}");
    let out = Command::new(&bin).output().expect("corre el binario nativo");
    let _ = std::fs::remove_file(&bin);
    assert!(out.status.success(), "el binario nativo falló: {}", String::from_utf8_lossy(&out.stderr));
    let lines: Vec<&str> = std::str::from_utf8(&out.stdout).unwrap_or("").lines().collect();
    assert_eq!(lines, EXPECTED, "el nativo diverge de la VM en la normalización");
}
