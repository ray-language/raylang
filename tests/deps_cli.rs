//! Pruebas del gestor de paquetes (M39c-2a): `ray fetch` y la auto-descarga en `run`/`build`
//! clonan las dependencias `git+<URL>@<ref>` a `.ray-deps/`. Todo **offline y determinista**:
//! la dependencia es un repositorio git local (`git init` + tag) servido por `file://`.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// Ejecuta el binario con `args` y `cwd`, devuelve (stdout, stderr, código).
fn ray(cwd: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(BIN).args(args).current_dir(cwd).output().expect("lanza el binario");
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
fn tmp(nombre: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ray_deps_{nombre}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("crea el dir temporal");
    d
}

/// Crea un "paquete publicado": un repo git en `<base>/<nombre>-repo` con `mod.ray` en su raíz
/// (la cápsula del paquete) y un tag `v1.0`. Devuelve su ruta absoluta.
fn publicar(base: &Path, nombre: &str, mod_ray: &str) -> std::path::PathBuf {
    let repo = base.join(format!("{nombre}-repo"));
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
    let manifiesto = format!(
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n{dep} = \"git+file://{}@v1.0\"\n",
        dep_repo.display()
    );
    std::fs::write(app.join("ray.toml"), manifiesto).unwrap();
    std::fs::write(app.join("src/main.ray"), main_ray).unwrap();
    app
}

#[test]
fn fetch_clona_la_dependencia_y_run_la_usa() {
    let base = tmp("fetch");
    let repo = publicar(&base, "geo", "pub fn duplicar(x: int) -> int { x * 2 }\n");
    let app = app_con_dep(
        &base,
        "geo",
        &repo,
        "from geo import duplicar;\nfn main() -> int { print(duplicar(21)); 0 }\n",
    );

    // `ray fetch` clona el repo al tag v1.0 dentro de .ray-deps/geo/.
    let (out, err, code) = ray(&app, &["fetch"]);
    assert_eq!(code, 0, "fetch OK\n{err}");
    assert!(out.contains("descargada") || out.contains("al día"), "{out}");
    assert!(app.join(".ray-deps/geo/mod.ray").is_file(), "la cápsula del paquete quedó en la caché");

    // `ray run` la usa (sin re-clonar, ya está cacheada).
    let (out, err, code) = ray(&app, &["run"]);
    assert!(out.contains("42"), "run usa la dependencia descargada\n{out}\n{err}");
    assert_eq!(code, 0);

    // fetch de nuevo: ya está al día (idempotente).
    let (out, _e, _c) = ray(&app, &["fetch"]);
    assert!(out.contains("al día"), "segundo fetch no re-descarga\n{out}");
}

#[test]
fn run_auto_descarga_las_dependencias_faltantes() {
    let base = tmp("autofetch");
    let repo = publicar(&base, "geo", "pub fn triple(x: int) -> int { x * 3 }\n");
    let app = app_con_dep(
        &base,
        "geo",
        &repo,
        "from geo import triple;\nfn main() -> int { print(triple(14)); 0 }\n",
    );
    // Sin `fetch` previo: `run` descarga lo que falta (estilo cargo) y luego ejecuta.
    let (out, err, code) = ray(&app, &["run"]);
    assert!(err.contains("descargando geo"), "avisa de la descarga automática\n{err}");
    assert!(out.contains("42"), "y ejecuta con la dependencia\n{out}");
    assert_eq!(code, 0);
}

#[test]
fn ref_inexistente_falla_y_deja_la_cache_limpia() {
    let base = tmp("badref");
    let repo = publicar(&base, "geo", "pub fn f() -> int { 1 }\n");
    let app = base.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    // Pide un tag que no existe.
    let manifiesto = format!(
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"git+file://{}@v9.9\"\n",
        repo.display()
    );
    std::fs::write(app.join("ray.toml"), manifiesto).unwrap();
    std::fs::write(app.join("src/main.ray"), "fn main() -> int { 0 }\n").unwrap();

    let (_o, err, code) = ray(&app, &["fetch"]);
    assert_eq!(code, 65, "una ref inexistente aborta\n{err}");
    assert!(err.contains("checkout") && err.contains("v9.9"), "{err}");
    // El clon a medio hacer no se queda en la caché.
    assert!(!app.join(".ray-deps/geo").exists(), "la caché queda limpia tras el fallo");
}
