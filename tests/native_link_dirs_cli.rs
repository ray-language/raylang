//! ray-daw #478 — una librería C PROPIA del proyecto (el puente que la FFI deja como salida para
//! lo que no cubre: callbacks retenidos, structs por valor, variádicas). `[native] link_dirs` dice
//! dónde está: el cargador de la VM la busca ahí, el binario nativo enlaza contra ella, y con
//! `[app.macos] frameworks` `ray bundle` la mete en el `.app`. Antes solo funcionaba con un
//! `LIBRARY_PATH` sin documentar y el bundle salía sin la librería.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

fn has(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Un proyecto con `out/libanswer.{dylib,so}` compilada de un C de una línea. `None` sin `cc`.
fn project(name: &str) -> Option<PathBuf> {
    if !has("cc") {
        return None;
    }
    let d = std::env::temp_dir().join(format!("ray_link_dirs_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::create_dir_all(d.join("out")).unwrap();
    std::fs::write(d.join("lib.c"), "int answer(void) { return 42; }\n").unwrap();
    let mut cc = Command::new("cc");
    if cfg!(target_os = "macos") {
        cc.args(["-dynamiclib", "lib.c", "-install_name", "@rpath/libanswer.dylib", "-o", "out/libanswer.dylib"]);
    } else {
        cc.args(["-shared", "-fPIC", "lib.c", "-o", "out/libanswer.so"]);
    }
    assert!(cc.current_dir(&d).status().unwrap().success(), "compila la librería de prueba");
    std::fs::write(
        d.join("ray.toml"),
        "[package]\nname = \"linkdemo\"\nversion = \"0.1.0\"\n\n[native]\nlink_dirs = [\"out\"]\n\n\
         [app]\nname = \"LinkDemo\"\nid = \"dev.example.linkdemo\"\n\n[app.macos]\nframeworks = [\"out/libanswer.dylib\"]\n",
    )
    .unwrap();
    std::fs::write(d.join("src/main.ray"), "extern \"answer\" {\n    fn answer() -> int;\n}\nfn main() {\n    print(answer());\n}\n").unwrap();
    Some(d)
}

fn run(dir: &Path, program: &str, args: &[&str]) -> (String, String, bool) {
    // Sin las variables que harían encontrar la librería por otra vía.
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env_remove("LIBRARY_PATH")
        .env_remove("DYLD_LIBRARY_PATH")
        .env_remove("LD_LIBRARY_PATH")
        .output()
        .unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.success())
}

#[test]
fn the_vm_loads_a_project_library_from_link_dirs() {
    let Some(d) = project("vm") else { return };
    let (out, err, ok) = run(&d, BIN, &["run"]);
    assert!(ok && out == "42\n", "{out}{err}");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_native_build_links_a_project_library_from_link_dirs() {
    if !has("rustc") {
        return;
    }
    let Some(d) = project("native") else { return };
    let (out, err, ok) = run(&d, BIN, &["build", "--native", "-o", "demo"]);
    assert!(ok, "build --native: {out}{err}");
    // El binario de desarrollo la encuentra donde se compiló (rpath), sin variables de entorno.
    let (out, err, ok) = run(&d, d.join("demo").to_str().unwrap(), &[]);
    assert!(ok && out == "42\n", "{out}{err}");
    let _ = std::fs::remove_dir_all(d);
}

/// `ray bundle` copia la librería a `Contents/Frameworks` y el binario del `.app` la carga de ahí.
#[cfg(target_os = "macos")]
#[test]
fn a_bundle_ships_the_project_library_in_frameworks() {
    if !has("rustc") {
        return;
    }
    let Some(d) = project("bundle") else { return };
    let (out, err, ok) = run(&d, BIN, &["bundle", "--sign", "none"]);
    assert!(ok, "bundle: {out}{err}");
    let app = d.join("LinkDemo.app");
    assert!(app.join("Contents/Frameworks/libanswer.dylib").is_file(), "la librería va en el .app\n{out}{err}");
    // Fuera del proyecto: solo vale lo que viaja dentro del `.app`.
    let moved = std::env::temp_dir().join(format!("ray_link_dirs_moved_{}.app", std::process::id()));
    let _ = std::fs::remove_dir_all(&moved);
    std::fs::rename(&app, &moved).unwrap();
    std::fs::remove_dir_all(d.join("out")).unwrap();
    let (out, err, ok) = run(&moved, moved.join("Contents/MacOS/linkdemo").to_str().unwrap(), &[]);
    assert!(ok && out == "42\n", "{out}{err}");
    let _ = std::fs::remove_dir_all(moved);
    let _ = std::fs::remove_dir_all(d);
}
