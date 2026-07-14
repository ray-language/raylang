//! Pruebas del gestor de paquetes (M39c-2a): `ray fetch` y la auto-descarga en `run`/`build`
//! clonan las dependencias `git+<URL>@<ref>` a `.ray-deps/`. Todo **offline y determinista**:
//! la dependencia es un repositorio git local (`git init` + tag) servido por `file://`.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// Ejecuta el binario con `args` y `cwd`, devuelve (stdout, stderr, código).
fn ray(cwd: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(BIN).args(args).current_dir(cwd).output().expect("lanza el binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Corre un comando `git` en `cwd`; entra en pánico con el stderr si falla.
fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(cwd).output().expect("git disponible");
    assert!(out.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
}

/// Un directorio temporal único por prueba.
fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ray_deps_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("crea el dir temporal");
    d
}

/// Crea un "paquete publicado": un repo git en `<base>/<nombre>-repo` con `mod.ray` en su raíz
/// (la cápsula del paquete) y un tag `v1.0`. Devuelve su ruta absoluta.
fn publish(base: &Path, name: &str, mod_ray: &str) -> std::path::PathBuf {
    let repo = base.join(format!("{name}-repo"));
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("mod.ray"), mod_ray).unwrap();
    git(&repo, &["add", "-A"]);
    // Identidad fija por comando: no depende de la config global del entorno (CI).
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "v1"]);
    git(&repo, &["tag", "v1.0"]);
    repo
}

/// Crea un proyecto que depende de `dep_repo` por `git+file://…@v1.0`.
fn app_con_dep(base: &Path, dep: &str, dep_repo: &Path, main_ray: &str) -> std::path::PathBuf {
    let app = base.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    let manifest = format!(
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n{dep} = \"git+file://{}@v1.0\"\n",
        dep_repo.display()
    );
    std::fs::write(app.join("ray.toml"), manifest).unwrap();
    std::fs::write(app.join("src/main.ray"), main_ray).unwrap();
    app
}

#[test]
fn fetch_clona_la_dependency_y_run_la_uses() {
    let base = tmp("fetch");
    let repo = publish(&base, "geo", "pub fn duplicate(x: int) -> int { x * 2 }\n");
    let app = app_con_dep(
        &base,
        "geo",
        &repo,
        "from geo import duplicate;\nfn main() -> int { print(duplicate(21)); 0 }\n",
    );

    // `ray fetch` clona el repo al tag v1.0 dentro de .ray-deps/geo/.
    let (out, err, code) = ray(&app, &["fetch"]);
    assert_eq!(code, 0, "fetch OK\n{err}");
    assert!(out.contains("downloaded") || out.contains("up to date"), "{out}");
    assert!(app.join(".ray-deps/geo/mod.ray").is_file(), "la cápsula del package quedó en la caché");

    // `ray run` la usa (sin re-clonar, ya está cacheada).
    let (out, err, code) = ray(&app, &["run"]);
    assert!(out.contains("42"), "run uses la dependency descargada\n{out}\n{err}");
    assert_eq!(code, 0);

    // fetch de nuevo: ya está al día (idempotente).
    let (out, _e, _c) = ray(&app, &["fetch"]);
    assert!(out.contains("up to date"), "segundo fetch no re-descarga\n{out}");
}

#[test]
fn run_auto_descarga_las_dependencies_faltantes() {
    let base = tmp("autofetch");
    let repo = publish(&base, "geo", "pub fn triple(x: int) -> int { x * 3 }\n");
    let app = app_con_dep(
        &base,
        "geo",
        &repo,
        "from geo import triple;\nfn main() -> int { print(triple(14)); 0 }\n",
    );
    // Sin `fetch` previo: `run` descarga lo que falta (estilo cargo) y luego ejecuta.
    let (out, err, code) = ray(&app, &["run"]);
    assert!(err.contains("downloading geo"), "avisa de la descarga automática\n{err}");
    assert!(out.contains("42"), "y ejecuta con la dependency\n{out}");
    assert_eq!(code, 0);
}

#[test]
fn ref_nonexistent_fails_y_deja_la_cache_limpia() {
    let base = tmp("badref");
    let repo = publish(&base, "geo", "pub fn f() -> int { 1 }\n");
    let app = base.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    // Pide un tag que no existe.
    let manifest = format!(
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"git+file://{}@v9.9\"\n",
        repo.display()
    );
    std::fs::write(app.join("ray.toml"), manifest).unwrap();
    std::fs::write(app.join("src/main.ray"), "fn main() -> int { 0 }\n").unwrap();

    let (_o, err, code) = ray(&app, &["fetch"]);
    assert_eq!(code, 65, "one ref nonexistent abort\n{err}");
    assert!(err.contains("check out") && err.contains("v9.9"), "{err}");
    // El clon a medio hacer no se queda en la caché.
    assert!(!app.join(".ray-deps/geo").exists(), "la caché queda limpia after el failure");
}

// ── M39c-2b: lockfile `ray.lock` con hashes + verificación de integridad ──────────────

#[test]
fn fetch_genera_y_verifies_el_lockfile() {
    let base = tmp("lockfile");
    let repo = publish(&base, "geo", "pub fn duplicate(x: int) -> int { x * 2 }\n");
    let app = app_con_dep(
        &base,
        "geo",
        &repo,
        "from geo import duplicate;\nfn main() -> int { duplicate(21) }\n",
    );
    // fetch genera ray.lock con la url/ref/commit y el hash de contenido.
    let (_o, err, code) = ray(&app, &["fetch"]);
    assert_eq!(code, 0, "{err}");
    let lock = std::fs::read_to_string(app.join("ray.lock")).expect("ray.lock existe");
    assert!(lock.contains("[geo]"), "{lock}");
    assert!(lock.contains("hash = \"sha256:"), "el lock trae el hash de contenido\n{lock}");
    assert!(lock.contains("commit = \""), "el lock trae el commit resuelto\n{lock}");
    // run vuelve a verificar contra el lock, sin error (exit = 42, el valor de `main`, no el 65
    // del error de supply-chain).
    assert_eq!(ray(&app, &["run"]).2, 42, "la verificación pasa about la caché intacta");
}

#[test]
fn la_verificacion_detecta_manipulacion() {
    let base = tmp("tamper");
    let repo = publish(&base, "geo", "pub fn duplicate(x: int) -> int { x * 2 }\n");
    let app = app_con_dep(
        &base,
        "geo",
        &repo,
        "from geo import duplicate;\nfn main() -> int { duplicate(21) }\n",
    );
    assert_eq!(ray(&app, &["fetch"]).2, 0);
    // Manipular un archivo de la dependencia cacheada.
    let modray = app.join(".ray-deps/geo/mod.ray");
    let mut contenido = std::fs::read_to_string(&modray).unwrap();
    contenido.push_str("\n// inyectado\n");
    std::fs::write(&modray, contenido).unwrap();
    // La próxima resolución detecta que el hash no coincide con el lock → aborta (supply-chain).
    let (_o, err, code) = ray(&app, &["run"]);
    assert_eq!(code, 65, "one dependency manipulada abort\n{err}");
    assert!(err.contains("ray.lock") && err.contains("content changed"), "{err}");
}

// ── M39c-3: dependencias transitivas ──────────────────────────────────────────────────

/// Publica un paquete con su propio `ray.toml` que declara dependencias (para las transitivas).
/// `deps` son pares `(nombre, spec)`.
fn publish_con_deps(base: &Path, name: &str, mod_ray: &str, deps: &[(&str, &Path)]) -> std::path::PathBuf {
    let repo = base.join(format!("{name}-repo"));
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("mod.ray"), mod_ray).unwrap();
    let mut toml = format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n\n[dependencies]\n");
    for (dn, drepo) in deps {
        toml.push_str(&format!("{dn} = \"git+file://{}@v1.0\"\n", drepo.display()));
    }
    std::fs::write(repo.join("ray.toml"), toml).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "v1"]);
    git(&repo, &["tag", "v1.0"]);
    repo
}

#[test]
fn resolves_dependencies_transitivas() {
    let base = tmp("transitivas");
    // mathx (hoja) ← geo (depende de mathx) ← app.
    let mathx = publish(&base, "mathx", "pub fn add10(x: int) -> int { x + 10 }\n");
    let geo = publish_con_deps(
        &base,
        "geo",
        "from mathx import add10;\npub fn calc(x: int) -> int { add10(x) * 2 }\n",
        &[("mathx", &mathx)],
    );
    let app = app_con_dep(
        &base,
        "geo",
        &geo,
        "from geo import calc;\nfn main() -> int { print(calc(5)); 0 }\n",
    );

    // fetch trae geo Y su transitiva mathx.
    let (out, err, code) = ray(&app, &["fetch"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("downloaded"), "{out}");
    assert!(app.join(".ray-deps/geo/mod.ray").is_file(), "geo en la caché");
    assert!(app.join(".ray-deps/mathx/mod.ray").is_file(), "la transitiva mathx también");

    // run: geo usa mathx → calc(5) = (5+10)*2 = 30.
    let (out, err, _code) = ray(&app, &["run"]);
    assert!(out.contains("30"), "uses la string app→geo→mathx\n{out}\n{err}");

    // El lock incluye AMBOS paquetes.
    let lock = std::fs::read_to_string(app.join("ray.lock")).unwrap();
    assert!(lock.contains("[geo]") && lock.contains("[mathx]"), "{lock}");
}
