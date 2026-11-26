//! `fs.data_dir` / `fs.cache_dir` (ray-remote #506): la carpeta de datos y la de caché de la app
//! en cada sistema. Dependen del entorno → subproceso con `HOME`, `APPDATA`… apuntando a una
//! carpeta de prueba, en ambos motores.

use std::path::{Path, PathBuf};
use std::process::Command;

const PROGRAM: &str = r#"
import std/fs;
fn show(r: Result<string, string>) {
    match (r) {
        Result.Ok(d) => print("ok " + d + " " + to_string(fs.is_dir(d))),
        Result.Err(e) => print("err " + e),
    }
}
fn main() {
    show(fs.data_dir("Probe App"));
    show(fs.cache_dir("Probe App"));
    show(fs.data_dir("Probe App"));
    show(fs.data_dir(""));
    show(fs.data_dir(".."));
    show(fs.cache_dir("a/b"));
    show(fs.data_dir("a\\b"));
    show(fs.data_dir(" "));
    show(fs.data_dir("App."));
    show(fs.cache_dir("a:b"));
}
"#;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_app_dirs_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crea la carpeta de prueba");
    dir
}

/// Corre el programa con un entorno controlado: las variables de `set` y ninguna de las otras
/// que las funciones consultan.
fn run(dir: &Path, vm: bool, set: &[(&str, &Path)]) -> Vec<String> {
    let src = dir.join("probe.ray");
    std::fs::write(&src, PROGRAM).expect("escribe el programa");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    cmd.arg(if vm { "--vm" } else { "--interp" }).arg(&src);
    for name in ["HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "APPDATA", "LOCALAPPDATA"] {
        cmd.env_remove(name);
    }
    for (name, value) in set {
        cmd.env(name, value);
    }
    let out = cmd.output().expect("lanza raylang");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect()
}

const REJECTED: [&str; 7] = [
    "err invalid app name '': it must be a single folder name",
    "err invalid app name '..': it must be a single folder name",
    "err invalid app name 'a/b': it must be a single folder name",
    "err invalid app name 'a\\b': it must be a single folder name",
    "err invalid app name ' ': it must be a single folder name",
    "err invalid app name 'App.': it must be a single folder name",
    "err invalid app name 'a:b': it must be a single folder name",
];

fn ok(path: PathBuf) -> String {
    format!("ok {} true", path.display())
}

#[test]
fn app_dirs_follow_the_platform_and_are_created() {
    for vm in [false, true] {
        let dir = scratch(if vm { "vm" } else { "in" });
        let home = dir.join("home");
        std::fs::create_dir_all(&home).expect("crea home");
        #[cfg(target_os = "macos")]
        let (lines, data, cache) = (
            run(&dir, vm, &[("HOME", &home)]),
            home.join("Library/Application Support/Probe App"),
            home.join("Library/Caches/Probe App"),
        );
        #[cfg(all(unix, not(target_os = "macos")))]
        let (lines, data, cache) = (
            run(&dir, vm, &[("HOME", &home)]),
            home.join(".local/share/Probe App"),
            home.join(".cache/Probe App"),
        );
        #[cfg(windows)]
        let (lines, data, cache) = (
            run(&dir, vm, &[("APPDATA", &home.join("Roaming")), ("LOCALAPPDATA", &home.join("Local"))]),
            home.join("Roaming").join("Probe App"),
            home.join("Local").join("Probe App").join("Cache"),
        );
        // La carpeta no existía: la crea, y la segunda llamada la encuentra hecha.
        assert_eq!(lines[0], ok(data.clone()), "vm={vm}");
        assert_eq!(lines[1], ok(cache), "vm={vm}");
        assert_eq!(lines[2], ok(data), "vm={vm}");
        assert_eq!(lines[3..], REJECTED, "vm={vm}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn app_dirs_fail_without_the_variable() {
    let dir = scratch("unset");
    let lines = run(&dir, true, &[]);
    let var = if cfg!(windows) { "APPDATA" } else { "HOME" };
    assert_eq!(lines[0], format!("err {var} is not set"));
    // Relativa: daría una carpeta distinta según desde dónde se lance el programa.
    let lines = run(&dir, true, &[(var, Path::new("rel"))]);
    assert_eq!(lines[0], format!("err {var} is not an absolute path: rel"));
    // Con separador final: la ruta no lleva `//`.
    let home = dir.join("home");
    let mut slashed = home.clone().into_os_string();
    slashed.push(std::path::MAIN_SEPARATOR_STR);
    let lines = run(&dir, true, &[("HOME", Path::new(&slashed)), ("APPDATA", Path::new(&slashed))]);
    assert!(lines[0].starts_with(&format!("ok {}{}", home.display(), std::path::MAIN_SEPARATOR)), "{lines:?}");
    assert!(!lines[0].contains("//") && !lines[0].contains("\\\\"), "{lines:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// El binario nativo no intercepta estas dos: emite su cuerpo raylang, y debe dar lo mismo.
#[test]
#[cfg(unix)]
fn native_binary_gives_the_same_folders() {
    if !Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        return;
    }
    let dir = scratch("native");
    let home = dir.join("home");
    let want = run(&dir, true, &[("HOME", &home)]);
    let _ = std::fs::remove_dir_all(&home);
    let bin = dir.join("probe_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", dir.join("probe.ray").to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza ray build");
    assert!(build.status.success(), "build --native: {}", String::from_utf8_lossy(&build.stderr));
    let out = Command::new(&bin)
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_CACHE_HOME")
        .env("HOME", &home)
        .output()
        .expect("lanza el binario");
    let got: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect();
    assert_eq!(got, want);
    assert!(want[0].starts_with("ok "), "{want:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[cfg(all(unix, not(target_os = "macos")))]
fn xdg_variables_win_when_absolute() {
    let dir = scratch("xdg");
    let home = dir.join("home");
    let data = dir.join("xdg-data");
    // Relativa: la norma manda ignorarla.
    let lines = run(&dir, true, &[("HOME", &home), ("XDG_DATA_HOME", &data), ("XDG_CACHE_HOME", Path::new("rel"))]);
    assert_eq!(lines[0], ok(data.join("Probe App")));
    assert_eq!(lines[1], ok(home.join(".cache/Probe App")));
    let _ = std::fs::remove_dir_all(&dir);
}
