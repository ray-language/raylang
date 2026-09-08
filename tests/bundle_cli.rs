//! M147c — `ray bundle`: el empaquetado de apps de escritorio. Se prueba la ESTRUCTURA del
//! bundle (árbol + Info.plist / .desktop) con un programa mínimo por la vía rustc pelada
//! (--without mimalloc,ahash: el release con-features tardaría minutos en CI). El .app real
//! con ventana + embed se verifica con dogfood manual en macOS. En Windows (M180) se comprueba el
//! subsistema del PE y el VERSIONINFO tal como lo lee el SO.

use std::process::Command;

fn have_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// M208 (feedback 25 de ray-remote): `--help` imprime el uso sin compilar nada y un flag desconocido
/// es error 64 con el uso — antes ambos hacían un bundle completo (17 s de release) en silencio.
#[test]
fn help_and_unknown_flags_do_not_build_anything() {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["bundle", "--help"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("usage: ray bundle") && text.contains("[app] name/icon/id"), "{text}");
    for bad in [&["bundle", "--bogus-flag"][..], &["bundle", "--native"][..], &["bundle", "--icon"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(bad).output().unwrap();
        assert_eq!(out.status.code(), Some(64), "{bad:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("usage: ray bundle"), "{err}");
    }
}

/// M208: `[app] name` e `id` del ray.toml alimentan el bundle (antes solo `--name`/`--id`).
#[test]
fn app_name_and_id_come_from_the_manifest() {
    if !have_rustc() {
        return;
    }
    let base = std::env::temp_dir().join("ray_bundle_cli_app");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(
        base.join("ray.toml"),
        "[package]\nname = \"mini-app\"\nversion = \"1.0.0\"\nentry = \"main.ray\"\n\n[app]\nname = \"Mini App\"\nid = \"org.example.mini\"\n",
    )
    .unwrap();
    std::fs::write(base.join("main.ray"), "fn main() { print(\"hi\"); }\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["bundle", "main.ray", "--without", "mimalloc,ahash,fibers", "-o", "."])
        .current_dir(&base)
        .output()
        .expect("corre");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    if cfg!(target_os = "macos") {
        let plist = std::fs::read_to_string(base.join("Mini App.app/Contents/Info.plist")).expect("el .app lleva el [app] name");
        assert!(plist.contains("<key>CFBundleIdentifier</key><string>org.example.mini</string>"), "{plist}");
    } else if cfg!(target_os = "linux") {
        assert!(base.join("Mini App").is_dir() || base.join("Mini App.desktop").exists() || base.read_dir().unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains("Mini App")), "el bundle lleva el [app] name");
    }
}

/// M209: `[app.plist]` va tal cual al Info.plist (cadena y bool) y el permiso de red local se añade
/// solo cuando el programa importa la red. Solo aplica al `.app` de macOS.
#[test]
fn plist_keys_and_local_network_permission() {
    if !have_rustc() || !cfg!(target_os = "macos") {
        return;
    }
    let base = std::env::temp_dir().join("ray_bundle_cli_plist");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(
        base.join("ray.toml"),
        "[package]\nname = \"netapp\"\nversion = \"1.0.0\"\nentry = \"main.ray\"\n\n[app.plist]\nLSUIElement = true\nCFBundleDisplayName = \"Net & Co <beta>\"\n",
    )
    .unwrap();
    std::fs::write(base.join("main.ray"), "import std/net;\nfn main() { print(net.local_port(0)); }\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["bundle", "main.ray", "--without", "mimalloc,ahash,fibers", "-o", "."])
        .current_dir(&base)
        .output()
        .expect("corre");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let plist = std::fs::read_to_string(base.join("netapp.app/Contents/Info.plist")).unwrap();
    for needle in [
        "<key>LSUIElement</key><true/>",
        "<key>CFBundleDisplayName</key><string>Net &amp; Co &lt;beta&gt;</string>",
        "<key>NSLocalNetworkUsageDescription</key><string>netapp connects to devices on your local network.</string>",
    ] {
        assert!(plist.contains(needle), "plist con {needle}:\n{plist}");
    }
    // Sin red y sin [app.plist]: ninguna de las dos claves.
    std::fs::write(base.join("ray.toml"), "[package]\nname = \"quiet\"\nversion = \"1.0.0\"\nentry = \"main.ray\"\n").unwrap();
    std::fs::write(base.join("main.ray"), "fn main() { print(1); }\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["bundle", "main.ray", "--without", "mimalloc,ahash,fibers", "-o", "."])
        .current_dir(&base)
        .output()
        .expect("corre");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let plist = std::fs::read_to_string(base.join("quiet.app/Contents/Info.plist")).unwrap();
    assert!(!plist.contains("NSLocalNetworkUsageDescription") && !plist.contains("LSUIElement"), "{plist}");
}

#[test]
fn bundle_produces_the_platform_structure() {
    if !have_rustc() {
        return;
    }
    let base = std::env::temp_dir().join("ray_bundle_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(
        base.join("ray.toml"),
        "[package]\nname = \"mini-app\"\nversion = \"2.5.0\"\nentry = \"main.ray\"\n",
    )
    .unwrap();
    std::fs::write(base.join("main.ray"), "fn main() { print(\"hi\"); }\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["bundle", "main.ray", "--without", "mimalloc,ahash,fibers", "-o", "."])
        .current_dir(&base)
        .output()
        .expect("corre");
    assert!(
        out.status.success(),
        "bundle ok\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // Sin [native] embed, el aviso de cwd=/ debe aparecer (el .app no podrá leer rutas relativas).
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no embedded assets"),
        "avisa del embed ausente"
    );

    if cfg!(target_os = "macos") {
        let app = base.join("mini-app.app");
        let exe = app.join("Contents/MacOS/mini-app");
        let plist_path = app.join("Contents/Info.plist");
        assert!(exe.is_file(), "el ejecutable del .app");
        let plist = std::fs::read_to_string(&plist_path).expect("Info.plist");
        for needle in [
            "<key>CFBundlePackageType</key><string>APPL</string>",
            "<key>CFBundleExecutable</key><string>mini-app</string>",
            "<key>CFBundleIdentifier</key><string>org.raylang.mini-app</string>",
            "<key>CFBundleShortVersionString</key><string>2.5.0</string>",
            "NSAllowsLocalNetworking",
        ] {
            assert!(plist.contains(needle), "plist con {needle}:\n{plist}");
        }
        // plutil valida el XML (siempre presente en macOS).
        let lint = Command::new("plutil").arg("-lint").arg(&plist_path).output().expect("plutil");
        assert!(lint.status.success(), "plutil -lint ok");
        // El binario del bundle corre (el runner de cargo no tiene GUI que abrir: programa de consola).
        let run = Command::new(&exe).output().expect("corre el .app binario");
        assert_eq!(String::from_utf8_lossy(&run.stdout), "hi\n");
    } else if cfg!(unix) {
        let dir = base.join("mini-app");
        assert!(dir.join("mini-app").is_file(), "el binario del bundle");
        let desktop = std::fs::read_to_string(dir.join("mini-app.desktop")).expect(".desktop");
        assert!(desktop.contains("[Desktop Entry]"), "cabecera .desktop");
        assert!(desktop.contains("Name=mini-app"), "Name");
        // El Exec= es ABSOLUTO (un lanzador no resuelve rutas relativas).
        let exec_line = desktop.lines().find(|l| l.starts_with("Exec=")).expect("Exec=");
        assert!(exec_line.starts_with("Exec=/"), "Exec absoluto: {exec_line}");
        let run = Command::new(dir.join("mini-app")).output().expect("corre el binario");
        assert_eq!(String::from_utf8_lossy(&run.stdout), "hi\n");
    } else if cfg!(windows) {
        // M180: directorio con `<name>.exe` (subsistema WINDOWS, VERSIONINFO) y el `.lnk`.
        let dir = base.join("mini-app");
        let exe = dir.join("mini-app.exe");
        assert!(exe.is_file(), "el binario del bundle");
        assert!(dir.join("mini-app.lnk").is_file(), "el acceso directo");
        let bytes = std::fs::read(&exe).unwrap();
        let pe = u32::from_le_bytes([bytes[0x3c], bytes[0x3d], bytes[0x3e], bytes[0x3f]]) as usize;
        assert_eq!(&bytes[pe..pe + 4], b"PE\0\0", "cabecera PE");
        let subsystem = u16::from_le_bytes([bytes[pe + 24 + 68], bytes[pe + 24 + 69]]);
        assert_eq!(subsystem, 2, "subsistema WINDOWS (sin consola al doble clic)");
        // El VERSIONINFO lo lee el propio SO (la pestaña Detalles de Propiedades).
        let info = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("$v = (Get-Item '{}').VersionInfo; $v.ProductName + '|' + $v.ProductVersion + '|' + $v.FileVersion", exe.display()),
            ])
            .output()
            .expect("powershell");
        assert_eq!(String::from_utf8_lossy(&info.stdout).trim(), "mini-app|2.5.0|2.5.0.0", "VERSIONINFO legible por Windows");
        // Corre aunque sea app de ventanas: con las salidas redirigidas, stdout llega igual.
        let run = Command::new(&exe).output().expect("corre el binario");
        assert_eq!(String::from_utf8_lossy(&run.stdout), "hi\n");
    }
}
