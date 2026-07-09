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

/// Como `ray_idx` pero SIN `RAY_INDEX` (para probar `[registry] index` del ray.toml).
fn ray_plain(cwd: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(BIN)
        .args(args)
        .current_dir(cwd)
        .env_remove("RAY_INDEX")
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
fn yank_excluye_la_version_de_nuevas_resoluciones() {
    let base = tmp("yank");
    let index = base.join("index");
    let r120 = publicar(&base, "geo", "1.2.0", "pub fn v() -> int { 120 }\n");
    let r130 = publicar(&base, "geo", "1.3.0", "pub fn v() -> int { 130 }\n");
    indexar(&index, "geo", &[("1.2.0", &r120), ("1.3.0", &r130)]);
    let app = app(&base, "from geo import v;\nfn main() -> int { print(v()); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"^1.2\"\n",
    )
    .unwrap();

    // Retira la 1.3.0 (usa el proyecto solo para localizar el índice vía RAY_INDEX).
    let (out, err, code) = ray_idx(&app, &index, &["yank", "geo@1.3.0"]);
    assert_eq!(code, 0, "yank OK\n{err}");
    assert!(out.contains("retirada"), "{out}");

    // Una resolución NUEVA (sin lock previo) elige la 1.2.0 (la 1.3.0 está retirada).
    let (out, err, _c) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("120"), "yank excluye la 1.3.0 en nueva resolución\n{out}\n{err}");

    // --undo la restaura; `ray update` re-resuelve a la 1.3.0 (el lock estaba fijado en 1.2.0).
    let (_o, _e, code) = ray_idx(&app, &index, &["yank", "geo@1.3.0", "--undo"]);
    assert_eq!(code, 0);
    let (_o, err, code) = ray_idx(&app, &index, &["update"]);
    assert_eq!(code, 0, "update tras --undo\n{err}");
    let (out, _e, _c) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("130"), "--undo + update restaura la 1.3.0\n{out}");
}

#[test]
fn el_lock_fija_la_version_y_update_la_sube() {
    let base = tmp("update");
    let index = base.join("index");
    let r120 = publicar(&base, "geo", "1.2.0", "pub fn v() -> int { 120 }\n");
    indexar(&index, "geo", &[("1.2.0", &r120)]);
    let app = app(&base, "from geo import v;\nfn main() -> int { print(v()); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"^1.2\"\n",
    )
    .unwrap();

    // Primera resolución: 1.2.0 (la única). Queda fijada en el lock.
    let (out, _e, _c) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("120"), "resuelve 1.2.0\n{out}");

    // Ahora aparece la 1.3.0 en el índice.
    let r130 = publicar(&base, "geo", "1.3.0", "pub fn v() -> int { 130 }\n");
    indexar(&index, "geo", &[("1.2.0", &r120), ("1.3.0", &r130)]);

    // `ray run` (sin update) RESPETA el lock → sigue en 1.2.0 (reproducible).
    let (out, _e, _c) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("120"), "el lock fija la versión pese a la nueva del índice\n{out}");

    // `ray update` sube a la 1.3.0 (la más alta que satisface ^1.2).
    let (_o, err, code) = ray_idx(&app, &index, &["update"]);
    assert_eq!(code, 0, "update OK\n{err}");
    let (out, _e, _c) = ray_idx(&app, &index, &["run"]);
    assert!(out.contains("130"), "update sube a la 1.3.0\n{out}");
}

#[test]
fn indice_remoto_por_git_se_clona_y_resuelve() {
    let base = tmp("remoteidx");
    // El índice es un REPO git (no un dir suelto): se crea, se escriben sus archivos y se commitea.
    let index_repo = base.join("index-repo");
    let repo = publicar(&base, "geo", "1.0.0", "pub fn v() -> int { 99 }\n");
    indexar(&index_repo, "geo", &[("1.0.0", &repo)]);
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

// ── M51d: endurecimiento (nombres, hash del índice, publish del tag, índice re-cacheado) ──

#[test]
fn nombre_de_paquete_invalido_se_rechaza() {
    let base = tmp("badname");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap();
    let app = app(&base, "fn main() -> int { 0 }\n");

    // `ray add` con un nombre que escaparía de la caché → rechazado antes de tocar nada.
    let (_o, err, code) = ray_idx(&app, &index, &["add", "../evil"]);
    assert_eq!(code, 64, "add con nombre inválido falla\n{err}");
    assert!(err.contains("nombre de paquete inválido"), "mensaje claro:\n{err}");

    // Una dep DIRECTA con nombre inválido en ray.toml → error al resolver (no se usa como ruta).
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n../evil = \"git+file:///nada@v1\"\n",
    )
    .unwrap();
    let (_o, err, code) = ray_idx(&app, &index, &["run"]);
    assert_ne!(code, 0, "dep directa con nombre inválido falla");
    assert!(err.contains("nombre de paquete inválido"), "mensaje claro:\n{err}");

    // La valla importante: una dep TRANSITIVA (el ray.toml de un paquete descargado, NO confiable)
    // con nombre malicioso → error, sin clonar ni borrar fuera de `.ray-deps/`.
    let malicioso = publicar(&base, "geo", "1.0.0", "pub fn v() -> int { 1 }\n");
    std::fs::write(
        malicioso.join("ray.toml"),
        "[package]\nname = \"geo\"\nversion = \"1.0.0\"\n\n[dependencies]\n../../pwn = \"git+file:///nada@v1\"\n",
    )
    .unwrap();
    git(&malicioso, &["add", "-A"]);
    git(&malicioso, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "mal"]);
    git(&malicioso, &["tag", "-f", "v1.0.0"]);
    indexar(&index, "geo", &[("1.0.0", &malicioso)]);
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"1.0.0\"\n",
    )
    .unwrap();
    let (_o, err, code) = ray_idx(&app, &index, &["run"]);
    assert_ne!(code, 0, "transitiva con nombre inválido falla");
    assert!(
        err.contains("nombre de paquete inválido") && err.contains("declarado por la dependencia 'geo'"),
        "señala al culpable:\n{err}"
    );
}

#[test]
fn publish_hashea_el_tag_no_el_working_tree() {
    let base = tmp("publishtag");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap();
    let work = repo_con_origin(&base, "mate", "1.0.0", "pub fn triple(x: int) -> int { x * 3 }\n");

    // Ensuciar el working tree DESPUÉS de taggear: cambios sin commitear + un archivo suelto.
    // El hash publicado debe ser el del TAG (lo que el consumidor descargará), no el del árbol sucio.
    std::fs::write(work.join("mod.ray"), "pub fn triple(x: int) -> int { x * 999 }\n").unwrap();
    std::fs::write(work.join("borrador.txt"), "no publicado").unwrap();
    let (out, err, code) = ray_idx(&work, &index, &["publish"]);
    assert_eq!(code, 0, "publish con working tree sucio OK (hashea el tag)\n{err}");
    assert!(out.contains("publicado mate 1.0.0"), "{out}");

    // El consumidor resuelve, descarga el tag y la verificación del hash del índice PASA
    // (con el hash del working tree sucio, fallaría).
    let app = app(&base, "from mate import triple;\nfn main() -> int { print(triple(14)); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nmate = \"1.0.0\"\n",
    )
    .unwrap();
    let (out, err, code) = ray_idx(&app, &index, &["run"]);
    assert_eq!(code, 0, "el consumidor corre y el hash del índice casa\n{err}");
    assert!(out.contains("42"), "usa el contenido del tag (x*3), no el árbol sucio (x*999)\n{out}");
}

#[test]
fn el_hash_del_indice_se_verifica() {
    let base = tmp("idxhash");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap();
    let work = repo_con_origin(&base, "mate", "1.0.0", "pub fn v() -> int { 7 }\n");
    let (_o, err, code) = ray_idx(&work, &index, &["publish"]);
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
    assert!(err.contains("hash publicado en el índice"), "mensaje claro:\n{err}");
}

#[test]
fn indice_remoto_recacheado_si_cambia_la_spec() {
    let base = tmp("reidx");
    // Dos índices-repo git distintos: el 1º publica geo 1.0.0 (imprime 100), el 2º geo 2.0.0 (200).
    let r100 = publicar(&base, "geo", "1.0.0", "pub fn v() -> int { 100 }\n");
    let r200 = publicar(&base, "geo", "2.0.0", "pub fn v() -> int { 200 }\n");
    let idx1 = base.join("idx1");
    indexar(&idx1, "geo", &[("1.0.0", &r100)]);
    git(&idx1, &["init", "-q"]);
    git(&idx1, &["add", "-A"]);
    git(&idx1, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "i1"]);
    let idx2 = base.join("idx2");
    indexar(&idx2, "geo", &[("2.0.0", &r200)]);
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
    assert_eq!(code, 0, "resuelve del índice 1\n{err}");
    assert!(out.contains("100"), "geo 1.0.0 del índice 1\n{out}");

    // Cambiar la spec del índice en ray.toml → la caché `.ray-deps/.index` debe descartarse y
    // re-clonarse (M51d; antes se quedaba obsoleta en silencio). `ray update` re-resuelve.
    std::fs::write(app.join("ray.toml"), manifest(&idx2)).unwrap();
    let (_o, err, code) = ray_plain(&app, &["update"]);
    assert_eq!(code, 0, "update con el índice cambiado\n{err}");
    let (out, err, code) = ray_plain(&app, &["run"]);
    assert_eq!(code, 0, "corre tras el cambio de índice\n{err}");
    assert!(out.contains("200"), "resolvió geo 2.0.0 del índice 2 (re-clonado)\n{out}\n{err}");
}

// ── M51e: H5 check semántico en publish · H6 pre-releases · H7 aviso de índice propio ──

#[test]
fn publish_corre_el_check_semantico() {
    let base = tmp("publishcheck");
    let index = base.join("index");
    std::fs::create_dir_all(&index).unwrap();

    // (a) Un paquete que lexea y parsea pero NO chequea (tipo de retorno mal) → publish falla.
    let roto = publicar(&base, "roto", "1.0.0", "pub fn v() -> int { true }\n");
    let repo_spec = format!("git+file://{}@v1.0.0", roto.display());
    let (_o, err, code) = ray_idx(&roto, &index, &["publish", "--repo", &repo_spec]);
    assert_eq!(code, 65, "publish de un paquete que no chequea falla\n{err}");
    assert!(err.contains("no supera el check semántico"), "mensaje claro:\n{err}");
    assert!(!index.join("roto.toml").exists(), "no se publicó nada");

    // (b) Un paquete CON dependencia por nombre: el check la resuelve (índice) y pasa.
    let geo = publicar(&base, "geo", "1.0.0", "pub fn v() -> int { 21 }\n");
    indexar(&index, "geo", &[("1.0.0", &geo)]);
    let calc = base.join("calc-work");
    std::fs::create_dir_all(&calc).unwrap();
    git(&calc, &["init", "-q"]);
    std::fs::write(calc.join("mod.ray"), "from geo import v;\npub fn doble() -> int { v() * 2 }\n").unwrap();
    std::fs::write(
        calc.join("ray.toml"),
        "[package]\nname = \"calc\"\nversion = \"1.0.0\"\n\n[dependencies]\ngeo = \"1.0.0\"\n",
    )
    .unwrap();
    git(&calc, &["add", "-A"]);
    git(&calc, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "calc"]);
    git(&calc, &["tag", "v1.0.0"]);
    let repo_spec = format!("git+file://{}@v1.0.0", calc.display());
    let (out, err, code) = ray_idx(&calc, &index, &["publish", "--repo", &repo_spec]);
    assert_eq!(code, 0, "publish con dep por nombre chequea y pasa\n{err}");
    assert!(out.contains("publicado calc 1.0.0"), "{out}");
}

#[test]
fn pre_releases_son_opt_in() {
    let base = tmp("prerel");
    let index = base.join("index");
    let r120 = publicar(&base, "geo", "1.2.0", "pub fn v() -> int { 120 }\n");
    let rrc = publicar(&base, "geo", "1.3.0-rc1", "pub fn v() -> int { 131 }\n");
    indexar(&index, "geo", &[("1.2.0", &r120), ("1.3.0-rc1", &rrc)]);
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
    assert_eq!(code, 0, "la pre-release explícita corre\n{err}");
    assert!(out.contains("131"), "instaló la 1.3.0-rc1 pedida\n{out}");
}

#[test]
fn transitiva_con_indice_propio_avisa() {
    let base = tmp("ownidx");
    let index = base.join("index");
    let util = publicar(&base, "util", "1.0.0", "pub fn u() -> int { 5 }\n");
    indexar(&index, "util", &[("1.0.0", &util)]);
    // `geo` declara su PROPIO índice y una dep por nombre → al consumirla, aviso de confusion.
    let geo = base.join("geo-repo");
    std::fs::create_dir_all(&geo).unwrap();
    git(&geo, &["init", "-q"]);
    std::fs::write(geo.join("mod.ray"), "pub fn v() -> int { 7 }\n").unwrap();
    std::fs::write(
        geo.join("ray.toml"),
        "[package]\nname = \"geo\"\nversion = \"1.0.0\"\n\n[registry]\nindex = \"git+https://otro.ejemplo/indice\"\n\n[dependencies]\nutil = \"1.0.0\"\n",
    )
    .unwrap();
    git(&geo, &["add", "-A"]);
    git(&geo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "geo"]);
    git(&geo, &["tag", "v1.0.0"]);
    indexar(&index, "geo", &[("1.0.0", &geo)]);

    let app = app(&base, "from geo import v;\nfn main() -> int { print(v()); 0 }\n");
    std::fs::write(
        app.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"1.0.0\"\n",
    )
    .unwrap();
    // Corre (util se resuelve contra NUESTRO índice) pero avisa del índice propio de geo.
    let (out, err, code) = ray_idx(&app, &index, &["run"]);
    assert_eq!(code, 0, "corre pese al aviso\n{err}");
    assert!(out.contains("7"), "{out}");
    assert!(err.contains("declara su propio índice"), "aviso de dependency confusion:\n{err}");
}

// ── M51f: ray remove + ray search ──

#[test]
fn ray_remove_elimina_dep_lock_y_cache() {
    let base = tmp("remove");
    let index = base.join("index");
    let geo = publicar(&base, "geo", "1.0.0", "pub fn v() -> int { 9 }\n");
    let util = publicar(&base, "util", "1.0.0", "pub fn u() -> int { 1 }\n");
    indexar(&index, "geo", &[("1.0.0", &geo)]);
    indexar(&index, "util", &[("1.0.0", &util)]);
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
    assert!(out.contains("eliminada"), "{out}");
    let toml = std::fs::read_to_string(app.join("ray.toml")).unwrap();
    assert!(!toml.contains("util"), "fuera del manifiesto:\n{toml}");
    let lock = std::fs::read_to_string(app.join("ray.lock")).unwrap();
    assert!(!lock.contains("[util]") && lock.contains("[geo]"), "lock re-resuelto:\n{lock}");
    assert!(!app.join(".ray-deps/util").exists(), "caché de util borrada");
    // El programa sigue corriendo con la dep restante.
    let (out, _e, code) = ray_idx(&app, &index, &["run"]);
    assert_eq!(code, 0);
    assert!(out.contains("9"), "{out}");

    // remove de algo no declarado → error claro.
    let (_o, err, code) = ray_idx(&app, &index, &["remove", "util"]);
    assert_eq!(code, 65, "remove de una dep inexistente falla");
    assert!(err.contains("no está declarada"), "{err}");
}

#[test]
fn ray_search_lista_el_indice() {
    let base = tmp("search");
    let index = base.join("index");
    let r12 = publicar(&base, "geo", "1.2.0", "pub fn v() -> int { 1 }\n");
    let r13 = publicar(&base, "geo", "1.3.0", "pub fn v() -> int { 2 }\n");
    let net = publicar(&base, "net-extra", "0.1.0", "pub fn n() -> int { 3 }\n");
    indexar(&index, "geo", &[("1.2.0", &r12), ("1.3.0", &r13)]);
    indexar(&index, "net-extra", &[("0.1.0", &net)]);
    let app = app(&base, "fn main() -> int { 0 }\n");

    // Con patrón: solo los que casan, con su última versión instalable.
    let (out, err, code) = ray_idx(&app, &index, &["search", "ge"]);
    assert_eq!(code, 0, "search OK\n{err}");
    assert!(out.contains("geo 1.3.0"), "geo con su última:\n{out}");
    assert!(!out.contains("net-extra"), "net-extra no casa 'ge':\n{out}");

    // Sin patrón: todos, ordenados.
    let (out, _e, code) = ray_idx(&app, &index, &["search"]);
    assert_eq!(code, 0);
    assert!(out.contains("geo 1.3.0") && out.contains("net-extra 0.1.0"), "lista completa:\n{out}");
    assert!(out.contains("2 paquete(s)"), "{out}");

    // Sin resultados → mensaje, código 0 (no es un error).
    let (out, _e, code) = ray_idx(&app, &index, &["search", "zzz"]);
    assert_eq!(code, 0);
    assert!(out.contains("sin resultados"), "{out}");
}
