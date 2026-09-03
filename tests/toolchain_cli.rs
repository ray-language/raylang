//! Pruebas de `ray toolchain` y de la resolución de `cargo`/`rustc` en `ray build --native`
//! (M170, IDEAS §85). Todas **offline**: la máquina "sin Rust" se simula con un `PATH` pelado y
//! un `RAY_TOOLCHAIN_HOME` vacío; el vendor, con un directorio con la forma esperada y un `cargo`
//! falso vía `RAY_CARGO` que vuelca el `.cargo/config.toml` del proyecto generado. La instalación
//! real (descarga rustup) no se automatiza: se validó a mano (DESIGN §162).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn ray(args: &[&str], envs: &[(&str, &str)]) -> (String, String, i32) {
    let mut cmd = Command::new(BIN);
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("lanza el binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ray_toolchain_cli_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn toolchain_usage_errors() {
    let (_o, err, code) = ray(&["toolchain"], &[]);
    assert_eq!(code, 64);
    assert!(err.contains("usage: ray toolchain"), "{err}");
    let (_o, err, code) = ray(&["toolchain", "bogus"], &[]);
    assert_eq!(code, 64, "{err}");
    let (_o, err, code) = ray(&["toolchain", "install", "--rust"], &[]);
    assert_eq!(code, 64, "--rust sin canal = uso\n{err}");
    let (_o, err, code) = ray(&["toolchain", "install", "--what"], &[]);
    assert_eq!(code, 64, "{err}");
}

#[test]
fn status_reports_home_and_tools() {
    // En la máquina de CI/dev hay cargo en el PATH (los tests corren bajo cargo) → exit 0.
    let home = temp_dir("status");
    let (out, err, code) = ray(&["toolchain", "status"], &[("RAY_TOOLCHAIN_HOME", home.to_str().unwrap())]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains(&format!("toolchain home: {}", home.display())), "{out}");
    assert!(out.contains("cargo: ") && out.contains("[PATH]"), "cargo del PATH:\n{out}");
    assert!(out.contains("rustc: "), "{out}");
    assert!(out.contains("system linker:"), "{out}");
    assert!(out.contains(&format!("ray-runtime vendor ({VERSION}): not installed")), "{out}");
    let _ = fs::remove_dir_all(&home);
}

#[cfg(unix)]
#[test]
fn machine_without_rust_gets_the_install_hint() {
    // PATH pelado (sin ~/.cargo/bin) + home privado vacío = el equipo recién instalado.
    let home = temp_dir("norust");
    let envs = [("PATH", "/usr/bin:/bin"), ("RAY_TOOLCHAIN_HOME", home.to_str().unwrap())];
    let (out, _err, code) = ray(&["toolchain", "status"], &envs);
    assert_eq!(code, 1, "sin cargo/rustc → 1\n{out}");
    assert!(out.contains("cargo: not found") && out.contains("rustc: not found"), "{out}");
    assert!(out.contains("ray toolchain install"), "{out}");

    let prog = home.join("hello.ray");
    fs::write(&prog, "fn main() { print(\"hi\") }\n").unwrap();
    let out_bin = home.join("hello_bin");
    // Vía Cargo (mimalloc/ahash/fibers por defecto): falta cargo → 65 + las dos pistas.
    let (_o, err, code) = ray(&["build", "--native", prog.to_str().unwrap(), "-o", out_bin.to_str().unwrap()], &envs);
    assert_eq!(code, 65, "{err}");
    assert!(err.contains("cargo not found"), "{err}");
    assert!(err.contains("ray toolchain install"), "{err}");
    assert!(err.contains("--without mimalloc,ahash,fibers"), "la vía rustc pelado como alternativa:\n{err}");
    // Vía rustc pelado: tampoco hay rustc → 65 + pista.
    let (_o, err, code) = ray(
        &["build", "--native", prog.to_str().unwrap(), "-o", out_bin.to_str().unwrap(), "--without", "mimalloc,ahash,fibers"],
        &envs,
    );
    assert_eq!(code, 65, "{err}");
    assert!(err.contains("rustc not found") && err.contains("ray toolchain install"), "{err}");
    assert!(!out_bin.exists());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_is_a_noop_when_cargo_is_on_path() {
    // Con cargo en el PATH no descarga nada (`--no-vendor` evita el intento de red del vendor).
    let home = temp_dir("noop");
    let (out, err, code) = ray(&["toolchain", "install", "--no-vendor"], &[("RAY_TOOLCHAIN_HOME", home.to_str().unwrap())]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("cargo already available"), "{out}");
    assert!(out.contains("--force"), "{out}");
    assert!(!home.join("cargo").exists() && !home.join("rustup").exists(), "no instaló nada");
    let _ = fs::remove_dir_all(&home);
}

#[cfg(unix)]
#[test]
fn native_build_uses_the_installed_vendor_and_ray_cargo() {
    use std::os::unix::fs::PermissionsExt;
    let home = temp_dir("vendor");
    // Un vendor "instalado": vendor/<versión>/{vendor/, Cargo.lock}.
    let vdir = home.join("vendor").join(VERSION);
    fs::create_dir_all(vdir.join("vendor")).unwrap();
    fs::write(vdir.join("Cargo.lock"), "# fake lock\n").unwrap();
    // Un `cargo` falso que vuelca el config del proyecto generado y "falla": prueba que RAY_CARGO
    // manda sobre el PATH y que el proyecto lleva el vendor. Sale 3 (≠ éxito) para no intentar
    // copiar un binario que no existe.
    let fake = home.join("fake-cargo");
    fs::write(&fake, "#!/bin/sh\necho FAKE-CARGO \"$@\" >&2\ncat .cargo/config.toml >&2\ncat Cargo.lock >&2\nexit 3\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let prog = home.join("hello.ray");
    fs::write(&prog, "fn main() { print(\"hi\") }\n").unwrap();
    let out_bin = home.join("hello_bin");
    let envs = [("RAY_TOOLCHAIN_HOME", home.to_str().unwrap()), ("RAY_CARGO", fake.to_str().unwrap())];
    let (_o, err, code) = ray(&["build", "--native", prog.to_str().unwrap(), "-o", out_bin.to_str().unwrap()], &envs);
    assert_eq!(code, 65, "{err}");
    assert!(err.contains("FAKE-CARGO build"), "usó RAY_CARGO:\n{err}");
    assert!(err.contains("replace-with = \"ray-vendor\""), "config del vendor:\n{err}");
    assert!(err.contains(&format!("{}/vendor\"", vdir.display())), "apunta al directorio del vendor:\n{err}");
    assert!(err.contains("# fake lock"), "usa el Cargo.lock del vendor:\n{err}");
    assert!(err.contains("cargo failed (code 3)"), "{err}");

    // El status lo lista como instalado y muestra RAY_CARGO como origen.
    let (out, _e, _c) = ray(&["toolchain", "status"], &envs);
    assert!(out.contains(&format!("ray-runtime vendor ({VERSION}): {}", vdir.display())), "{out}");
    assert!(out.contains("[RAY_CARGO]"), "{out}");
    let _ = fs::remove_dir_all(&home);
}
