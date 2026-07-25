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
        .expect("lanza el binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Como `ray_idx` pero SIN `RAY_INDEX` (para probar `[registry] index` del ray.toml).
fn ray_plain(cwd: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(BIN)
        .args(args)
        .current_dir(cwd)
        .env_remove("RAY_INDEX")
        .output()
        .expect("lanza el binary");
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

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ray_registry_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("crea el dir temporal");
    d
}

/// "Publica" un paquete: un repo git en `<base>/<nombre>-repo` con `mod.ray` + `ray.toml` (nombre y
/// versión) y un tag `v<ver>`. Devuelve la ruta absoluta del repo.
fn publish(base: &Path, name: &str, see: &str, mod_ray: &str) -> std::path::PathBuf {
    let repo = base.join(format!("{name}-{see}-repo"));
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("mod.ray"), mod_ray).unwrap();
    std::fs::write(
        repo.join("ray.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"{see}\"\n"),
    )
    .unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "pub"]);
    git(&repo, &["tag", &format!("v{see}")]);
    repo
}

/// Escribe el archivo de índice `<index>/<nombre>.toml` con las versiones dadas `(ver, repo)`.
fn write_index(index: &Path, name: &str, versiones: &[(&str, &Path)]) {
    std::fs::create_dir_all(index).unwrap();
    let mut s = format!("# index of {name}\n");
    for (see, repo) in versiones {
        s.push_str(&format!(
            "\n[{see}]\ngit = \"git+file://{}@v{see}\"\n",
            repo.display()
        ));
    }
    std::fs::write(index.join(format!("{name}.toml")), s).unwrap();
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
fn dependency_by_name_from_the_index() {
    let base = tmp("byname");
    let index = base.join("index");
    let repo = publish(&base, "geo", "1.2.0", "pub fn duplicate(x: int) -> int { x * 2 }\n");
    write_index(&index, "geo", &[("1.2.0", &repo)]);

    // El proyecto declara la dep POR NOMBRE (sin URL git).
    let app = app(&base, "from geo import duplicate;\nfn main() -> int { print(duplicate(21)); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"1.2.0\"\n",
    )
    .unwrap();

    // `ray run` resuelve `geo = "1.2.0"` por el índice, descarga el repo y lo usa.
    let (out, err, code) = ray_idx(&app, &index, &["run"]);
    assert_eq!(code, 0, "run OK\n{err}");
    assert!(out.contains("42"), "uses la dep resuelta por el índice\n{out}\n{err}");
    assert!(app.join(".ray-deps/geo/mod.ray").is_file(), "el package quedó en la caché");
    // Se generó el lock con la versión resuelta.
    let lock = std::fs::read_to_string(app.join("ray.lock")).unwrap();
    assert!(lock.contains("[geo]") && lock.contains("v1.2.0"), "lock con la versión resuelta:\n{lock}");
}

#[test]
fn ray_add_writes_manifest_and_downloads() {
    let base = tmp("add");
    let index = base.join("index");
    let repo = publish(&base, "util", "0.3.1", "pub fn greeting() -> string { \"hello\" }\n");
    write_index(&index, "util", &[("0.3.1", &repo)]);
    let app = app(&base, "from util import greeting;\nfn main() -> int { print(greeting()); 0 }\n");

    // `ray add util` (sin versión) → escribe `util = "^0.3.1"` y descarga.
    let (out, err, code) = ray_idx(&app, &index, &["add", "util"]);
    assert_eq!(code, 0, "add OK\n{err}");
    assert!(out.contains("added"), "{out}");
    let toml = std::fs::read_to_string(app.join("ray.toml")).unwrap();
    assert!(toml.contains("util = \"^0.3.1\""), "el manifest quedó actualizado:\n{toml}");
    assert!(app.join(".ray-deps/util/mod.ray").is_file(), "descargó el package");

    // Y el programa corre con la dep recién añadida.
    let (out, err, code) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("hello"), "run after add\n{out}\n{err}");
    assert_eq!(code, 0);
}

#[test]
fn ray_add_with_exact_version_respects_requirement() {
    let base = tmp("addexact");
    let index = base.join("index");
    let r120 = publish(&base, "geo", "1.2.0", "pub fn v() -> int { 120 }\n");
    let r130 = publish(&base, "geo", "1.3.0", "pub fn v() -> int { 130 }\n");
    write_index(&index, "geo", &[("1.2.0", &r120), ("1.3.0", &r130)]);
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
fn caret_picks_the_highest_compatible() {
    let base = tmp("caret");
    let index = base.join("index");
    let r120 = publish(&base, "geo", "1.2.0", "pub fn v() -> int { 120 }\n");
    let r130 = publish(&base, "geo", "1.3.0", "pub fn v() -> int { 130 }\n");
    let r200 = publish(&base, "geo", "2.0.0", "pub fn v() -> int { 200 }\n");
    write_index(&index, "geo", &[("1.2.0", &r120), ("1.3.0", &r130), ("2.0.0", &r200)]);
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
fn package_nonexistent_da_error_claro() {
    let base = tmp("missing");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap(); // índice vacío
    let app = app(&base, "fn main() -> int { 0 }\n");
    let (_o, err, code) = ray_idx(&app, &index, &["add", "noexiste"]);
    assert_eq!(code, 65, "fails al añadir un package what no está en el índice");
    assert!(err.contains("is not in the index"), "mensaje claro:\n{err}");
    // Y no tocó el manifiesto.
    let toml = std::fs::read_to_string(app.join("ray.toml")).unwrap();
    assert!(!toml.contains("noexiste"), "no escribió la dep fallida:\n{toml}");
}

/// Un repo git "publicable" con remoto `origin` (un bare local): crea el bare, clona a un working
/// dir con `ray.toml`+`mod.ray`, commitea, taggea `v<ver>` y empuja rama + tags. Devuelve el working dir.
fn repo_con_origin(base: &Path, name: &str, see: &str, mod_ray: &str) -> std::path::PathBuf {
    let bare = base.join(format!("{name}.git"));
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "--bare", "-q"]);
    let work = base.join(format!("{name}-work"));
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q"]);
    git(&work, &["remote", "add", "origin", &bare.to_string_lossy()]);
    std::fs::write(work.join("mod.ray"), mod_ray).unwrap();
    std::fs::write(
        work.join("ray.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"{see}\"\n"),
    )
    .unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "pub"]);
    git(&work, &["tag", &format!("v{see}")]);
    git(&work, &["push", "-q", "origin", "HEAD"]);
    git(&work, &["push", "-q", "origin", "--tags"]);
    work
}

#[test]
fn ray_publish_añade_al_index_y_un_consumidor_lo_resolves() {
    let base = tmp("publish");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap();
    // Paquete `mate` con remoto origin + tag v1.0.0.
    let work = repo_con_origin(&base, "mate", "1.0.0", "pub fn triple(x: int) -> int { x * 3 }\n");

    // `ray publish` desde el paquete: deriva git+<origin>@v1.0.0, hashea y añade la entrada al índice.
    let (out, err, code) = ray_idx(&work, &index, &["registry", "publish"]);
    assert_eq!(code, 0, "publish OK\n{err}");
    assert!(out.contains("published mate 1.0.0"), "{out}");
    let entry = std::fs::read_to_string(index.join("mate.toml")).unwrap();
    assert!(entry.contains("[1.0.0]") && entry.contains("@v1.0.0") && entry.contains("hash ="), "entry en el índice:\n{entry}");

    // Republicar la MISMA versión → error de inmutabilidad, sin duplicar en el índice.
    let (_o, err, code) = ray_idx(&work, &index, &["registry", "publish"]);
    assert_eq!(code, 65, "republicar la misma versión fails");
    assert!(err.contains("is already published"), "{err}");

    // Un consumidor la resuelve por nombre desde el índice y la ejecuta (clona del origin al tag).
    let app = app(&base, "from mate import triple;\nfn main() -> int { print(triple(14)); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nmate = \"1.0.0\"\n",
    )
    .unwrap();
    let (out, err, code) = ray_idx(&app, &index, &["run"]);
    assert_eq!(code, 0, "el consumidor runs\n{err}");
    assert!(out.contains("42"), "usó el package publicado\n{out}\n{err}");
}

#[test]
fn ray_publish_without_tag_fails_clearly() {
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
    let (_o, err, code) = ray_idx(&work, &index, &["registry", "publish"]);
    assert_eq!(code, 65, "sin tag fails");
    assert!(err.contains("the tag 'v2.0.0' does not exist"), "mensaje claro:\n{err}");
}

#[test]
fn yank_excludes_version_from_new_resolutions() {
    let base = tmp("yank");
    let index = base.join("index");
    let r120 = publish(&base, "geo", "1.2.0", "pub fn v() -> int { 120 }\n");
    let r130 = publish(&base, "geo", "1.3.0", "pub fn v() -> int { 130 }\n");
    write_index(&index, "geo", &[("1.2.0", &r120), ("1.3.0", &r130)]);
    let app = app(&base, "from geo import v;\nfn main() -> int { print(v()); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"^1.2\"\n",
    )
    .unwrap();

    // Retira la 1.3.0 (usa el proyecto solo para localizar el índice vía RAY_INDEX).
    let (out, err, code) = ray_idx(&app, &index, &["registry", "yank", "geo@1.3.0"]);
    assert_eq!(code, 0, "yank OK\n{err}");
    assert!(out.contains("yanked"), "{out}");

    // Una resolución NUEVA (sin lock previo) elige la 1.2.0 (la 1.3.0 está retirada).
    let (out, err, _c) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("120"), "yank excluye la 1.3.0 en new resolución\n{out}\n{err}");

    // --undo la restaura; `ray update` re-resuelve a la 1.3.0 (el lock estaba fijado en 1.2.0).
    let (_o, _e, code) = ray_idx(&app, &index, &["registry", "yank", "geo@1.3.0", "--undo"]);
    assert_eq!(code, 0);
    let (_o, err, code) = ray_idx(&app, &index, &["update"]);
    assert_eq!(code, 0, "update after --undo\n{err}");
    let (out, _e, _c) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("130"), "--undo + update restaura la 1.3.0\n{out}");
}

#[test]
fn lock_pins_version_and_update_bumps_it() {
    let base = tmp("update");
    let index = base.join("index");
    let r120 = publish(&base, "geo", "1.2.0", "pub fn v() -> int { 120 }\n");
    write_index(&index, "geo", &[("1.2.0", &r120)]);
    let app = app(&base, "from geo import v;\nfn main() -> int { print(v()); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"^1.2\"\n",
    )
    .unwrap();

    // Primera resolución: 1.2.0 (la única). Queda fijada en el lock.
    let (out, _e, _c) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("120"), "resolves 1.2.0\n{out}");

    // Ahora aparece la 1.3.0 en el índice.
    let r130 = publish(&base, "geo", "1.3.0", "pub fn v() -> int { 130 }\n");
    write_index(&index, "geo", &[("1.2.0", &r120), ("1.3.0", &r130)]);

    // `ray run` (sin update) RESPETA el lock → sigue en 1.2.0 (reproducible).
    let (out, _e, _c) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("120"), "el lock fixes la versión pese a la new del índice\n{out}");

    // `ray update` sube a la 1.3.0 (la más alta que satisface ^1.2).
    let (_o, err, code) = ray_idx(&app, &index, &["update"]);
    assert_eq!(code, 0, "update OK\n{err}");
    let (out, _e, _c) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("130"), "update sube a la 1.3.0\n{out}");
}

#[test]
fn remote_git_index_clones_and_resolves() {
    let base = tmp("remoteidx");
    // El índice es un REPO git (no un dir suelto): se crea, se escriben sus archivos y se commitea.
    let index_repo = base.join("index-repo");
    let repo = publish(&base, "geo", "1.0.0", "pub fn v() -> int { 99 }\n");
    write_index(&index_repo, "geo", &[("1.0.0", &repo)]);
    git(&index_repo, &["init", "-q"]);
    git(&index_repo, &["add", "-A"]);
    git(&index_repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "idx"]);

    // El proyecto apunta al índice por git+file:// en [registry].
    let app = app(&base, "from geo import v;\nfn main() -> int { print(v()); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[registry]\nindex = \"git+file://{}\"\n\n[dependencies]\ngeo = \"1.0.0\"\n",
            index_repo.display()
        ),
    )
    .unwrap();

    // `ray run` clona el índice a .ray-deps/.index, resuelve geo y lo ejecuta.
    let (out, err, code) = ray_plain(&app, &["run"]);
    assert_eq!(code, 0, "run con índice remoto OK\n{err}");
    assert!(out.contains("99"), "resolvió por el índice remoto clonado\n{out}\n{err}");
    assert!(app.join(".ray-deps/.index/geo.toml").is_file(), "el índice quedó cacheado");
}

#[test]
fn spec_by_name_without_configured_index_warns() {
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
    assert!(err.contains("no index") || err.contains("index"), "avisa de la falta de índice:\n{err}");
}

// ── M51d: endurecimiento (nombres, hash del índice, publish del tag, índice re-cacheado) ──

#[test]
fn invalid_package_name_is_rejected() {
    let base = tmp("badname");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap();
    let app = app(&base, "fn main() -> int { 0 }\n");

    // `ray add` con un nombre que escaparía de la caché → rechazado antes de tocar nada.
    let (_o, err, code) = ray_idx(&app, &index, &["add", "../evil"]);
    assert_eq!(code, 64, "add con name inválido fails\n{err}");
    assert!(err.contains("invalid package name"), "mensaje claro:\n{err}");

    // Una dep DIRECTA con nombre inválido en ray.toml → error al resolver (no se usa como ruta).
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n../evil = \"git+file:///nada@v1\"\n",
    )
    .unwrap();
    let (_o, err, code) = ray_idx(&app, &index, &["run"]);
    assert_ne!(code, 0, "dep directa con name inválido fails");
    assert!(err.contains("invalid package name"), "mensaje claro:\n{err}");

    // La valla importante: una dep TRANSITIVA (el ray.toml de un paquete descargado, NO confiable)
    // con nombre malicious → error, sin clonar ni borrar fuera de `.ray-deps/`.
    let malicious = publish(&base, "geo", "1.0.0", "pub fn v() -> int { 1 }\n");
    std::fs::write(
        malicious.join("ray.toml"),
        "[package]\nname = \"geo\"\nversion = \"1.0.0\"\n\n[dependencies]\n../../pwn = \"git+file:///nada@v1\"\n",
    )
    .unwrap();
    git(&malicious, &["add", "-A"]);
    git(&malicious, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "mal"]);
    git(&malicious, &["tag", "-f", "v1.0.0"]);
    write_index(&index, "geo", &[("1.0.0", &malicious)]);
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"1.0.0\"\n",
    )
    .unwrap();
    let (_o, err, code) = ray_idx(&app, &index, &["run"]);
    assert_ne!(code, 0, "transitiva con name inválido fails");
    assert!(
        err.contains("invalid package name") && err.contains("declared by dependency 'geo'"),
        "señala al culpable:\n{err}"
    );
}

#[test]
fn publish_hashes_the_tag_not_the_working_tree() {
    let base = tmp("publishtag");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap();
    let work = repo_con_origin(&base, "mate", "1.0.0", "pub fn triple(x: int) -> int { x * 3 }\n");

    // Ensuciar el working tree DESPUÉS de taggear: cambios sin commitear + un archivo suelto.
    // El hash publicado debe ser el del TAG (lo que el consumidor descargará), no el del árbol sucio.
    std::fs::write(work.join("mod.ray"), "pub fn triple(x: int) -> int { x * 999 }\n").unwrap();
    std::fs::write(work.join("borrador.txt"), "no publicado").unwrap();
    let (out, err, code) = ray_idx(&work, &index, &["registry", "publish"]);
    assert_eq!(code, 0, "publish con working tree sucio OK (hashea el tag)\n{err}");
    assert!(out.contains("published mate 1.0.0"), "{out}");

    // El consumidor resuelve, descarga el tag y la verificación del hash del índice PASA
    // (con el hash del working tree sucio, fallaría).
    let app = app(&base, "from mate import triple;\nfn main() -> int { print(triple(14)); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nmate = \"1.0.0\"\n",
    )
    .unwrap();
    let (out, err, code) = ray_idx(&app, &index, &["run"]);
    assert_eq!(code, 0, "el consumidor runs y el hash del índice casa\n{err}");
    assert!(out.contains("42"), "uses el contenido del tag (x*3), no el árbol sucio (x*999)\n{out}");
}

#[test]
fn el_hash_del_index_se_verifies() {
    let base = tmp("idxhash");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap();
    let work = repo_con_origin(&base, "mate", "1.0.0", "pub fn v() -> int { 7 }\n");
    let (_o, err, code) = ray_idx(&work, &index, &["registry", "publish"]);
    assert_eq!(code, 0, "publish OK\n{err}");

    // Manipular el hash publicado (simula un índice que avala OTRO contenido que el del repo).
    let entry_path = index.join("mate.toml");
    let entry = std::fs::read_to_string(&entry_path).unwrap();
    let tampered: String = entry
        .lines()
        .map(|l| if l.trim_start().starts_with("hash") { "hash = \"sha256:0000\"".to_string() } else { l.to_string() })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&entry_path, tampered).unwrap();

    // El consumidor descarga y la verificación contra el hash del índice FALLA con mensaje claro.
    let app = app(&base, "from mate import v;\nfn main() -> int { print(v()); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nmate = \"1.0.0\"\n",
    )
    .unwrap();
    let (_o, err, code) = ray_idx(&app, &index, &["run"]);
    assert_ne!(code, 0, "el hash del índice manipulado corta la resolución");
    assert!(err.contains("hash published in the index"), "mensaje claro:\n{err}");
}

#[test]
fn remote_index_recached_when_spec_changes() {
    let base = tmp("reidx");
    // Dos índices-repo git distintos: el 1º publica geo 1.0.0 (imprime 100), el 2º geo 2.0.0 (200).
    let r100 = publish(&base, "geo", "1.0.0", "pub fn v() -> int { 100 }\n");
    let r200 = publish(&base, "geo", "2.0.0", "pub fn v() -> int { 200 }\n");
    let idx1 = base.join("idx1");
    write_index(&idx1, "geo", &[("1.0.0", &r100)]);
    git(&idx1, &["init", "-q"]);
    git(&idx1, &["add", "-A"]);
    git(&idx1, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "i1"]);
    let idx2 = base.join("idx2");
    write_index(&idx2, "geo", &[("2.0.0", &r200)]);
    git(&idx2, &["init", "-q"]);
    git(&idx2, &["add", "-A"]);
    git(&idx2, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "i2"]);

    let manifest = |idx: &Path| {
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[registry]\nindex = \"git+file://{}\"\n\n[dependencies]\ngeo = \"*\"\n",
            idx.display()
        )
    };
    let app = app(&base, "from geo import v;\nfn main() -> int { print(v()); 0 }\n");
    std::fs::write(app.join("ray.toml"), manifest(&idx1)).unwrap();
    let (out, err, code) = ray_plain(&app, &["run"]);
    assert_eq!(code, 0, "resolves del índice 1\n{err}");
    assert!(out.contains("100"), "geo 1.0.0 del índice 1\n{out}");

    // Cambiar la spec del índice en ray.toml → la caché `.ray-deps/.index` debe descartarse y
    // re-clonarse (M51d; antes se quedaba obsoleta en silencio). `ray update` re-resuelve.
    std::fs::write(app.join("ray.toml"), manifest(&idx2)).unwrap();
    let (_o, err, code) = ray_plain(&app, &["update"]);
    assert_eq!(code, 0, "update con el índice cambiado\n{err}");
    let (out, err, code) = ray_plain(&app, &["run"]);
    assert_eq!(code, 0, "runs after el cambio de índice\n{err}");
    assert!(out.contains("200"), "resolvió geo 2.0.0 del índice 2 (re-clonado)\n{out}\n{err}");
}

// ── M51e: H5 check semántico en publish · H6 pre-releases · H7 aviso de índice propio ──

#[test]
fn publish_runs_the_semantic_check() {
    let base = tmp("publishcheck");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap();

    // (a) Un paquete que lexea y parsea pero NO chequea (tipo de retorno mal) → publish falla.
    let roto = publish(&base, "roto", "1.0.0", "pub fn v() -> int { true }\n");
    let repo_spec = format!("git+file://{}@v1.0.0", roto.display());
    let (_o, err, code) = ray_idx(&roto, &index, &["registry", "publish", "--repo", &repo_spec]);
    assert_eq!(code, 65, "publish de un package what no chequea fails\n{err}");
    assert!(err.contains("does not pass the semantic check"), "mensaje claro:\n{err}");
    assert!(!index.join("roto.toml").exists(), "no se publicó nada");

    // (b) Un paquete CON dependencia por nombre: el check la resuelve (índice) y pasa.
    let geo = publish(&base, "geo", "1.0.0", "pub fn v() -> int { 21 }\n");
    write_index(&index, "geo", &[("1.0.0", &geo)]);
    let calc = base.join("calc-work");
    std::fs::create_dir_all(&calc).unwrap();
    git(&calc, &["init", "-q"]);
    std::fs::write(calc.join("mod.ray"), "from geo import v;\npub fn double() -> int { v() * 2 }\n").unwrap();
    std::fs::write(
        calc.join("ray.toml"),
        "[package]\nname = \"calc\"\nversion = \"1.0.0\"\n\n[dependencies]\ngeo = \"1.0.0\"\n",
    )
    .unwrap();
    git(&calc, &["add", "-A"]);
    git(&calc, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "calc"]);
    git(&calc, &["tag", "v1.0.0"]);
    let repo_spec = format!("git+file://{}@v1.0.0", calc.display());
    let (out, err, code) = ray_idx(&calc, &index, &["registry", "publish", "--repo", &repo_spec]);
    assert_eq!(code, 0, "publish con dep por name chequea y pasa\n{err}");
    assert!(out.contains("published calc 1.0.0"), "{out}");
}

#[test]
fn pre_releases_son_opt_in() {
    let base = tmp("prerel");
    let index = base.join("index");
    let r120 = publish(&base, "geo", "1.2.0", "pub fn v() -> int { 120 }\n");
    let rrc = publish(&base, "geo", "1.3.0-rc1", "pub fn v() -> int { 131 }\n");
    write_index(&index, "geo", &[("1.2.0", &r120), ("1.3.0-rc1", &rrc)]);
    let app = app(&base, "from geo import v;\nfn main() -> int { print(v()); 0 }\n");

    // Un rango (^1.2) NUNCA elige la pre-release por sorpresa.
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"^1.2\"\n",
    )
    .unwrap();
    let (out, err, _c) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("120"), "el caret excluye la 1.3.0-rc1\n{out}\n{err}");

    // `ray add geo` (sin versión) también elige la FINAL más alta, no la rc.
    let (out, err, code) = ray_idx(&app, &index, &["add", "geo"]);
    assert_eq!(code, 0, "add OK\n{err}");
    assert!(out.contains("geo = \"^1.2.0\""), "latest ignora la pre-release:\n{out}");

    // Pedirla EXPLÍCITAMENTE sí la instala.
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"1.3.0-rc1\"\n",
    )
    .unwrap();
    let (out, err, code) = ray_idx(&app, &index, &["run"]);
    assert_eq!(code, 0, "la pre-release explícita runs\n{err}");
    assert!(out.contains("131"), "instaló la 1.3.0-rc1 pedida\n{out}");
}

#[test]
fn transitive_dep_with_own_index_warns() {
    let base = tmp("ownidx");
    let index = base.join("index");
    let util = publish(&base, "util", "1.0.0", "pub fn u() -> int { 5 }\n");
    write_index(&index, "util", &[("1.0.0", &util)]);
    // `geo` declara su PROPIO índice y una dep por nombre → al consumirla, aviso de confusion.
    let geo = base.join("geo-repo");
    std::fs::create_dir_all(&geo).unwrap();
    git(&geo, &["init", "-q"]);
    std::fs::write(geo.join("mod.ray"), "pub fn v() -> int { 7 }\n").unwrap();
    std::fs::write(
        geo.join("ray.toml"),
        "[package]\nname = \"geo\"\nversion = \"1.0.0\"\n\n[registry]\nindex = \"git+https://other.ejemplo/index\"\n\n[dependencies]\nutil = \"1.0.0\"\n",
    )
    .unwrap();
    git(&geo, &["add", "-A"]);
    git(&geo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "geo"]);
    git(&geo, &["tag", "v1.0.0"]);
    write_index(&index, "geo", &[("1.0.0", &geo)]);

    let app = app(&base, "from geo import v;\nfn main() -> int { print(v()); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"1.0.0\"\n",
    )
    .unwrap();
    // Corre (util se resuelve contra NUESTRO índice) pero avisa del índice propio de geo.
    let (out, err, code) = ray_idx(&app, &index, &["run"]);
    assert_eq!(code, 0, "runs pese al aviso\n{err}");
    assert!(out.contains("7"), "{out}");
    assert!(err.contains("declares its own index"), "aviso de dependency confusion:\n{err}");
}

// ── M51f: ray remove + ray search ──

#[test]
fn ray_remove_removes_dep_lock_and_cache() {
    let base = tmp("remove");
    let index = base.join("index");
    let geo = publish(&base, "geo", "1.0.0", "pub fn v() -> int { 9 }\n");
    let util = publish(&base, "util", "1.0.0", "pub fn u() -> int { 1 }\n");
    write_index(&index, "geo", &[("1.0.0", &geo)]);
    write_index(&index, "util", &[("1.0.0", &util)]);
    let app = app(&base, "from geo import v;\nfn main() -> int { print(v()); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"1.0.0\"\nutil = \"1.0.0\"\n",
    )
    .unwrap();
    let (_o, err, code) = ray_idx(&app, &index, &["fetch"]);
    assert_eq!(code, 0, "fetch inicial\n{err}");
    assert!(app.join(".ray-deps/util/mod.ray").is_file(), "util en la caché");

    // remove: quita del manifiesto, reescribe el lock y borra la caché (nadie más usa util).
    let (out, err, code) = ray_idx(&app, &index, &["remove", "util"]);
    assert_eq!(code, 0, "remove OK\n{err}");
    assert!(out.contains("removed"), "{out}");
    let toml = std::fs::read_to_string(app.join("ray.toml")).unwrap();
    assert!(!toml.contains("util"), "outside del manifest:\n{toml}");
    let lock = std::fs::read_to_string(app.join("ray.lock")).unwrap();
    assert!(!lock.contains("[util]") && lock.contains("[geo]"), "lock re-resuelto:\n{lock}");
    assert!(!app.join(".ray-deps/util").exists(), "caché de util borrada");
    // El programa sigue corriendo con la dep restante.
    let (out, _e, code) = ray_idx(&app, &index, &["run"]);
    assert_eq!(code, 0);
    assert!(out.contains("9"), "{out}");

    // remove de algo no declarado → error claro.
    let (_o, err, code) = ray_idx(&app, &index, &["remove", "util"]);
    assert_eq!(code, 65, "remove de one dep nonexistent fails");
    assert!(err.contains("is not declared"), "{err}");
}

#[test]
fn ray_search_list_el_index() {
    let base = tmp("search");
    let index = base.join("index");
    let r12 = publish(&base, "geo", "1.2.0", "pub fn v() -> int { 1 }\n");
    let r13 = publish(&base, "geo", "1.3.0", "pub fn v() -> int { 2 }\n");
    let net = publish(&base, "net-extra", "0.1.0", "pub fn n() -> int { 3 }\n");
    write_index(&index, "geo", &[("1.2.0", &r12), ("1.3.0", &r13)]);
    write_index(&index, "net-extra", &[("0.1.0", &net)]);
    let app = app(&base, "fn main() -> int { 0 }\n");

    // Con patrón: solo los que casan, con su última versión instalable.
    let (out, err, code) = ray_idx(&app, &index, &["search", "ge"]);
    assert_eq!(code, 0, "search OK\n{err}");
    assert!(out.contains("geo 1.3.0"), "geo con su última:\n{out}");
    assert!(!out.contains("net-extra"), "net-extra no casa 'ge':\n{out}");

    // Sin patrón: todos, ordenados.
    let (out, _e, code) = ray_idx(&app, &index, &["search"]);
    assert_eq!(code, 0);
    assert!(out.contains("geo 1.3.0") && out.contains("net-extra 0.1.0"), "list complete:\n{out}");
    assert!(out.contains("2 package(s)"), "{out}");

    // Sin resultados → mensaje, código 0 (no es un error).
    let (out, _e, code) = ray_idx(&app, &index, &["search", "zzz"]);
    assert_eq!(code, 0);
    assert!(out.contains("no results"), "{out}");
}

// ─────────────────────────────────────────────────────────────────────────────
// M83b/c — dueños de nombre + firmas de publicación.
// ─────────────────────────────────────────────────────────────────────────────

/// Como `ray_idx` pero con una clave de publicación (`RAY_KEY`) apuntada.
fn ray_signed(cwd: &Path, index: &Path, key: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(BIN)
        .args(args)
        .current_dir(cwd)
        .env("RAY_INDEX", index)
        .env("RAY_KEY", key)
        .output()
        .expect("lanza el binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// keygen → publish --sign: reclama el nombre (owners.toml + TOFU de la pubkey), firma la
/// entrada, y un consumidor resuelve con la firma VERIFICADA. index-verify da OK.
#[test]
fn signed_publish_claims_verifies_and_audits() {
    let base = tmp("sign");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap();
    let key = base.join("publish.key");
    let work = repo_con_origin(&base, "mate", "1.0.0", "pub fn seis() -> int { 6 }\n");

    let (out, err, code) = ray_signed(&work, &index, &key, &["registry", "keygen"]);
    assert_eq!(code, 0, "keygen OK\n{err}");
    assert!(out.contains("pubkey: ed25519:"), "{out}");

    let (out, err, code) = ray_signed(&work, &index, &key, &["registry", "publish", "--sign"]);
    assert_eq!(code, 0, "publish --sign OK\n{err}");
    assert!(out.contains("claimed in the index"), "first publicación reclama\n{out}");
    assert!(out.contains("signature: ed25519"), "{out}");
    let entry = std::fs::read_to_string(index.join("mate.toml")).unwrap();
    assert!(entry.contains("sig = \"ed25519:"), "la entry lleva la signature\n{entry}");
    let owners = std::fs::read_to_string(index.join("mate.owners.toml")).unwrap();
    assert!(owners.contains("pubkey = \"ed25519:"), "el sidecar registra la pubkey\n{owners}");

    // El consumidor resuelve (la verificación de firma pasa) y corre.
    let app = app(&base, "from mate import seis;\nfn main() -> int { print(seis() * 7); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nmate = \"1.0.0\"\n",
    )
    .unwrap();
    let (out, err, code) = ray_idx(&app, &index, &["run"]);
    assert_eq!(code, 0, "el consumidor runs con la signature verificada\n{err}");
    assert!(out.contains("42"), "{out}");

    // La auditoría del índice (para el CI del repo del índice) está verde.
    let (out, err, code) = ray_idx(&app, &index, &["registry", "verify", index.to_str().unwrap()]);
    assert_eq!(code, 0, "index-verify OK\n{err}");
    assert!(out.contains("1 signed and verified"), "{out}");
}

/// Una firma MANIPULADA (o que no casa con el dueño) rompe la resolución y la auditoría.
#[test]
fn tampered_signature_breaks_resolution_and_audit() {
    let base = tmp("signtamper");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap();
    let key = base.join("publish.key");
    let work = repo_con_origin(&base, "mate", "1.0.0", "pub fn v() -> int { 7 }\n");
    ray_signed(&work, &index, &key, &["registry", "keygen"]);
    let (_o, err, code) = ray_signed(&work, &index, &key, &["registry", "publish", "--sign"]);
    assert_eq!(code, 0, "publish --sign OK\n{err}");

    // Voltear la firma publicada (índice comprometido que avala otra cosa).
    let path = index.join("mate.toml");
    let entry = std::fs::read_to_string(&path).unwrap();
    let tampered: String = entry
        .lines()
        .map(|l| {
            if l.starts_with("sig = ") {
                format!("sig = \"ed25519:{}\"", "ab".repeat(64))
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, tampered).unwrap();

    let app = app(&base, "from mate import v;\nfn main() -> int { print(v()); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nmate = \"1.0.0\"\n",
    )
    .unwrap();
    let (_out, err, code) = ray_idx(&app, &index, &["run"]);
    assert_ne!(code, 0, "la invalid signature must romper la resolución");
    assert!(err.contains("SIGNATURE") && err.contains("does not verify"), "{err}");

    let (_out, err, code) = ray_idx(&app, &index, &["registry", "verify", index.to_str().unwrap()]);
    assert_ne!(code, 0, "index-verify must fallar\n{err}");
    assert!(err.contains("FALLO"), "{err}");
}

/// El nombre reclamado PROTEGE: otra clave no puede publicar --sign sobre él.
#[test]
fn other_key_cannot_publish_a_claimed_name() {
    let base = tmp("signowner");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap();
    let key1 = base.join("k1.key");
    let key2 = base.join("k2.key");
    let work = repo_con_origin(&base, "mate", "1.0.0", "pub fn v() -> int { 7 }\n");
    ray_signed(&work, &index, &key1, &["registry", "keygen"]);
    ray_signed(&work, &index, &key2, &["registry", "keygen", "--out", key2.to_str().unwrap()]);
    let (_o, err, code) = ray_signed(&work, &index, &key1, &["registry", "publish", "--sign"]);
    assert_eq!(code, 0, "el dueño public\n{err}");

    // v1.1.0 con OTRA clave → rechazado por el dueño registrado.
    std::fs::write(
        work.join("ray.toml"),
        "[package]\nname = \"mate\"\nversion = \"1.1.0\"\n\n[dependencies]\n",
    )
    .unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-m", "v1.1.0"]);
    git(&work, &["tag", "v1.1.0"]);
    let (_out, err, code) = ray_signed(&work, &index, &key2, &["registry", "publish", "--sign"]);
    assert_ne!(code, 0, "other clave no public un name ajeno");
    assert!(err.contains("your key does NOT match"), "{err}");
}

// ── Mirrors de paquetes (M90.1) ──────────────────────────────────────────────────────

/// Crea el clon-mirror de `repo` bajo `mirror_root` respetando la regla de reescritura
/// `prefijo/<url-sin-esquema>`: el repo `file:///a/b/geo` vive en `<mirror_root>/a/b/geo`.
fn clone_into_mirror(mirror_root: &Path, repo: &Path) {
    let rel = repo.to_string_lossy();
    let dest = mirror_root.join(rel.trim_start_matches('/'));
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    let out = Command::new("git")
        .args(["clone", "--quiet", &format!("file://{rel}"), &dest.to_string_lossy()])
        .output()
        .expect("git disponible");
    assert!(out.status.success(), "clona el mirror: {}", String::from_utf8_lossy(&out.stderr));
}

/// El mirror sirve el paquete aunque el origen haya DESAPARECIDO (disponibilidad); el hash
/// del índice verifica el contenido igual (mirror trustless).
#[test]
fn mirror_serves_package_if_origin_disappears() {
    let base = tmp("mirror");
    let index = base.join("index");
    let repo = publish(&base, "geo", "1.2.0", "pub fn v() -> int { 120 }\n");
    write_index(&index, "geo", &[("1.2.0", &repo)]);
    let mirror_root = base.join("mirror");
    clone_into_mirror(&mirror_root, &repo);
    // El origen cae: el índice sigue apuntando a él, pero ya no existe.
    std::fs::remove_dir_all(&repo).unwrap();

    let app = app(&base, "from geo import v;\nfn main() -> int { print(v()); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[registry]\nmirror = \"file://{}\"\n\n\
             [dependencies]\ngeo = \"1.2.0\"\n",
            mirror_root.display()
        ),
    )
    .unwrap();
    let (out, err, code) = ray_idx(&app, &index, &["run"]);
    assert_eq!(code, 0, "run OK vía mirror\n{err}");
    assert!(out.contains("120"), "uses el package servido por el mirror\n{out}");
    // El lock registra la URL ORIGINAL (el mirror es transporte, no identidad).
    let lock = std::fs::read_to_string(app.join("ray.lock")).unwrap();
    assert!(!lock.contains("/mirror/"), "el lock no must apuntar al mirror:\n{lock}");
}

/// Un mirror que no tiene el paquete NO rompe la resolución: se avisa y se cae al origen.
#[test]
fn mirror_down_falls_back_to_original_url() {
    let base = tmp("mirrorfall");
    let index = base.join("index");
    let repo = publish(&base, "geo", "1.2.0", "pub fn v() -> int { 120 }\n");
    write_index(&index, "geo", &[("1.2.0", &repo)]);

    let app = app(&base, "from geo import v;\nfn main() -> int { print(v()); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[registry]\nmirror = \"file://{}\"\n\n\
             [dependencies]\ngeo = \"1.2.0\"\n",
            base.join("mirror-empty").display() // no existe
        ),
    )
    .unwrap();
    let (out, err, code) = ray_idx(&app, &index, &["run"]);
    assert_eq!(code, 0, "run OK cayendo al origen\n{err}");
    assert!(out.contains("120"), "{out}");
    assert!(err.contains("warning: the mirror did not serve 'geo'"), "avisa del fallback:\n{err}");
}
