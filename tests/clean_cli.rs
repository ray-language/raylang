//! M437: `ray clean [--dry-run]` sobre una caché nativa reubicada con `RAY_NATIVE_CACHE`.
//! La poda automática se prueba a nivel unitario en `cli::tests`; aquí, el subcomando de cara
//! al usuario: el informe, el borrado y que nunca toque nada fuera de la caché.

use std::path::PathBuf;
use std::process::Command;

fn ray(cache: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).env("RAY_NATIVE_CACHE", cache).output().expect("ray");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn clean_reports_sizes_per_target_and_removes_only_the_cache() {
    let root = std::env::temp_dir().join(format!("ray_clean_cli_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let cache = root.join("native-cache");
    let sibling = root.join("keys");
    std::fs::create_dir_all(cache.join("debug").join("deps")).unwrap();
    std::fs::create_dir_all(cache.join("aarch64-apple-ios").join("release")).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(cache.join("debug").join("deps").join("libring-0a1b.rlib"), vec![0u8; 3 * 1024 * 1024]).unwrap();
    std::fs::write(cache.join("aarch64-apple-ios").join("release").join("x"), vec![0u8; 1024 * 1024 + 1]).unwrap();
    std::fs::write(cache.join("ray-native.Cargo.lock"), b"[]").unwrap();
    std::fs::write(sibling.join("app.key"), b"secret").unwrap();

    // --dry-run: informa por target, de mayor a menor, y no borra nada.
    let (out, err, code) = ray(&cache, &["clean", "--dry-run"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains(&format!("native cache: {}", cache.display())), "{out}");
    let debug_at = out.find("  debug").expect(&out);
    let ios_at = out.find("  aarch64-apple-ios").expect(&out);
    assert!(debug_at < ios_at, "mayor primero:\n{out}");
    assert!(out.contains("3.0 MiB") && out.contains("4.0 MiB  total"), "{out}");
    assert!(!out.contains("Cargo.lock"), "lo menor de 1 MiB no se lista:\n{out}");
    assert!(cache.join("debug").is_dir());

    // Sin flag: borra la caché entera y solo la caché.
    let (out, err, code) = ray(&cache, &["clean"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("ok: removed"), "{out}");
    assert!(!cache.exists());
    assert_eq!(std::fs::read(sibling.join("app.key")).unwrap(), b"secret");

    // Ya vacía: lo dice y sale 0.
    let (out, _err, code) = ray(&cache, &["clean"]);
    assert_eq!(code, 0);
    assert!(out.contains("(empty)"), "{out}");

    // Argumento desconocido: uso y 64.
    let (_out, err, code) = ray(&cache, &["clean", "--all"]);
    assert_eq!(code, 64);
    assert!(err.contains("usage: ray clean [--dry-run]"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}
