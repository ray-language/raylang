//! CLI de raylang — el ejecutable `ray` (M39a), como módulo de la lib para que los dos
//! binarios (`ray` y el alias `raylang`) sean envoltorios de una línea sobre `cli::main`.
//!
//! Interfaz de **subcomandos** (estilo `cargo`/`go`):
//!   `ray new <nombre>`      — crea un proyecto nuevo (ray.toml + src/main.ray).
//!   `ray run [archivo]`     — ejecuta (por defecto `src/main.ray`) en la VM (M35).
//!   `ray build [archivo]`   — chequea y compila sin ejecutar (para CI); 0 ok / 65 error.
//!   `ray test [archivo]`    — corre las funciones `@test` (M10.1).
//!   `ray fmt <archivo>`     — imprime la versión canónica por stdout (M29.2).
//!   `ray lsp`               — arranca el Language Server (M10.2).
//!   `ray repl`              — REPL interactivo (M8.2).
//!   `ray version`          — versión del lenguaje (M34).
//!   `ray help`              — esta ayuda.
//!
//! `run`/`build`/`test` aceptan `--interp` (fuerza el intérprete, oráculo de desarrollo;
//! la VM es el motor de producto, M35) y, tras el archivo, argumentos del programa
//! (`args()`, M11.2b). El binario también se instala como `raylang`.
//!
//! **Compatibilidad**: la interfaz previa por flags (`raylang [--vm|--interp|--test|--fmt]
//! <archivo>`, `--lsp`, `--repl`, `--version`) se mantiene aceptada — un primer argumento
//! que no sea un subcomando conocido cae al modo legado.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use crate::manifest::Manifest;
use crate::runtime::Value;
use crate::{checker, compiler, diagnostic, loader, lsp, repl, runtime, test_runner, vm};

pub fn main() {
    // M13.3a: todo el trabajo corre en un hilo con pila grande, para que la recursión
    // profunda (parser de descenso recursivo, intérprete tree-walking) dé un error
    // limpio (al tope de `MAX_CALL_DEPTH`) en vez de desbordar la pila y morir con
    // SIGSEGV. `run` siempre acaba en `process::exit`, así que el `join` no retorna.
    crate::with_big_stack_or_ice(run);
}

fn run() {
    let args: Vec<String> = env::args().collect();
    let rest = &args[1..]; // sin el nombre del binario

    // Dispatch por subcomando (M39a). Un primer argumento que no case cae al modo legado
    // (interfaz por flags), para no romper scripts ni la suite de tests.
    match rest.first().map(String::as_str) {
        Some("new") => cmd_new(&rest[1..]),
        Some("run") => cmd_run(&rest[1..]),
        Some("build") => cmd_build(&rest[1..]),
        Some("test") => cmd_test_sub(&rest[1..]),
        Some("add") => cmd_add(&rest[1..]),
        Some("publish") => cmd_publish(&rest[1..]),
        Some("update") => cmd_update(&rest[1..]),
        Some("yank") => cmd_yank(&rest[1..]),
        Some("fetch") => cmd_fetch(&rest[1..]),
        Some("fmt") => cmd_fmt(&rest[1..]),
        Some("doc") => cmd_doc(&rest[1..]),
        Some("lsp") => lsp::run(),
        Some("repl") | None => repl::run(),
        Some("version") | Some("--version") | Some("-V") => {
            println!("raylang {}", env!("CARGO_PKG_VERSION"));
        }
        Some("help") | Some("-h") | Some("--help") => print_help(),
        // Modo legado por flags (compat): --lsp, --repl, --fmt, --vm/--interp/--test, o `<archivo>`.
        _ => legacy(rest),
    }
}

fn print_help() {
    print!(
        "\
raylang {v} — lenguaje de programación

Uso: ray <subcomando> [opciones]

  new <nombre>      crea un proyecto nuevo (ray.toml + src/main.ray)
  run [archivo]     ejecuta (por defecto src/main.ray) [--interp] [--deterministic] [--fuel N] [--heap N] [args...]
  build [archivo]   chequea y compila sin ejecutar (0 ok / 65 error)
  test [archivo]    corre las funciones @test [filtro]
  add <nombre>[@req]  añade una dependencia del índice a ray.toml y la descarga
  publish [--repo S]  publica la versión de este paquete en el índice
  update            re-resuelve las dependencias del índice a las más nuevas compatibles
  yank <nom>@<ver>  retira (o --undo restaura) una versión publicada en el índice
  fetch             descarga las dependencias de ray.toml a .ray-deps/
  fmt <archivo>     imprime la versión canónica por stdout
  doc <archivo>     genera la documentación Markdown de su superficie pública
  lsp               arranca el Language Server
  repl              REPL interactivo
  version           versión del lenguaje
  help              esta ayuda
",
        v = env!("CARGO_PKG_VERSION")
    );
}

// ── Subcomandos ──────────────────────────────────────────────────────────────────────

/// `ray new <nombre>`: crea el esqueleto de un proyecto — `ray.toml` (el manifiesto que
/// leerá el gestor de paquetes, M39b) + `src/main.ray` con un hola-mundo + `.gitignore`.
fn cmd_new(args: &[String]) {
    let Some(name) = args.first() else {
        eprintln!("uso: ray new <nombre>");
        process::exit(64);
    };
    let root = Path::new(name);
    if root.exists() {
        eprintln!("'{name}' ya existe");
        process::exit(65);
    }
    let manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[dependencies]\n"
    );
    let main_ray = format!("fn main() -> int {{\n    print(\"hola desde {name}\");\n    0\n}}\n");
    let gitignore = "# dependencias descargadas por el gestor de paquetes (M39c)\n.ray-deps/\n";
    let write_file = |path: std::path::PathBuf, content: &str| {
        if let Some(parent) = path.parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            eprintln!("no se pudo crear '{}': {e}", parent.display());
            process::exit(73); // EX_CANTCREAT
        }
        if let Err(e) = fs::write(&path, content) {
            eprintln!("no se pudo escribir '{}': {e}", path.display());
            process::exit(73);
        }
    };
    write_file(root.join("ray.toml"), &manifest);
    write_file(root.join("src/main.ray"), &main_ray);
    write_file(root.join(".gitignore"), gitignore);
    println!("proyecto '{name}' creado. Para correrlo:\n  cd {name} && ray run");
}

/// `ray run [--interp] [archivo] [args...]`: ejecuta el programa. Sin archivo usa
/// `src/main.ray` (convención de proyecto). Los args tras el archivo van a `args()`.
fn cmd_run(args: &[String]) {
    // M38.4: `--deterministic` fuerza el scheduler M:1 reproducible (un hilo, orden FIFO), aunque el default
    // sea multicore. Útil para salida reproducible; inocuo con `--interp` (ya es secuencial).
    let (deterministic, args) = take_flag_bool(args, "--deterministic");
    if deterministic {
        crate::vm::set_deterministic(true);
    }
    let (use_interp, rest) = take_interp(&args);
    let (fuel, rest) = take_flag_num(&rest, "--fuel", "un número de instrucciones (p. ej. --fuel 1000000)");
    let (heap, rest) = take_flag_num(&rest, "--heap", "un número de objetos (p. ej. --heap 1000000)");
    let (explicit, prog_args) = match rest.split_first() {
        Some((p, rest)) => (Some(p.as_str()), rest.to_vec()),
        None => (None, Vec::new()),
    };
    let path = resolve_entry(explicit, false);
    run_file(&path, prog_args, use_interp, fuel, heap.map(|n| n as usize));
}

/// Separa una opción `--flag <N>` inicial con valor entero. La usan `--fuel` (M42.1, límite de
/// instrucciones) y `--heap` (M42.2, tope de objetos vivos), los dos recursos de la VM para embeber
/// raylang confinado. `<N>` debe ser un entero no negativo; `descripcion` es el texto de ayuda al fallar.
fn take_flag_num(args: &[String], flag: &str, description: &str) -> (Option<u64>, Vec<String>) {
    if let Some((f, rest)) = args.split_first()
        && f == flag
    {
        match rest.split_first() {
            Some((n, tail)) => match n.parse::<u64>() {
                Ok(v) => return (Some(v), tail.to_vec()),
                Err(_) => {
                    eprintln!("{flag} requiere {description}");
                    process::exit(64);
                }
            },
            None => {
                eprintln!("{flag} requiere {description}");
                process::exit(64);
            }
        }
    }
    (None, args.to_vec())
}

/// `ray build [archivo]`: chequea y **compila** el programa sin ejecutarlo (útil para CI y
/// para validar antes de publicar). Sale 0 si compila, 65 si hay errores de compilación.
fn cmd_build(args: &[String]) {
    let path = resolve_entry(args.first().map(String::as_str), true);
    let (mut program, locate, multi) = load_and_locate(&path);
    check_or_exit(&mut program, &locate, multi);
    match compiler::compile_program(&program) {
        Ok(_) => println!("ok: '{path}' compila"),
        Err(mut e) => {
            let (source, name, local) = locate(e.line);
            e.line = local;
            let head = if multi { format!("[{}] {}", name, e) } else { e.to_string() };
            eprintln!("{}", diagnostic::render(&source, local, e.col, 1, &head));
            process::exit(65);
        }
    }
}

/// `ray test [archivo] [filtro]`: corre las funciones `@test`.
fn cmd_test_sub(args: &[String]) {
    let path = resolve_entry(args.first().map(String::as_str), false);
    let filter = args.get(1).map(String::as_str);
    run_tests(&path, filter);
}

/// `ray add <nombre>[@<req>]`: añade una dependencia **del índice** (por nombre) a `ray.toml` y la
/// descarga (M51a). Sin `@<req>`, usa la versión más alta publicada como `^<latest>` (compatible,
/// estilo cargo); con `@<req>`, lo respeta (`1.2.0` exacta, `^1.2`, `~1.2.3`, `*`). Valida que la
/// versión exista en el índice **antes** de tocar el manifiesto (fail-fast ante un typo).
fn cmd_add(args: &[String]) {
    let Some(spec) = args.first().map(String::as_str) else {
        eprintln!("uso: ray add <nombre>[@<versión>]");
        process::exit(64);
    };
    let (name, req_opt) = match spec.split_once('@') {
        Some((n, r)) => (n, Some(r)),
        None => (spec, None),
    };
    if name.is_empty() {
        eprintln!("uso: ray add <nombre>[@<versión>]");
        process::exit(64);
    }
    // M51d: el nombre construye rutas (caché, archivo del índice) — validarlo antes de nada.
    if !crate::deps::valid_package_name(name) {
        eprintln!("nombre de paquete inválido '{name}': solo letras, dígitos, '-' y '_'");
        process::exit(64);
    }
    let Some(m) = load_manifest() else {
        eprintln!("no hay proyecto: falta 'ray.toml' (crea uno con 'ray new')");
        process::exit(64);
    };
    // Localiza el índice (RAY_INDEX o [registry] index).
    let index = match crate::deps::index_dir(&m) {
        Ok(Some(dir)) => dir,
        Ok(None) => {
            eprintln!(
                "no hay índice de paquetes configurado: declara '[registry] index = \"<dir>\"' en \
                 ray.toml o exporta RAY_INDEX (para deps git usa 'nombre = \"git+URL@ref\"' a mano)"
            );
            process::exit(65);
        }
        Err(e) => {
            eprintln!("{e}");
            process::exit(65);
        }
    };
    // Requisito: el dado, o `^<latest>` si no se especifica versión.
    let req = match req_opt {
        Some(r) => r.to_string(),
        None => match crate::index::latest(&index, name) {
            Ok(v) => format!("^{v}"),
            Err(e) => {
                eprintln!("{e}");
                process::exit(65);
            }
        },
    };
    // Fail-fast: valida que la versión exista antes de escribir el manifiesto.
    if let Err(e) = crate::index::resolve(&index, name, &req) {
        eprintln!("{e}");
        process::exit(65);
    }
    // Edición mínima del ray.toml (inserta/reemplaza en [dependencies]).
    let toml_path = m.root.join("ray.toml");
    let src = match fs::read_to_string(&toml_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("no se pudo leer '{}': {e}", toml_path.display());
            process::exit(66);
        }
    };
    let updated = crate::manifest::upsert_dependency(&src, name, &req);
    if let Err(e) = fs::write(&toml_path, &updated) {
        eprintln!("no se pudo escribir '{}': {e}", toml_path.display());
        process::exit(73);
    }
    println!("añadida la dependencia '{name} = \"{req}\"'");
    // Descarga (recarga el manifiesto para que `ensure` vea la nueva dep).
    match crate::manifest::Manifest::load(&m.root) {
        Ok(Some(m2)) => match crate::deps::ensure(&m2) {
            Ok(_) => println!("dependencias al día"),
            Err(e) => {
                eprintln!("error descargando: {e}");
                process::exit(65);
            }
        },
        Ok(None) | Err(_) => {} // el manifiesto acaba de escribirse; improbable
    }
}

/// `ray publish [--repo <git+URL@ref>]`: publica la versión de este paquete en el índice (M51b).
/// Valida (name+version semver) y **añade** la entrada de versión al índice, de forma **inmutable**
/// (no sobrescribe). La spec git de dónde vive el código: `--repo` si se da, o se deriva del remoto
/// `origin` del repo + el tag `v<version>` (que debe existir). **M51d**: la validación (la cara del
/// paquete existe, todos los `.ray` lexean+parsean) y el **hash de contenido** se calculan sobre un
/// **clon limpio de la ref publicada** (el tag), NO sobre el working tree — lo que se avala en el
/// índice es EXACTAMENTE lo que un consumidor descargará (cambios sin commitear no contaminan el
/// hash). El índice se localiza como en `ray add` (`RAY_INDEX`/`[registry] index`). No hace
/// commit/push del índice —eso lo hace el autor.
fn cmd_publish(args: &[String]) {
    let repo_override = match args.split_first() {
        Some((flag, rest)) if flag == "--repo" => match rest.first() {
            Some(spec) => Some(spec.clone()),
            None => {
                eprintln!("--repo requiere una spec 'git+<URL>@<ref>'");
                process::exit(64);
            }
        },
        _ => None,
    };
    let Some(m) = load_manifest() else {
        eprintln!("no hay proyecto: falta 'ray.toml' (crea uno con 'ray new')");
        process::exit(64);
    };
    // Validación: nombre válido (construye rutas en índice/caché, M51d) + version semver.
    if !crate::deps::valid_package_name(&m.name) {
        eprintln!("nombre de paquete inválido '{}': solo letras, dígitos, '-' y '_'", m.name);
        process::exit(65);
    }
    if crate::index::parse_version(&m.version).is_none() {
        eprintln!("la versión del paquete '{}' no es semver válido: '{}'", m.name, m.version);
        process::exit(65);
    }
    // Spec git: la dada, o derivada de `origin` + tag `v<version>`.
    let git_spec = match repo_override {
        Some(s) => s,
        None => match derive_git_spec(&m.root, &m.version) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                process::exit(65);
            }
        },
    };
    // Índice de destino.
    let index = match crate::deps::index_dir(&m) {
        Ok(Some(dir)) => dir,
        Ok(None) => {
            eprintln!(
                "no hay índice configurado: declara '[registry] index = \"<dir>\"' en ray.toml o \
                 exporta RAY_INDEX"
            );
            process::exit(65);
        }
        Err(e) => {
            eprintln!("{e}");
            process::exit(65);
        }
    };
    // M51d: validar y hashear el **contenido de la ref publicada** (clon limpio del repo local en
    // el tag), no el working tree — el hash del índice debe corresponder a lo que el consumidor
    // descargará; cambios sin commitear o archivos sueltos no cuentan.
    let hash = match hash_publicado(&m, &git_spec) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("{e}");
            process::exit(65);
        }
    };
    match crate::index::append_version(&index, &m.name, &m.version, &git_spec, Some(&hash)) {
        Ok(()) => {
            println!("publicado {} {} en el índice", m.name, m.version);
            println!("  git:  {git_spec}");
            println!("  hash: {hash}");
            println!(
                "nota: el índice es un repo git; haz commit y push de '{}.toml' para compartirlo.",
                m.name
            );
        }
        Err(e) => {
            eprintln!("{e}");
            process::exit(65);
        }
    }
}

/// M51d: valida y hashea el **contenido publicado** de un paquete: clona el repo local (`m.root`)
/// haciendo checkout de la ref de `git_spec` en un directorio temporal —exactamente lo que un
/// consumidor obtendrá—, comprueba que la cara del paquete existe (`mod.ray` o la entrada) y que
/// **todos** los `.ray` lexean y parsean, y devuelve su `deps::hash_package`. El clon temporal se
/// borra siempre. (El check semántico completo del grafo queda diferido: exigiría resolver las
/// dependencias del paquete al publicar; DESIGN §54.7.)
fn hash_publicado(m: &crate::manifest::Manifest, git_spec: &str) -> Result<String, String> {
    let spec = crate::deps::parse_spec(git_spec)?;
    let tmp = std::env::temp_dir().join(format!("ray-publish-{}-{}", m.name, process::id()));
    let _ = fs::remove_dir_all(&tmp);
    // Clon del repo LOCAL en la ref publicada (para --repo, la ref debe existir también aquí).
    crate::deps::fetch(&m.name, &crate::deps::GitSpec { url: m.root.to_string_lossy().into_owned(), git_ref: spec.git_ref.clone() }, &tmp)
        .map_err(|e| {
            format!(
                "no se pudo obtener el contenido de la ref '{}' desde el repo local (el contenido \
                 publicado se valida y hashea desde un clon limpio): {e}",
                spec.git_ref
            )
        })?;
    let result = (|| {
        // La cara del paquete debe existir EN EL CLON (no solo en el working tree).
        let face = if tmp.join("mod.ray").is_file() { tmp.join("mod.ray") } else { tmp.join(&m.entry) };
        if !face.is_file() {
            return Err(format!(
                "el contenido de '{}' no tiene cara de paquete: falta 'mod.ray' (o la entrada '{}'); \
                 ¿olvidaste commitearla antes de taggear?",
                spec.git_ref, m.entry
            ));
        }
        // Todos los .ray publicados deben lexear y parsear.
        let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
        crate::deps::collect_files(&tmp, &tmp, &mut files)?;
        for (rel, abs) in files.iter().filter(|(r, _)| r.ends_with(".ray")) {
            let src = fs::read_to_string(abs)
                .map_err(|e| format!("no se pudo leer '{rel}' del contenido publicado: {e}"))?;
            let tokens = crate::lexer::lex(&src)
                .map_err(|e| format!("'{rel}' del contenido publicado no lexea: {e}"))?;
            crate::parser::parse(tokens)
                .map_err(|e| format!("'{rel}' del contenido publicado no parsea: {e}"))?;
        }
        crate::deps::hash_package(&tmp)
    })();
    let _ = fs::remove_dir_all(&tmp);
    result
}

/// Deriva la spec git de un paquete a publicar: `git+<origin>@v<version>`, tomando la URL del remoto
/// `origin` del repo en `root` y exigiendo que el tag `v<version>` exista (se publica un commit fijado).
fn derive_git_spec(root: &Path, version: &str) -> Result<String, String> {
    let origin = git_capture(root, &["remote", "get-url", "origin"]).map_err(|_| {
        "el paquete no tiene remoto 'origin' (publica desde un repo git con remoto, o pasa \
         --repo 'git+<URL>@<ref>')"
            .to_string()
    })?;
    let origin = origin.trim();
    if origin.is_empty() {
        return Err("el remoto 'origin' está vacío; usa --repo 'git+<URL>@<ref>'".to_string());
    }
    let tag = format!("v{version}");
    // El tag debe existir (se publica un punto fijo, no el working tree).
    git_capture(root, &["rev-parse", "--verify", "--quiet", &format!("refs/tags/{tag}")])
        .map_err(|_| format!("no existe el tag '{tag}' en el repo; créalo (git tag {tag}) antes de publicar"))?;
    Ok(format!("git+{origin}@{tag}"))
}

/// Corre `git -C <cwd> <args>` y devuelve su stdout, o `Err` si el estado no es 0.
fn git_capture(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|e| format!("no se pudo ejecutar git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// `ray update`: refresca el índice remoto y **re-resuelve** las dependencias del índice a la versión
/// más alta que satisface su requisito (ignora el lock previo), reescribiendo `ray.lock` (M51c).
fn cmd_update(_args: &[String]) {
    let Some(m) = load_manifest() else {
        eprintln!("no hay proyecto: falta 'ray.toml'");
        process::exit(64);
    };
    if m.dependencies.is_empty() {
        println!("'{}' no declara dependencias", m.name);
        return;
    }
    match crate::deps::update(&m) {
        Ok(_) => println!("dependencias actualizadas a las versiones más nuevas compatibles"),
        Err(e) => {
            eprintln!("error actualizando dependencias: {e}");
            process::exit(65);
        }
    }
}

/// `ray yank <nombre>@<versión> [--undo]`: marca una versión publicada como **retirada** en el índice
/// (o la restaura con `--undo`), M51c. Una versión retirada no se elige en nuevas resoluciones, pero
/// un lock que ya la fijó la sigue usando (no rompe builds existentes). Edita el índice local; el
/// autor hace commit/push del repo del índice.
fn cmd_yank(args: &[String]) {
    let (undo, rest) = take_flag_bool(args, "--undo");
    let Some(spec) = rest.first().map(String::as_str) else {
        eprintln!("uso: ray yank <nombre>@<versión> [--undo]");
        process::exit(64);
    };
    let Some((name, ver)) = spec.split_once('@') else {
        eprintln!("uso: ray yank <nombre>@<versión> (la versión es obligatoria)");
        process::exit(64);
    };
    let Some(m) = load_manifest() else {
        eprintln!("no hay proyecto: falta 'ray.toml' (para localizar el índice)");
        process::exit(64);
    };
    let index = match crate::deps::index_dir(&m) {
        Ok(Some(dir)) => dir,
        Ok(None) => {
            eprintln!("no hay índice configurado ('[registry] index' o RAY_INDEX)");
            process::exit(65);
        }
        Err(e) => {
            eprintln!("{e}");
            process::exit(65);
        }
    };
    match crate::index::set_yanked(&index, name, ver, !undo) {
        Ok(()) => {
            let verb = if undo { "restaurada" } else { "retirada" };
            println!("versión {name} {ver} {verb} en el índice");
            println!("nota: haz commit y push de '{name}.toml' para compartir el cambio.");
        }
        Err(e) => {
            eprintln!("{e}");
            process::exit(65);
        }
    }
}

/// `ray fetch`: descarga a `.ray-deps/` las dependencias declaradas en `ray.toml` que aún no
/// estén presentes (M39c-2a). Requiere estar en un proyecto (con manifiesto).
fn cmd_fetch(_args: &[String]) {
    let Some(m) = load_manifest() else {
        eprintln!("no hay proyecto: falta 'ray.toml' con las dependencias a descargar");
        process::exit(64);
    };
    if m.dependencies.is_empty() {
        println!("'{}' no declara dependencias", m.name);
        return;
    }
    // `asegurar` resuelve el grafo COMPLETO (directas + transitivas) y devuelve cuántas descargó.
    match crate::deps::ensure(&m) {
        Ok(0) => println!("dependencias al día"),
        Ok(n) => println!("{n} dependencia(s) descargada(s) (incluidas transitivas)"),
        Err(e) => {
            eprintln!("error descargando dependencias: {e}");
            process::exit(65);
        }
    }
}

/// `ray fmt <archivo>`: imprime la versión canónica por stdout.
fn cmd_fmt(args: &[String]) {
    let Some(path) = args.first() else {
        eprintln!("uso: ray fmt <archivo>");
        process::exit(64);
    };
    format_file(path);
}

// M40.4: `ray doc <archivo>` imprime la documentación Markdown de la superficie pública del archivo.
fn cmd_doc(args: &[String]) {
    let Some(path) = args.first() else {
        eprintln!("uso: ray doc <archivo>");
        process::exit(64);
    };
    let title = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);
    match crate::raydoc::generate(&read_source(path), title) {
        Ok(md) => print!("{md}"),
        Err(e) => {
            eprintln!("error de documentación: {e}");
            process::exit(65);
        }
    }
}

// ── Modo legado (compatibilidad con la interfaz por flags) ───────────────────────────

fn legacy(rest: &[String]) {
    // M38.4: `--deterministic` (order-independent) fuerza el scheduler M:1 reproducible. Se extrae antes de
    // todo el parseo por-posición del modo legado.
    let (deterministic, rest) = take_flag_bool(rest, "--deterministic");
    if deterministic {
        crate::vm::set_deterministic(true);
    }
    let rest = &rest[..];
    // --lsp / --repl sin archivo.
    if rest.len() == 1 && rest[0] == "--lsp" {
        lsp::run();
        return;
    }
    if rest.len() == 1 && rest[0] == "--repl" {
        repl::run();
        return;
    }
    // --fmt <archivo>.
    if rest.len() == 2 && rest[0] == "--fmt" {
        format_file(&rest[1]);
        return;
    }
    // [--vm | --interp | --test] <archivo> [args...].
    let mut idx = 0;
    let (mut use_interp, mut test_mode) = (false, false);
    match rest.first().map(String::as_str) {
        Some("--interp") => {
            use_interp = true;
            idx = 1;
        }
        Some("--vm") => idx = 1, // ya es el default; se acepta por compatibilidad
        Some("--test") => {
            test_mode = true;
            idx = 1;
        }
        _ => {}
    }
    if idx >= rest.len() {
        eprintln!("uso: ray <subcomando>   (ray help para la lista)   |   ray run <archivo>");
        process::exit(64); // EX_USAGE
    }
    let path = rest[idx].clone();
    if test_mode {
        run_tests(&path, rest.get(idx + 1).map(String::as_str));
    } else {
        run_file(&path, rest[idx + 1..].to_vec(), use_interp, None, None);
    }
}

// ── Piezas compartidas ───────────────────────────────────────────────────────────────

/// Resuelve el archivo a procesar (run/build/test) y el contexto de proyecto (M39b).
/// `explicit`: el archivo dado en la línea de comandos, si lo hay. `banner`: imprime
/// "compilando <nombre> v<versión>" (para `build`). Prioridad: (1) el archivo explícito;
/// (2) la entrada del manifiesto (`ray.toml` subiendo desde el cwd); (3) `src/main.ray` en
/// el cwd; si nada, error de uso. Avisa —una vez— si el manifiesto declara dependencias
/// (aún no se resuelven, M39c).
fn resolve_entry(explicit: Option<&str>, banner: bool) -> String {
    let manifest = load_manifest();
    if let Some(m) = &manifest {
        if banner {
            eprintln!("compilando {} v{}", m.name, m.version);
        }
        // Auto-descarga (M39c-2a, estilo cargo): asegura que las dependencias declaradas estén en
        // `.ray-deps/` antes de cargar el programa. Las presentes se saltan (sin red); si falta
        // alguna se clona de git. Un fallo de descarga aborta con 65 (no se puede compilar sin ella).
        if !m.dependencies.is_empty()
            && let Err(e) = crate::deps::ensure(m)
        {
            eprintln!("error resolviendo dependencias: {e}");
            process::exit(65);
        }
    }
    if let Some(p) = explicit {
        return p.to_string();
    }
    if let Some(m) = &manifest {
        let entry = m.entry_path();
        if !entry.is_file() {
            eprintln!("el manifiesto '{}' apunta a una entrada inexistente: '{}'", m.name, entry.display());
            process::exit(66);
        }
        return entry.to_string_lossy().into_owned();
    }
    let def = "src/main.ray";
    if Path::new(def).exists() {
        def.to_string()
    } else {
        eprintln!("no se indicó archivo y no hay proyecto (falta 'ray.toml' o 'src/main.ray')");
        process::exit(64);
    }
}

/// Las raíces de dependencias para el loader (M39c): la caché `.ray-deps/` en la raíz del
/// proyecto (junto al `ray.toml`), si existe. Un paquete descargado vive en `.ray-deps/<dep>/`
/// como cápsula; el loader busca ahí tras la raíz del proyecto. Sin proyecto ni caché, `[]`
/// (comportamiento idéntico a antes: el loader resuelve solo contra la raíz del archivo).
fn dependency_roots() -> Vec<PathBuf> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = Manifest::find(&cwd)
        .and_then(|toml| toml.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| cwd.clone());
    let cache = root.join(".ray-deps");
    let mut roots = Vec::new();
    if cache.is_dir() {
        roots.push(cache);
    }
    // M40.8a: dependencias por **ruta local** (`nombre = "path:<dir>"`). No se descargan; la carpeta del
    // paquete se registra como raíz de módulos, así `import <nombre>;` resuelve `<dir>/mod.ray` (cápsula)
    // como cualquier dependencia. Se añade el **padre** de `<dir>` (el loader busca `<raíz>/<nombre>/…`).
    if let Ok(Some(m)) = Manifest::load(&cwd) {
        for (_name, spec) in &m.dependencies {
            if let Some(p) = crate::deps::path_of_path_dep(spec) {
                let dir = m.root.join(p);
                if let Some(parent) = dir.parent().map(Path::to_path_buf)
                    && dir.exists()
                    && !roots.contains(&parent)
                {
                    roots.push(parent);
                }
            }
        }
    }
    // La biblioteca estándar (`import std/…`) ya no es una raíz de disco: desde M40.5 va **embebida**
    // en el binario (`crate::stdlib`, auto-contención), y el loader la resuelve antes que el filesystem.
    roots
}

/// Carga el manifiesto del proyecto que contiene el directorio actual. `None` si no hay
/// proyecto; un `ray.toml` mal formado aborta con 65 (error de compilación de la config).
fn load_manifest() -> Option<Manifest> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match Manifest::load(&cwd) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            process::exit(65);
        }
    }
}

/// Separa un `--interp` inicial del resto de argumentos.
fn take_interp(args: &[String]) -> (bool, Vec<String>) {
    match args.split_first() {
        Some((f, rest)) if f == "--interp" => (true, rest.to_vec()),
        _ => (false, args.to_vec()),
    }
}

/// M38.4: extrae un flag booleano sin valor (p. ej. `--deterministic`) de CUALQUIER posición de la lista y
/// devuelve `(presente, resto_sin_el_flag)`. Order-independent (a diferencia de `take_interp`), para que
/// `--deterministic` pueda combinarse libremente con `--interp`/`--fuel`/el archivo.
fn take_flag_bool(args: &[String], flag: &str) -> (bool, Vec<String>) {
    let present = args.iter().any(|a| a == flag);
    let rest = args.iter().filter(|a| a.as_str() != flag).cloned().collect();
    (present, rest)
}

/// Lee el fuente de un archivo o aborta con el código de E/S adecuado.
fn read_source(path: &str) -> String {
    match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("no se pudo leer '{}': {}", path, e);
            process::exit(66); // EX_NOINPUT
        }
    }
}

/// Runner de `@test` (M10.1): sale con el número de fallos como código.
fn run_tests(path: &str, filter: Option<&str>) {
    process::exit(test_runner::run(&read_source(path), filter));
}

/// Formateador (M29.2): imprime la versión canónica o aborta con el error.
fn format_file(path: &str) {
    let unit = resolve_indent(std::path::Path::new(path));
    match crate::fmt::format_source_with_indent(&read_source(path), &unit) {
        Ok(out) => print!("{}", out),
        Err(e) => {
            eprintln!("error de formato: {}", e);
            process::exit(65);
        }
    }
}

/// La **unidad de indentación** para formatear `file`, resolviendo la config del proyecto (para que
/// `ray fmt` no imponga siempre 4 espacios). Precedencia: (1) `.editorconfig` más cercano
/// (`indent_style`/`indent_size`), (2) `ray.toml [fmt]`, (3) canónico = 4 espacios. `.editorconfig`
/// gana por ser el estándar dedicado; cada fuente rellena solo lo que la anterior no fijó.
fn resolve_indent(file: &std::path::Path) -> String {
    let (mut style, mut size) = crate::editorconfig::indent_for(file);
    if style.is_none() || size.is_none() {
        let dir = file.parent().unwrap_or(std::path::Path::new("."));
        if let Ok(Some(m)) = crate::manifest::Manifest::load(dir) {
            style = style.or(m.indent_style);
            size = size.or(m.indent_size);
        }
    }
    match style.as_deref() {
        Some("tab") => "\t".to_string(),
        _ => " ".repeat(size.unwrap_or(4).max(1)), // "space" o sin declarar → N espacios (def. 4)
    }
}

/// Un localizador de líneas globales→(fuente del módulo, nombre, línea local), para
/// renderizar errores contra el archivo correcto en programas multi-módulo (L3).
type Locate = Box<dyn Fn(usize) -> (String, String, usize)>;

/// Carga el archivo de entrada y sus imports (loader, M11.3), devolviendo el programa
/// fusionado, un localizador de líneas y si hay más de un módulo.
fn load_and_locate(path: &str) -> (crate::ast::Program, Locate, bool) {
    let loaded = match loader::load_with_deps(Path::new(path), &dependency_roots()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}", e.message);
            process::exit(65);
        }
    };
    let modules = loaded.modules;
    let multi = modules.len() > 1;
    let locate: Locate = Box::new(move |gline: usize| {
        let m = modules.iter().rev().find(|m| m.start_line <= gline).unwrap_or(&modules[0]);
        // `saturating_sub`: una posición fallback `(0,0)` (p.ej. un error de runtime sin línea concreta,
        // como el deadlock declarado por un worker ocioso en M:N, o el fallback de fuel) da `gline < start_line`
        // → sin esto, restar underflowaría (usize). Para posiciones válidas (`gline >= start_line`) es idéntico.
        (m.source.clone(), m.name.clone(), gline.saturating_sub(m.start_line) + 1)
    });
    (loaded.program, locate, multi)
}

/// Chequea el programa; si falla, re-corre la variante acumuladora y muestra TODOS los
/// errores (M33c) contra su módulo, y sale con 65.
fn check_or_exit(program: &mut crate::ast::Program, locate: &Locate, multi: bool) {
    let backup = program.clone();
    if checker::check(program).is_err() {
        let mut copy = backup;
        for mut e in checker::check_all(&mut copy) {
            let (source, name, local) = locate(e.line);
            e.line = local;
            let head = if multi { format!("[{}] {}", name, e) } else { e.to_string() };
            eprintln!("{}", diagnostic::render(&source, local, e.col, e.len, &head));
        }
        process::exit(65);
    }
}

/// Carga, chequea y ejecuta un archivo (VM por defecto, `--interp` para el intérprete).
fn run_file(path: &str, prog_args: Vec<String>, use_interp: bool, fuel: Option<u64>, heap: Option<usize>) {
    if (fuel.is_some() || heap.is_some()) && use_interp {
        eprintln!("--fuel/--heap son límites de la VM (motor de producto); no se aplican con --interp");
        process::exit(64);
    }
    runtime::set_program_args(prog_args);
    let (mut program, locate, multi) = load_and_locate(path);
    check_or_exit(&mut program, &locate, multi);

    // Backend: VM por defecto (M35, el motor de producto), intérprete con `--interp`.
    // M35b: el intérprete solo está si la feature `interp` está activa; una release mínima
    // (`--no-default-features`) no lo trae y `--interp` avisa con claridad.
    let result = if use_interp {
        #[cfg(feature = "interp")]
        {
            crate::interpreter::run(&program)
        }
        #[cfg(not(feature = "interp"))]
        {
            let _ = &program;
            eprintln!("esta build no incluye el intérprete (compilada con --no-default-features); ejecuta en la VM, sin --interp");
            process::exit(64);
        }
    } else {
        match compiler::compile_program(&program) {
            Ok(compiled) => vm::run_program_with_limit(&compiled, fuel, heap),
            Err(mut e) => {
                let (source, name, local) = locate(e.line);
                e.line = local;
                let head = if multi { format!("[{}] {}", name, e) } else { e.to_string() };
                eprintln!("{}", diagnostic::render(&source, local, e.col, 1, &head));
                process::exit(65);
            }
        }
    };

    match result {
        Ok(Value::Int(code)) => process::exit((code & 0xFF) as i32),
        Ok(_) => process::exit(0),
        Err(mut e) => {
            let (source, name, local) = locate(e.line);
            e.line = local;
            let head = if multi { format!("[{}] {}", name, e) } else { e.to_string() };
            eprintln!("{}", diagnostic::render(&source, local, e.col, 1, &head));
            process::exit(70); // EX_SOFTWARE
        }
    }
}
