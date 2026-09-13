//! M249 (IDEAS §89): `ray bundle --sign IDENTITY` firma el `.app` con hardened runtime, timestamp y
//! entitlements, y verifica la firma; una identidad inexistente es error 74 (no un bundle "ok" a
//! medio firmar). Solo macOS; se salta si la máquina no tiene ninguna identidad de firma (el CI).
//! La notarización (perfil de keychain + Apple) no se prueba aquí: exige credenciales reales.
#![cfg(target_os = "macos")]

use std::path::PathBuf;
use std::process::Command;

fn project(name: &str) -> PathBuf {
    // Un directorio por test: corren en paralelo dentro del mismo proceso.
    let base = std::env::temp_dir().join(format!("raylang_test_bundle_sign_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(base.join("ray.toml"), "[package]\nname = \"signed\"\nversion = \"0.1.0\"\nentry = \"src/main.ray\"\n\n[app]\nid = \"dev.rayala.signed\"\n").unwrap();
    std::fs::write(base.join("src/main.ray"), "fn main() -> int {\n    print(\"hola\");\n    0\n}\n").unwrap();
    base
}

/// La primera identidad de firma de la máquina ("Apple Development: …" o "Developer ID …"), si hay.
fn local_identity() -> Option<String> {
    let out = Command::new("security").args(["find-identity", "-v", "-p", "codesigning"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.contains('"'))?;
    let start = line.find('"')? + 1;
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

#[test]
fn bundle_signs_with_hardened_runtime_when_an_identity_is_given() {
    let Some(identity) = local_identity() else {
        eprintln!("sin identidad de firma en esta máquina: test saltado");
        return;
    };
    let base = project("ok");
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["bundle", "-o", "out", "--sign", &identity, "--without", "mimalloc,ahash,fibers"])
        .current_dir(&base)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "bundle --sign: {stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("hardened runtime"), "{stdout}");
    let app = base.join("out/signed.app");
    let info = Command::new("codesign").args(["-d", "-vv"]).arg(&app).output().unwrap();
    let details = String::from_utf8_lossy(&info.stderr);
    assert!(details.contains("(runtime)"), "sin hardened runtime:\n{details}");
    assert!(details.contains(&format!("Authority={identity}")), "firmado con otra identidad:\n{details}");
    let verify = Command::new("codesign").args(["--verify", "--deep", "--strict"]).arg(&app).status().unwrap();
    assert!(verify.success(), "codesign --verify");
}

#[test]
fn bundle_fails_loudly_with_an_unknown_identity() {
    let base = project("bad");
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["bundle", "-o", "out", "--sign", "Nope Identity (XXXXXXXXXX)", "--without", "mimalloc,ahash,fibers"])
        .current_dir(&base)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(74), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("codesign failed with identity"));
}
