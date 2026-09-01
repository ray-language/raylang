//! M156 (§80b Android) — `ray build --native --lib --target aarch64-linux-android`: el
//! programa raylang como cdylib (.so) cargable por `System.loadLibrary`. Va `#[ignore]`
//! porque exige el NDK + el target rustup (se salta con aviso si faltan); se corre a mano:
//! `cargo test --test android_lib_cli -- --ignored`. Verifica el contrato completo del .so:
//! los símbolos JNI en la TABLA DINÁMICA (fat-LTO no debe barrerlos — C3 del plan) y la
//! alineación de 16KB que Android 15+ exige (C4).

use std::path::PathBuf;
use std::process::Command;

fn ndk_bin() -> Option<PathBuf> {
    let host = if cfg!(target_os = "macos") { "darwin-x86_64" } else { "linux-x86_64" };
    let candidates = [
        std::env::var("ANDROID_NDK_HOME").ok().map(PathBuf::from),
        std::env::var("HOME").ok().map(|h| {
            let ndk = PathBuf::from(h).join("Library/Android/sdk/ndk");
            let mut vs: Vec<_> = std::fs::read_dir(&ndk)
                .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
                .unwrap_or_default();
            vs.sort();
            vs.pop().unwrap_or(ndk)
        }),
    ];
    for c in candidates.into_iter().flatten() {
        let bin = c.join("toolchains/llvm/prebuilt").join(host).join("bin");
        if bin.is_dir() {
            return Some(bin);
        }
    }
    None
}

#[test]
#[ignore = "needs the Android NDK + rustup target; run by hand: cargo test --test android_lib_cli -- --ignored"]
fn the_android_cdylib_exports_the_jni_symbols_aligned_to_16k() {
    let Some(ndk) = ndk_bin() else {
        eprintln!("skip: Android NDK not found");
        return;
    };
    let targets = Command::new("rustup").args(["target", "list", "--installed"]).output();
    let targets =
        targets.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    if !targets.contains("aarch64-linux-android") {
        eprintln!("skip: the aarch64-linux-android rustup target is missing");
        return;
    }
    let base = std::env::temp_dir().join("ray_android_lib");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(
        base.join("ray.toml"),
        "[package]\nname = \"mini-android\"\nversion = \"0.1.0\"\nentry = \"main.ray\"\n",
    )
    .unwrap();
    std::fs::write(
        base.join("main.ray"),
        "import std/ui;\nimport std/time;\n\nfn main() {\n    print(\"hello android\");\n    let _ = ui.open(\"Mini\", \"http://127.0.0.1:1/\", 375, 667);\n    while (true) {\n        time.sleep(1000);\n    }\n}\n",
    )
    .unwrap();
    let so = base.join("libray_app.so");
    let st = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args([
            "build",
            "--native",
            "--lib",
            "--release",
            "--target",
            "aarch64-linux-android",
            "main.ray",
            "-o",
        ])
        .arg(&so)
        .current_dir(&base)
        .output()
        .expect("runs ray build");
    assert!(
        st.status.success(),
        "the android cdylib builds:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    // Los símbolos JNI deben estar en la tabla DINÁMICA (nm -D): el shell Java resuelve por
    // dlsym al cargar; fat-LTO no debe barrer ningún no_mangle.
    let nm = Command::new(ndk.join("llvm-nm")).args(["-D", "--defined-only"]).arg(&so).output().expect("llvm-nm");
    let syms = String::from_utf8_lossy(&nm.stdout);
    for s in [
        "ray_start",
        "JNI_OnLoad",
        "Java_org_raylang_shell_RayBridge_start",
        "Java_org_raylang_shell_RayBridge_pushEvent",
        "ray_ui_set_handlers",
        "ray_ui_push_event",
    ] {
        assert!(syms.contains(s), "dynamic symbol '{s}' present:\n{syms}");
    }
    // Alineación 16KB (Android 15+): todo LOAD con align 0x4000.
    let re = Command::new(ndk.join("llvm-readelf")).arg("-l").arg(&so).output().expect("readelf");
    let phdrs = String::from_utf8_lossy(&re.stdout);
    assert!(phdrs.contains("0x4000"), "16KB page alignment:\n{phdrs}");
    assert!(!phdrs.contains("align 0x1000\n"), "no 4KB-aligned LOAD segments:\n{phdrs}");
    let _ = std::fs::remove_dir_all(&base);
}
