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
        Some("dev") => cmd_dev(&rest[1..]),
        Some("build") => cmd_build(&rest[1..]),
        Some("test") => cmd_test_sub(&rest[1..]),
        Some("add") => cmd_add(&rest[1..]),
        Some("remove") => cmd_remove(&rest[1..]),
        Some("search") => cmd_search(&rest[1..]),
        Some("publish") => cmd_publish(&rest[1..]),
        Some("keygen") => cmd_keygen(&rest[1..]),
        Some("index-verify") => cmd_index_verify(&rest[1..]),
        Some("update") => cmd_update(&rest[1..]),
        Some("yank") => cmd_yank(&rest[1..]),
        Some("fetch") => cmd_fetch(&rest[1..]),
        Some("fmt") => cmd_fmt(&rest[1..]),
        Some("templ") => cmd_templ(&rest[1..]),
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
  dev [archivo]     como run, pero REINICIA ante cambios en .ray/.ray.html/ray.toml (modo desarrollo)
  build [archivo]   chequea y compila sin ejecutar (0 ok / 65 error)
  test [archivo]    corre las funciones @test [filtro]
  add <nombre>[@req]  añade una dependencia del índice a ray.toml y la descarga
  remove <nombre>   elimina una dependencia de ray.toml (y su caché si nadie más la usa)
  search [patrón]   lista los paquetes del índice (que contengan el patrón)
  publish [--repo S] [--sign]  publica la versión de este paquete en el índice (--sign la firma)
  keygen [--out F]  genera la clave Ed25519 de publicación (RAY_KEY o ~/.ray/publish.key)
  index-verify [dir]  audita las firmas de un índice (para el CI del repo del índice)
  update            re-resuelve las dependencias del índice a las más nuevas compatibles
  yank <nom>@<ver>  retira (o --undo restaura) una versión publicada en el índice
  fetch             descarga las dependencias de ray.toml a .ray-deps/
  fmt <archivo>     imprime la versión canónica por stdout
  templ <ruta>...   compila templates .ray.html a módulos raylang tipados
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
    regen_stale_templates(Path::new(&path)); // M55: los .ray.html desactualizados, al día
    run_file(&path, prog_args, use_interp, fuel, heap.map(|n| n as usize));
}

// ── `ray dev` (M92.1): modo desarrollo — watcher + reinicio con drenado ─────────────────────

/// `ray dev [archivo] [flags de run] [args...]`: corre el programa como `ray run` y lo REINICIA
/// ante cambios en los fuentes del proyecto (`.ray`, `.ray.html`, `ray.toml`). El watcher es
/// POLLING de mtimes (~200 ms): portable y cero deps — el mismo mecanismo que la regeneración de
/// templates, que el hijo ejecuta al arrancar (un `.ray.html` editado dispara reinicio → regen).
/// El reinicio manda **SIGTERM** — un servidor con `serve_graceful` (M88.1b) drena sus conexiones
/// antes de morir — y escala a SIGKILL a los 3 s. Un programa que termina solo (un CLI, un crash)
/// queda a la espera y se relanza al siguiente cambio.
fn cmd_dev(args: &[String]) {
    let exe = env::current_exe().unwrap_or_else(|_| PathBuf::from("ray"));
    // La raíz vigilada: la del proyecto (manifiesto hacia arriba desde el cwd); sin manifiesto,
    // el directorio de la entrada explícita, o el cwd.
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = Manifest::find(&cwd)
        .and_then(|toml| toml.parent().map(Path::to_path_buf))
        .or_else(|| {
            args.iter()
                .find(|a| !a.starts_with("--"))
                .and_then(|a| Path::new(a).parent().map(Path::to_path_buf))
                .filter(|p| p.as_os_str().len() > 0)
        })
        .unwrap_or(cwd);
    eprintln!("[dev] vigilando {} (.ray, .ray.html, ray.toml); Ctrl-C para salir", root.display());
    install_cleanup_on_death();

    let mut snapshot = scan_sources(&root);
    loop {
        // Lanza el programa como `ray run <args...>` (mismo binario): hereda la resolución de
        // entrada, la regeneración de templates y los flags (--interp/--fuel/…).
        let mut child = match process::Command::new(&exe).arg("run").args(args).spawn() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[dev] no se pudo lanzar el programa: {e}");
                process::exit(70);
            }
        };
        DEV_CHILD.store(child.id() as i32, std::sync::atomic::Ordering::SeqCst);
        // Vigila hasta el próximo cambio; si el programa termina solo, sigue vigilando sin él.
        let mut running = true;
        let change = loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let actual = scan_sources(&root);
            if let Some(c) = first_change(&snapshot, &actual) {
                snapshot = actual;
                break c;
            }
            if running && let Ok(Some(status)) = child.try_wait() {
                running = false;
                eprintln!("[dev] el programa terminó ({status}); esperando cambios…");
            }
        };
        eprintln!("[dev] cambio en {change}: reiniciando…");
        if running {
            terminate_gracefully(&mut child);
        }
    }
}

/// Los fuentes vigilados bajo `root`: `(ruta, mtime)` de cada `.ray`/`.ray.html`/`ray.toml`,
/// ordenados (comparable como snapshot). Salta las carpetas de artefactos y las ocultas.
fn scan_sources(root: &Path) -> Vec<(PathBuf, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                // `.ray-deps` (caché), `.git`, `target`, `node_modules` y ocultas: fuera.
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                pending.push(path);
            } else if name.ends_with(".ray") || name.ends_with(".ray.html") || name == "ray.toml" {
                // Un `.ray` con un `.ray.html` hermano es el módulo GENERADO por `ray templ`
                // (derivado): se vigila el fuente (el .html), no el artefacto — si no, cada
                // edición de template causaría un segundo reinicio al regenerarlo el hijo.
                if name.ends_with(".ray") && !name.ends_with(".ray.html") {
                    let html = path.with_extension("ray.html");
                    if html.exists() {
                        continue;
                    }
                }
                if let Ok(meta) = entry.metadata()
                    && let Ok(mtime) = meta.modified()
                {
                    out.push((path, mtime));
                }
            }
        }
    }
    out.sort();
    out
}

/// El primer archivo que difiere entre dos snapshots (nuevo, borrado o con otro mtime), para el
/// mensaje de reinicio. `None` si son idénticos.
fn first_change(
    antes: &[(PathBuf, std::time::SystemTime)],
    ahora: &[(PathBuf, std::time::SystemTime)],
) -> Option<String> {
    if antes == ahora {
        return None;
    }
    let viejos: std::collections::HashMap<_, _> = antes.iter().cloned().collect();
    for (p, m) in ahora {
        if viejos.get(p) != Some(m) {
            return Some(p.display().to_string());
        }
    }
    // Nada nuevo ni tocado pero difieren → algo se borró.
    let nuevos: std::collections::HashMap<_, _> = ahora.iter().cloned().collect();
    antes
        .iter()
        .find(|(p, _)| !nuevos.contains_key(p))
        .map(|(p, _)| format!("{} (borrado)", p.display()))
}

/// El pid del hijo en curso de `ray dev` (0 = ninguno), para que el handler de señales del PADRE
/// lo arrastre al morir: un `kill` al supervisor no debe dejar al programa huérfano reteniendo el
/// puerto (Ctrl-C de terminal ya mata al grupo; esto cubre el kill por pid).
static DEV_CHILD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Instala el handler SIGTERM/SIGINT del supervisor: reenvía SIGTERM al hijo y sale. Solo unix
/// (el mismo alcance que `signals()`, M88.1); async-signal-safe (kill + _exit, sin asignar).
fn install_cleanup_on_death() {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn signal(sig: i32, handler: usize) -> usize;
        }
        extern "C" fn on_death(_sig: i32) {
            unsafe extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
                fn _exit(code: i32) -> !;
            }
            let pid = DEV_CHILD.load(std::sync::atomic::Ordering::SeqCst);
            if pid > 0 {
                unsafe {
                    kill(pid, 15); // SIGTERM: el hijo drena (serve_graceful) o muere por defecto
                }
            }
            unsafe { _exit(130) }
        }
        const SIGINT: i32 = 2;
        const SIGTERM: i32 = 15;
        unsafe {
            signal(SIGINT, on_death as *const () as usize);
            signal(SIGTERM, on_death as *const () as usize);
        }
    }
}

/// Termina el hijo con SIGTERM (drenado ordenado vía `serve_graceful`) y, si a los ~3 s sigue
/// vivo, escala a SIGKILL. En no-unix va directo al kill duro de std.
fn terminate_gracefully(child: &mut process::Child) {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        const SIGTERM: i32 = 15;
        unsafe {
            kill(child.id() as i32, SIGTERM);
        }
        for _ in 0..30 {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        eprintln!("[dev] el programa no drenó a tiempo; terminación forzosa");
    }
    let _ = child.kill();
    let _ = child.wait();
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
    regen_stale_templates(Path::new(&path)); // M55: los .ray.html desactualizados, al día
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
    regen_stale_templates(Path::new(&path)); // M55: los .ray.html desactualizados, al día
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

/// `ray remove <nombre>`: elimina una dependencia de `ray.toml` (M51f, la operación inversa de
/// `ray add`), **re-resuelve** el grafo (reescribe `ray.lock` sin ella) y borra su caché
/// `.ray-deps/<nombre>` **solo si ya nadie la usa** (podría seguir siendo transitiva de otra dep;
/// el `ray.lock` recién escrito es quien lo sabe).
fn cmd_remove(args: &[String]) {
    let Some(name) = args.first().map(String::as_str) else {
        eprintln!("uso: ray remove <nombre>");
        process::exit(64);
    };
    if !crate::deps::valid_package_name(name) {
        eprintln!("nombre de paquete inválido '{name}': solo letras, dígitos, '-' y '_'");
        process::exit(64);
    }
    let Some(m) = load_manifest() else {
        eprintln!("no hay proyecto: falta 'ray.toml'");
        process::exit(64);
    };
    let toml_path = m.root.join("ray.toml");
    let src = match fs::read_to_string(&toml_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("no se pudo leer '{}': {e}", toml_path.display());
            process::exit(66);
        }
    };
    let Some(updated) = crate::manifest::remove_dependency(&src, name) else {
        eprintln!("la dependencia '{name}' no está declarada en ray.toml");
        process::exit(65);
    };
    if let Err(e) = fs::write(&toml_path, &updated) {
        eprintln!("no se pudo escribir '{}': {e}", toml_path.display());
        process::exit(73);
    }
    println!("dependencia '{name}' eliminada de ray.toml");
    // Re-resolver con el manifiesto ya editado: reescribe `ray.lock` sin la dep (o con ella si
    // sigue siendo transitiva de otra). Después, la caché se borra solo si el lock ya no la lista.
    match crate::manifest::Manifest::load(&m.root) {
        Ok(Some(m2)) => {
            if let Err(e) = crate::deps::ensure(&m2) {
                eprintln!("error re-resolviendo dependencias: {e}");
                process::exit(65);
            }
            let cache = m.root.join(".ray-deps").join(name);
            if cache.is_dir() && !crate::deps::locked_names(&m.root).iter().any(|n| n == name) {
                let _ = fs::remove_dir_all(&cache);
                println!("caché '.ray-deps/{name}' eliminada");
            }
        }
        Ok(None) | Err(_) => {} // el manifiesto acaba de escribirse; improbable
    }
}

/// `ray search [patrón]`: lista los paquetes del **índice** (M51f) cuyo nombre contenga el patrón
/// (sin patrón, todos), con su versión instalable más alta (final, no retirada — como `ray add`).
/// El índice se localiza como siempre (`RAY_INDEX`/`[registry] index`; el remoto se clona/cachea).
fn cmd_search(args: &[String]) {
    let pattern = args.first().map(|s| s.to_lowercase()).unwrap_or_default();
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
    let entries = match fs::read_dir(&index) {
        Ok(rd) => rd,
        Err(e) => {
            eprintln!("no se pudo listar el índice '{}': {e}", index.display());
            process::exit(66);
        }
    };
    // Un paquete = un `<nombre>.toml` en el índice (se ignora cualquier otro archivo del repo).
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let name = (p.extension().is_some_and(|x| x == "toml"))
                .then(|| p.file_stem()?.to_str().map(str::to_string))
                .flatten()?;
            (crate::deps::valid_package_name(&name) && name.to_lowercase().contains(&pattern))
                .then_some(name)
        })
        .collect();
    names.sort();
    if names.is_empty() {
        println!("sin resultados en el índice{}", if pattern.is_empty() { String::new() } else { format!(" para '{pattern}'") });
        return;
    }
    for name in &names {
        match crate::index::latest(&index, name) {
            Ok(v) => println!("{name} {v}"),
            Err(_) => println!("{name} (sin versión instalable)"),
        }
    }
    println!("{} paquete(s)", names.len());
}

/// `ray publish [--repo <git+URL@ref>]`: publica la versión de este paquete en el índice (M51b).
/// Valida (name+version semver) y **añade** la entrada de versión al índice, de forma **inmutable**
/// (no sobrescribe). La spec git de dónde vive el código: `--repo` si se da, o se deriva del remoto

// ── M83b/c: claves de publicación, firma y auditoría del índice ─────────────────────

/// La ruta del archivo de clave de publicación: `RAY_KEY` (tests/CI) o `~/.ray/publish.key`.
fn key_path() -> PathBuf {
    if let Ok(p) = env::var("RAY_KEY")
        && !p.is_empty()
    {
        return PathBuf::from(p);
    }
    let home = env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".ray").join("publish.key")
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Lee la SEED Ed25519 (32 octetos, hex) del archivo de clave.
fn load_signing_seed() -> Result<Vec<u8>, String> {
    let path = key_path();
    let hex = fs::read_to_string(&path).map_err(|_| {
        format!(
            "no hay clave de publicación en '{}' (génerala con 'ray keygen', o apunta RAY_KEY)",
            path.display()
        )
    })?;
    let seed = crate::index::decode_ed25519(&format!("ed25519:{}", hex.trim()), "la clave")?;
    if seed.len() != 32 {
        return Err(format!("la clave de '{}' no tiene 32 octetos", path.display()));
    }
    Ok(seed)
}

/// `ray keygen [--out F]`: genera la clave Ed25519 de publicación (seed de 32 octetos del
/// CSPRNG, en hex) y muestra la pública. Rechaza pisar una clave existente.
fn cmd_keygen(args: &[String]) {
    // M89.2: las firmas usan Ed25519 (ring); un binario slim no puede generarlas.
    if !crate::builtins::net_tls_available() {
        eprintln!("ray keygen: {}", crate::builtins::NET_TLS_UNAVAILABLE);
        process::exit(69);
    }
    let out = match args.split_first() {
        Some((flag, rest)) if flag == "--out" => match rest.first() {
            Some(p) => PathBuf::from(p),
            None => {
                eprintln!("--out requiere una ruta");
                process::exit(64);
            }
        },
        Some((other, _)) => {
            eprintln!("argumento no reconocido: '{other}' (uso: ray keygen [--out F])");
            process::exit(64);
        }
        None => key_path(),
    };
    if out.exists() {
        eprintln!("'{}' ya existe (no se pisa una clave; bórrala tú si de verdad quieres otra)", out.display());
        process::exit(65);
    }
    let seed = crate::builtins::crypto_random_bytes(32);
    if seed.len() != 32 {
        eprintln!("no hay CSPRNG disponible en esta build");
        process::exit(70);
    }
    let Some(pk) = crate::builtins::ed25519_public_key(&seed) else {
        eprintln!("no se pudo derivar la clave pública");
        process::exit(70);
    };
    if let Some(parent) = out.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        eprintln!("no se pudo crear '{}': {e}", parent.display());
        process::exit(73);
    }
    if let Err(e) = fs::write(&out, format!("{}\n", hex_of(&seed))) {
        eprintln!("no se pudo escribir '{}': {e}", out.display());
        process::exit(73);
    }
    println!("clave de publicación generada en {}", out.display());
    println!("  pubkey: ed25519:{}", hex_of(&pk));
    println!("guárdala bien: es tu identidad de publicador (la pública se fija en el índice al publicar --sign).");
}

/// M83c: firma una publicación y reclama (o verifica) el DUEÑO del nombre en el índice.
/// Primera publicación firmada → escribe `<nombre>.owners.toml` con nuestra pubkey (TOFU).
/// Nombre ya reclamado → nuestra pubkey debe coincidir con la registrada.
fn sign_publication(index: &Path, name: &str, version: &str, hash: &str) -> Result<String, String> {
    // M89.2: sin 'net-tls' no hay Ed25519 → error claro (no una firma vacía).
    if !crate::builtins::net_tls_available() {
        return Err(crate::builtins::NET_TLS_UNAVAILABLE.to_string());
    }
    let seed = load_signing_seed()?;
    let pk = crate::builtins::ed25519_public_key(&seed)
        .ok_or_else(|| "no se pudo derivar la clave pública".to_string())?;
    let my_pub = format!("ed25519:{}", hex_of(&pk));
    match crate::index::read_owners(index, name)? {
        Some(o) => {
            if o.pubkey != my_pub {
                return Err(format!(
                    "'{name}' ya tiene dueño registrado en el índice y tu clave NO coincide \
                     ('{}.owners.toml'); si el nombre es tuyo, firma con la clave original",
                    name
                ));
            }
        }
        None => {
            // Reclamación (TOFU): el handle informativo sale de git, si está configurado.
            let owner = git_capture(Path::new("."), &["config", "user.name"]).unwrap_or_default();
            crate::index::write_owners(
                index,
                name,
                &crate::index::Owners { owner: owner.trim().to_string(), pubkey: my_pub },
            )?;
            println!("nombre '{name}' reclamado en el índice ('{name}.owners.toml') — commitéalo junto a la entrada");
        }
    }
    let msg = crate::index::signing_message(name, version, hash);
    let sig = crate::builtins::ed25519_sign(&seed, msg.as_bytes())
        .ok_or_else(|| "no se pudo firmar (¿build sin ring?)".to_string())?;
    Ok(format!("ed25519:{}", hex_of(&sig)))
}

/// `ray index-verify [dir]`: audita un índice completo — cada entrada firmada debe
/// verificar contra el dueño registrado de su paquete. Pensado para el CI del repo del
/// índice (la otra mitad del enforcement — que un PR solo toque paquetes de su autor —
/// es del hosting: CODEOWNERS/branch protection). Sale 0 si todo verifica; 65 si no.
fn cmd_index_verify(args: &[String]) {
    // M89.2: auditar firmas exige Ed25519 (ring); un binario slim no puede.
    if !crate::builtins::net_tls_available() {
        eprintln!("ray index-verify: {}", crate::builtins::NET_TLS_UNAVAILABLE);
        process::exit(69);
    }
    let dir = match args.first() {
        Some(d) => PathBuf::from(d),
        None => match load_manifest().and_then(|m| crate::deps::index_dir(&m).ok().flatten()) {
            Some(d) => d,
            None => {
                eprintln!("uso: ray index-verify <dir> (o corre en un proyecto con índice configurado)");
                process::exit(64);
            }
        },
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        eprintln!("no se pudo leer el índice '{}'", dir.display());
        process::exit(65);
    };
    let mut packages = 0usize;
    let mut versions = 0usize;
    let mut signed = 0usize;
    let mut problemas: Vec<String> = Vec::new();
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.ends_with(".toml") && !n.ends_with(".owners.toml"))
        .filter_map(|n| n.strip_suffix(".toml").map(str::to_string))
        .collect();
    names.sort();
    for name in &names {
        packages += 1;
        match crate::index::read_package(&dir, name) {
            Ok(pkg_entries) => {
                for e in &pkg_entries {
                    versions += 1;
                    if e.sig.is_some() {
                        signed += 1;
                    }
                    if let Err(err) = crate::index::check_signature(&dir, name, e) {
                        problemas.push(err);
                    }
                }
            }
            Err(e) => problemas.push(e),
        }
    }
    if problemas.is_empty() {
        println!(
            "índice OK: {packages} paquetes, {versions} versiones ({signed} firmadas y verificadas)"
        );
    } else {
        for p in &problemas {
            eprintln!("FALLO: {p}");
        }
        eprintln!("índice con {} problema(s)", problemas.len());
        process::exit(65);
    }
}

/// `origin` del repo + el tag `v<version>` (que debe existir). **M51d**: la validación (la cara del
/// paquete existe, todos los `.ray` lexean+parsean) y el **hash de contenido** se calculan sobre un
/// **clon limpio de la ref publicada** (el tag), NO sobre el working tree — lo que se avala en el
/// índice es EXACTAMENTE lo que un consumidor descargará (cambios sin commitear no contaminan el
/// hash). El índice se localiza como en `ray add` (`RAY_INDEX`/`[registry] index`). No hace
/// commit/push del índice —eso lo hace el autor.
fn cmd_publish(args: &[String]) {
    let mut repo_override: Option<String> = None;
    let mut sign = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--repo" => match it.next() {
                Some(spec) => repo_override = Some(spec.clone()),
                None => {
                    eprintln!("--repo requiere una spec 'git+<URL>@<ref>'");
                    process::exit(64);
                }
            },
            "--sign" => sign = true, // M83c
            other => {
                eprintln!("argumento no reconocido: '{other}' (uso: ray publish [--repo S] [--sign])");
                process::exit(64);
            }
        }
    }
    let Some(m) = load_manifest() else {
        eprintln!("no hay proyecto: falta 'ray.toml' (crea uno con 'ray new')");
        process::exit(64);
    };
    // Validación: nombre válido (construye rutas en índice/caché, M51d) + version semver.
    if !crate::deps::valid_package_name(&m.name) {
        eprintln!("nombre de paquete inválido '{}': solo letras, dígitos, '-' y '_'", m.name);
        process::exit(65);
    }
    if crate::semver::parse_version(&m.version).is_none() {
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
    let hash = match published_hash(&m, &git_spec) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("{e}");
            process::exit(65);
        }
    };
    // M83b/c: firmar la publicación y reclamar (o verificar) el dueño del nombre.
    let mut sig: Option<String> = None;
    if sign {
        match sign_publication(&index, &m.name, &m.version, &hash) {
            Ok(sg) => sig = Some(sg),
            Err(e) => {
                eprintln!("{e}");
                process::exit(65);
            }
        }
    }
    match crate::index::append_version(&index, &m.name, &m.version, &git_spec, Some(&hash), sig.as_deref()) {
        Ok(()) => {
            println!("publicado {} {} en el índice", m.name, m.version);
            println!("  git:  {git_spec}");
            println!("  hash: {hash}");
            if sig.is_some() {
                println!("  firma: ed25519 (dueño en '{}.owners.toml')", m.name);
            }
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
/// consumidor obtendrá—, comprueba que la cara del paquete existe (`mod.ray` o la entrada), que
/// **todos** los `.ray` lexean y parsean, y (M51e) que el paquete **supera el check semántico
/// completo** (carga la cara con el loader —resolviendo antes sus dependencias, si las declara— y
/// lo verifica con el checker, sin exigir `main`). Devuelve su `deps::hash_package` — calculado
/// ANTES de resolver las deps del clon, que escriben `.ray-deps/`/`ray.lock` dentro y no son parte
/// del contenido. El clon temporal se borra siempre.
fn published_hash(m: &crate::manifest::Manifest, git_spec: &str) -> Result<String, String> {
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
        // Todos los .ray publicados deben lexear y parsear (también los no importados por la cara).
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
        // El hash, ANTES del check: resolver deps escribe `.ray-deps/`/`ray.lock` dentro del clon.
        let hash = crate::deps::hash_package(&tmp)?;
        check_published(&tmp, &face)?;
        Ok(hash)
    })();
    let _ = fs::remove_dir_all(&tmp);
    result
}

/// M51e: el **check semántico completo** del contenido a publicar (cierra el diferido de M51d):
/// resuelve las dependencias que el clon declare (por el índice o por git; escriben dentro del
/// clon temporal), carga la cara con el loader (imports internos + deps + `std/` embebida) y la
/// verifica con `check_all_modulo` (el checker SIN exigir `main`: un paquete es una librería).
/// Un error se reporta contra su archivo y línea local (vía `Loaded::locate`).
fn check_published(tmp: &Path, face: &Path) -> Result<(), String> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if tmp.join("ray.toml").is_file()
        && let Ok(Some(mc)) = crate::manifest::Manifest::load(tmp)
    {
        if mc.dependencies.iter().any(|(_, s)| crate::deps::path_of_path_dep(s).is_none()) {
            crate::deps::ensure(&mc).map_err(|e| {
                format!("no se pudieron resolver las dependencias del paquete para el check: {e}")
            })?;
        }
        let cache = mc.root.join(".ray-deps");
        if cache.is_dir() {
            roots.push(cache);
        }
        // Path-deps del paquete (raras al publicar, pero el check las honra igual que `ray run`).
        for (_n, s) in &mc.dependencies {
            if let Some(p) = crate::deps::path_of_path_dep(s) {
                let dir = mc.root.join(p);
                if let Some(parent) = dir.parent().map(Path::to_path_buf)
                    && dir.exists()
                    && !roots.contains(&parent)
                {
                    roots.push(parent);
                }
            }
        }
    }
    let mut loaded = crate::loader::load_with_deps(face, &roots)
        .map_err(|e| format!("el paquete no carga: {}", e.message))?;
    let errors = crate::checker::check_all_modulo(&mut loaded.program);
    if let Some(e) = errors.first() {
        let (modulo, _fuente, linea) = loaded.locate(e.line);
        return Err(format!(
            "el paquete no supera el check semántico ({modulo}.ray, línea {linea}): {}",
            e.msg
        ));
    }
    Ok(())
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
/// `ray templ <ruta>...`: compila cada template `.ray.html` (o todos los de un directorio,
/// recursivo) a su módulo raylang generado (`.ray` al lado, commiteable). M55.
fn cmd_templ(args: &[String]) {
    if args.is_empty() {
        eprintln!("uso: ray templ <archivo.ray.html | directorio>...");
        process::exit(64);
    }
    let mut entries: Vec<PathBuf> = Vec::new();
    for a in args {
        let p = Path::new(a);
        if p.is_dir() {
            collect_templates(p, &mut entries);
        } else if a.ends_with(".ray.html") {
            entries.push(p.to_path_buf());
        } else {
            eprintln!("'{a}' no es un .ray.html ni un directorio");
            process::exit(64);
        }
    }
    entries.sort();
    if entries.is_empty() {
        eprintln!("no se encontraron templates .ray.html");
        process::exit(64);
    }
    for e in &entries {
        match crate::templ::generate_file(e) {
            Ok(out) => println!("generado: {}", out.display()),
            Err(msg) => {
                eprintln!("{msg}");
                process::exit(65);
            }
        }
    }
}

// Recolecta los `.ray.html` de un directorio, recursivo (orden estable: se ordenan al final).
// Salta los directorios ocultos (`.git`, `.ray-deps`): sus templates no son del proyecto.
fn collect_templates(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                collect_templates(&p, out);
            } else if p.to_string_lossy().ends_with(".ray.html") {
                out.push(p);
            }
        }
    }
}

/// M55: **regeneración automática de templates** — antes de compilar/correr, cada `.ray.html` bajo
/// el directorio de la entrada cuyo `.ray` generado **falte** o esté **desactualizado** (mtime
/// anterior al del template) se regenera, como si se hubiera corrido `ray templ`. El aviso va por
/// **stderr** (stdout es del programa). Un template con error de sintaxis aborta con 65 (el build
/// habría fallado igual al compilar el generado viejo, pero con peor señal). Con los generados al
/// día el coste es un stat por template (cero sin `.ray.html`).
fn regen_stale_templates(entry: &Path) {
    let dir = match entry.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let mut tpls = Vec::new();
    collect_templates(dir, &mut tpls);
    tpls.sort();
    for t in tpls {
        let generated = PathBuf::from(t.to_string_lossy().trim_end_matches(".html").to_string());
        let mtime = |p: &Path| fs::metadata(p).and_then(|m| m.modified());
        let stale = match (mtime(&t), mtime(&generated)) {
            (Ok(tm), Ok(gm)) => gm < tm,
            (_, Err(_)) => true,  // no hay generado
            (Err(_), _) => false, // el template ni se puede leer: lo reportará quien lo importe
        };
        if !stale {
            continue;
        }
        match crate::templ::generate_file(&t) {
            Ok(out) => eprintln!("template regenerado: {}", out.display()),
            Err(msg) => {
                eprintln!("{msg}");
                process::exit(65);
            }
        }
    }
}

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
    // M40.8a: caché `.ray-deps/` + el padre de cada dependencia por ruta. La lógica vive en
    // `deps::dependency_roots_for` (compartida con el LSP → un archivo diagnostica con las MISMAS
    // raíces con las que corre). La stdlib no es raíz de disco: va embebida (M40.5).
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    crate::deps::dependency_roots_for(&cwd)
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
    // M55: un template `.ray.html` se formatea con SU formateador (etiquetas en su línea +
    // indentación por bloques del template), no con el de raylang.
    if path.ends_with(".ray.html") {
        match crate::templ::format_template(&read_source(path), &unit) {
            Some(out) => print!("{}", out),
            None => {
                eprintln!("error de formato: el template no tokeniza (delimitador sin cerrar)");
                process::exit(65);
            }
        }
        return;
    }
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
    if std::env::var("RAYLANG_TIME").is_ok() {
    }

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
            let trace = std::mem::take(&mut e.trace);
            // M79c: si el error cayó en el prelude o en la std (código que el usuario no
            // tiene delante), la cabecera y el `^` se reposicionan al PRIMER marco de
            // usuario — el assert fallido apunta al `assert(...)` del usuario, no al
            // `panic` del prelude. La traza completa se imprime igual (la entrada 0
            // sigue contando el sitio real). Sin marco de usuario → posición original.
            if let Some((line, col)) = first_user_frame(&trace, &locate) {
                e.line = line;
                e.col = col;
            }
            let (source, name, local) = locate(e.line);
            e.line = local;
            let head = if multi { format!("[{}] {}", name, e) } else { e.to_string() };
            eprintln!("{}", diagnostic::render(&source, local, e.col, 1, &head));
            for l in render_trace(&trace, &locate) {
                eprintln!("{}", l);
            }
            process::exit(70); // EX_SOFTWARE
        }
    }
}

/// M79c: el primer marco de la traza que es código de USUARIO — en banda (no el prelude,
/// el único fuente inyectado sin banda propia) y fuera de la std embebida (`std` /
/// `std/…`). Devuelve su posición GLOBAL (quien llama la localiza después). `None` si no
/// hay traza o ningún marco califica.
fn first_user_frame(trace: &[runtime::TraceFrame], locate: &Locate) -> Option<(usize, usize)> {
    trace.iter().find_map(|f| {
        let (source, name, local) = locate(f.line);
        let in_band = local <= source.lines().count();
        let is_std = name == "std" || name.starts_with("std/");
        if in_band && !is_std { Some((f.line, f.col)) } else { None }
    })
}

/// M79: renderiza la traza de llamadas de un error de runtime, una línea por marco
/// (`en <fn> (<módulo>:L:C)` el más interno, `desde …` los llamadores), localizada por
/// bandas como la cabecera. Una posición **fuera de banda** (línea local mayor que el
/// fuente del módulo) solo puede venir del prelude —el único fuente inyectado sin banda
/// propia— y se etiqueta `prelude` con su línea original. Con un solo marco no se
/// imprime nada (la cabecera ya lo dice todo); con recursión profunda se trunca
/// (primeros 6 + `…` + últimos 5), la traza completa no aporta.
fn render_trace(trace: &[runtime::TraceFrame], locate: &Locate) -> Vec<String> {
    if trace.len() < 2 {
        return Vec::new();
    }
    let render_frame = |prefix: &str, f: &runtime::TraceFrame| {
        let (source, name, local) = locate(f.line);
        if local > source.lines().count() {
            format!("  {} {} (prelude:{}:{})", prefix, f.name, f.line, f.col)
        } else {
            format!("  {} {} ({}:{}:{})", prefix, f.name, name, local, f.col)
        }
    };
    let mut out = Vec::new();
    let n = trace.len();
    let (head, tail) = if n > 12 { (6, n - 5) } else { (n, n) };
    for (i, f) in trace.iter().enumerate() {
        if i >= head && i < tail {
            if i == head {
                out.push(format!("  … ({} marcos omitidos)", tail - head));
            }
            continue;
        }
        out.push(render_frame(if i == 0 { "en" } else { "desde" }, f));
    }
    out
}
