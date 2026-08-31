//! §80b — `ray bundle --ios`: el proyecto Xcode generado. El smoke compila la app para el
//! SDK del simulador (sin firmar, sin arrancar ningún simulador). Va `#[ignore]` porque exige
//! Xcode + los dos targets rustup de iOS y cuesta minutos la primera vez (los cross-builds de
//! release con LTO); se corre a mano en macOS: `cargo test --test bundle_ios_cli -- --ignored`.
//! La validación de EJECUCIÓN (simulador real: boot+install+launch, webserver sirviendo y
//! webview cargado) se hizo en el arco y queda como dogfood documentado — DESIGN §146.

use std::process::Command;

fn have(cmd: &str, arg: &str) -> bool {
    Command::new(cmd).arg(arg).output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
#[ignore = "exige Xcode + rustup targets ios; minutos en frío — correr a mano en macOS"]
#[cfg(target_os = "macos")]
fn the_generated_ios_project_builds_for_the_simulator_sdk() {
    if !have("xcodebuild", "-version") || !have("rustc", "--version") {
        return;
    }
    let targets = Command::new("rustup").args(["target", "list", "--installed"]).output();
    let targets = targets.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    if !targets.contains("aarch64-apple-ios\n") || !targets.contains("aarch64-apple-ios-sim") {
        eprintln!("skip: faltan los targets rustup de iOS");
        return;
    }
    let base = std::env::temp_dir().join("ray_bundle_ios");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(
        base.join("ray.toml"),
        "[package]\nname = \"mini-ios\"\nversion = \"0.1.0\"\nentry = \"main.ray\"\n",
    )
    .unwrap();
    std::fs::write(
        base.join("main.ray"),
        "import std/ui;\nimport std/time;\n\nfn main() {\n    let _ = ui.open(\"Mini\", \"http://127.0.0.1:1/\", 375, 667);\n    while (true) {\n        time.sleep(1000);\n    }\n}\n",
    )
    .unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["bundle", "main.ray", "--ios", "-o", "."])
        .current_dir(&base)
        .output()
        .expect("bundle --ios");
    assert!(st.status.success(), "bundle --ios ok\n{}", String::from_utf8_lossy(&st.stderr));
    let proj = base.join("mini-ios-ios");
    for f in ["libs/libray_app.a", "libs-sim/libray_app.a", "Shell/AppDelegate.m", "App.xcconfig"] {
        assert!(proj.join(f).is_file(), "{f} generado");
    }

    let xc = Command::new("xcodebuild")
        .args([
            "-project", "mini-ios.xcodeproj",
            "-target", "mini-ios",
            "-sdk", "iphonesimulator",
            "-configuration", "Debug",
            "build",
            "CODE_SIGNING_ALLOWED=NO",
        ])
        .current_dir(&proj)
        .output()
        .expect("xcodebuild");
    assert!(
        xc.status.success(),
        "xcodebuild simulador ok\n{}",
        String::from_utf8_lossy(&xc.stdout).lines().rev().take(25).collect::<Vec<_>>().join("\n")
    );
    assert!(
        proj.join("build/Debug-iphonesimulator/mini-ios.app/mini-ios").is_file(),
        "el binario de la app existe"
    );
}
