//! Toolchain de Rust AUTOCONTENIDA para `ray build --native` (M171, IDEAS §85).
//!
//! El backend nativo transpila a Rust y compila con `cargo`/`rustc`. En un equipo recién
//! instalado no hay ninguno de los dos, y el usuario tenía que instalar rustup por su cuenta —
//! contra la prioridad DX del proyecto (el tooling es parte del alcance de cada feature). Este
//! módulo resuelve tres cosas, sin cambiar el lenguaje ni el transpilador:
//!
//! 1. **Resolución** de `cargo`/`rustc` con un orden fijo: la variable explícita (`RAY_CARGO` /
//!    `RAY_RUSTC`) → el `PATH` del usuario → la toolchain PRIVADA de raylang
//!    (`~/.ray/toolchain`, o `RAY_TOOLCHAIN_HOME`). La privada es la última porque el Rust del
//!    usuario, si existe, es el que él controla; la nuestra es el respaldo.
//! 2. **`ray toolchain install`**: descarga `rustup-init` del canal oficial (rustup.rs) y lo
//!    instala con perfil `minimal` bajo el directorio privado (`RUSTUP_HOME`/`CARGO_HOME`
//!    propios), sin tocar el Rust del usuario ni su PATH. Las herramientas de esa toolchain se
//!    lanzan SIEMPRE con esas dos variables puestas: los proxies de rustup (`cargo/bin/cargo`)
//!    buscan la toolchain en `RUSTUP_HOME` (por defecto `~/.rustup`, que aquí no existe).
//! 3. **Vendor de las dependencias de `ray-runtime`** (IDEAS §85, 2a): cada release publica
//!    `ray-runtime-vendor.tar.gz` (`cargo vendor` de TODAS las features + su `Cargo.lock`);
//!    `install` lo deja en `<home>/vendor/<versión>/` y el proyecto Cargo generado por el build
//!    nativo lo usa como fuente de crates (`.cargo/config.toml` con `replace-with`) → el primer
//!    build tras la instalación NO necesita red.
//!
//! Lo que este módulo NO elimina, y dice honestamente: el **enlazador del sistema** que `rustc`
//! necesita (Xcode Command Line Tools / `build-essential` / MSVC Build Tools). `status` lo
//! comprueba y `install` lo avisa al final.
//!
//! Dependencias del ENTORNO (como `ray upgrade`): `curl` y `tar`/`sh` del sistema. Cero crates.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

/// El directorio privado de la toolchain: `RAY_TOOLCHAIN_HOME` o `~/.ray/toolchain`. Si no hay
/// HOME/USERPROFILE (entornos exóticos), cae al temporal — como `native_cache_dir` del CLI.
pub fn home() -> PathBuf {
    if let Some(h) = env::var_os("RAY_TOOLCHAIN_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    match env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
        Some(home) => Path::new(&home).join(".ray").join("toolchain"),
        None => env::temp_dir().join("ray_toolchain"),
    }
}

/// `RUSTUP_HOME` de la toolchain privada.
fn rustup_home(home: &Path) -> PathBuf {
    home.join("rustup")
}

/// `CARGO_HOME` de la toolchain privada (sus binarios viven en `<cargo>/bin`).
fn cargo_home(home: &Path) -> PathBuf {
    home.join("cargo")
}

/// Directorio del vendor de `ray-runtime` para ESTA versión de `ray` (el runtime incrustado es
/// exactamente el de esta versión, así que sus deps también van por versión).
pub fn vendor_dir(home: &Path) -> PathBuf {
    home.join("vendor").join(env!("CARGO_PKG_VERSION"))
}

/// De dónde salió una herramienta resuelta (para `status` y para los mensajes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `RAY_CARGO`/`RAY_RUSTC`.
    EnvVar,
    /// El `PATH` del usuario.
    Path,
    /// La toolchain privada de raylang.
    Private,
}

/// Una herramienta resuelta: su ruta y de dónde salió.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub path: PathBuf,
    pub source: Source,
}

/// Resuelve `tool` (`cargo`/`rustc`) con el orden fijo del módulo. Pura sobre sus entradas para
/// poder testearla: `env_override` es el valor de `RAY_<TOOL>`, `path_var` el `PATH`, `home` el
/// directorio privado. `None` = no está en ningún sitio.
pub fn resolve_in(tool: &str, env_override: Option<&OsString>, path_var: Option<&OsString>, home: &Path) -> Option<Resolved> {
    if let Some(p) = env_override.filter(|p| !p.is_empty()) {
        return Some(Resolved { path: PathBuf::from(p), source: Source::EnvVar });
    }
    let exe = format!("{tool}{}", env::consts::EXE_SUFFIX);
    if let Some(path_var) = path_var {
        for dir in env::split_paths(path_var) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            let candidate = dir.join(&exe);
            if candidate.is_file() {
                return Some(Resolved { path: candidate, source: Source::Path });
            }
        }
    }
    let private = cargo_home(home).join("bin").join(&exe);
    if private.is_file() {
        return Some(Resolved { path: private, source: Source::Private });
    }
    None
}

/// `resolve_in` con el entorno real del proceso.
pub fn resolve(tool: &str) -> Option<Resolved> {
    let var = format!("RAY_{}", tool.to_ascii_uppercase());
    resolve_in(tool, env::var_os(var).as_ref(), env::var_os("PATH").as_ref(), &home())
}

/// Un `Command` listo para lanzar `tool` tal como se resolvió. Si viene de la toolchain privada,
/// lleva `RUSTUP_HOME`/`CARGO_HOME` apuntando a ella (sin eso los proxies de rustup buscarían
/// `~/.rustup`, que no existe en el equipo que motiva todo esto). `None` = no hay herramienta;
/// el llamador decide el mensaje (`missing_hint` da el estándar).
pub fn command(tool: &str) -> Option<Command> {
    let r = resolve(tool)?;
    let mut cmd = Command::new(&r.path);
    if r.source == Source::Private {
        let h = home();
        cmd.env("RUSTUP_HOME", rustup_home(&h)).env("CARGO_HOME", cargo_home(&h));
    }
    Some(cmd)
}

/// La pista estándar cuando falta `cargo`/`rustc`: la vía autocontenida primero, rustup después.
pub fn missing_hint(tool: &str) -> String {
    format!(
        "hint: install a private Rust toolchain for ray with `ray toolchain install` \
         (or install Rust from https://rustup.rs and make sure `{tool}` is on PATH)"
    )
}

/// El `.cargo/config.toml` que hace que el proyecto generado tome sus crates del vendor en vez de
/// crates.io. Pura (recibe el directorio) para testear el TOML; la ruta va con `/` aunque sea
/// Windows (TOML y cargo la aceptan, y las `\` exigirían escaparse).
pub fn vendor_cargo_config(vendor: &Path) -> String {
    let dir = vendor.join("vendor").to_string_lossy().replace('\\', "/");
    format!(
        "# Generated by ray build --native (M171): crates from the release vendor, no network.\n\
         [source.crates-io]\nreplace-with = \"ray-vendor\"\n\n[source.ray-vendor]\ndirectory = \"{dir}\"\n"
    )
}

/// El vendor instalado para esta versión, si está completo (directorio `vendor/` + `Cargo.lock`).
pub fn installed_vendor() -> Option<PathBuf> {
    let dir = vendor_dir(&home());
    (dir.join("vendor").is_dir() && dir.join("Cargo.lock").is_file()).then_some(dir)
}

/// El linker/toolchain de C que `rustc` necesita en esta plataforma, si se detecta. Devuelve
/// `Ok(descripción)` o `Err(cómo instalarlo)`. Heurística honesta, no una garantía: en macOS
/// pregunta a `xcode-select`, en Linux busca `cc` en el PATH, en Windows `link.exe`/`cl.exe`.
pub fn system_linker() -> Result<String, String> {
    let on_path = |exe: &str| {
        let exe = format!("{exe}{}", env::consts::EXE_SUFFIX);
        env::var_os("PATH")
            .map(|p| env::split_paths(&p).any(|d| !d.as_os_str().is_empty() && d.join(&exe).is_file()))
            .unwrap_or(false)
    };
    match env::consts::OS {
        "macos" => match Command::new("xcode-select").arg("-p").output() {
            Ok(o) if o.status.success() => Ok(format!(
                "Xcode Command Line Tools at {}",
                String::from_utf8_lossy(&o.stdout).trim()
            )),
            _ => Err("run `xcode-select --install` (Xcode Command Line Tools provide the linker)".to_string()),
        },
        "windows" => {
            if on_path("link") || on_path("cl") {
                Ok("MSVC linker on PATH".to_string())
            } else {
                Err("install the Visual Studio Build Tools (C++ workload) — rustc on Windows links with MSVC".to_string())
            }
        }
        _ => {
            if on_path("cc") {
                Ok("cc on PATH".to_string())
            } else {
                Err("install a C toolchain (Debian/Ubuntu: `sudo apt install build-essential`; Fedora: `sudo dnf install gcc`)".to_string())
            }
        }
    }
}

const USAGE: &str = "usage: ray toolchain install [--rust <channel>] [--force] [--no-vendor] | status";

/// Punto de entrada del subcomando `ray toolchain`.
pub fn run(args: &[String]) {
    match args.first().map(String::as_str) {
        Some("install") => install(&args[1..]),
        Some("status") => status(),
        _ => {
            eprintln!("{USAGE}");
            process::exit(64);
        }
    }
}

/// `ray toolchain status`: qué `cargo`/`rustc` usaría `ray build --native`, de dónde, su versión,
/// si hay linker del sistema y si el vendor de esta versión está instalado. Exit 0 siempre que
/// haya cargo Y rustc; 1 si falta alguno (para scripts: «¿puedo compilar nativo aquí?»).
fn status() {
    let h = home();
    println!("toolchain home: {}", h.display());
    let mut missing = false;
    for tool in ["cargo", "rustc"] {
        match resolve(tool) {
            Some(r) => {
                let from = match r.source {
                    Source::EnvVar => format!("RAY_{}", tool.to_ascii_uppercase()),
                    Source::Path => "PATH".to_string(),
                    Source::Private => "private toolchain".to_string(),
                };
                let version = command(tool)
                    .and_then(|mut c| c.arg("--version").output().ok())
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_else(|| "(does not run)".to_string());
                println!("{tool}: {} [{from}] — {version}", r.path.display());
            }
            None => {
                missing = true;
                println!("{tool}: not found");
            }
        }
    }
    match system_linker() {
        Ok(d) => println!("system linker: {d}"),
        Err(how) => println!("system linker: not found — {how}"),
    }
    match installed_vendor() {
        Some(v) => println!("ray-runtime vendor ({}): {}", env!("CARGO_PKG_VERSION"), v.display()),
        None => println!(
            "ray-runtime vendor ({}): not installed (first native build downloads crates from crates.io)",
            env!("CARGO_PKG_VERSION")
        ),
    }
    if missing {
        println!("{}", missing_hint("cargo"));
        process::exit(1);
    }
}

/// `ray toolchain install`: instala la toolchain privada (y el vendor). Idempotente: con `cargo`
/// ya en el PATH no instala nada salvo `--force`; con la privada ya presente la actualiza (rustup
/// es idempotente sobre su propio `RUSTUP_HOME`).
fn install(args: &[String]) {
    let mut channel = "stable".to_string();
    let mut force = false;
    let mut vendor = true;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--force" => force = true,
            "--no-vendor" => vendor = false,
            "--rust" => {
                i += 1;
                match args.get(i) {
                    Some(c) if !c.starts_with('-') => channel = c.clone(),
                    _ => {
                        eprintln!("{USAGE}");
                        process::exit(64);
                    }
                }
            }
            _ => {
                eprintln!("{USAGE}");
                process::exit(64);
            }
        }
        i += 1;
    }
    let h = home();
    if let Some(r) = resolve("cargo").filter(|r| !force && r.source != Source::Private) {
        println!(
            "cargo already available at {} — nothing to install (use --force to install a private toolchain anyway)",
            r.path.display()
        );
        if vendor {
            install_vendor(&h);
        }
        return;
    }
    if let Err(e) = fs::create_dir_all(&h) {
        eprintln!("could not create '{}': {e}", h.display());
        process::exit(73);
    }
    install_rustup(&h, &channel);
    // Verificar ANTES de declarar éxito: el cargo privado debe correr con las variables puestas.
    match command("cargo").and_then(|mut c| c.arg("--version").output().ok()) {
        Some(o) if o.status.success() => {
            println!("installed: {} (private toolchain at {})", String::from_utf8_lossy(&o.stdout).trim(), h.display());
        }
        _ => {
            eprintln!("the private toolchain was installed but `cargo --version` does not run from {}", h.display());
            process::exit(70);
        }
    }
    if vendor {
        install_vendor(&h);
    }
    match system_linker() {
        Ok(_) => {}
        Err(how) => {
            eprintln!("note: no system linker detected — rustc needs one to produce binaries: {how}");
        }
    }
    println!("ray build --native will use this toolchain when cargo/rustc are not on PATH (see `ray toolchain status`)");
}

/// Descarga y ejecuta `rustup-init` del canal oficial sobre el `RUSTUP_HOME`/`CARGO_HOME`
/// privados. `--no-modify-path`: no toca el perfil del shell del usuario; `--profile minimal`:
/// rustc+cargo+std, sin docs ni clippy (~200 MB menos).
fn install_rustup(h: &Path, channel: &str) {
    let tmp = env::temp_dir().join(format!("ray-toolchain-{}", process::id()));
    let _ = fs::remove_dir_all(&tmp);
    if let Err(e) = fs::create_dir_all(&tmp) {
        eprintln!("could not create '{}': {e}", tmp.display());
        process::exit(73);
    }
    let cleanup = |code: i32| -> ! {
        let _ = fs::remove_dir_all(&tmp);
        process::exit(code);
    };
    let (url, init) = if cfg!(windows) {
        let arch = if env::consts::ARCH == "aarch64" { "aarch64" } else { "x86_64" };
        (format!("https://win.rustup.rs/{arch}"), tmp.join("rustup-init.exe"))
    } else {
        ("https://sh.rustup.rs".to_string(), tmp.join("rustup-init.sh"))
    };
    eprintln!("downloading {url}");
    let dl = Command::new("curl")
        .args(["-sSfL", "-o"])
        .arg(&init)
        .arg(&url)
        .status();
    match dl {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("could not download rustup-init ({url}): curl exited with {}", s.code().unwrap_or(-1));
            cleanup(69);
        }
        Err(e) => {
            eprintln!("could not run curl: {e} (is it installed?)");
            cleanup(69);
        }
    }
    // `-q`: sin el progreso `info:` de rustup. Su pancarta final ("add cargo/bin to your PATH")
    // sale por STDOUT aunque haya `-q`, y aquí sería engañosa — la toolchain privada NO va al PATH
    // a propósito — así que stdout se captura (y solo se muestra si rustup-init falla); stderr,
    // con los errores y avisos reales, se hereda.
    let common = ["-y", "-q", "--no-modify-path", "--profile", "minimal", "--default-toolchain", channel];
    let mut cmd = if cfg!(windows) {
        Command::new(&init)
    } else {
        let mut c = Command::new("sh");
        c.arg(&init);
        c
    };
    eprintln!("installing Rust {channel} (minimal profile) under {}", h.display());
    let output = cmd
        .args(common)
        .env("RUSTUP_HOME", rustup_home(h))
        .env("CARGO_HOME", cargo_home(h))
        .stderr(process::Stdio::inherit())
        .output();
    match output {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            eprint!("{}", String::from_utf8_lossy(&o.stdout));
            eprintln!("rustup-init failed (code {})", o.status.code().unwrap_or(-1));
            cleanup(70);
        }
        Err(e) => {
            eprintln!("could not run rustup-init: {e}");
            cleanup(70);
        }
    }
    let _ = fs::remove_dir_all(&tmp);
}

/// Descarga el vendor de `ray-runtime` de la release de ESTA versión a `<home>/vendor/<versión>/`.
/// No es fatal: una versión de desarrollo no tiene release, y sin vendor el build nativo sigue
/// funcionando con red (crates.io). Se descarga a un temporal y se coloca por rename: nunca queda
/// un vendor a medias que el build tome por completo (`installed_vendor` exige `vendor/` + lock).
fn install_vendor(h: &Path) {
    let dest = vendor_dir(h);
    if dest.join("vendor").is_dir() && dest.join("Cargo.lock").is_file() {
        println!("ray-runtime vendor already installed at {}", dest.display());
        return;
    }
    let repo = env::var("RAYLANG_REPO").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "ray-language/raylang".to_string());
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    let url = format!("https://github.com/{repo}/releases/download/{tag}/{VENDOR_ASSET}");
    let tmp = env::temp_dir().join(format!("ray-vendor-{}", process::id()));
    let _ = fs::remove_dir_all(&tmp);
    if fs::create_dir_all(&tmp).is_err() {
        eprintln!("note: could not create a temporary directory for the vendor; skipping");
        return;
    }
    eprintln!("downloading {url}");
    let archive = tmp.join(VENDOR_ASSET);
    // `-s` sin `-S`: un 404 (versión de desarrollo sin release) es un caso normal con su propia nota.
    let ok = Command::new("curl")
        .args(["-sfL", "-o"])
        .arg(&archive)
        .arg(&url)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        eprintln!(
            "note: no ray-runtime vendor for {tag} ({url}); the first native build will download crates from crates.io"
        );
        let _ = fs::remove_dir_all(&tmp);
        return;
    }
    let ok = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .current_dir(&tmp)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let _ = fs::remove_file(&archive);
    if !ok || !tmp.join("vendor").is_dir() || !tmp.join("Cargo.lock").is_file() {
        eprintln!("note: the vendor archive is not usable (expected vendor/ + Cargo.lock); skipping");
        let _ = fs::remove_dir_all(&tmp);
        return;
    }
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::remove_dir_all(&dest);
    // `rename` puede cruzar filesystems (temp vs HOME) → fallback a copia recursiva.
    if fs::rename(&tmp, &dest).is_err() {
        if let Err(e) = copy_dir(&tmp, &dest) {
            eprintln!("note: could not place the vendor at {}: {e}; skipping", dest.display());
            let _ = fs::remove_dir_all(&dest);
        }
        let _ = fs::remove_dir_all(&tmp);
    }
    if dest.join("vendor").is_dir() {
        println!("ray-runtime vendor installed at {} (native builds of this version need no network)", dest.display());
    }
}

/// Nombre del asset de vendor en la release (sin versión en el nombre, como los binarios: el tag
/// la lleva). Lo produce `tools/vendor-runtime.sh` en `release.yml`.
pub const VENDOR_ASSET: &str = "ray-runtime-vendor.tar.gz";

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = env::temp_dir().join(format!("ray_toolchain_test_{tag}_{}", process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "").unwrap();
    }

    fn exe(name: &str) -> String {
        format!("{name}{}", env::consts::EXE_SUFFIX)
    }

    #[test]
    fn env_override_wins_over_everything() {
        let d = temp_dir("env");
        let on_path = d.join("bin");
        touch(&on_path.join(exe("cargo")));
        let path_var = env::join_paths([&on_path]).unwrap();
        let over = OsString::from("/custom/cargo");
        let r = resolve_in("cargo", Some(&over), Some(&path_var), &d).unwrap();
        assert_eq!(r.source, Source::EnvVar);
        assert_eq!(r.path, PathBuf::from("/custom/cargo"));
        // Vacía = como si no estuviera.
        let empty = OsString::new();
        let r = resolve_in("cargo", Some(&empty), Some(&path_var), &d).unwrap();
        assert_eq!(r.source, Source::Path);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn path_wins_over_private_toolchain() {
        let d = temp_dir("path");
        let on_path = d.join("bin");
        touch(&on_path.join(exe("cargo")));
        touch(&cargo_home(&d).join("bin").join(exe("cargo")));
        let path_var = env::join_paths([&on_path]).unwrap();
        let r = resolve_in("cargo", None, Some(&path_var), &d).unwrap();
        assert_eq!(r.source, Source::Path);
        assert_eq!(r.path, on_path.join(exe("cargo")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn private_toolchain_is_the_fallback() {
        let d = temp_dir("private");
        let empty_dir = d.join("empty");
        fs::create_dir_all(&empty_dir).unwrap();
        let path_var = env::join_paths([&empty_dir]).unwrap();
        assert!(resolve_in("cargo", None, Some(&path_var), &d).is_none(), "sin nada → None");
        touch(&cargo_home(&d).join("bin").join(exe("cargo")));
        let r = resolve_in("cargo", None, Some(&path_var), &d).unwrap();
        assert_eq!(r.source, Source::Private);
        // rustc no instalado en la privada → None aunque cargo sí.
        assert!(resolve_in("rustc", None, Some(&path_var), &d).is_none());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn vendor_config_points_to_the_vendor_directory() {
        let cfg = vendor_cargo_config(Path::new("/home/u/.ray/toolchain/vendor/1.5.1"));
        assert!(cfg.contains("[source.crates-io]\nreplace-with = \"ray-vendor\""), "{cfg}");
        assert!(cfg.contains("directory = \"/home/u/.ray/toolchain/vendor/1.5.1/vendor\""), "{cfg}");
        // Rutas Windows: sin backslashes en el TOML.
        let cfg = vendor_cargo_config(Path::new(r"C:\Users\u\.ray\toolchain\vendor\1.5.1"));
        assert!(!cfg.contains('\\'), "{cfg}");
    }

    #[test]
    fn vendor_dir_is_per_ray_version() {
        let d = Path::new("/x");
        assert_eq!(vendor_dir(d), Path::new("/x/vendor").join(env!("CARGO_PKG_VERSION")));
    }
}
