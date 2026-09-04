//! M147c — `ray bundle`: el empaquetado de apps de escritorio. Se prueba la ESTRUCTURA del
//! bundle (árbol + Info.plist / .desktop) con un programa mínimo por la vía rustc pelada
//! (--without mimalloc,ahash: el release con-features tardaría minutos en CI). El .app real
//! con ventana + embed se verifica con dogfood manual en macOS. En Windows (M180) se comprueba el
//! subsistema del PE y el VERSIONINFO tal como lo lee el SO.

use std::process::Command;

fn have_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
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
