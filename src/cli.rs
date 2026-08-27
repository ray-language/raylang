//! CLI de raylang — el ejecutable `ray` (M39a), como módulo de la lib para que los dos
//! binarios (`ray` y el alias `raylang`) sean envoltorios de una línea sobre `cli::main`.
//!
//! Interfaz de **subcomandos** (estilo `cargo`/`go`), agrupada por ROL — DESIGN.md §88 (M99):
//!   `ray new <nombre>`      — crea un proyecto nuevo (ray.toml + src/main.ray).
//!   `ray run [archivo]`     — ejecuta (por defecto `src/main.ray`) en la VM (M35).
//!   `ray build [archivo]`   — chequea y compila sin ejecutar (para CI); 0 ok / 65 error.
//!                             con `--native [-o <salida>] [--release]` transpila a Rust y compila con
//!                             rustc → binario nativo (P2.b, requiere `rustc`). `--release` = opt3+lto+
//!                             codegen-units=1+target-cpu=native (más lento de compilar, no portable).
//!                             con `--templates-only [ruta...]` solo compila los `.ray.html` (M99).
//!   `ray test [archivo]`    — corre las funciones `@test` (M10.1).
//!   `ray fmt <archivo>`     — imprime la versión canónica por stdout (M29.2).
//!   `ray doc <archivo>`     — genera la documentación Markdown (raydoc).
//!   `ray add|remove|update|search|fetch` — gestión de dependencias, uso diario (M39b/M51).
//!   `ray registry <sub>`    — comandos del PUBLICADOR: `publish`/`yank`/`keygen`/`verify` (M51/M99).
//!   `ray lsp`               — arranca el Language Server (M10.2).
//!   `ray mcp`               — arranca el servidor MCP.
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
use crate::{checker, compiler, diagnostic, loader, lsp, mcp, repl, runtime, test_runner, vm};

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
        Some("update") => cmd_update(&rest[1..]),
        Some("fetch") => cmd_fetch(&rest[1..]),
        Some("registry") => cmd_registry(&rest[1..]),
        Some("fmt") => cmd_fmt(&rest[1..]),
        Some("doc") => cmd_doc(&rest[1..]),
        Some("lsp") => lsp::run(),
        Some("mcp") => mcp::run(),
        Some("repl") | None => repl::run(),
        Some("upgrade") => cmd_upgrade(&rest[1..]),
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
raylang {v} — programming language

Usage: ray <subcommand> [options]

Project:
  new <name>        create a new project (ray.toml + src/main.ray)
  run [file]        run (src/main.ray by default) [--interp] [--deterministic] [--fuel N] [--heap N] [args...]
  dev [file]        like run, but RESTARTS on changes to .ray/.ray.html/ray.toml (development mode)
  build [file]      check and compile without running (0 ok / 65 error) [--native [-o out] [--release] [--fast] [--target triple] [--without crypto,tls,sqlite,mimalloc,ahash,regex,fibers,process,watch]] [--templates-only [path...]]
  test [file]       run the project's @test functions (entry modules + tests/*.ray) [filter]
  fmt <file>...     print the canonical version to stdout (--write / -w: rewrite in place)
  doc <file>        generate the Markdown documentation of its public surface

Packages:
  add <name>[@req]  add a dependency from the index to ray.toml and download it
  remove <name>     remove a dependency from ray.toml (and its cache if nobody else uses it)
  update            re-resolve the index dependencies to the newest compatible ones
  search [pattern]  list the index packages (that contain the pattern)
  fetch             download the ray.toml dependencies to .ray-deps/

Registry (package authors):
  registry publish [--repo S] [--sign]   publish this package's version in the index
  registry yank <name>@<ver> [--undo]    yank (or restore) a published version in the index
  registry keygen [--out F]              generate the Ed25519 publish key (RAY_KEY or ~/.ray/publish.key)
  registry verify [dir]                  audit the signatures of an index (for the index repo's CI)

Tooling:
  lsp               start the Language Server
  mcp               start the MCP server (tools for AI agents: check/run/test/fmt/doc)
  repl              interactive REPL
  upgrade [tag]     update ray to the latest release (--check: only report; 0 = up to date)
  version           the language version
  help              this help
",
        v = env!("CARGO_PKG_VERSION")
    );
}

/// `ray registry <sub>` (M99): los comandos del **publicador** de paquetes — los que escriben en el
/// índice compartido o manejan sus claves de firma. Se agrupan porque comparten ROL (mantenedor, no
/// consumidor) y frecuencia (rara); los de consumo —`add`/`remove`/`update`/`search`/`fetch`— viven
/// en la raíz porque son de uso diario. Ver DESIGN.md §88.
fn cmd_registry(args: &[String]) {
    match args.first().map(String::as_str) {
        Some("publish") => cmd_publish(&args[1..]),
        Some("yank") => cmd_yank(&args[1..]),
        Some("keygen") => cmd_keygen(&args[1..]),
        Some("verify") => cmd_index_verify(&args[1..]),
        other => {
            if let Some(sub) = other {
                eprintln!("unknown registry subcommand: '{sub}'");
            }
            eprintln!(
                "\
usage: ray registry <subcommand>

  publish [--repo S] [--sign]   publish this package's version in the index
  yank <name>@<ver> [--undo]    yank (or restore) a published version in the index
  keygen [--out F]              generate the Ed25519 publish key
  verify [dir]                  audit the signatures of an index"
            );
            process::exit(64);
        }
    }
}

// ── Subcomandos ──────────────────────────────────────────────────────────────────────

/// `ray new <nombre>`: crea el esqueleto de un proyecto — `ray.toml` (el manifiesto que
/// leerá el gestor de paquetes, M39b) + `src/main.ray` con un hola-mundo + `.gitignore`.
fn cmd_new(args: &[String]) {
    let Some(name) = args.first() else {
        eprintln!("usage: ray new <name>");
        process::exit(64);
    };
    let root = Path::new(name);
    if root.exists() {
        eprintln!("'{name}' already exists");
        process::exit(65);
    }
    let manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[dependencies]\n"
    );
    let main_ray = format!("fn main() -> int {{\n    print(\"hello from {name}\");\n    0\n}}\n");
    let gitignore = "# dependencies downloaded by the package manager (M39c)\n.ray-deps/\n";
    let write_file = |path: std::path::PathBuf, content: &str| {
        if let Some(parent) = path.parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            eprintln!("could not create '{}': {e}", parent.display());
            process::exit(73); // EX_CANTCREAT
        }
        if let Err(e) = fs::write(&path, content) {
            eprintln!("could not write '{}': {e}", path.display());
            process::exit(73);
        }
    };
    write_file(root.join("ray.toml"), &manifest);
    write_file(root.join("src/main.ray"), &main_ray);
    write_file(root.join(".gitignore"), gitignore);
    println!("project '{name}' created. To run it:\n  cd {name} && ray run");
}

// ── `ray upgrade` (M137): autoactualización del toolchain desde las GitHub Releases ─────────

/// El repo de releases del toolchain. `RAYLANG_REPO` lo sobrescribe (forks, tests) — la misma
/// variable que entiende `install.sh`.
fn upgrade_repo() -> String {
    std::env::var("RAYLANG_REPO")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "ray-language/raylang".to_string())
}

/// El nombre del asset de release para una plataforma (el mismo esquema que publica
/// `release.yml` y consume `install.sh`). `None` = plataforma sin asset tar.gz (Windows va
/// por zip manual). Pura para poder testearla; el llamador pasa los `cfg!` reales.
fn upgrade_asset(os: &str, arch: &str) -> Option<String> {
    let suffix = match os {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        _ => return None,
    };
    let arch = match arch {
        "x86_64" | "aarch64" => arch,
        _ => return None,
    };
    Some(format!("raylang-{arch}-{suffix}.tar.gz"))
}

/// Extrae el tag de la URL final de `releases/latest` (GitHub redirige a
/// `…/releases/tag/<tag>`). `None` si no hay redirección a un tag (repo sin releases).
fn tag_from_latest_url(url: &str) -> Option<String> {
    let (_, tag) = url.trim_end_matches('/').rsplit_once("/releases/tag/")?;
    Some(tag.to_string()).filter(|t| !t.is_empty() && !t.contains('/'))
}

/// Corre un comando externo y devuelve su stdout (o el stderr como error). El equivalente
/// del helper `git` de deps.rs, para `curl`/`tar`: dependencias del ENTORNO (como git),
/// no crates del árbol de compilación.
fn sh_capture(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut cmd = process::Command::new(program);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let out = cmd
        .args(args)
        .output()
        .map_err(|e| format!("could not run '{program}': {e} (is it installed?)"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// `ray upgrade [tag] [--check]`: actualiza los binarios instalados (`ray` + `raylang`) a la
/// última release publicada, o al tag pedido. `--check` solo informa (exit 0 = al día,
/// 1 = hay versión nueva; para scripts/CI). La descarga delega en `curl` y `tar` del sistema.
/// El binario descargado se VERIFICA (`ray version` desde un dir temporal) antes de tocar
/// nada, y el reemplazo es un rename dentro del directorio de instalación (atómico en POSIX,
/// válido con el binario en ejecución).
fn cmd_upgrade(args: &[String]) {
    let (check, rest) = take_flag_bool(args, "--check");
    if rest.len() > 1 {
        eprintln!("usage: ray upgrade [tag] [--check]");
        process::exit(64);
    }
    let repo = upgrade_repo();
    let Some(asset) = upgrade_asset(std::env::consts::OS, std::env::consts::ARCH) else {
        eprintln!(
            "ray upgrade does not support this platform ({}-{}); on Windows download the \
             .zip from https://github.com/{repo}/releases",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        process::exit(69); // EX_UNAVAILABLE
    };
    // El tag objetivo: el pedido (con o sin la `v`), o el de la última release publicada.
    let tag = match rest.first() {
        Some(t) if t.starts_with('v') => t.clone(),
        Some(t) => format!("v{t}"),
        None => {
            let latest_url = format!("https://github.com/{repo}/releases/latest");
            let effective = sh_capture(
                "curl",
                &["-sSfL", "-o", "/dev/null", "-w", "%{url_effective}", &latest_url],
                None,
            )
            .unwrap_or_else(|e| {
                eprintln!("could not query the latest release ({latest_url}): {e}");
                process::exit(69);
            });
            tag_from_latest_url(&effective).unwrap_or_else(|| {
                eprintln!("no releases published in https://github.com/{repo}/releases");
                process::exit(69);
            })
        }
    };
    let current = env!("CARGO_PKG_VERSION");
    let target = tag.trim_start_matches('v');
    if target == current {
        println!("raylang {current} is already up to date");
        return;
    }
    if check {
        println!("raylang {current} installed; {target} available (run 'ray upgrade')");
        process::exit(1);
    }

    // El directorio de instalación: el del propio ejecutable (resolviendo symlinks).
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .unwrap_or_else(|e| {
            eprintln!("could not locate the current executable: {e}");
            process::exit(70);
        });
    let Some(install_dir) = exe.parent().map(Path::to_path_buf) else {
        eprintln!("could not locate the installation directory of '{}'", exe.display());
        process::exit(70);
    };

    // Descargar y desempaquetar en un dir temporal propio.
    let tmp = std::env::temp_dir().join(format!("ray-upgrade-{}", process::id()));
    let _ = fs::remove_dir_all(&tmp);
    if let Err(e) = fs::create_dir_all(&tmp) {
        eprintln!("could not create '{}': {e}", tmp.display());
        process::exit(73);
    }
    // Limpieza del temporal pase lo que pase de aquí en adelante (el éxito sale por `return`).
    let cleanup = |code: i32| -> ! {
        let _ = fs::remove_dir_all(&tmp);
        process::exit(code);
    };
    let url = format!("https://github.com/{repo}/releases/download/{tag}/{asset}");
    eprintln!("downloading {url}");
    let archive = tmp.join(&asset);
    if let Err(e) = sh_capture("curl", &["-sSfL", "-o", &archive.to_string_lossy(), &url], None) {
        eprintln!("could not download the release ({url}): {e}");
        eprintln!("(does the tag '{tag}' exist? see https://github.com/{repo}/releases)");
        cleanup(69);
    }
    if let Err(e) = sh_capture("tar", &["-xzf", &archive.to_string_lossy()], Some(&tmp)) {
        eprintln!("could not unpack '{asset}': {e}");
        cleanup(69);
    }

    // Verificar el binario ANTES de reemplazar nada: debe correr y reportar la versión pedida.
    match sh_capture(&tmp.join("ray").to_string_lossy(), &["version"], None) {
        Ok(v) if v.contains(target) => {}
        Ok(v) => {
            eprintln!(
                "the downloaded binary reports '{}' but the tag is '{tag}': not installing",
                v.trim()
            );
            cleanup(65);
        }
        Err(e) => {
            eprintln!("the downloaded binary does not run on this machine: {e}");
            cleanup(65);
        }
    }

    // Instalar por rename: copiar al directorio destino como `.<bin>.new` y renombrar encima
    // (mismo filesystem → atómico; reemplazar un binario en ejecución es válido en POSIX).
    for bin in ["ray", "raylang"] {
        let src = tmp.join(bin);
        if !src.is_file() {
            eprintln!("the release package does not contain '{bin}': not installing");
            cleanup(65);
        }
        let staged = install_dir.join(format!(".{bin}.new"));
        let dest = install_dir.join(bin);
        let result = fs::copy(&src, &staged).map_err(|e| e.to_string()).and_then(|_| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&staged, fs::Permissions::from_mode(0o755));
            }
            fs::rename(&staged, &dest).map_err(|e| e.to_string())
        });
        if let Err(e) = result {
            let _ = fs::remove_file(&staged);
            eprintln!("could not install '{}': {e}", dest.display());
            eprintln!(
                "(no write permission? re-run the installer choosing a writable directory: \
                 RAYLANG_BIN_DIR=<dir> install.sh)"
            );
            cleanup(73);
        }
    }
    let _ = fs::remove_dir_all(&tmp);
    println!("installed: raylang {current} → {target} ({})", install_dir.join("ray").display());
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
    let (fuel, rest) = take_flag_num(&rest, "--fuel", "a number of instructions (e.g. --fuel 1000000)");
    let (heap, rest) = take_flag_num(&rest, "--heap", "a number of objects (e.g. --heap 1000000)");
    let (explicit, prog_args) = match rest.split_first() {
        Some((p, rest)) => (Some(p.as_str()), rest.to_vec()),
        None => (None, Vec::new()),
    };
    let path = resolve_entry(explicit, false);
    run_file(&path, prog_args, use_interp, fuel, heap.map(|n| n as usize));
}

// ── `ray dev` (M92.1): modo desarrollo — watcher + reinicio con drenado ─────────────────────

/// `ray dev [archivo] [flags de run] [args...]`: corre el programa como `ray run` y lo REINICIA
/// ante cambios en los fuentes del proyecto (`.ray`, `.ray.html`, `ray.toml`). El watcher usa
/// EVENTOS DE KERNEL (la misma pieza de `fs.watch`, M115.4 — la justificación "polling: cero
/// deps" caducó cuando `notify` entró al árbol) con fallback a polling de mtimes (~200 ms) en
/// builds `--without watch` o no-unix; ver `DevWatcher`. Un `.ray.html` editado dispara
/// reinicio; el hijo lo compila en memoria al arrancar (M102: sin `.ray` generado en disco).
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
    // Socket-activation (M92.3): `--port N`/`--listen host:port` (o `[dev] listen` del ray.toml) → el
    // supervisor pre-abre y RETIENE ese socket; los hijos lo adoptan (fd heredado) en vez de re-bind, así
    // sobrevive a los reinicios (cero conexiones rechazadas). `--port`/`--listen` NO se reenvían al hijo.
    let (cli_listen, fwd_args) = take_listen(args);
    let listen_addr = cli_listen.or_else(|| load_manifest().and_then(|m| m.dev_listen));
    // Retiene el socket durante toda la sesión (vive hasta que `ray dev` muere). Solo unix (fd passing).
    #[cfg(unix)]
    let dev_sock = listen_addr.as_ref().and_then(|addr| match std::net::TcpListener::bind(addr) {
        Ok(l) => {
            eprintln!("[dev] holding {addr} across restarts (socket-activation)");
            Some(l)
        }
        Err(e) => {
            eprintln!("[dev] could not pre-open {addr}: {e}; restarts will re-bind normally");
            None
        }
    });
    #[cfg(not(unix))]
    let dev_sock: Option<std::net::TcpListener> = {
        if listen_addr.is_some() {
            eprintln!("[dev] --port/--listen (socket-activation) is unix-only; ignoring");
        }
        None
    };
    #[cfg(unix)]
    let listen_pair = {
        use std::os::unix::io::AsRawFd;
        dev_sock.as_ref().zip(listen_addr.as_ref()).map(|(l, a)| (l.as_raw_fd(), a.as_str()))
    };
    #[cfg(not(unix))]
    let listen_pair: Option<(i32, &str)> = None;
    let _ = &dev_sock; // se retiene por su lado (el fd vive mientras `dev_sock` no se dropee)

    // Live-reload del navegador (M92.4): el hub SSE emite `reload` en cada reinicio; el webserver,
    // viendo `RAY_DEV_RELOAD`, inyecta el snippet en las respuestas HTML. Arranca SIEMPRE: detectar
    // "es una app web" no es asunto del supervisor — la inyección ya vive en el webserver (solo dispara
    // al servir text/html; un programa CLI nunca inyecta nada y el hub ocioso cuesta un hilo). Sin
    // `--port` no hay socket retenido, así que el snippet reintenta hasta que el hijo re-binde.
    let reload = start_reload_hub();
    let reload_port = reload.as_ref().map(|(_, p)| *p);
    if let Some(p) = reload_port {
        eprintln!("[dev] live-reload on http://127.0.0.1:{p} (browser refresh on restart)");
    }

    // La entrada que el hijo usará (para el check-before-restart): se despojan los flags de `run`
    // (mismos que `cmd_run`), y el primer resto es el archivo explícito (o `None` → default del proyecto).
    let entry = dev_entry(&fwd_args);
    eprintln!("[dev] watching {} (.ray, .ray.html, ray.toml); Ctrl-C to exit", root.display());
    install_cleanup_on_death();

    let mut snapshot = scan_sources(&root);
    let mut hashes = content_hashes(&snapshot);
    let mut watcher = DevWatcher::new(&root);
    let mut child = spawn_dev_child(&exe, &fwd_args, listen_pair, reload_port);
    let mut running = true;
    loop {
        // Vigila hasta el próximo cambio; si el programa termina solo, sigue vigilando sin él.
        let change = loop {
            if let Some(c) = watcher.wait_change(&root, &mut snapshot) {
                break c;
            }
            if running && let Ok(Some(status)) = child.try_wait() {
                running = false;
                eprintln!("[dev] the program finished ({status}); waiting for changes…");
            }
        };
        // Debounce: coalesce una ráfaga (un guardado + el formateador del editor = varios eventos)
        // esperando a que los fuentes se estabilicen antes de actuar → un solo reinicio.
        watcher.debounce(&root, &mut snapshot);
        // Confirmación por contenido: un mtime tocado con los MISMOS bytes (guardado sin editar,
        // formateador idempotente, `touch`) no reinicia ni recarga nada.
        let current_hashes = content_hashes(&snapshot);
        if current_hashes == hashes {
            eprintln!("[dev] change in {change}: contents unchanged — ignoring");
            continue;
        }
        hashes = current_hashes;
        // Check-before-restart: compila primero (ms). Si NO compila, mantén el programa en marcha e
        // imprime el diagnóstico — no mates un servidor que funciona por un error a medio escribir.
        if let Err(diag) = dev_check_compiles(&exe, &entry) {
            eprintln!("[dev] change in {change}: does not compile — keeping the running program:");
            eprint!("{diag}");
            continue;
        }
        // Verde → reinicia (drena el viejo, relanza fresco).
        eprintln!("[dev] change in {change}: restarting…");
        if running {
            terminate_gracefully(&mut child);
        }
        // Arma la recarga ANTES de relanzar: el hub la emitirá cuando el hijo avise `/ready` (el
        // webserver, al bindear) → el navegador recarga justo cuando el servidor nuevo ya escucha,
        // sin un fetch de sondeo que falle mientras re-binde.
        if let Some((hub, _)) = &reload {
            hub.arm();
        }
        child = spawn_dev_child(&exe, &fwd_args, listen_pair, reload_port);
        running = true;
    }
}

/// El archivo de entrada que `ray dev` pasará al hijo (para el check-before-restart), despojando los
/// mismos flags que `ray run` consume; `None` = el default del proyecto (`src/main.ray`).
fn dev_entry(args: &[String]) -> Option<String> {
    let (_det, a) = take_flag_bool(args, "--deterministic");
    let (_interp, a) = take_interp(&a);
    let (_fuel, a) = take_flag_num(&a, "--fuel", "");
    let (_heap, a) = take_flag_num(&a, "--heap", "");
    a.first().cloned()
}

/// Lanza el programa como `ray run <args...>` (mismo binario): hereda la resolución de entrada, la
/// regeneración de templates y los flags. Registra el pid en `DEV_CHILD` para la limpieza por señal.
/// Si `listen` está (unix, socket-activation M92.3): dup2-ea el socket retenido del supervisor al fd 3
/// del hijo (antes del exec) y le pasa `RAY_LISTEN_FD`/`RAY_LISTEN_ADDR` → el hijo lo ADOPTA en `tcp_listen`.
fn spawn_dev_child(
    exe: &Path,
    args: &[String],
    listen: Option<(i32, &str)>,
    reload_port: Option<u16>,
) -> process::Child {
    let mut cmd = process::Command::new(exe);
    cmd.arg("run").args(args);
    // M92.4: el hijo aprende el puerto del hub de live-reload; el webserver inyecta el snippet SSE.
    if let Some(p) = reload_port {
        cmd.env("RAY_DEV_RELOAD", p.to_string());
    }
    #[cfg(unix)]
    if let Some((fd, addr)) = listen {
        use std::os::unix::process::CommandExt;
        const TARGET_FD: i32 = 3; // convención systemd (SD_LISTEN_FDS_START)
        cmd.env("RAY_LISTEN_FD", TARGET_FD.to_string()).env("RAY_LISTEN_ADDR", addr);
        // SAFETY: `pre_exec` corre en el hijo tras `fork` y antes de `exec`; solo se llama a `dup2`/`fcntl`
        // (async-signal-safe). `fd` (el listener del supervisor) es válido en el hijo por herencia del fork.
        // Se limpia CLOEXEC en el fd destino EXPLÍCITAMENTE: si `fd` ya ERA 3 (típico: primer libre tras
        // stdio), `dup2(3,3)` es un no-op que NO limpia CLOEXEC → sin esto, el fd 3 se cerraría en el exec.
        unsafe {
            cmd.pre_exec(move || {
                unsafe extern "C" {
                    fn dup2(oldfd: i32, newfd: i32) -> i32;
                    // VARIÁDICA, como la declaración de builtins.rs (aridad fija = UB en arm64 y
                    // `clashing_extern_declarations` entre ambas).
                    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
                }
                const F_SETFD: i32 = 2; // limpiar los flags del descriptor (quita FD_CLOEXEC)
                // (sin `unsafe` interior: el closure ya corre dentro del bloque unsafe de pre_exec)
                if dup2(fd, TARGET_FD) < 0 || fcntl(TARGET_FD, F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let _ = &listen; // en no-unix el parámetro no se usa
    match cmd.spawn() {
        Ok(c) => {
            DEV_CHILD.store(c.id() as i32, std::sync::atomic::Ordering::SeqCst);
            c
        }
        Err(e) => {
            eprintln!("[dev] could not launch the program: {e}");
            process::exit(70);
        }
    }
}

/// Separa el socket a retener entre reinicios (M92.3) de los args a reenviar: `--listen host:port` o
/// `--port N` (→ `127.0.0.1:N`). Devuelve `(addr, resto_de_args)`. Sin el flag, `(None, args)`.
fn take_listen(args: &[String]) -> (Option<String>, Vec<String>) {
    let mut listen = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--listen" => listen = it.next().cloned(),
            "--port" => listen = it.next().map(|p| format!("127.0.0.1:{p}")),
            _ => rest.push(a.clone()),
        }
    }
    (listen, rest)
}

/// M92.4 — hub de **live-reload del navegador**: un servidor SSE mínimo en un puerto lateral del
/// supervisor. Los navegadores conectan un `EventSource` (el webserver inyecta el snippet bajo dev) y el
/// supervisor les emite un evento `reload` en cada reinicio → la página se refresca sola. Cliente externo,
/// cero cambios en la VM; se apoya en que el webserver ya habla SSE.
struct ReloadHub {
    clients: std::sync::Mutex<Vec<std::net::TcpStream>>,
    /// Recarga ARMADA a la espera del `/ready` del hijo (el webserver avisa al bindear). Emitir al
    /// armar competiría con el re-bind del hijo → el navegador vería "connection refused"; retenerla
    /// hasta el aviso hace que la recarga llegue justo cuando el servidor ya escucha.
    pending: std::sync::atomic::AtomicBool,
}

impl ReloadHub {
    /// Emite `data: reload` a cada navegador conectado; descarta los que ya cerraron.
    fn broadcast(&self) {
        use std::io::Write;
        let mut clients = self.clients.lock().unwrap();
        clients.retain_mut(|c| c.write_all(b"data: reload\n\n").and_then(|_| c.flush()).is_ok());
    }

    /// Arma la recarga del próximo reinicio: se emitirá al recibir el `/ready` del hijo. Fallback: si
    /// en 2 s nadie avisa (la app no usa `net/webserver`, o murió al arrancar), se emite igualmente —
    /// mejor una recarga que puede fallar que un navegador congelado en la versión vieja.
    fn arm(self: &std::sync::Arc<Self>) {
        use std::sync::atomic::Ordering;
        self.pending.store(true, Ordering::SeqCst);
        let hub = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            if hub.pending.swap(false, Ordering::SeqCst) {
                hub.broadcast();
            }
        });
    }
}

/// Arranca el hub SSE en un puerto libre (hilo de fondo que acepta navegadores y los registra tras
/// enviarles las cabeceras SSE). Devuelve `(hub, puerto)`, o `None` si no se pudo abrir el socket.
fn start_reload_hub() -> Option<(std::sync::Arc<ReloadHub>, u16)> {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
    let port = listener.local_addr().ok()?.port();
    let hub = std::sync::Arc::new(ReloadHub {
        clients: std::sync::Mutex::new(Vec::new()),
        pending: std::sync::atomic::AtomicBool::new(false),
    });
    let hub_bg = hub.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut s = stream;
            let _ = s.set_nodelay(true);
            // Lee la petición: `GET /ready` es el aviso del hijo (ya escucha → suelta la recarga
            // armada); cualquier otra cosa es un `EventSource` de un navegador.
            let _ = s.set_read_timeout(Some(std::time::Duration::from_millis(200)));
            let mut buf = [0u8; 1024];
            let n = s.read(&mut buf).unwrap_or(0);
            let _ = s.set_read_timeout(None);
            if buf[..n].starts_with(b"GET /ready") {
                use std::sync::atomic::Ordering;
                if hub_bg.pending.swap(false, Ordering::SeqCst) {
                    hub_bg.broadcast();
                }
                let _ = s.write_all(b"HTTP/1.1 204 No Content\r\n\r\n");
                continue;
            }
            // Cabeceras SSE + un comentario inicial para establecer el stream (CORS abierto: dev local).
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\nAccess-Control-Allow-Origin: *\r\n\r\n: connected\n\n";
            if s.write_all(head.as_bytes()).and_then(|_| s.flush()).is_ok() {
                hub_bg.clients.lock().unwrap().push(s);
            }
        }
    });
    Some((hub, port))
}

/// Corre `ray build <entry>` (chequea + compila, sin ejecutar) como el gate del reinicio: `Ok(())` si
/// compila, `Err(diagnóstico)` con el stderr renderizado si no. Reusa exactamente la salida de `ray build`.
fn dev_check_compiles(exe: &Path, entry: &Option<String>) -> Result<(), String> {
    let mut cmd = process::Command::new(exe);
    cmd.arg("build");
    if let Some(e) = entry {
        cmd.arg(e);
    }
    match cmd.output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).into_owned()),
        Err(e) => Err(format!("could not run the compile check: {e}\n")),
    }
}

/// Debounce: espera a que los fuentes se estabilicen (~120 ms sin cambios) antes de continuar, para
/// coalescer una ráfaga de eventos (guardado + formateador) en una sola acción. Actualiza `snapshot`.
/// La fuente de "hubo un cambio" de `ray dev`. La vía preferida son los EVENTOS DE KERNEL —
/// la misma pieza que `fs.watch` (M115.4: `notify`, FSEvents/inotify): latencia de decenas de
/// ms tras el guardado y cero coste en reposo (el polling re-stat-eaba el árbol entero 5 veces
/// por segundo). El polling de mtimes queda como respaldo: builds `--without watch`, no-unix,
/// o un árbol donde el watcher no consiga abrirse. Ambas vías alimentan el MISMO bucle: el
/// debounce y la confirmación por hash de contenido siguen aguas abajo, idénticos.
enum DevWatcher {
    #[cfg(all(feature = "watch", unix))]
    Events {
        watcher: ray_runtime::watch::FsWatcher,
        /// Raíz canonicalizada: los eventos llegan con la ruta real del filesystem (p. ej.
        /// `/private/tmp/...` en macOS) y hay que compararlos contra lo mismo.
        canon_root: PathBuf,
    },
    Polling,
}

impl DevWatcher {
    fn new(root: &Path) -> Self {
        #[cfg(all(feature = "watch", unix))]
        {
            match ray_runtime::watch::watch(&root.display().to_string()) {
                Ok(watcher) => {
                    let canon_root =
                        fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
                    return DevWatcher::Events { watcher, canon_root };
                }
                Err(e) => {
                    eprintln!("[dev] kernel events unavailable ({e}); falling back to mtime polling");
                }
            }
        }
        let _ = root;
        DevWatcher::Polling
    }

    /// Un paso de espera (~200 ms de cota): `Some(descripción)` si hubo un cambio relevante, con
    /// `snapshot` ya actualizado. La cota corta permite al llamador vigilar también al hijo
    /// (`try_wait`) sin hilo aparte.
    fn wait_change(
        &mut self,
        root: &Path,
        snapshot: &mut Vec<(PathBuf, std::time::SystemTime)>,
    ) -> Option<String> {
        match self {
            #[cfg(all(feature = "watch", unix))]
            DevWatcher::Events { watcher, canon_root } => {
                match watcher.next_timeout(200) {
                    Ok(Some((_kind, path))) => {
                        let path = PathBuf::from(path);
                        if !is_watched_source(canon_root, &path) {
                            return None;
                        }
                        *snapshot = scan_sources(root);
                        let deleted = if path.exists() { "" } else { " (deleted)" };
                        Some(format!("{}{deleted}", path.display()))
                    }
                    Ok(None) => None,
                    Err(e) => {
                        // El watcher murió (p. ej. la raíz desapareció): degradar a polling y
                        // seguir — perder el modo dev entero sería peor que perder la latencia.
                        eprintln!("[dev] kernel events stopped ({e}); falling back to mtime polling");
                        *self = DevWatcher::Polling;
                        None
                    }
                }
            }
            DevWatcher::Polling => {
                std::thread::sleep(std::time::Duration::from_millis(200));
                let current = scan_sources(root);
                let change = first_change(snapshot, &current);
                if change.is_some() {
                    *snapshot = current;
                }
                change
            }
        }
    }

    /// Coalesce la ráfaga de un guardado: espera a que pasen ~120 ms sin eventos RELEVANTES
    /// (los irrelevantes — artefactos, ocultos — no alargan la espera) y deja el snapshot al día.
    fn debounce(&mut self, root: &Path, snapshot: &mut Vec<(PathBuf, std::time::SystemTime)>) {
        match self {
            #[cfg(all(feature = "watch", unix))]
            DevWatcher::Events { watcher, canon_root } => {
                let mut deadline = std::time::Instant::now() + std::time::Duration::from_millis(120);
                loop {
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    match watcher.next_timeout(left.as_millis().max(1) as i64) {
                        Ok(Some((_kind, path))) => {
                            if is_watched_source(canon_root, Path::new(&path)) {
                                deadline = std::time::Instant::now()
                                    + std::time::Duration::from_millis(120);
                            }
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
                *snapshot = scan_sources(root);
            }
            DevWatcher::Polling => dev_debounce(root, snapshot),
        }
    }
}

/// ¿Es `path` (ruta de un evento de kernel, absoluta) un fuente que `ray dev` vigila? El mismo
/// criterio de `scan_sources`, aplicado a una ruta suelta: bajo la raíz, fuera de carpetas de
/// artefactos/ocultas, extensión de fuente, y la regla del `.ray` generado con `.ray.html`
/// hermano (se vigila el fuente, no el artefacto). Los temporales de guardado atómico de los
/// editores (`.tmp`, `~`, el `4913` de vim) caen solos por la extensión.
#[cfg(all(feature = "watch", unix))]
fn is_watched_source(canon_root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(canon_root) else { return false };
    if let Some(parent) = rel.parent() {
        for comp in parent.components() {
            let name = comp.as_os_str().to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                return false;
            }
        }
    }
    let Some(name) = path.file_name() else { return false };
    let name = name.to_string_lossy();
    if !(name.ends_with(".ray") || name.ends_with(".ray.html") || name == "ray.toml") {
        return false;
    }
    if name.ends_with(".ray") && !name.ends_with(".ray.html") {
        let html = path.with_extension("ray.html");
        if html.exists() {
            return false;
        }
    }
    true
}

fn dev_debounce(root: &Path, snapshot: &mut Vec<(PathBuf, std::time::SystemTime)>) {
    loop {
        std::thread::sleep(std::time::Duration::from_millis(120));
        let current = scan_sources(root);
        if current == *snapshot {
            break;
        }
        *snapshot = current;
    }
}

/// Huella de CONTENIDO de los fuentes vigilados (ruta → hash de los bytes). Distingue un guardado
/// real de un mtime tocado sin cambios (Cmd+S sin editar, formateador idempotente, `touch`, un
/// checkout que restaura lo mismo): el polling sigue siendo por metadatos (barato); esto solo se
/// computa cuando el mtime acusa un cambio, para confirmarlo antes de reiniciar.
fn content_hashes(snapshot: &[(PathBuf, std::time::SystemTime)]) -> std::collections::HashMap<PathBuf, u64> {
    use std::hash::{Hash, Hasher};
    snapshot
        .iter()
        .map(|(p, _)| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            fs::read(p).unwrap_or_default().hash(&mut h);
            (p.clone(), h.finish())
        })
        .collect()
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
                // Un `.ray` con un `.ray.html` hermano es un generado de `ray build
                // --templates-only` (derivado, además IGNORADO por el loader desde M102): se
                // vigila el fuente (el .html), no el artefacto.
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
    before: &[(PathBuf, std::time::SystemTime)],
    after: &[(PathBuf, std::time::SystemTime)],
) -> Option<String> {
    if before == after {
        return None;
    }
    let old: std::collections::HashMap<_, _> = before.iter().cloned().collect();
    for (p, m) in after {
        if old.get(p) != Some(m) {
            return Some(p.display().to_string());
        }
    }
    // Nada nuevo ni tocado pero difieren → algo se borró.
    let new: std::collections::HashMap<_, _> = after.iter().cloned().collect();
    before
        .iter()
        .find(|(p, _)| !new.contains_key(p))
        .map(|(p, _)| format!("{} (deleted)", p.display()))
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
        eprintln!("[dev] the program did not drain in time; forced termination");
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
                    eprintln!("{flag} requires {description}");
                    process::exit(64);
                }
            },
            None => {
                eprintln!("{flag} requires {description}");
                process::exit(64);
            }
        }
    }
    (None, args.to_vec())
}

/// `ray build [archivo]`: chequea y **compila** el programa sin ejecutarlo (útil para CI y
/// para validar antes de publicar). Sale 0 si compila, 65 si hay errores de compilación.
fn cmd_build(args: &[String]) {
    // `--templates-only [ruta...]` (M99): compila los `.ray.html` y termina, SIN chequear ni compilar
    // el programa. Es el reemplazo del subcomando `ray build --templates-only`: la compilación de templates es un paso
    // del build, no un comando de usuario (DESIGN.md §88.4). A diferencia de la regeneración
    // automática de `run`/`build`/`test` —que es incremental por mtime—, esta **fuerza** la
    // regeneración de todos: es el escape para cuando cambia el GENERADOR (p. ej. al actualizar `ray`)
    // y los mtimes no reflejan que el `.ray` de salida quedó obsoleto. Sin rutas, escanea la raíz del
    // proyecto (o el directorio actual si no hay `ray.toml`).
    if args.iter().any(|a| a == "--templates-only") {
        let paths: Vec<String> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .cloned()
            .collect();
        build_templates(&paths);
        return;
    }
    // `--native [-o <salida>] [--release]` (P2.b): transpila a Rust y lo compila con rustc → binario
    // nativo. El resto de flags/archivo se pasan igual; el archivo es el primer no-flag.
    let native = args.iter().any(|a| a == "--native");
    let release = args.iter().any(|a| a == "--release");
    // `--fast` (H6): aritmética de int ENVOLVENTE (wrapping) en vez de checked — renuncia a la paridad
    // de overflow con la VM a cambio del último tramo de rendimiento (div/mod por cero siguen chequeados).
    let fast = args.iter().any(|a| a == "--fast");
    // Fibras (arco de concurrencia nativa, jul 2026): la concurrencia del binario nativo corre sobre
    // el scheduler M:N de fibras (corosensei + reactor kqueue/epoll) POR DEFECTO — decisión tomada
    // tras F5 (banco en red real: techo +16 % sobre hilo-por-tarea, 14 hilos / 8 MB donde el modelo
    // viejo levantaba uno por conexión) y F4 (TLS/UDP cubiertos). Escape: `--without fibers` recupera
    // el hilo-por-tarea (y es lo que exige la vía rustc pelada, que no puede traer corosensei). El
    // flag `--fibers` se acepta por compatibilidad (hoy es el default); combinarlo con el escape es
    // contradictorio → error. Ver docs/diseno-concurrencia-nativa.md.
    let fibers_flag = args.iter().any(|a| a == "--fibers");
    let output = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1)).cloned();
    // `--target <triple>` (P2.b, H20): cross-compilation. Se pasa tal cual a rustc/cargo (el usuario debe
    // tener el target instalado: `rustup target add <triple>`). Con `--target`, `--release` NO usa
    // `target-cpu=native` (sería la CPU del host, no la del target) → binario release PORTABLE al target.
    let target = args.iter().position(|a| a == "--target").and_then(|i| args.get(i + 1)).cloned();
    // `--without <lista>` (P2.b): excluye subsistemas con-crate (crypto/tls/sqlite/mimalloc/ahash) del
    // binario nativo. Un subsistema de USO (crypto/…) excluido cae en un stub que panica; `mimalloc` (N1) y
    // `ahash` (N2) — siempre-on sin detección de uso — excluidos vuelven al malloc del sistema / al HashMap
    // std. El binario compila por la vía rápida (rustc pelado, sin cargo/red) solo si no queda NINGUNA
    // feature — es decir, hoy exige `--without mimalloc,ahash`. Escape para builds herméticos/cross/policy.
    // Se UNE a la política estable del proyecto (`[native] without` en ray.toml).
    // Cada exclusión rastrea su ORIGEN (`--without` de CLI vs `ray.toml`) para que un typo apunte al sitio
    // que hay que corregir: un error en un ray.toml versionado afecta a todo el equipo.
    let without_arg = args.iter().position(|a| a == "--without").and_then(|i| args.get(i + 1)).cloned();
    let mut exclude: Vec<(String, &'static str)> = without_arg
        .as_deref()
        .map(|s| s.split(',').map(str::trim).filter(|p| !p.is_empty()).map(|p| (p.to_string(), "--without")).collect())
        .unwrap_or_default();
    // Une la política del ray.toml ([native] without): la exclusión versionada con el repo + la ad-hoc de
    // CLI. `load_manifest` sale con error si el ray.toml está mal formado; `None` = sin proyecto (solo CLI).
    if native {
        if let Some(m) = load_manifest() {
            for dep in m.native_without {
                if !exclude.iter().any(|(d, _)| d == &dep) {
                    exclude.push((dep, "ray.toml"));
                }
            }
        }
    }
    // Valida los nombres (CLI + ray.toml) fail-fast, como `ray add`. El mensaje nombra el origen del typo.
    const RT_SUBSYSTEMS: &[&str] = &["crypto", "tls", "sqlite", "mimalloc", "ahash", "regex", "fibers", "process", "watch", "unicode"];
    for (dep, origin) in &exclude {
        if !RT_SUBSYSTEMS.contains(&dep.as_str()) {
            eprintln!(
                "unknown subsystem in {origin}: '{dep}' (valid: {})",
                RT_SUBSYSTEMS.join(", ")
            );
            process::exit(64);
        }
    }
    let exclude: Vec<String> = exclude.into_iter().map(|(d, _)| d).collect();
    // Resolución del modo fibras: default ON; `--without fibers` lo apaga; ambos a la vez es un
    // contrasentido (fail-fast, como los typos de subsistema). En un target sin poller propio
    // (Windows: el reactor es kqueue/epoll) se apaga solo, con aviso — el hilo-por-tarea sigue
    // siendo el respaldo completo.
    let fibers = {
        let without_fibers = exclude.iter().any(|d| d == "fibers");
        if fibers_flag && without_fibers {
            eprintln!("--fibers and '--without fibers' contradict each other; pick one");
            process::exit(64);
        }
        let windows = target.as_deref().is_some_and(|t| t.contains("windows"));
        if windows && !without_fibers {
            eprintln!("note: fibers are not available on Windows targets yet; building with the thread-per-task model");
        }
        !without_fibers && !windows
    };
    let file = args
        .iter()
        .find(|a| {
            !a.starts_with('-')
                && Some(a.as_str()) != output.as_deref()
                && Some(a.as_str()) != without_arg.as_deref()
                && Some(a.as_str()) != target.as_deref()
        })
        .map(String::as_str);
    let path = resolve_entry(file, true);
    let (mut program, locate, multi) = load_and_locate(&path);
    check_or_exit(&mut program, &locate, multi);
    if native {
        build_native(&path, output.as_deref(), release, &exclude, target.as_deref(), fast, fibers);
        return;
    }
    match compiler::compile_program(&program) {
        Ok(_) => println!("ok: '{path}' compiles"),
        Err(mut e) => {
            let (source, name, local, col, len) = locate(e.line, e.col, 1);
            e.line = local;
            e.col = col;
            let head = if multi { format!("[{}] {}", name, e) } else { e.to_string() };
            eprintln!("{}", diagnostic::render(&source, local, col, len, &head));
            process::exit(65);
        }
    }
}

/// `ray build --native [--release]` (P2.b, transpilar-a-Rust): transpila el programa **ya chequeado** a
/// Rust, lo escribe a un `.rs` temporal y lo compila con rustc → binario nativo. El nombre por defecto es
/// el *stem* del archivo de entrada en el directorio actual; `-o <ruta>` lo cambia. Requiere `rustc` en el
/// PATH. El programa se re-carga/re-chequea aquí (barato) para no cambiar la firma de `cmd_build`.
///
/// **Tiers de optimización** (elegidos por medición — ver PERFORMANCE.md §P2 fase 33):
/// - por defecto: `-O` (opt-level=2). Compila rápido (~0.2 s) y PORTABLE; el mejor equilibrio para dev.
/// - `--release`: `opt-level=3 + lto=fat + codegen-units=1 + target-cpu=native`. ~10 % más rápido en
///   cargas de asignación/Map (nada en cómputo puro, ya óptimo), a cambio de ~9× de tiempo de compilación
///   y un binario **no portable** (usa las features de la CPU del host). PGO se **descartó** (sin ganancia
///   medible + alta complejidad).
fn build_native(path: &str, output: Option<&str>, release: bool, exclude: &[String], target: Option<&str>, fast: bool, fibers: bool) {
    let (mut program, locate, multi) = load_and_locate(path);
    check_or_exit(&mut program, &locate, multi);
    let transpiled = match crate::transpile::transpile_full(&program, exclude, fast, fibers) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("native build: {e}");
            process::exit(65);
        }
    };
    // AVISO de stubs (H7): funciones cuyo cuerpo cayó fuera del subconjunto se emitieron como stub que
    // panica. El binario compila, pero llamarlas aborta → sin este aviso el "ok" ocultaría una divergencia
    // en runtime. Se listan (nombre + motivo) para que el usuario sepa qué NO se soporta.
    if !transpiled.stubbed.is_empty() {
        eprintln!(
            "native build: warning: {} function(s) not supported in the native subset — emitted as stubs \
             that panic if called at runtime:",
            transpiled.stubbed.len()
        );
        for (name, reason) in &transpiled.stubbed {
            eprintln!("  · {name}: {reason}");
        }
    }
    // Nombre de salida: `-o` > el `name` del ray.toml del proyecto (si la entrada ES la del
    // proyecto, como Cargo) > el stem del archivo de entrada (archivo suelto).
    let stem = Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("a");
    let out_bin = output.map(String::from).unwrap_or_else(|| {
        let entry_dir = match Path::new(path).parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        Manifest::load(entry_dir)
            .ok()
            .flatten()
            .filter(|m| canon(&m.entry_path()) == canon(Path::new(path)))
            .map(|m| m.name)
            .unwrap_or_else(|| stem.to_string())
    });
    // **Bifurcación bajo demanda** (P2.b, docs/transpilador-nativo.md §4.5): sin features de `ray-runtime`
    // → `rustc` pelado, rápido y sin red. Con features → un proyecto Cargo generado que enlaza `ray-runtime`
    // (mismo código que la VM). N1/N2: como `mimalloc` y `ahash` van POR DEFECTO, el camino común hoy es el
    // Cargo (con la caché compartida ~/.ray/native-cache, se compilan una vez por máquina); `--without
    // mimalloc,ahash` (sin otros subsistemas) recupera el rustc pelado.
    if transpiled.rt_features.is_empty() {
        build_native_rustc(&transpiled.source, stem, &out_bin, release, target);
    } else {
        build_native_cargo(&transpiled.source, &transpiled.rt_features, path, stem, &out_bin, release, target);
    }
}

/// Directorio de caché de builds nativos, PERSISTENTE entre sesiones (`~/.ray/native-cache/`, decidido en
/// docs/transpilador-nativo.md §3.3). Sobrevive a la purga de `/tmp` (macOS: 3 días sin uso; Linux: reboot)
/// → el target compartido (ring/rustls compilados una vez por máquina) NO se pierde periódicamente. Si no
/// hay HOME/USERPROFILE, cae al temporal (comportamiento anterior).
fn native_cache_dir() -> std::path::PathBuf {
    match std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        Some(home) => Path::new(&home).join(".ray").join("native-cache"),
        None => std::env::temp_dir().join("ray_native_cache"),
    }
}

/// Camino rápido: transpila a un `.rs` autocontenido y lo compila con `rustc` directo (sin Cargo). Para
/// programas que no usan ningún crate externo — el 90 % de los casos. `-O` (dev) / opt3+lto+native (release).
fn build_native_rustc(rust: &str, stem: &str, out_bin: &str, release: bool, target: Option<&str>) {
    // El `.rs` temporal incluye el PID para que dos `ray build --native` CONCURRENTES (o con el mismo stem)
    // no colisionen sobre el mismo temporal.
    let rs_path = std::env::temp_dir().join(format!("ray_native_{stem}_{}.rs", process::id()));
    if let Err(e) = std::fs::write(&rs_path, rust) {
        eprintln!("native build: could not write the temporary Rust file: {e}");
        process::exit(65);
    }
    // Flags de rustc según el tier (`-A warnings` silencia los warnings de estilo del código generado).
    // Con `--target` (cross-compile), `target-cpu=native` no aplica (sería la CPU del host) → release
    // PORTABLE al target.
    let flags: Vec<&str> = if release && target.is_none() {
        vec!["-C", "opt-level=3", "-C", "lto=fat", "-C", "codegen-units=1", "-C", "target-cpu=native", "-A", "warnings"]
    } else if release {
        vec!["-C", "opt-level=3", "-C", "lto=fat", "-C", "codegen-units=1", "-A", "warnings"]
    } else {
        vec!["-O", "-A", "warnings"]
    };
    let mut cmd = process::Command::new("rustc");
    cmd.args(&flags);
    // MISMA edition que el proyecto Cargo generado (edition 2024 en su Cargo.toml). Sin el flag, rustc
    // pelado caía a la 2015 → los dos caminos compilaban el MISMO Rust generado bajo reglas distintas
    // (p. ej. `gen` es keyword solo en 2024: un identificador podía pasar por un tier y romper por el otro).
    cmd.arg("--edition").arg("2024");
    if let Some(t) = target {
        cmd.arg("--target").arg(t);
    }
    let status = cmd.arg(&rs_path).arg("-o").arg(out_bin).status();
    match status {
        Ok(s) if s.success() => {
            let _ = std::fs::remove_file(&rs_path); // build ok → no dejar el `.rs` temporal (sin fugas)
            let tier = match (release, target) {
                (true, Some(t)) => format!(" (release: opt3+lto, target: {t})"),
                (true, None) => " (release: opt3+lto+native)".to_string(),
                (false, Some(t)) => format!(" (target: {t})"),
                (false, None) => String::new(),
            };
            println!("ok: native binary '{out_bin}'{tier}");
        }
        Ok(s) => {
            // Falló: se CONSERVA el `.rs` y se nombra su ruta, para poder inspeccionar el Rust generado.
            eprintln!(
                "native build: rustc failed (code {}); generated Rust at {}",
                s.code().unwrap_or(-1),
                rs_path.display()
            );
            process::exit(65);
        }
        Err(e) => {
            eprintln!("native build: could not run rustc (is it on PATH?): {e}");
            process::exit(65);
        }
    }
}

// Fuentes de `ray-runtime` INCRUSTADAS en el binario `ray` (como `prelude.ray`): al generar un proyecto
// Cargo se escriben tal cual → el runtime es EXACTAMENTE el de esta versión de `ray` (paridad con la VM por
// construcción), sin publicar el crate ni depender de red salvo la primera descarga de sus deps (ring…).
const RT_CARGO_TOML: &str = include_str!("../crates/ray-runtime/Cargo.toml");
const RT_LIB_RS: &str = include_str!("../crates/ray-runtime/src/lib.rs");
const RT_CRYPTO_RS: &str = include_str!("../crates/ray-runtime/src/crypto.rs");
const RT_TLS_RS: &str = include_str!("../crates/ray-runtime/src/tls.rs");
const RT_SQLITE_RS: &str = include_str!("../crates/ray-runtime/src/sqlite.rs");
const RT_REGEX_RS: &str = include_str!("../crates/ray-runtime/src/regex.rs");
const RT_FIBERS_RS: &str = include_str!("../crates/ray-runtime/src/fibers.rs");
const RT_PROCESS_RS: &str = include_str!("../crates/ray-runtime/src/process.rs");
const RT_WATCH_RS: &str = include_str!("../crates/ray-runtime/src/watch.rs");
const RT_X509_RS: &str = include_str!("../crates/ray-runtime/src/x509.rs");
const RT_UNICODE_RS: &str = include_str!("../crates/ray-runtime/src/unicode.rs");

/// Camino Cargo: el programa usa un subsistema con crate externo (cripto/…). Se genera un proyecto Cargo
/// temporal (`src/main.rs` + una copia de `ray-runtime` con las fuentes incrustadas) y se compila con
/// `cargo build`, activando SOLO las features detectadas. Un `CARGO_TARGET_DIR` compartido compila los
/// crates (ring…) una vez por máquina; builds siguientes solo recompilan `main.rs`.
fn build_native_cargo(rust: &str, rt_features: &[&str], src_path: &str, stem: &str, out_bin: &str, release: bool, target: Option<&str>) {
    // Nombre de paquete Cargo válido (letras/dígitos/`_`/`-`, no empieza por dígito): el stem saneado.
    let mut pkg: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    if pkg.is_empty() || pkg.chars().next().map_or(true, |c| c.is_ascii_digit()) {
        pkg.insert(0, 'p');
    }
    // H14: la caché de target es COMPARTIDA y persistente → el artefacto vive en `<caché>/<profile>/<pkg>`.
    // Con solo el stem, dos programas DISTINTOS llamados `prog.ray` (o `a.b.ray` vs `a_b.ray`, que el saneado
    // colapsa) compartirían la ruta: el build B pisa el binario y A copiaría el de B (carrera silenciosa de
    // corrección). El pkg incorpora un hash corto de la ruta CANÓNICA del fuente → artefactos disjuntos por
    // programa. (DefaultHasher no está garantizado entre versiones de Rust: si cambia, el coste es una
    // recompilación, nunca corrección.)
    {
        use std::hash::{Hash, Hasher};
        let canon = std::fs::canonicalize(src_path).unwrap_or_else(|_| std::path::PathBuf::from(src_path));
        let mut h = std::collections::hash_map::DefaultHasher::new();
        canon.hash(&mut h);
        pkg.push_str(&format!("_{:08x}", h.finish() as u32));
    }
    let proj = std::env::temp_dir().join(format!("ray_native_{stem}_{}", process::id()));
    let write = |rel: &str, content: &str| -> std::io::Result<()> {
        let p = proj.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, content)
    };
    // Las features detectadas, como lista TOML (`"crypto", "tls"`).
    let feats: String = rt_features.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(", ");
    // `[workspace]` vacío: el proyecto es su PROPIA raíz de workspace (no hereda una ancestra por azar). Los
    // perfiles espejan los tiers de rustc: dev=opt2 (rápido), release=opt3+lto+cu1 (target-cpu vía RUSTFLAGS).
    let cargo_toml = format!(
        "[package]\nname = \"{pkg}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n\n\
         [dependencies]\nray-runtime = {{ path = \"ray-runtime\", default-features = false, features = [{feats}] }}\n\n\
         [profile.dev]\nopt-level = 2\n\n[profile.release]\nopt-level = 3\nlto = \"fat\"\ncodegen-units = 1\n"
    );
    let files = [
        ("Cargo.toml", cargo_toml.as_str()),
        ("src/main.rs", rust),
        ("ray-runtime/Cargo.toml", RT_CARGO_TOML),
        ("ray-runtime/src/lib.rs", RT_LIB_RS),
        ("ray-runtime/src/unicode.rs", RT_UNICODE_RS),
        ("ray-runtime/src/crypto.rs", RT_CRYPTO_RS),
        ("ray-runtime/src/tls.rs", RT_TLS_RS),
        ("ray-runtime/src/sqlite.rs", RT_SQLITE_RS),
        ("ray-runtime/src/regex.rs", RT_REGEX_RS),
        ("ray-runtime/src/fibers.rs", RT_FIBERS_RS), // sin efecto salvo feature `fibers` (cfg en lib.rs)
        ("ray-runtime/src/process.rs", RT_PROCESS_RS), // ídem: solo con la feature `process` (M100)
        ("ray-runtime/src/watch.rs", RT_WATCH_RS), // ídem: solo con la feature `watch` (M115.4)
        ("ray-runtime/src/x509.rs", RT_X509_RS), // ídem: la arrastra `tls` (M124, tls_peer_cert)
    ];
    for (rel, content) in files {
        if let Err(e) = write(rel, content) {
            eprintln!("native build: could not write the Cargo project ({rel}): {e}");
            process::exit(65);
        }
    }
    // Caché de target COMPARTIDA entre builds y PERSISTENTE (`~/.ray/native-cache/`) → ring/rustls se
    // compilan una vez por máquina y no se pierden con la purga de /tmp.
    let target_dir = native_cache_dir();
    // Reproducibilidad (H20): las deps de ray-runtime son rangos (`0.23`/`0.17`) → sin un lock, dos builds
    // podrían resolver versiones distintas. Se PERSISTE el `Cargo.lock` resuelto en la caché y se reusa: los
    // builds siguientes en esta máquina fijan las MISMAS versiones (y se saltan la re-resolución).
    let cached_lock = target_dir.join("ray-native.Cargo.lock");
    if cached_lock.is_file() {
        let _ = std::fs::copy(&cached_lock, proj.join("Cargo.lock")); // proj ya existe (files escritos arriba)
    }
    let mut cmd = process::Command::new("cargo");
    cmd.arg("build").current_dir(&proj).env("CARGO_TARGET_DIR", &target_dir);
    if let Some(t) = target {
        cmd.arg("--target").arg(t);
    }
    // `target-cpu=native` solo en release SIN cross-compile (con `--target` sería la CPU del host → no
    // portable al target).
    if release && target.is_none() {
        cmd.arg("--release").env("RUSTFLAGS", "-C target-cpu=native -A warnings");
    } else if release {
        cmd.arg("--release").env("RUSTFLAGS", "-A warnings");
    } else {
        cmd.env("RUSTFLAGS", "-A warnings");
    }
    match cmd.status() {
        Ok(s) if s.success() => {
            let sub = if release { "release" } else { "debug" };
            // Con `--target`, cargo pone el binario en `target/<triple>/<profile>/<pkg>`.
            let produced = match target {
                Some(t) => target_dir.join(t).join(sub).join(&pkg),
                None => target_dir.join(sub).join(&pkg),
            };
            let copied = std::fs::copy(&produced, out_bin);
            let _ = std::fs::copy(proj.join("Cargo.lock"), &cached_lock); // persiste el lock resuelto (H20)
            let _ = std::fs::remove_dir_all(&proj); // build ok → borrar el proyecto Cargo temporal (el binario
                                                    // ya vive en la caché compartida, no en `proj/target`)
            if let Err(e) = copied {
                eprintln!("native build: could not copy the binary ({}): {e}", produced.display());
                process::exit(65);
            }
            let tier = match (release, target) {
                (true, Some(t)) => format!(" (release: opt3+lto, target: {t})"),
                (true, None) => " (release: opt3+lto+native)".to_string(),
                (false, Some(t)) => format!(" (target: {t})"),
                (false, None) => String::new(),
            };
            println!("ok: native binary '{out_bin}'{tier} [ray-runtime: {}]", rt_features.join("+"));
        }
        Ok(s) => {
            // Falló: se CONSERVA el proyecto y se nombra su ruta, para inspeccionar el Rust generado.
            eprintln!(
                "native build: cargo failed (code {}); project at {}",
                s.code().unwrap_or(-1),
                proj.display()
            );
            process::exit(65);
        }
        Err(e) => {
            eprintln!("native build: could not run cargo (is it on PATH?): {e}");
            // N1/N2: mimalloc+ahash-por-defecto traen el camino Cargo al caso común; sin cargo aún se
            // puede compilar con rustc pelado excluyendo las features siempre-on (las de USO no: el
            // programa las necesita de verdad).
            if rt_features.iter().all(|f| *f == "mimalloc" || *f == "ahash") {
                let list = rt_features.join(",");
                eprintln!("hint: build without cargo (plain rustc) with: ray build --native --without {list}");
            }
            process::exit(65);
        }
    }
}

/// `ray test [archivo] [filtro]`: corre las funciones `@test` (a nivel proyecto, M101).
/// Sin archivo explícito, las suites son la **entrada del proyecto** (sus `@test` y las de todos
/// los módulos que importa) más cada **`tests/*.ray`** junto al `ray.toml` (pruebas de
/// integración: importan los módulos del proyecto porque la raíz de la entrada va como raíz
/// extra del loader). Un primer argumento que no termina en `.ray` se toma como filtro.
fn cmd_test_sub(args: &[String]) {
    let (explicit, filter) = match args.first().map(String::as_str) {
        Some(a) if a.ends_with(".ray") => (Some(a), args.get(1).map(String::as_str)),
        first => (None, first),
    };
    let entry = resolve_entry(explicit, false);

    let mut suites = vec![PathBuf::from(&entry)];
    if explicit.is_none() {
        let root = load_manifest().map(|m| m.root).unwrap_or_else(|| PathBuf::from("."));
        suites.extend(discover_test_files(&root.join("tests")));
    }
    // La raíz de la entrada como raíz extra: un `tests/*.ray` resuelve `import m;` contra `src/`.
    let mut roots = dependency_roots();
    if let Some(parent) = Path::new(&entry).parent() {
        roots.push(parent.to_path_buf());
    }
    process::exit(test_runner::run(&suites, &roots, filter));
}

/// Los archivos `.ray` bajo `dir` (recursivo, orden estable por ruta): las suites de integración
/// de `ray test`. Un directorio inexistente devuelve `[]` (la convención `tests/` es opcional).
fn discover_test_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            found.extend(discover_test_files(&path));
        } else if path.extension().is_some_and(|e| e == "ray") {
            found.push(path);
        }
    }
    found
}

/// `ray add <nombre>[@<req>]`: añade una dependencia **del índice** (por nombre) a `ray.toml` y la
/// descarga (M51a). Sin `@<req>`, usa la versión más alta publicada como `^<latest>` (compatible,
/// estilo cargo); con `@<req>`, lo respeta (`1.2.0` exacta, `^1.2`, `~1.2.3`, `*`). Valida que la
/// versión exista en el índice **antes** de tocar el manifiesto (fail-fast ante un typo).
fn cmd_add(args: &[String]) {
    let Some(spec) = args.first().map(String::as_str) else {
        eprintln!("usage: ray add <name>[@<version>]");
        process::exit(64);
    };
    let (name, req_opt) = match spec.split_once('@') {
        Some((n, r)) => (n, Some(r)),
        None => (spec, None),
    };
    if name.is_empty() {
        eprintln!("usage: ray add <name>[@<version>]");
        process::exit(64);
    }
    // M51d: el nombre construye rutas (caché, archivo del índice) — validarlo antes de nada.
    if !crate::deps::valid_package_name(name) {
        eprintln!("invalid package name '{name}': only letters, digits, '-' and '_'");
        process::exit(64);
    }
    let Some(m) = load_manifest() else {
        eprintln!("no project: missing 'ray.toml' (create one with 'ray new')");
        process::exit(64);
    };
    // Localiza el índice (RAY_INDEX, [registry] index, o el oficial por defecto — M136).
    let index = match crate::deps::index_dir(&m) {
        Ok(Some(dir)) => dir,
        Ok(None) => {
            eprintln!(
                "the package index is disabled ('index = \"\"' or empty RAY_INDEX): declare \
                 '[registry] index = \"<dir>\"' in ray.toml or export RAY_INDEX (for git deps \
                 use 'name = \"git+URL@ref\"' by hand)"
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
            eprintln!("could not read '{}': {e}", toml_path.display());
            process::exit(66);
        }
    };
    let updated = crate::manifest::upsert_dependency(&src, name, &req);
    if let Err(e) = fs::write(&toml_path, &updated) {
        eprintln!("could not write '{}': {e}", toml_path.display());
        process::exit(73);
    }
    println!("added dependency '{name} = \"{req}\"'");
    // Descarga (recarga el manifiesto para que `ensure` vea la nueva dep).
    match crate::manifest::Manifest::load(&m.root) {
        Ok(Some(m2)) => match crate::deps::ensure(&m2) {
            Ok(_) => println!("dependencies up to date"),
            Err(e) => {
                eprintln!("download error: {e}");
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
        eprintln!("usage: ray remove <name>");
        process::exit(64);
    };
    if !crate::deps::valid_package_name(name) {
        eprintln!("invalid package name '{name}': only letters, digits, '-' and '_'");
        process::exit(64);
    }
    let Some(m) = load_manifest() else {
        eprintln!("no project: missing 'ray.toml'");
        process::exit(64);
    };
    let toml_path = m.root.join("ray.toml");
    let src = match fs::read_to_string(&toml_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not read '{}': {e}", toml_path.display());
            process::exit(66);
        }
    };
    let Some(updated) = crate::manifest::remove_dependency(&src, name) else {
        eprintln!("dependency '{name}' is not declared in ray.toml");
        process::exit(65);
    };
    if let Err(e) = fs::write(&toml_path, &updated) {
        eprintln!("could not write '{}': {e}", toml_path.display());
        process::exit(73);
    }
    println!("dependency '{name}' removed from ray.toml");
    // Re-resolver con el manifiesto ya editado: reescribe `ray.lock` sin la dep (o con ella si
    // sigue siendo transitiva de otra). Después, la caché se borra solo si el lock ya no la lista.
    match crate::manifest::Manifest::load(&m.root) {
        Ok(Some(m2)) => {
            if let Err(e) = crate::deps::ensure(&m2) {
                eprintln!("error re-resolving dependencies: {e}");
                process::exit(65);
            }
            let cache = m.root.join(".ray-deps").join(name);
            if cache.is_dir() && !crate::deps::locked_names(&m.root).iter().any(|n| n == name) {
                let _ = fs::remove_dir_all(&cache);
                println!("cache '.ray-deps/{name}' removed");
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
        eprintln!("no project: missing 'ray.toml' (to locate the index)");
        process::exit(64);
    };
    let index = match crate::deps::index_dir(&m) {
        Ok(Some(dir)) => dir,
        Ok(None) => {
            eprintln!("the package index is disabled ('index = \"\"' or empty RAY_INDEX)");
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
            eprintln!("could not list the index '{}': {e}", index.display());
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
        println!("no results in the index{}", if pattern.is_empty() { String::new() } else { format!(" for '{pattern}'") });
        return;
    }
    for name in &names {
        match crate::index::latest(&index, name) {
            Ok(v) => println!("{name} {v}"),
            Err(_) => println!("{name} (no installable version)"),
        }
    }
    println!("{} package(s)", names.len());
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
            "no publish key in '{}' (generate it with 'ray keygen', or point RAY_KEY)",
            path.display()
        )
    })?;
    let seed = crate::index::decode_ed25519(&format!("ed25519:{}", hex.trim()), "the key")?;
    if seed.len() != 32 {
        return Err(format!("the key of '{}' does not have 32 bytes", path.display()));
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
                eprintln!("--out requires a path");
                process::exit(64);
            }
        },
        Some((other, _)) => {
            eprintln!("unrecognized argument: '{other}' (usage: ray keygen [--out F])");
            process::exit(64);
        }
        None => key_path(),
    };
    if out.exists() {
        eprintln!("'{}' already exists (a key is not overwritten; delete it yourself if you really want another)", out.display());
        process::exit(65);
    }
    let seed = crate::builtins::crypto_random_bytes(32);
    if seed.len() != 32 {
        eprintln!("no CSPRNG available in this build");
        process::exit(70);
    }
    let Some(pk) = crate::builtins::ed25519_public_key(&seed) else {
        eprintln!("could not derive the public key");
        process::exit(70);
    };
    if let Some(parent) = out.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        eprintln!("could not create '{}': {e}", parent.display());
        process::exit(73);
    }
    if let Err(e) = fs::write(&out, format!("{}\n", hex_of(&seed))) {
        eprintln!("could not write '{}': {e}", out.display());
        process::exit(73);
    }
    println!("publish key generated at {}", out.display());
    println!("  pubkey: ed25519:{}", hex_of(&pk));
    println!("keep it safe: it is your publisher identity (the public one is fixed in the index on publish --sign).");
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
        .ok_or_else(|| "could not derive the public key".to_string())?;
    let my_pub = format!("ed25519:{}", hex_of(&pk));
    match crate::index::read_owners(index, name)? {
        Some(o) => {
            if o.pubkey != my_pub {
                return Err(format!(
                    "'{name}' already has a registered owner in the index and your key does NOT match \
                     ('{}.owners.toml'); if the name is yours, sign with the original key",
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
            println!("name '{name}' claimed in the index ('{name}.owners.toml') — commit it along with the entry");
        }
    }
    let msg = crate::index::signing_message(name, version, hash);
    let sig = crate::builtins::ed25519_sign(&seed, msg.as_bytes())
        .ok_or_else(|| "could not sign (build without ring?)".to_string())?;
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
                eprintln!("usage: ray index-verify <dir> (or run in a project with an index configured)");
                process::exit(64);
            }
        },
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        eprintln!("could not read the index '{}'", dir.display());
        process::exit(65);
    };
    let mut packages = 0usize;
    let mut versions = 0usize;
    let mut signed = 0usize;
    let mut problems: Vec<String> = Vec::new();
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
                        problems.push(err);
                    }
                }
            }
            Err(e) => problems.push(e),
        }
    }
    if problems.is_empty() {
        println!(
            "index OK: {packages} packages, {versions} versions ({signed} signed and verified)"
        );
    } else {
        for p in &problems {
            eprintln!("FALLO: {p}");
        }
        eprintln!("index with {} problem(s)", problems.len());
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
                    eprintln!("--repo requires a spec 'git+<URL>@<ref>'");
                    process::exit(64);
                }
            },
            "--sign" => sign = true, // M83c
            other => {
                eprintln!("unrecognized argument: '{other}' (usage: ray publish [--repo S] [--sign])");
                process::exit(64);
            }
        }
    }
    let Some(m) = load_manifest() else {
        eprintln!("no project: missing 'ray.toml' (create one with 'ray new')");
        process::exit(64);
    };
    // Validación: nombre válido (construye rutas en índice/caché, M51d) + version semver.
    if !crate::deps::valid_package_name(&m.name) {
        eprintln!("invalid package name '{}': only letters, digits, '-' and '_'", m.name);
        process::exit(65);
    }
    if crate::semver::parse_version(&m.version).is_none() {
        eprintln!("the package version '{}' is not valid semver: '{}'", m.name, m.version);
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
                "the package index is disabled ('index = \"\"' or empty RAY_INDEX): declare \
                 '[registry] index = \"<dir>\"' in ray.toml or export RAY_INDEX"
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
            println!("published {} {} in the index", m.name, m.version);
            println!("  git:  {git_spec}");
            println!("  hash: {hash}");
            if sig.is_some() {
                println!("  signature: ed25519 (owner in '{}.owners.toml')", m.name);
            }
            println!(
                "note: the index is a git repo; commit and push '{}.toml' to share it.",
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
    // M134: el clon vive como `<base>/<nombre>` para que el check cargue el paquete EXACTAMENTE
    // como lo verá un consumidor (`.ray-deps/<nombre>`): los imports internos calificados con el
    // nombre del paquete (`import greeting/shout;`) resuelven con el padre como raíz de módulos.
    let base = std::env::temp_dir().join(format!("ray-publish-{}-{}", m.name, process::id()));
    let tmp = base.join(&m.name);
    let _ = fs::remove_dir_all(&base);
    // Clon del repo LOCAL en la ref publicada (para --repo, la ref debe existir también aquí).
    crate::deps::fetch(&m.name, &crate::deps::GitSpec { url: m.root.to_string_lossy().into_owned(), git_ref: spec.git_ref.clone() }, &tmp)
        .map_err(|e| {
            format!(
                "could not obtain the content of ref '{}' from the local repo (the published \
                 content is validated and hashed from a clean clone): {e}",
                spec.git_ref
            )
        })?;
    let result = (|| {
        // La cara del paquete debe existir EN EL CLON (no solo en el working tree).
        let face = if tmp.join("mod.ray").is_file() { tmp.join("mod.ray") } else { tmp.join(&m.entry) };
        if !face.is_file() {
            return Err(format!(
                "the content of '{}' has no package face: missing 'mod.ray' (or the entry '{}'); \
                 did you forget to commit it before tagging?",
                spec.git_ref, m.entry
            ));
        }
        // Todos los .ray publicados deben lexear y parsear (también los no importados por la cara).
        let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
        crate::deps::collect_files(&tmp, &tmp, &mut files)?;
        for (rel, abs) in files.iter().filter(|(r, _)| r.ends_with(".ray")) {
            let src = fs::read_to_string(abs)
                .map_err(|e| format!("could not read '{rel}' of the published content: {e}"))?;
            let tokens = crate::lexer::lex(&src)
                .map_err(|e| format!("'{rel}' of the published content does not lex: {e}"))?;
            crate::parser::parse(tokens)
                .map_err(|e| format!("'{rel}' of the published content does not parse: {e}"))?;
        }
        // El hash, ANTES del check: resolver deps escribe `.ray-deps/`/`ray.lock` dentro del clon.
        let hash = crate::deps::hash_package(&tmp)?;
        check_published(&tmp, &face)?;
        Ok(hash)
    })();
    let _ = fs::remove_dir_all(&base);
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
                format!("could not resolve the package dependencies for the check: {e}")
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
    // M134/M135b: la cara se valida EXACTAMENTE como la verá un consumidor — con una entrada
    // SINTÉTICA que la importa desde `<base>` (el clon vive como `<base>/<nombre>`, la geometría
    // de `.ray-deps/`). Cargarla directamente como entrada era una geometría que ningún
    // consumidor ve: los módulos del paquete llegaban con nombres PELADOS (no namespacados) y
    // reglas como la de redefinición-de-builtin (que exime `M::f`) fallaban en falso — una
    // función privada `send` de un módulo del paquete es legal para todo consumidor real.
    let base = tmp.parent().unwrap_or(tmp);
    let pkg_name = tmp.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let import_path = if face.file_name().is_some_and(|n| n == "mod.ray") {
        pkg_name.clone()
    } else {
        let stem = face.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("{pkg_name}/{stem}")
    };
    let synth = format!("import {import_path};\n");
    let entry_path = base.join("__ray_publish_entry.ray");
    let mut loaded = crate::loader::load_source_module(&entry_path, &synth, base, &roots)
        .map_err(|e| format!("the package does not load: {}", e.message))?;
    let errors = crate::checker::check_all_modulo(&mut loaded.program);
    if let Some(e) = errors.first() {
        let (modulo, _source, linea, _col, _len) = loaded.locate(e.line, 1, 1);
        return Err(format!(
            "the package does not pass the semantic check ({modulo}.ray, line {linea}): {}",
            e.msg
        ));
    }
    Ok(())
}

/// Deriva la spec git de un paquete a publicar: `git+<origin>@v<version>`, tomando la URL del remoto
/// `origin` del repo en `root` y exigiendo que el tag `v<version>` exista (se publica un commit fijado).
fn derive_git_spec(root: &Path, version: &str) -> Result<String, String> {
    let origin = git_capture(root, &["remote", "get-url", "origin"]).map_err(|_| {
        "the package has no 'origin' remote (publish from a git repo with a remote, or pass \
         --repo 'git+<URL>@<ref>')"
            .to_string()
    })?;
    let origin = origin.trim();
    if origin.is_empty() {
        return Err("the 'origin' remote is empty; use --repo 'git+<URL>@<ref>'".to_string());
    }
    let tag = format!("v{version}");
    // El tag debe existir (se publica un punto fijo, no el working tree).
    git_capture(root, &["rev-parse", "--verify", "--quiet", &format!("refs/tags/{tag}")])
        .map_err(|_| format!("the tag '{tag}' does not exist in the repo; create it (git tag {tag}) before publish"))?;
    Ok(format!("git+{origin}@{tag}"))
}

/// Corre `git -C <cwd> <args>` y devuelve su stdout, o `Err` si el estado no es 0.
fn git_capture(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
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
        eprintln!("no project: missing 'ray.toml'");
        process::exit(64);
    };
    if m.dependencies.is_empty() {
        println!("'{}' declares no dependencies", m.name);
        return;
    }
    match crate::deps::update(&m) {
        Ok(_) => println!("dependencies updated to the newest compatible versions"),
        Err(e) => {
            eprintln!("error updating dependencies: {e}");
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
        eprintln!("usage: ray yank <name>@<version> [--undo]");
        process::exit(64);
    };
    let Some((name, see)) = spec.split_once('@') else {
        eprintln!("usage: ray yank <name>@<version> (the version is required)");
        process::exit(64);
    };
    let Some(m) = load_manifest() else {
        eprintln!("no project: missing 'ray.toml' (to locate the index)");
        process::exit(64);
    };
    let index = match crate::deps::index_dir(&m) {
        Ok(Some(dir)) => dir,
        Ok(None) => {
            eprintln!("the package index is disabled ('index = \"\"' or empty RAY_INDEX)");
            process::exit(65);
        }
        Err(e) => {
            eprintln!("{e}");
            process::exit(65);
        }
    };
    match crate::index::set_yanked(&index, name, see, !undo) {
        Ok(()) => {
            let verb = if undo { "restored" } else { "yanked" };
            println!("version {name} {see} {verb} in the index");
            println!("note: commit and push '{name}.toml' to share the change.");
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
        eprintln!("no project: missing 'ray.toml' with the dependencies to download");
        process::exit(64);
    };
    if m.dependencies.is_empty() {
        println!("'{}' declares no dependencies", m.name);
        return;
    }
    // `asegurar` resuelve el grafo COMPLETO (directas + transitivas) y devuelve cuántas descargó.
    match crate::deps::ensure(&m) {
        Ok(0) => println!("dependencies up to date"),
        Ok(n) => println!("{n} dependency(ies) downloaded (including transitive)"),
        Err(e) => {
            eprintln!("error downloading dependencies: {e}");
            process::exit(65);
        }
    }
}

/// `ray fmt <archivo>`: imprime la versión canónica por stdout.
/// `ray fmt <file>… [--write]`. Sin `--write` imprime la versión canónica a stdout (un solo archivo,
/// el comportamiento de siempre); con `--write` la escribe EN EL SITIO, y admite varios archivos
/// (`ray fmt --write src/*.ray`). No recorre directorios: qué extensiones entran y qué se ignora es
/// una decisión propia, y el glob del shell ya cubre el caso común.
fn cmd_fmt(args: &[String]) {
    let write = args.iter().any(|a| a == "--write" || a == "-w");
    let paths: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    if paths.is_empty() {
        eprintln!("usage: ray fmt <file>... [--write]");
        process::exit(64);
    }
    if !write {
        if paths.len() > 1 {
            eprintln!("usage: ray fmt <file> (several files require --write)");
            process::exit(64);
        }
        format_file(paths[0]);
        return;
    }
    let mut changed = 0usize;
    for path in &paths {
        // Solo se reescribe si el texto CAMBIA: así `--write` no toca el mtime de lo que ya está
        // canónico (importante para make/watchers y para que el diff de un repo formateado sea vacío).
        let source = read_source(path);
        let formatted = format_source_of(path, &source);
        if formatted != source {
            if let Err(e) = std::fs::write(path.as_str(), &formatted) {
                eprintln!("format error: could not write {}: {}", path, e);
                process::exit(74);
            }
            println!("formatted {}", path);
            changed += 1;
        }
    }
    if changed == 0 {
        println!("already formatted ({} file(s))", paths.len());
    }
}

// M40.4: `ray doc <archivo>` imprime la documentación Markdown de la superficie pública del archivo.
/// `ray templ <ruta>...`: compila cada template `.ray.html` (o todos los de un directorio,
/// recursivo) a su módulo raylang generado (`.ray` al lado, commiteable). M55.
/// `ray build --templates-only [ruta...]` (M99, antes el subcomando `ray templ` de M55): compila los
/// `.ray.html` dados —archivos o directorios, recursivo— a módulos raylang tipados. Sin rutas usa la
/// raíz del proyecto (`ray.toml`) o el directorio actual. Regenera SIEMPRE, sin mirar mtimes.
fn build_templates(args: &[String]) {
    // Sin rutas: la raíz del proyecto, o `.` si se invoca fuera de un proyecto.
    let default_root = load_manifest()
        .map(|m| m.root.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string());
    let owned: Vec<String>;
    let args: &[String] = if args.is_empty() {
        owned = vec![default_root];
        &owned
    } else {
        args
    };
    let mut entries: Vec<PathBuf> = Vec::new();
    for a in args {
        let p = Path::new(a);
        if p.is_dir() {
            collect_templates(p, &mut entries);
        } else if a.ends_with(".ray.html") {
            entries.push(p.to_path_buf());
        } else {
            eprintln!("'{a}' is not a .ray.html nor a directory");
            process::exit(64);
        }
    }
    entries.sort();
    if entries.is_empty() {
        eprintln!("no .ray.html templates found");
        process::exit(64);
    }
    for e in &entries {
        match crate::templ::generate_file(e) {
            Ok(out) => println!("generated: {}", out.display()),
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

// M102: la regeneración automática de templates (M55, `regen_stale_templates`) desapareció — el
// loader compila cada `.ray.html` EN MEMORIA al resolver su import; no hay `.ray` generado en el
// proyecto ni staleness por mtime. `ray build --templates-only` sigue materializando el generado
// bajo demanda (inspección).

fn cmd_doc(args: &[String]) {
    let Some(path) = args.first() else {
        eprintln!("usage: ray doc <file>");
        process::exit(64);
    };
    let title = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);
    match crate::raydoc::generate(&read_source(path), title) {
        Ok(md) => print!("{md}"),
        Err(e) => {
            eprintln!("documentation error: {e}");
            process::exit(65);
        }
    }
}

// ── Modo legado (compatibilidad con la interfaz por flags) ───────────────────────────

fn legacy(rest: &[String]) {
    // M99: un subcomando que SE MOVIÓ cae aquí y se interpretaría como nombre de archivo, con un
    // "could not read module 'publish'" que no dice nada. Se intercepta antes para señalar el destino.
    // (raylang no está publicado: son redirecciones de cortesía, no alias — el comando viejo NO corre.)
    if let Some(first) = rest.first() {
        let moved = match first.as_str() {
            "publish" => Some("ray registry publish"),
            "yank" => Some("ray registry yank"),
            "keygen" => Some("ray registry keygen"),
            "index-verify" => Some("ray registry verify"),
            "templ" => Some("ray build --templates-only"),
            _ => None,
        };
        if let Some(dest) = moved {
            eprintln!("'ray {first}' moved: use `{dest}` (see `ray help`)");
            process::exit(64);
        }
    }
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
    // SPIKE P2.b: emite el código Rust del subconjunto soportado a stdout (evaluación de rendimiento).
    if rest.len() == 2 && rest[0] == "--emit-rust" {
        emit_rust(&rest[1]);
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
        eprintln!("usage: ray <subcommand>   (ray help for the list)   |   ray run <file>");
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
            eprintln!("compiling {} v{}", m.name, m.version);
        }
        // Auto-descarga (M39c-2a, estilo cargo): asegura que las dependencias declaradas estén en
        // `.ray-deps/` antes de cargar el programa. Las presentes se saltan (sin red); si falta
        // alguna se clona de git. Un fallo de descarga aborta con 65 (no se puede compilar sin ella).
        if !m.dependencies.is_empty()
            && let Err(e) = crate::deps::ensure(m)
        {
            eprintln!("error resolving dependencies: {e}");
            process::exit(65);
        }
    }
    if let Some(p) = explicit {
        return p.to_string();
    }
    if let Some(m) = &manifest {
        let entry = m.entry_path();
        if !entry.is_file() {
            eprintln!("the manifest '{}' points to a nonexistent entry: '{}'", m.name, entry.display());
            process::exit(66);
        }
        return entry.to_string_lossy().into_owned();
    }
    let def = "src/main.ray";
    if Path::new(def).exists() {
        def.to_string()
    } else {
        eprintln!("no file given and no project (missing 'ray.toml' or 'src/main.ray')");
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
            eprintln!("could not read '{}': {}", path, e);
            process::exit(66); // EX_NOINPUT
        }
    }
}

/// Runner de `@test` (M10.1; sobre el loader desde M101): sale con 0 (verde), 1 (fallos) o 65
/// (no compila). Interfaz legada `--test <archivo>`: una sola suite, con la raíz del archivo y
/// la caché de dependencias como raíces del loader.
fn run_tests(path: &str, filter: Option<&str>) {
    if !Path::new(path).is_file() {
        eprintln!("could not read '{}': no such file", path);
        process::exit(66); // EX_NOINPUT
    }
    process::exit(test_runner::run(&[PathBuf::from(path)], &dependency_roots(), filter));
}

/// Formateador (M29.2): imprime la versión canónica o aborta con el error.
/// SPIKE P2.b: carga + chequea + transpila a Rust e imprime el resultado (o el error del spike).
fn emit_rust(path: &str) {
    let (mut program, locate, multi) = load_and_locate(path);
    check_or_exit(&mut program, &locate, multi);
    match crate::transpile::transpile(&program) {
        Ok(t) => print!("{}", t.source),
        Err(e) => {
            eprintln!("{}", e);
            process::exit(65);
        }
    }
}

fn format_file(path: &str) {
    print!("{}", format_source_of(path, &read_source(path)));
}

/// El texto canónico de `source` según la extensión de `path`. Sale con 65 si no se puede formatear
/// (mismo código que el resto de errores de compilación). Lo comparten `ray fmt` y `ray fmt --write`.
fn format_source_of(path: &str, source: &str) -> String {
    let unit = resolve_indent(std::path::Path::new(path));
    // M55: un template `.ray.html` se formatea con SU formateador (etiquetas en su línea +
    // indentación por bloques del template), no con el de raylang.
    if path.ends_with(".ray.html") {
        match crate::templ::format_template(source, &unit) {
            Some(out) => return out,
            None => {
                eprintln!("format error: the template does not tokenize (unterminated delimiter)");
                process::exit(65);
            }
        }
    }
    match crate::fmt::format_source_with_indent(source, &unit) {
        Ok(out) => out,
        Err(e) => {
            eprintln!("format error: {}", e);
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

/// Un localizador de posiciones globales `(línea, col, len)` → `(fuente del módulo, nombre, línea
/// local, col, len)`, para renderizar errores contra el archivo correcto en programas multi-módulo
/// (L3). `col`/`len` viajan por él porque un módulo-TEMPLATE (M102-A2) los reescribe: la línea se
/// traduce al `.ray.html`, la fuente devuelta es la del template y el cursor degrada a línea
/// completa (col 1) — usar SIEMPRE los valores devueltos.
type Locate = Box<dyn Fn(usize, usize, usize) -> (String, String, usize, usize, usize)>;

/// Carga el archivo de entrada y sus imports (loader, M11.3), devolviendo el programa
/// fusionado, un localizador de posiciones y si hay más de un módulo.
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
    let locate: Locate = Box::new(move |gline: usize, col: usize, len: usize| {
        let m = modules.iter().rev().find(|m| m.start_line <= gline).unwrap_or(&modules[0]);
        // `saturating_sub`: una posición fallback `(0,0)` (p.ej. un error de runtime sin línea concreta,
        // como el deadlock declarado por un worker ocioso en M:N, o el fallback de fuel) da `gline < start_line`
        // → sin esto, restar underflowaría (usize). Para posiciones válidas (`gline >= start_line`) es idéntico.
        let local = gline.saturating_sub(m.start_line) + 1;
        let (source, local, col, len) = m.present(local, col, len);
        (source.to_string(), m.name.clone(), local, col, len)
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
            let (source, name, local, col, len) = locate(e.line, e.col, e.len);
            e.line = local;
            e.col = col;
            let head = if multi { format!("[{}] {}", name, e) } else { e.to_string() };
            eprintln!("{}", diagnostic::render(&source, local, col, len, &head));
        }
        process::exit(65);
    }
}

/// Carga, chequea y ejecuta un archivo (VM por defecto, `--interp` para el intérprete).
fn run_file(path: &str, prog_args: Vec<String>, use_interp: bool, fuel: Option<u64>, heap: Option<usize>) {
    if (fuel.is_some() || heap.is_some()) && use_interp {
        eprintln!("--fuel/--heap are VM limits (product engine); they do not apply with --interp");
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
            eprintln!("this build does not include the interpreter (compiled with --no-default-features); run on the VM, without --interp");
            process::exit(64);
        }
    } else {
        match compiler::compile_program(&program) {
            Ok(compiled) => vm::run_program_with_limit(&compiled, fuel, heap),
            Err(mut e) => {
                let (source, name, local, col, len) = locate(e.line, e.col, 1);
                e.line = local;
                e.col = col;
                let head = if multi { format!("[{}] {}", name, e) } else { e.to_string() };
                eprintln!("{}", diagnostic::render(&source, local, col, len, &head));
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
            let (source, name, local, col, len) = locate(e.line, e.col, 1);
            e.line = local;
            e.col = col;
            let head = if multi { format!("[{}] {}", name, e) } else { e.to_string() };
            eprintln!("{}", diagnostic::render(&source, local, col, len, &head));
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
        let (source, name, local, _col, _len) = locate(f.line, f.col, 1);
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
        // `col` sale del locate: en un módulo-template es 1 (la columna del generado no existe
        // en el `.ray.html`); en uno normal es la de la traza (M102-A2).
        let (source, name, local, col, _len) = locate(f.line, f.col, 1);
        if local > source.lines().count() {
            format!("  {} {} (prelude:{}:{})", prefix, f.name, f.line, f.col)
        } else {
            format!("  {} {} ({}:{}:{})", prefix, f.name, name, local, col)
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
        out.push(render_frame(if i == 0 { "en" } else { "from" }, f));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(feature = "watch", unix))]
    #[test]
    fn dev_event_relevance_matches_the_scan_criteria() {
        let dir = std::env::temp_dir().join(format!("ray-dev-pred-{}", std::process::id()));
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        let root = fs::canonicalize(&dir).unwrap();
        // Fuentes vigilados: .ray, .ray.html y el manifiesto.
        assert!(is_watched_source(&root, &root.join("src/main.ray")));
        assert!(is_watched_source(&root, &root.join("pages/index.ray.html")));
        assert!(is_watched_source(&root, &root.join("ray.toml")));
        // Fuera: otras extensiones (incluye temporales de guardado atómico), artefactos, ocultos
        // y rutas ajenas a la raíz.
        assert!(!is_watched_source(&root, &root.join("src/main.ray.tmp")));
        assert!(!is_watched_source(&root, &root.join("notes.md")));
        assert!(!is_watched_source(&root, &root.join("target/debug/x.ray")));
        assert!(!is_watched_source(&root, &root.join(".git/x.ray")));
        assert!(!is_watched_source(&root, Path::new("/elsewhere/main.ray")));
        // La regla del generado: un .ray con .ray.html hermano es artefacto, no fuente.
        fs::write(src.join("page.ray.html"), "<b>x</b>").unwrap();
        fs::write(src.join("page.ray"), "// generado").unwrap();
        assert!(!is_watched_source(&root, &root.join("src/page.ray")));
        assert!(is_watched_source(&root, &root.join("src/page.ray.html")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dev_polling_fallback_still_detects_changes() {
        // La variante de respaldo (builds --without watch / no-unix) se prueba directo.
        let dir = std::env::temp_dir().join(format!("ray-dev-poll-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("main.ray"), "fn main() {}\n").unwrap();
        let mut w = DevWatcher::Polling;
        let mut snapshot = scan_sources(&dir);
        assert_eq!(w.wait_change(&dir, &mut snapshot), None, "sin cambios no hay cambio");
        fs::write(dir.join("main.ray"), "fn main() { print(1); }\n").unwrap();
        let change = w.wait_change(&dir, &mut snapshot);
        assert!(change.is_some_and(|c| c.contains("main.ray")), "el cambio debe detectarse");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn upgrade_asset_matches_release_scheme() {
        // M137: el nombre del asset debe coincidir con lo que publica release.yml.
        assert_eq!(
            upgrade_asset("macos", "aarch64").as_deref(),
            Some("raylang-aarch64-apple-darwin.tar.gz")
        );
        assert_eq!(
            upgrade_asset("linux", "x86_64").as_deref(),
            Some("raylang-x86_64-unknown-linux-gnu.tar.gz")
        );
        // Windows va por zip manual; una arquitectura desconocida tampoco tiene asset.
        assert_eq!(upgrade_asset("windows", "x86_64"), None);
        assert_eq!(upgrade_asset("linux", "riscv64"), None);
    }

    #[test]
    fn upgrade_tag_from_latest_redirect() {
        // GitHub redirige `releases/latest` a `releases/tag/<tag>`.
        let url = "https://github.com/ray-language/raylang/releases/tag/v1.2.0";
        assert_eq!(tag_from_latest_url(url).as_deref(), Some("v1.2.0"));
        assert_eq!(tag_from_latest_url("https://github.com/o/r/releases/tag/v2.0.0/"), Some("v2.0.0".into()));
        // Sin releases no hay redirección a un tag → None (error claro aguas arriba).
        assert_eq!(tag_from_latest_url("https://github.com/o/r/releases"), None);
        assert_eq!(tag_from_latest_url("https://github.com/o/r/releases/tag/"), None);
    }
}
