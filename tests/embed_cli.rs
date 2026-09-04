//! M147 — std/embed: assets del proyecto (`[native] embed`). La batería monta un proyecto real
//! (ray.toml + assets/) y exige BYTE-IDENTIDAD entre los tres motores — incluida la clave del
//! diseño: el binario `--native` (tabla horneada por include_bytes!) corre DESDE OTRO cwd y
//! sirve exactamente lo mismo que la VM leyendo del disco en vivo.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("ray_embed_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

const PROG: &str = r#"import std/embed;

fn main() {
    match (embed.list()) {
        Result.Err(e) => print("list err: " + e),
        Result.Ok(keys) => {
            var i = 0;
            while (i < keys.len()) {
                print("key: " + keys[i]);
                i = i + 1;
            }
        },
    }
    match (embed.read("assets/css/app.css")) {
        Result.Ok(d) => print("css bytes: " + to_string(d.len())),
        Result.Err(e) => print("err: " + e),
    }
    match (embed.read("assets/img dir/logo.png")) {
        Result.Ok(d) => print("logo bytes: " + to_string(d.len())),
        Result.Err(e) => print("err: " + e),
    }
    match (embed.read("assets/.hidden")) {
        Result.Ok(_) => print("bad: hidden readable"),
        Result.Err(e) => print("hidden rejected: " + to_string(e.contains("no embedded file"))),
    }
    match (embed.read("assets/../main.ray")) {
        Result.Ok(_) => print("bad: traversal"),
        Result.Err(e) => print("traversal rejected: " + to_string(e.contains("no embedded file"))),
    }
}
"#;

const WANT: &str = "key: assets/css/app.css\nkey: assets/img dir/logo.png\ncss bytes: 20\n\
logo bytes: 7\nhidden rejected: true\ntraversal rejected: true\n";

fn write_project(base: &std::path::Path) {
    // Un dir CON ESPACIO a propósito: la ruta viaja a un include_bytes! y debe escapar bien.
    std::fs::create_dir_all(base.join("assets/css")).unwrap();
    std::fs::create_dir_all(base.join("assets/img dir")).unwrap();
    std::fs::write(base.join("assets/css/app.css"), "body { color: red }\n").unwrap();
    std::fs::write(base.join("assets/img dir/logo.png"), "PNGDATA").unwrap();
    std::fs::write(base.join("assets/.hidden"), "secret").unwrap();
    std::fs::write(
        base.join("ray.toml"),
        "[package]\nname = \"embed-demo\"\nversion = \"0.1.0\"\nentry = \"main.ray\"\n\n[native]\nembed = [\"assets\"]\n",
    )
    .unwrap();
    std::fs::write(base.join("main.ray"), PROG).unwrap();
}

#[test]
fn the_embed_namespace_is_byte_identical_on_all_three_engines() {
    let base = tmp("battery");
    write_project(&base);
    for engine in ["--vm", "--interp"] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args([engine, "main.ray"])
            .current_dir(&base)
            .output()
            .expect("corre");
        assert_eq!(out.status.code(), Some(0), "{engine}: exit 0");
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{engine}: salida exacta");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "main.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        // La prueba del horneado: correr desde OTRO cwd (sin acceso relativo a assets/).
        let out = Command::new(&bin).current_dir(std::env::temp_dir()).output().expect("corre");
        assert_eq!(out.status.code(), Some(0), "nativo: exit 0");
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo ≡ VM (desde otro cwd)");
    }
}

#[test]
fn without_config_every_engine_gives_the_same_clean_error() {
    let base = tmp("noconfig");
    std::fs::write(base.join("loose.ray"), "import std/embed;\n\nfn main() {\n    match (embed.list()) {\n        Result.Ok(_) => print(\"bad\"),\n        Result.Err(e) => print(e),\n    }\n}\n").unwrap();
    const WANT_ERR: &str = "embed: no embedded assets configured (add [native] embed = [\"assets\"] to ray.toml)\n";
    for engine in ["--vm", "--interp"] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args([engine, "loose.ray"])
            .current_dir(&base)
            .output()
            .expect("corre");
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT_ERR, "{engine}: mensaje de sin-config");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("loose_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "loose.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin).output().expect("corre");
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT_ERR, "nativo ≡ VM (sin config)");
    }
}

#[test]
fn a_missing_embed_directory_fails_the_build_naming_the_origin() {
    let base = tmp("baddir");
    std::fs::write(base.join("main.ray"), "fn main() { print(\"hi\"); }\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "main.ray", "--native", "--embed", "nope", "-o", "/dev/null"])
        .current_dir(&base)
        .output()
        .expect("corre");
    assert_eq!(out.status.code(), Some(64), "exit 64");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("embed directory in --embed does not exist: 'nope'"), "nombra el origen: {err}");
}
