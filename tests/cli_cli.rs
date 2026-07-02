//! Pruebas del CLI de subcomandos (M39a) sobre el binario: `new`, `run`, `build`, `test`,
//! `help`, `version`, y la compatibilidad con la interfaz legada por flags.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// Ejecuta el binario con `args` y `cwd`, devuelve (stdout, stderr, código).
fn ray(cwd: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(BIN).args(args).current_dir(cwd).output().expect("lanza el binario");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Un directorio temporal único por prueba (evita choques entre tests paralelos).
fn tmp(nombre: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ray_cli_{nombre}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("crea el dir temporal");
    d
}

#[test]
fn new_crea_el_esqueleto_y_run_lo_ejecuta() {
    let base = tmp("new");
    // `ray new proj` crea ray.toml + src/main.ray + .gitignore.
    let (out, _err, code) = ray(&base, &["new", "proj"]);
    assert_eq!(code, 0, "new debe salir 0\n{out}");
    let proj = base.join("proj");
    assert!(proj.join("ray.toml").is_file(), "falta ray.toml");
    assert!(proj.join("src/main.ray").is_file(), "falta src/main.ray");
    assert!(proj.join(".gitignore").is_file(), "falta .gitignore");
    let manifiesto = std::fs::read_to_string(proj.join("ray.toml")).unwrap();
    assert!(manifiesto.contains("name = \"proj\""), "el manifiesto nombra el proyecto\n{manifiesto}");

    // `ray run` sin archivo usa src/main.ray (convención de proyecto).
    let (out, _err, code) = ray(&proj, &["run"]);
    assert!(out.contains("hola desde proj"), "run ejecuta el hola-mundo\n{out}");
    assert_eq!(code, 0);
}

#[test]
fn new_falla_si_el_destino_existe() {
    let base = tmp("new_dup");
    assert_eq!(ray(&base, &["new", "dup"]).2, 0);
    let (_o, err, code) = ray(&base, &["new", "dup"]);
    assert_ne!(code, 0, "no debe sobrescribir un directorio existente");
    assert!(err.contains("ya existe"), "{err}");
}

#[test]
fn run_pasa_los_args_del_programa() {
    let base = tmp("run_args");
    std::fs::write(
        base.join("prog.ray"),
        "fn main() -> int { print(len(args())); 0 }\n",
    )
    .unwrap();
    // Los argumentos tras el archivo llegan a `args()`.
    let (out, _err, _code) = ray(&base, &["run", "prog.ray", "uno", "dos", "tres"]);
    assert!(out.contains("3"), "args() ve los 3 argumentos\n{out}");
}

#[test]
fn build_compila_ok_y_reporta_errores() {
    let base = tmp("build");
    // Programa válido: build sale 0.
    std::fs::write(base.join("ok.ray"), "fn main() -> int { 1 + 2 }\n").unwrap();
    let (out, _err, code) = ray(&base, &["build", "ok.ray"]);
    assert_eq!(code, 0, "build de un programa válido sale 0\n{out}");
    assert!(out.contains("compila"), "{out}");
    // build NO ejecuta: un programa que devolvería 42 igual sale 0 (solo compiló).
    std::fs::write(base.join("cuarenta.ray"), "fn main() -> int { 42 }\n").unwrap();
    assert_eq!(ray(&base, &["build", "cuarenta.ray"]).2, 0, "build no corre el programa");
    // Programa con error de tipos: build sale 65 y no dice 'compila'.
    std::fs::write(base.join("mal.ray"), "fn main() -> int { 1 + true }\n").unwrap();
    let (_o, err, code) = ray(&base, &["build", "mal.ray"]);
    assert_eq!(code, 65, "build de un programa con error sale 65\n{err}");
    assert!(err.contains("error de tipos"), "{err}");
}

#[test]
fn test_subcomando_corre_las_pruebas() {
    let base = tmp("test");
    std::fs::write(
        base.join("suite.ray"),
        "@test\nfn pasa() -> bool { true }\n@test\nfn falla() -> bool { false }\nfn main() -> int { 0 }\n",
    )
    .unwrap();
    let (out, _err, code) = ray(&base, &["test", "suite.ray"]);
    assert!(out.contains("pasa") && out.contains("falla"), "informa ambas pruebas\n{out}");
    assert_eq!(code, 1, "el código de salida es el número de fallos (1)");
    // Filtro por subcadena: solo la que pasa.
    let (out, _err, code) = ray(&base, &["test", "suite.ray", "pasa"]);
    assert!(out.contains("pasa") && !out.contains("FALLO"), "el filtro deja solo 'pasa'\n{out}");
    assert_eq!(code, 0);
}

#[test]
fn help_y_version() {
    let cwd = std::env::temp_dir();
    let (out, _err, code) = ray(&cwd, &["help"]);
    assert_eq!(code, 0);
    assert!(out.contains("Uso: ray") && out.contains("new") && out.contains("build"), "{out}");
    let (out, _err, code) = ray(&cwd, &["version"]);
    assert_eq!(code, 0);
    assert!(out.contains("raylang 1."), "la versión del lenguaje\n{out}");
}

#[test]
fn compat_flags_legadas() {
    let base = tmp("legacy");
    std::fs::write(base.join("p.ray"), "fn main() -> int { 7 }\n").unwrap();
    // La interfaz previa por flags sigue funcionando (un `<archivo>` directo, y --vm).
    assert_eq!(ray(&base, &["p.ray"]).2, 7, "raylang <archivo> directo");
    assert_eq!(ray(&base, &["--vm", "p.ray"]).2, 7, "raylang --vm <archivo>");
}

// ── M39b: el manifiesto ray.toml dirige build/run/test ───────────────────────────────

/// Crea un proyecto con un `ray.toml` a medida y un archivo de entrada.
fn proyecto(nombre: &str, manifiesto: &str, entry_rel: &str, entry_src: &str) -> std::path::PathBuf {
    let raiz = tmp(nombre);
    std::fs::write(raiz.join("ray.toml"), manifiesto).unwrap();
    let entry = raiz.join(entry_rel);
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(entry, entry_src).unwrap();
    raiz
}

#[test]
fn build_y_run_usan_la_entry_del_manifiesto() {
    let raiz = proyecto(
        "manifest_entry",
        "[package]\nname = \"miapp\"\nversion = \"2.0.0\"\nentry = \"src/arranque.ray\"\n",
        "src/arranque.ray",
        "fn main() -> int { print(\"arranque\"); 5 }\n",
    );
    // build: banner con nombre+versión (a stderr) y compila la entry del manifiesto.
    let (out, err, code) = ray(&raiz, &["build"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("compilando miapp v2.0.0"), "banner del proyecto\n{err}");
    assert!(out.contains("arranque.ray") && out.contains("compila"), "{out}");
    // run: ejecuta la entry del manifiesto (sin pasar archivo).
    let (out, _err, code) = ray(&raiz, &["run"]);
    assert!(out.contains("arranque"), "{out}");
    assert_eq!(code, 5, "el exit es el int de main");
}

#[test]
fn run_sube_a_la_raiz_desde_un_subdirectorio() {
    let raiz = proyecto(
        "manifest_subdir",
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n",
        "src/main.ray",
        "fn main() -> int { print(\"raiz\"); 0 }\n",
    );
    // Ejecutar desde src/: el CLI sube buscando ray.toml (como cargo/git).
    let (out, _err, code) = ray(&raiz.join("src"), &["run"]);
    assert!(out.contains("raiz"), "encuentra el proyecto subiendo\n{out}");
    assert_eq!(code, 0);
}

#[test]
fn avisa_de_dependencias_no_resueltas() {
    let raiz = proyecto(
        "manifest_deps",
        "[package]\nname = \"condeps\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"git+https://x/geo@v1\"\n",
        "src/main.ray",
        "fn main() -> int { 0 }\n",
    );
    let (_out, err, code) = ray(&raiz, &["run"]);
    assert_eq!(code, 0);
    assert!(err.contains("dependencia") && err.contains("M39c"), "avisa de deps sin resolver\n{err}");
}

#[test]
fn manifiesto_mal_formado_falla_claro() {
    let raiz = tmp("manifest_bad");
    std::fs::write(raiz.join("ray.toml"), "[package]\nname = sinComillas\n").unwrap();
    std::fs::create_dir_all(raiz.join("src")).unwrap();
    std::fs::write(raiz.join("src/main.ray"), "fn main() -> int { 0 }\n").unwrap();
    let (_out, err, code) = ray(&raiz, &["build"]);
    assert_eq!(code, 65, "un ray.toml mal formado es error de compilación\n{err}");
    assert!(err.contains("ray.toml:2"), "el error trae la línea\n{err}");
}

// ── M39c-1: la caché `.ray-deps/` es raíz de módulos (un paquete = una cápsula) ──────

/// Escribe un paquete `nombre` en la caché `.ray-deps/` del proyecto `raiz`, con su `mod.ray`.
fn dep(raiz: &std::path::Path, nombre: &str, mod_ray: &str) {
    let d = raiz.join(".ray-deps").join(nombre);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("mod.ray"), mod_ray).unwrap();
}

#[test]
fn dependencia_de_ray_deps_es_importable() {
    let raiz = proyecto(
        "dep_import",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"git+https://x/geo@v1\"\n",
        "src/main.ray",
        "from geo import duplicar;\nfn main() -> int { print(duplicar(21)); 0 }\n",
    );
    dep(&raiz, "geo", "pub fn duplicar(x: int) -> int { x * 2 }\n");
    // La dependencia está en la caché → el loader la encuentra y `from geo import` funciona.
    let (out, err, code) = ray(&raiz, &["run"]);
    assert!(out.contains("42"), "usa la función de la dependencia\n{out}\n{err}");
    assert_eq!(code, 0);
    // Y como está presente, NO se avisa de dependencia sin descargar.
    assert!(!err.contains("sin descargar"), "no debe avisar de una dep presente\n{err}");
}

#[test]
fn dependencia_calificada_y_capsula_protege_internos() {
    let raiz = proyecto(
        "dep_capsula",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        "src/main.ray",
        "import geo;\nfn main() -> int { print(geo.triplicar(10)); 0 }\n",
    );
    // El paquete geo es una cápsula que usa su propio submódulo interno.
    dep(
        &raiz,
        "geo",
        "from geo/interno import triple;\npub fn triplicar(x: int) -> int { triple(x) }\n",
    );
    std::fs::write(
        raiz.join(".ray-deps/geo/interno.ray"),
        "pub fn triple(x: int) -> int { x * 3 }\n",
    )
    .unwrap();
    // Acceso calificado a la cara pública del paquete.
    let (out, err, code) = ray(&raiz, &["run"]);
    assert!(out.contains("30"), "geo.triplicar via su interno\n{out}\n{err}");
    assert_eq!(code, 0);

    // La app NO puede alcanzar el submódulo interno del paquete (enforcement de cápsula, M11.6b).
    std::fs::write(
        raiz.join("src/main.ray"),
        "import geo/interno;\nfn main() -> int { print(geo.interno.triple(5)); 0 }\n",
    )
    .unwrap();
    let (_o, err, code) = ray(&raiz, &["run"]);
    assert_eq!(code, 65, "alcanzar el interno de una dependencia es error\n{err}");
    assert!(err.contains("interno a la cápsula 'geo'"), "{err}");
}

#[test]
fn lo_local_tapa_a_la_dependencia_del_mismo_nombre() {
    let raiz = proyecto(
        "dep_shadow",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        "src/main.ray",
        "from util import saludo;\nfn main() -> int { print(saludo()); 0 }\n",
    );
    // Módulo local `util` en el proyecto...
    std::fs::write(raiz.join("src/util.ray"), "pub fn saludo() -> int { 1 }\n").unwrap();
    // ...y una dependencia `util` homónima en la caché.
    dep(&raiz, "util", "pub fn saludo() -> int { 999 }\n");
    // El proyecto se busca antes que la caché: gana el módulo local.
    let (out, err, code) = ray(&raiz, &["run"]);
    assert!(out.contains("1") && !out.contains("999"), "lo local tapa a la dependencia\n{out}\n{err}");
    assert_eq!(code, 0);
}
