//! Pruebas del **índice de paquetes** (M51a): resolver dependencias **por nombre** (`foo = "1.2.0"`)
//! contra un índice, y `ray add`. Todo **offline y determinista**: el índice es un **directorio
//! local** (`RAY_INDEX`) con un `<nombre>.toml` por paquete, y cada versión apunta a un repositorio
//! git local servido por `file://` (como en `deps_cli.rs`).

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// Ejecuta el binario con `args`, `cwd` y `RAY_INDEX` apuntando a `index`. Devuelve (stdout, stderr, código).
fn ray_idx(cwd: &Path, index: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(BIN)
        .args(args)
        .current_dir(cwd)
        .env("RAY_INDEX", index)
        .output()
        .expect("lanza el binario");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(cwd).output().expect("git disponible");
    assert!(out.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
}

fn tmp(nombre: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ray_registry_{nombre}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("crea el dir temporal");
    d
}

/// "Publica" un paquete: un repo git en `<base>/<nombre>-repo` con `mod.ray` + `ray.toml` (nombre y
/// versión) y un tag `v<ver>`. Devuelve la ruta absoluta del repo.
fn publicar(base: &Path, nombre: &str, ver: &str, mod_ray: &str) -> std::path::PathBuf {
    let repo = base.join(format!("{nombre}-{ver}-repo"));
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("mod.ray"), mod_ray).unwrap();
    std::fs::write(
        repo.join("ray.toml"),
        format!("[package]\nname = \"{nombre}\"\nversion = \"{ver}\"\n"),
    )
    .unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "pub"]);
    git(&repo, &["tag", &format!("v{ver}")]);
    repo
}

/// Escribe el archivo de índice `<index>/<nombre>.toml` con las versiones dadas `(ver, repo)`.
fn indexar(index: &Path, nombre: &str, versiones: &[(&str, &Path)]) {
    std::fs::create_dir_all(index).unwrap();
    let mut s = format!("# índice de {nombre}\n");
    for (ver, repo) in versiones {
        s.push_str(&format!(
            "\n[{ver}]\ngit = \"git+file://{}@v{ver}\"\n",
            repo.display()
        ));
    }
    std::fs::write(index.join(format!("{nombre}.toml")), s).unwrap();
}

/// Crea un proyecto vacío (sin dependencias) en `<base>/app`.
fn app(base: &Path, main_ray: &str) -> std::path::PathBuf {
    let app = base.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(app.join("ray.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n").unwrap();
    std::fs::write(app.join("src/main.ray"), main_ray).unwrap();
    app
}

#[test]
fn dependencia_por_nombre_desde_el_indice() {
    let base = tmp("byname");
    let index = base.join("index");
    let repo = publicar(&base, "geo", "1.2.0", "pub fn duplicar(x: int) -> int { x * 2 }\n");
    indexar(&index, "geo", &[("1.2.0", &repo)]);

    // El proyecto declara la dep POR NOMBRE (sin URL git).
    let app = app(&base, "from geo import duplicar;\nfn main() -> int { print(duplicar(21)); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"1.2.0\"\n",
    )
    .unwrap();

    // `ray run` resuelve `geo = "1.2.0"` por el índice, descarga el repo y lo usa.
    let (out, err, code) = ray_idx(&app, &index, &["run"]);
    assert_eq!(code, 0, "run OK\n{err}");
    assert!(out.contains("42"), "usa la dep resuelta por el índice\n{out}\n{err}");
    assert!(app.join(".ray-deps/geo/mod.ray").is_file(), "el paquete quedó en la caché");
    // Se generó el lock con la versión resuelta.
    let lock = std::fs::read_to_string(app.join("ray.lock")).unwrap();
    assert!(lock.contains("[geo]") && lock.contains("v1.2.0"), "lock con la versión resuelta:\n{lock}");
}

#[test]
fn ray_add_escribe_el_manifiesto_y_descarga() {
    let base = tmp("add");
    let index = base.join("index");
    let repo = publicar(&base, "util", "0.3.1", "pub fn saludo() -> string { \"hola\" }\n");
    indexar(&index, "util", &[("0.3.1", &repo)]);
    let app = app(&base, "from util import saludo;\nfn main() -> int { print(saludo()); 0 }\n");

    // `ray add util` (sin versión) → escribe `util = "^0.3.1"` y descarga.
    let (out, err, code) = ray_idx(&app, &index, &["add", "util"]);
    assert_eq!(code, 0, "add OK\n{err}");
    assert!(out.contains("añadida"), "{out}");
    let toml = std::fs::read_to_string(app.join("ray.toml")).unwrap();
    assert!(toml.contains("util = \"^0.3.1\""), "el manifiesto quedó actualizado:\n{toml}");
    assert!(app.join(".ray-deps/util/mod.ray").is_file(), "descargó el paquete");

    // Y el programa corre con la dep recién añadida.
    let (out, err, code) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("hola"), "run tras add\n{out}\n{err}");
    assert_eq!(code, 0);
}

#[test]
fn ray_add_con_version_exacta_respeta_el_requisito() {
    let base = tmp("addexact");
    let index = base.join("index");
    let r120 = publicar(&base, "geo", "1.2.0", "pub fn v() -> int { 120 }\n");
    let r130 = publicar(&base, "geo", "1.3.0", "pub fn v() -> int { 130 }\n");
    indexar(&index, "geo", &[("1.2.0", &r120), ("1.3.0", &r130)]);
    let app = app(&base, "from geo import v;\nfn main() -> int { print(v()); 0 }\n");

    // `ray add geo@1.2.0` → requisito EXACTO, aunque exista 1.3.0.
    let (_o, err, code) = ray_idx(&app, &index, &["add", "geo@1.2.0"]);
    assert_eq!(code, 0, "add exacto OK\n{err}");
    let toml = std::fs::read_to_string(app.join("ray.toml")).unwrap();
    assert!(toml.contains("geo = \"1.2.0\""), "requisito exacto:\n{toml}");
    let (out, _e, _c) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("120"), "usó la 1.2.0 exacta, no la 1.3.0\n{out}");
}

#[test]
fn caret_elige_la_mas_alta_compatible() {
    let base = tmp("caret");
    let index = base.join("index");
    let r120 = publicar(&base, "geo", "1.2.0", "pub fn v() -> int { 120 }\n");
    let r130 = publicar(&base, "geo", "1.3.0", "pub fn v() -> int { 130 }\n");
    let r200 = publicar(&base, "geo", "2.0.0", "pub fn v() -> int { 200 }\n");
    indexar(&index, "geo", &[("1.2.0", &r120), ("1.3.0", &r130), ("2.0.0", &r200)]);
    let app = app(&base, "from geo import v;\nfn main() -> int { print(v()); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"^1.2\"\n",
    )
    .unwrap();
    // `^1.2` casa 1.2.0 y 1.3.0 pero no 2.0.0 → elige la 1.3.0.
    let (out, err, _c) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("130"), "caret elige la más alta <2.0.0\n{out}\n{err}");
}

#[test]
fn paquete_inexistente_da_error_claro() {
    let base = tmp("missing");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap(); // índice vacío
    let app = app(&base, "fn main() -> int { 0 }\n");
    let (_o, err, code) = ray_idx(&app, &index, &["add", "noexiste"]);
    assert_eq!(code, 65, "falla al añadir un paquete que no está en el índice");
    assert!(err.contains("no está en el índice"), "mensaje claro:\n{err}");
    // Y no tocó el manifiesto.
    let toml = std::fs::read_to_string(app.join("ray.toml")).unwrap();
    assert!(!toml.contains("noexiste"), "no escribió la dep fallida:\n{toml}");
}

/// Un repo git "publicable" con remoto `origin` (un bare local): crea el bare, clona a un working
/// dir con `ray.toml`+`mod.ray`, commitea, taggea `v<ver>` y empuja rama + tags. Devuelve el working dir.
fn repo_con_origin(base: &Path, nombre: &str, ver: &str, mod_ray: &str) -> std::path::PathBuf {
    let bare = base.join(format!("{nombre}.git"));
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "--bare", "-q"]);
    let work = base.join(format!("{nombre}-work"));
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q"]);
    git(&work, &["remote", "add", "origin", &bare.to_string_lossy()]);
    std::fs::write(work.join("mod.ray"), mod_ray).unwrap();
    std::fs::write(
        work.join("ray.toml"),
        format!("[package]\nname = \"{nombre}\"\nversion = \"{ver}\"\n"),
    )
    .unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "pub"]);
    git(&work, &["tag", &format!("v{ver}")]);
    git(&work, &["push", "-q", "origin", "HEAD"]);
    git(&work, &["push", "-q", "origin", "--tags"]);
    work
}

#[test]
fn ray_publish_añade_al_indice_y_un_consumidor_lo_resuelve() {
    let base = tmp("publish");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap();
    // Paquete `mate` con remoto origin + tag v1.0.0.
    let work = repo_con_origin(&base, "mate", "1.0.0", "pub fn triple(x: int) -> int { x * 3 }\n");

    // `ray publish` desde el paquete: deriva git+<origin>@v1.0.0, hashea y añade la entrada al índice.
    let (out, err, code) = ray_idx(&work, &index, &["publish"]);
    assert_eq!(code, 0, "publish OK\n{err}");
    assert!(out.contains("publicado mate 1.0.0"), "{out}");
    let entry = std::fs::read_to_string(index.join("mate.toml")).unwrap();
    assert!(entry.contains("[1.0.0]") && entry.contains("@v1.0.0") && entry.contains("hash ="), "entrada en el índice:\n{entry}");

    // Republicar la MISMA versión → error de inmutabilidad, sin duplicar en el índice.
    let (_o, err, code) = ray_idx(&work, &index, &["publish"]);
    assert_eq!(code, 65, "republicar la misma versión falla");
    assert!(err.contains("ya está publicada"), "{err}");

    // Un consumidor la resuelve por nombre desde el índice y la ejecuta (clona del origin al tag).
    let app = app(&base, "from mate import triple;\nfn main() -> int { print(triple(14)); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nmate = \"1.0.0\"\n",
    )
    .unwrap();
    let (out, err, code) = ray_idx(&app, &index, &["run"]);
    assert_eq!(code, 0, "el consumidor corre\n{err}");
    assert!(out.contains("42"), "usó el paquete publicado\n{out}\n{err}");
}

#[test]
fn ray_publish_sin_tag_falla_claro() {
    let base = tmp("publishnotag");
    let index = base.join("index");
    // Repo con origin pero SIN el tag v2.0.0 que declara su versión.
    let bare = base.join("x.git");
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "--bare", "-q"]);
    let work = base.join("x-work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q"]);
    git(&work, &["remote", "add", "origin", &bare.to_string_lossy()]);
    std::fs::write(work.join("mod.ray"), "pub fn f() -> int { 1 }\n").unwrap();
    std::fs::write(work.join("ray.toml"), "[package]\nname = \"x\"\nversion = \"2.0.0\"\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "c"]);
    let (_o, err, code) = ray_idx(&work, &index, &["publish"]);
    assert_eq!(code, 65, "sin tag falla");
    assert!(err.contains("no existe el tag 'v2.0.0'"), "mensaje claro:\n{err}");
}

#[test]
fn spec_por_nombre_sin_indice_configurado_avisa() {
    let base = tmp("noindex");
    let app = app(&base, "fn main() -> int { 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"1.0.0\"\n",
    )
    .unwrap();
    // Sin RAY_INDEX ni [registry] → error claro al intentar resolver por nombre.
    let out = Command::new(BIN).args(["run"]).current_dir(&app).env_remove("RAY_INDEX").output().unwrap();
    assert_eq!(out.status.code().unwrap_or(-1), 65);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no hay índice") || err.contains("índice"), "avisa de la falta de índice:\n{err}");
}
