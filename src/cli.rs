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
//!   `ray test [archivo]`    — corre las funciones `@test` (M10.1); `--watch` re-corre ante cambios (M140).
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
        Some("bundle") => cmd_bundle(&rest[1..]),
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
        Some("toolchain") => crate::toolchain::run(&rest[1..]),
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
  build [file]      check and compile without running (0 ok / 65 error) [--native [-o out] [--release] [--fast] [--target triple] [--without crypto,tls,sqlite,mimalloc,ahash,regex,fibers,process,watch,audio,ui] [--embed dirs] [--lib]] [--templates-only [path...]]
  bundle [file]     package an app (M147c): --release native build + .app (macOS) / dir + .desktop (Linux) / dir + .exe with icon, version info and a .lnk shortcut (Windows; no console window); --ios (§80b) generates an Xcode project instead (WKWebView shell + device/simulator static libs; excludes process,audio; --ios-target device|sim|both picks which libs to build — both by default, the other side's lib is preserved) [--name N] [--icon icon.png] [--id com.x.y] [-o dir] [--without list]. NOTE: a bundled app launches with cwd=/ — embed its assets ([native] embed); unsigned apps downloaded on macOS 15+ need approval in System Settings > Privacy & Security (no signing/notarization in v1)
  test [file]       run the project's @test functions (entry modules + tests/*.ray) [filter] [--watch]
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
  toolchain <cmd>   Rust toolchain for `build --native` (M171): `install [--rust ch] [--force] [--no-vendor]` sets up a private rustup under ~/.ray/toolchain (+ the release's ray-runtime vendor, so the first build needs no network); `status` shows which cargo/rustc a native build would use (RAY_CARGO/RAY_RUSTC → PATH → private), the system linker and the vendor
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
/// `release.yml` y consumen `install.sh`/`install.ps1`): tar.gz en unix, zip en Windows (M165).
/// M185: Windows ARM64 tiene build propia — las cuatro plataformas van en arm64 y x86_64.
/// `None` = plataforma sin asset. Pura para poder testearla; el llamador pasa los `cfg!` reales.
fn upgrade_asset(os: &str, arch: &str) -> Option<String> {
    let (suffix, ext) = match os {
        "macos" => ("apple-darwin", "tar.gz"),
        "linux" => ("unknown-linux-gnu", "tar.gz"),
        "windows" => ("pc-windows-msvc", "zip"),
        _ => return None,
    };
    let arch = match arch {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    Some(format!("raylang-{arch}-{suffix}.{ext}"))
}

/// M187: el aviso de que este `ray` corre EMULADO sobre una máquina cuya arquitectura ya tiene
/// build propia (Windows ARM64 ejecutando la x86_64: M184/M185). `upgrade` no cambia de
/// arquitectura —eso es reinstalar—, pero sí lo dice, o el usuario se queda emulado para siempre.
/// `None` cuando no hay nada que contar. Pura: el llamador pasa las dos arquitecturas.
fn emulated_arch_note(os: &str, process_arch: &str, machine_arch: &str) -> Option<String> {
    if process_arch == machine_arch {
        return None;
    }
    let native = upgrade_asset(os, machine_arch)?;
    let reinstall = if os == "windows" {
        "irm https://raylang.dev/install.ps1 | iex"
    } else {
        "curl -sSfL https://raylang.dev/install.sh | sh"
    };
    Some(format!(
        "note: this is the {process_arch} build running on a {machine_arch} machine (emulated), and \
         {native} is published\n      'ray upgrade' keeps the current architecture — reinstall to \
         switch: {reinstall}"
    ))
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
/// válido con el binario en ejecución). En Windows (M165) el zip lo abre el `tar` del sistema
/// (bsdtar, Windows 10+) y el `.exe` en ejecución se APARTA a `.old` antes de colocar el nuevo:
/// no se puede sobrescribir, pero sí renombrar.
fn cmd_upgrade(args: &[String]) {
    let (check, rest) = take_flag_bool(args, "--check");
    if rest.len() > 1 {
        eprintln!("usage: ray upgrade [tag] [--check]");
        process::exit(64);
    }
    let repo = upgrade_repo();
    // M187: `upgrade` conserva la arquitectura A PROPÓSITO — actualiza lo instalado, no lo cambia
    // por otra cosa. Pero callar que existe una build nativa dejaría al usuario emulado para
    // siempre (solo una reinstalación cambia de arquitectura), así que se dice.
    if let Some(note) = emulated_arch_note(std::env::consts::OS, std::env::consts::ARCH, crate::toolchain::machine_arch()) {
        eprintln!("{note}");
    }
    let Some(asset) = upgrade_asset(std::env::consts::OS, std::env::consts::ARCH) else {
        eprintln!(
            "ray upgrade does not support this platform ({}-{}); see the assets in \
             https://github.com/{repo}/releases",
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
            let null_dev = if cfg!(windows) { "NUL" } else { "/dev/null" };
            let effective = sh_capture(
                "curl",
                &["-sSfL", "-o", null_dev, "-w", "%{url_effective}", &latest_url],
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
    // `-xzf` para el tar.gz; el zip de Windows lo abre `tar -xf` (bsdtar detecta el formato).
    let untar = if asset.ends_with(".zip") { "-xf" } else { "-xzf" };
    if let Err(e) = sh_capture("tar", &[untar, &archive.to_string_lossy()], Some(&tmp)) {
        eprintln!("could not unpack '{asset}': {e}");
        cleanup(69);
    }
    let exe = std::env::consts::EXE_SUFFIX;
    let bins = [format!("ray{exe}"), format!("raylang{exe}")];

    // Verificar el binario ANTES de reemplazar nada: debe correr y reportar la versión pedida.
    match sh_capture(&tmp.join(&bins[0]).to_string_lossy(), &["version"], None) {
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
    for bin in &bins {
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
            #[cfg(windows)]
            {
                // El .exe que está corriendo (este mismo) no admite sobrescritura, pero sí
                // renombrado: se aparta a `.old` y se limpia después (o en el próximo upgrade).
                let old = install_dir.join(format!("{bin}.old"));
                let _ = fs::remove_file(&old);
                if dest.exists() {
                    fs::rename(&dest, &old).map_err(|e| e.to_string())?;
                }
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
    #[cfg(windows)]
    for bin in &bins {
        let _ = fs::remove_file(install_dir.join(format!("{bin}.old")));
    }
    let _ = fs::remove_dir_all(&tmp);
    println!("installed: raylang {current} → {target} ({})", install_dir.join(&bins[0]).display());
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
/// El reinicio manda **SIGTERM** (Windows: `CTRL_BREAK` al grupo del hijo, M172) — un servidor con
/// `serve_graceful` (M88.1b) drena sus conexiones antes de morir — y escala al kill duro a los 3 s. Un programa que termina solo (un CLI, un crash)
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
    // Retiene el socket durante toda la sesión (vive hasta que `ray dev` muere). Unix (fd
    // heredado) y Windows (handle heredable, M172); ver `dev_host::pass_listener`.
    let dev_sock = listen_addr.as_ref().and_then(|addr| {
        if !crate::dev_host::supports_socket_activation() {
            eprintln!("[dev] --port/--listen (socket-activation) is not supported on this platform; ignoring");
            return None;
        }
        match std::net::TcpListener::bind(addr) {
            Ok(l) => {
                eprintln!("[dev] holding {addr} across restarts (socket-activation)");
                Some(l)
            }
            Err(e) => {
                eprintln!("[dev] could not pre-open {addr}: {e}; restarts will re-bind normally");
                None
            }
        }
    });
    // El socket vive mientras `dev_sock` no se dropee: toda la sesión.
    let listen_pair = dev_sock.as_ref().zip(listen_addr.as_deref());

    // Live-reload del navegador (M92.4): el hub SSE emite `reload` en cada reinicio; el webserver,
    // viendo `RAY_DEV_RELOAD`, inyecta el snippet en las respuestas HTML. Arranca SIEMPRE: detectar
    // "es una app web" no es asunto del supervisor — la inyección ya vive en el webserver (solo dispara
    // al servir text/html; un programa CLI nunca inyecta nada y el hub ocioso cuesta un hilo). Sin
    // `--port` no hay socket retenido, así que el snippet reintenta hasta que el hijo re-binde.
    let reload = start_reload_hub();
    let reload_port = reload.as_ref().map(|(_, p)| *p);
    if let Some(p) = reload_port {
        // La coletilla importa: para una app de consola el hub es INERTE (solo el webserver
        // inyecta el snippet, y solo al servir HTML) — sin ella la línea confunde en una TUI.
        eprintln!("[dev] web live-reload on http://127.0.0.1:{p} (only used when the app serves HTML)");
    }

    // La entrada que el hijo usará (para el check-before-restart): se despojan los flags de `run`
    // (mismos que `cmd_run`), y el primer resto es el archivo explícito (o `None` → default del proyecto).
    let entry = dev_entry(&fwd_args);
    // M147: los dirs de `[native] embed` también se vigilan — un cambio ahí no reinicia (la
    // lectura de std/embed es en vivo): solo se recarga el navegador vía el hub.
    let embed_dirs: Vec<PathBuf> = Manifest::load(&root)
        .ok()
        .flatten()
        .map(|m| m.native_embed.iter().map(PathBuf::from).collect())
        .unwrap_or_default();
    let watching_embed = !embed_dirs.is_empty();
    let _ = DEV_EMBED_DIRS.set(embed_dirs);
    if watching_embed {
        eprintln!(
            "[dev] watching {} (.ray, .ray.html, ray.toml + embedded assets); Ctrl-C to exit",
            root.display()
        );
    } else {
        eprintln!("[dev] watching {} (.ray, .ray.html, ray.toml); Ctrl-C to exit", root.display());
    }
    install_cleanup_on_death();

    let mut snapshot = scan_sources(&root);
    let mut hashes = content_hashes(&snapshot);
    let mut watcher = DevWatcher::new(&root);
    // Huella del termios de stdin ANTES del primer hijo. Una app INTERACTIVA (una TUI) entra a
    // modo crudo y la huella cambia; muestrearla mientras el hijo corre distingue "el usuario
    // cerró la app con su propia tecla" (→ `ray dev` sale con ella, cero teclas extra) de "un
    // script terminó" (→ esperar cambios y re-correr, el contrato del modo watch).
    let baseline_tty = crate::builtins::term_attrs_fingerprint();
    let mut child = spawn_dev_child(&exe, &fwd_args, listen_pair, reload_port);
    let mut running = true;
    // ¿El hijo en curso cambió el terminal alguna vez? (reset en cada relanzamiento)
    let mut interactive_child = false;
    // ¿El supervisor tiene stdin en modo crudo, escuchando la tecla de salir? Solo sin hijo vivo.
    let mut keys_armed = false;
    loop {
        // Vigila hasta el próximo cambio; si el programa termina solo, sigue vigilando sin él.
        let change = loop {
            if let Some((_, label)) = watcher.wait_change(&root, &mut snapshot) {
                break label;
            }
            if running {
                // Mientras el hijo corre: ¿tocó el terminal? (una TUI entra a crudo). El
                // muestreo cada ~200 ms es un tcgetattr — gratis.
                if !interactive_child
                    && let (Some(base), Some(now)) =
                        (baseline_tty.as_ref(), crate::builtins::term_attrs_fingerprint())
                    && now != *base
                {
                    interactive_child = true;
                }
                if let Ok(Some(status)) = child.try_wait() {
                    running = false;
                    // Una app INTERACTIVA que salió limpia la cerró el usuario con su propia
                    // tecla: `ray dev` sale con ella — pedir OTRA tecla para salir del
                    // supervisor es fricción sin sentido. Si crasheó (status != 0), sí se
                    // espera: el usuario va a editar el fix y quiere el relanzamiento.
                    // El `\r` inicial de estos mensajes importa: una TUI deja el cursor en
                    // cualquier columna al salir — sin él, la línea aparece "tabulada".
                    let windowed = reload
                        .as_ref()
                        .is_some_and(|(h, _)| h.ui_child.load(std::sync::atomic::Ordering::SeqCst));
                    if (interactive_child || windowed) && status.success() {
                        if windowed {
                            eprintln!("\r[dev] the window closed; bye");
                        } else {
                            eprintln!("\r[dev] the program exited; bye");
                        }
                        std::process::exit(0);
                    }
                    // Tecla-única para el resto (un script en bucle de edición): el terminal es
                    // del supervisor y entra a CRUDO — una sola `q` sale, sin Enter. El hint va
                    // ANTES del raw_on (en crudo, `\n` baja sin retornar carro y la línea
                    // siguiente hereda la columna).
                    let tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
                    if tty {
                        eprintln!("\r[dev] the program finished ({status}); waiting for changes… (press q to exit)");
                    } else {
                        eprintln!("\r[dev] the program finished ({status}); waiting for changes… (q⏎ or Ctrl-C exits)");
                    }
                    keys_armed = tty && crate::builtins::term_raw_on().is_ok();
                }
            }
            if keys_armed && dev_raw_key_quit() {
                let _ = crate::builtins::term_raw_off();
                eprintln!("\r[dev] bye");
                std::process::exit(0);
            }
            if !running && !keys_armed && dev_stdin_quit() {
                eprintln!("[dev] bye");
                std::process::exit(0);
            }
        };
        // Hubo cambio: el terminal vuelve a modo normal ANTES de imprimir nada más o relanzar
        // (el hijo debe heredar un terminal sano; en crudo los mensajes se escalonan).
        if keys_armed {
            let _ = crate::builtins::term_raw_off();
            keys_armed = false;
        }
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
        // M147: ¿el cambio es SOLO de assets embebidos? Reiniciar sería inútil (std/embed lee
        // en vivo) — basta recargar el navegador. Se decide sobre el DIFF real de hashes (una
        // ráfaga que mezcle un .ray y un asset debe reiniciar, y el reinicio ya recarga).
        let assets_only = {
            let touched = current_hashes
                .iter()
                .filter(|(p, h)| hashes.get(*p) != Some(h))
                .map(|(p, _)| p)
                .chain(hashes.keys().filter(|p| !current_hashes.contains_key(*p)));
            let mut any = false;
            let mut all_assets = true;
            for p in touched {
                any = true;
                if !is_embed_asset(&root, p) {
                    all_assets = false;
                }
            }
            any && all_assets
        };
        hashes = current_hashes;
        if assets_only {
            eprintln!("[dev] change in {change}: embedded asset — reloading the browser (no restart)");
            if running
                && let Some((hub, _)) = &reload
            {
                hub.broadcast();
            }
            continue;
        }
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
            // Cinturón: si el hijo murió dejando el terminal cambiado (TUI en crudo matada
            // por SIGKILL, crash), reponer la baseline ANTES de relanzar — el hijo nuevo
            // guardaría el termios envenenado como su "original" y lo perpetuaría.
            if let (Some(base), Some(now)) =
                (baseline_tty.as_ref(), crate::builtins::term_attrs_fingerprint())
                && now != *base
            {
                let _ = crate::builtins::term_attrs_restore(base);
            }
        }
        // Arma la recarga ANTES de relanzar: el hub la emitirá cuando el hijo avise `/ready` (el
        // webserver, al bindear) → el navegador recarga justo cuando el servidor nuevo ya escucha,
        // sin un fetch de sondeo que falle mientras re-binde.
        if let Some((hub, _)) = &reload {
            hub.arm();
        }
        if let Some((hub, _)) = &reload {
            hub.ui_child.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        child = spawn_dev_child(&exe, &fwd_args, listen_pair, reload_port);
        running = true;
        interactive_child = false;
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
/// Si `listen` está (socket-activation M92.3): el socket retenido del supervisor se pasa al hijo
/// (`dev_host::pass_listener`: fd 3 en unix, handle heredable en Windows) con
/// `RAY_LISTEN_FD`/`RAY_LISTEN_ADDR` → el hijo lo ADOPTA en `tcp_listen`.
fn spawn_dev_child(
    exe: &Path,
    args: &[String],
    listen: Option<(&std::net::TcpListener, &str)>,
    reload_port: Option<u16>,
) -> process::Child {
    let mut cmd = process::Command::new(exe);
    cmd.arg("run").args(args);
    // M92.4: el hijo aprende el puerto del hub de live-reload; el webserver inyecta el snippet SSE.
    if let Some(p) = reload_port {
        cmd.env("RAY_DEV_RELOAD", p.to_string());
    }
    if let Some((listener, addr)) = listen {
        crate::dev_host::pass_listener(&mut cmd, listener, addr);
    }
    spawn_supervised(cmd, "[dev] could not launch the program")
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
    /// M147b: el hijo en curso abrió una VENTANA (`GET /ui` de ray_runtime::ui) — al salir
    /// limpio, el usuario cerró la app y `ray dev` sale con ella (el contrato TUI de M139).
    /// Se resetea en cada relanzamiento.
    ui_child: std::sync::atomic::AtomicBool,
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
        ui_child: std::sync::atomic::AtomicBool::new(false),
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
            // M147b: el aviso "soy una app con ventana" de ray_runtime::ui (una vez por hijo).
            if buf[..n].starts_with(b"GET /ui") {
                hub_bg.ui_child.store(true, std::sync::atomic::Ordering::SeqCst);
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
    #[cfg(all(feature = "watch", any(unix, windows)))]
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
        #[cfg(all(feature = "watch", any(unix, windows)))]
        {
            let canon_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
            // M181 (Windows): `canonicalize` da `\\?\C:\…` (y la forma LARGA de un `TEMP` en 8.3,
            // `RUNNER~1` en el runner de CI); notify entrega rutas llanas bajo la ruta que se le
            // dio. Se vigila la raíz ya canónica y sin prefijo: así los eventos y `canon_root`
            // hablan la misma forma y `is_watched_source` los reconoce.
            let canon_root = match canon_root.to_string_lossy().strip_prefix(r"\\?\") {
                Some(plain) => PathBuf::from(plain),
                None => canon_root,
            };
            match ray_runtime::watch::watch(&canon_root.display().to_string()) {
                Ok(watcher) => {
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

    /// Un paso de espera (~200 ms de cota): `Some((ruta, descripción))` si hubo un cambio
    /// relevante, con `snapshot` ya actualizado. La ruta alimenta la selección de suites del
    /// watch de tests (M141); la descripción es para el usuario. La cota corta permite al
    /// llamador vigilar también al hijo (`try_wait`) sin hilo aparte.
    fn wait_change(
        &mut self,
        root: &Path,
        snapshot: &mut Vec<(PathBuf, std::time::SystemTime)>,
    ) -> Option<(PathBuf, String)> {
        match self {
            #[cfg(all(feature = "watch", any(unix, windows)))]
            DevWatcher::Events { watcher, canon_root } => {
                match watcher.next_timeout(200) {
                    Ok(Some((_kind, path))) => {
                        let path = PathBuf::from(path);
                        if !is_watched_source(canon_root, &path) {
                            return None;
                        }
                        *snapshot = scan_sources(root);
                        let deleted = if path.exists() { "" } else { " (deleted)" };
                        let label = format!("{}{deleted}", path.display());
                        Some((path, label))
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
                change.map(|(path, deleted)| {
                    let suffix = if deleted { " (deleted)" } else { "" };
                    let label = format!("{}{suffix}", path.display());
                    (path, label)
                })
            }
        }
    }

    /// Coalesce la ráfaga de un guardado: espera a que pasen ~120 ms sin eventos RELEVANTES
    /// (los irrelevantes — artefactos, ocultos — no alargan la espera) y deja el snapshot al día.
    fn debounce(&mut self, root: &Path, snapshot: &mut Vec<(PathBuf, std::time::SystemTime)>) {
        match self {
            #[cfg(all(feature = "watch", any(unix, windows)))]
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

/// ¿Llegó la tecla de salir con el stdin del supervisor en modo CRUDO? Un byte por pulsación:
/// `q`/`Q` salen; en crudo ISIG está apagado, así que Ctrl-C (0x03) y Ctrl-D (0x04) llegan como
/// bytes y se honran igual (el hint promete Ctrl-C). Cualquier otra tecla se ignora.
fn dev_raw_key_quit() -> bool {
    match crate::poll::wait(&[0], &[], 0) {
        crate::poll::PollResult::Ready(fds) if fds.contains(&0) => {
            let mut b = [0u8; 1];
            match std::io::Read::read(&mut std::io::stdin().lock(), &mut b) {
                Ok(0) => true, // EOF
                Ok(_) => matches!(b[0], b'q' | b'Q' | 0x03 | 0x04),
                Err(_) => false,
            }
        }
        _ => false,
    }
}

/// Respaldo en modo COOKED (stdin no es terminal o el modo crudo falló): solo aplica SIN hijo en
/// marcha (jamás se compite por el stdin de un programa vivo) y SOLO con stdin en un terminal —
/// en un pipe o CI, stdin cerrado daría EOF inmediato y `ray dev` moriría al primer programa
/// terminado; el modo watch debe seguir esperando cambios ahí. Sondea sin bloquear y consume la
/// línea disponible: `q` o EOF (Ctrl-D) terminan; cualquier otra línea se ignora.
fn dev_stdin_quit() -> bool {
    use std::io::{BufRead, IsTerminal};
    if !std::io::stdin().is_terminal() {
        return false;
    }
    match crate::poll::wait(&[0], &[], 0) {
        crate::poll::PollResult::Ready(fds) if fds.contains(&0) => {
            let mut line = String::new();
            match std::io::stdin().lock().read_line(&mut line) {
                Ok(0) => true,
                Ok(_) => line.trim().eq_ignore_ascii_case("q"),
                Err(_) => false,
            }
        }
        _ => false,
    }
}

/// M147: los dirs de `[native] embed` que `ray dev` vigila ADEMÁS de los fuentes (rutas
/// relativas a la raíz). Un cambio ahí NO reinicia — la lectura de std/embed ya es en vivo —
/// solo recarga el navegador vía el hub. Lo fija `cmd_dev`; test-watch y demás no lo tocan.
static DEV_EMBED_DIRS: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();

/// ¿Es `path` un asset del espacio embed vigilado por `ray dev`? (Bajo un dir configurado,
/// sin componentes ocultos — el mismo criterio del walker de std/embed.)
fn is_embed_asset(root: &Path, path: &Path) -> bool {
    let Some(dirs) = DEV_EMBED_DIRS.get() else {
        return false;
    };
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    if rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')) {
        return false;
    }
    dirs.iter().any(|d| rel.starts_with(d))
}

/// ¿Es `path` (ruta de un evento de kernel, absoluta) un fuente que `ray dev` vigila? El mismo
/// criterio de `scan_sources`, aplicado a una ruta suelta: bajo la raíz, fuera de carpetas de
/// artefactos/ocultas, extensión de fuente, y la regla del `.ray` generado con `.ray.html`
/// hermano (se vigila el fuente, no el artefacto). Los temporales de guardado atómico de los
/// editores (`.tmp`, `~`, el `4913` de vim) caen solos por la extensión. M147: los assets de
/// `[native] embed` también se vigilan (para la recarga del navegador, no para reiniciar).
#[cfg(all(feature = "watch", any(unix, windows)))]
fn is_watched_source(canon_root: &Path, path: &Path) -> bool {
    if is_embed_asset(canon_root, path) {
        return true;
    }
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
    // Un archivo OCULTO no es fuente aunque termine en .ray: es el temporal de un guardado
    // atómico (`.!NNN!x.ray` de sed -i, los puntos de vim/emacs) — con eventos se ve siempre
    // (el polling no lo alcanzaba: muere entre escaneos).
    if name.starts_with('.') {
        return false;
    }
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
            } else if name.ends_with(".ray")
                || name.ends_with(".ray.html")
                || name == "ray.toml"
                || is_embed_asset(root, &path)
            {
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
) -> Option<(PathBuf, bool)> {
    if before == after {
        return None;
    }
    let old: std::collections::HashMap<_, _> = before.iter().cloned().collect();
    for (p, m) in after {
        if old.get(p) != Some(m) {
            return Some((p.clone(), false));
        }
    }
    // Nada nuevo ni tocado pero difieren → algo se borró.
    let new: std::collections::HashMap<_, _> = after.iter().cloned().collect();
    before
        .iter()
        .find(|(p, _)| !new.contains_key(p))
        .map(|(p, _)| (p.clone(), true))
}

/// El pid del hijo en curso de `ray dev` (0 = ninguno), para que el handler de señales del PADRE
/// lo arrastre al morir: un `kill` al supervisor no debe dejar al programa huérfano reteniendo el
/// puerto (Ctrl-C de terminal ya mata al grupo; esto cubre el kill por pid). En Windows lo lee el
/// handler de consola de `dev_host` (y el Job Object cubre la muerte sin handler).
static DEV_CHILD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Instala la limpieza "si el supervisor muere, el hijo también" (M172: unix por señales,
/// Windows por handler de consola + Job Object; ver `dev_host`). El hijo en curso se lee de
/// `DEV_CHILD`.
fn install_cleanup_on_death() {
    crate::dev_host::install_cleanup_on_death(&DEV_CHILD);
}

/// Termina el hijo con una petición de cierre ordenado (SIGTERM / CTRL_BREAK: drenado vía
/// `serve_graceful`) y, si a los ~3 s sigue vivo, escala al kill duro (`dev_host`).
fn terminate_gracefully(child: &mut process::Child) {
    crate::dev_host::terminate_gracefully(child);
}

/// Lanza un hijo SUPERVISADO (`ray dev`, `ray test --watch`): con la preparación de plataforma
/// (grupo de procesos propio en Windows), registrado para la limpieza (Job Object en Windows)
/// y con su pid en `DEV_CHILD`. `what` nombra al hijo en el mensaje de error.
fn spawn_supervised(mut cmd: process::Command, what: &str) -> process::Child {
    crate::dev_host::prepare(&mut cmd);
    match cmd.spawn() {
        Ok(c) => {
            crate::dev_host::adopt(&c);
            DEV_CHILD.store(c.id() as i32, std::sync::atomic::Ordering::SeqCst);
            c
        }
        Err(e) => {
            eprintln!("{what}: {e}");
            process::exit(70);
        }
    }
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
/// M147c — `ray bundle`: empaqueta una app de escritorio distribuible. Compone `ray build
/// --native --release` (con el embed del ray.toml — OBLIGADO moralmente: el .app lanza con
/// cwd=/) y produce el formato del SO: `.app` en macOS (Info.plist + icns por sips/iconutil +
/// codesign ad-hoc best-effort) o un directorio con `.desktop` en Linux. En Windows (M180): directorio con `<name>.exe` (subsistema WINDOWS, icono y VERSIONINFO embebidos) y `<name>.lnk`, en `src/bundle_windows.rs`. Sin firma/notarización
/// en v1 (documentado en el help). Tooling puro: no toca los motores.
fn cmd_bundle(args: &[String]) {
    let flag_value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let name_arg = flag_value("--name");
    let icon = flag_value("--icon");
    let id_arg = flag_value("--id");
    let out_arg = flag_value("-o");
    let without_arg = flag_value("--without");
    // M155b: `--ios-target device|sim|both` — qué staticlib(s) construye `--ios` (both por
    // defecto; iterando contra un solo destino, el otro build son ~15-20 s tirados).
    let ios_target_arg = flag_value("--ios-target");
    // `--ios` (§80b): en vez del .app/.desktop del HOST, genera el proyecto Xcode de una app
    // iOS (shell WKWebView + staticlibs de dispositivo y simulador). Solo host macOS.
    let ios = args.iter().any(|a| a == "--ios");
    // M156: `--android` genera el proyecto GRADLE (shell Java + WebView + el programa como
    // cdylib en jniLibs); `--android-abi arm64|x86_64|all` elige los .so (espejo --ios-target).
    let android = args.iter().any(|a| a == "--android");
    let android_abi_arg = flag_value("--android-abi");
    let values: Vec<&String> = [&name_arg, &icon, &id_arg, &out_arg, &without_arg, &ios_target_arg, &android_abi_arg].iter().filter_map(|o| o.as_ref()).collect();
    let file = args
        .iter()
        .find(|a| !a.starts_with('-') && !values.iter().any(|v| v.as_str() == a.as_str()))
        .map(String::as_str);
    let path = resolve_entry(file, true);

    // El manifiesto del ENTRY da nombre/versión/exclusiones por defecto (como build_native).
    let entry_dir = match Path::new(&path).parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let entry_dir = entry_dir.canonicalize().unwrap_or(entry_dir);
    let manifest = Manifest::load(&entry_dir).ok().flatten();
    let name = name_arg
        .or_else(|| manifest.as_ref().map(|m| m.name.clone()))
        .unwrap_or_else(|| {
            Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("app").to_string()
        });
    let version = manifest.as_ref().map(|m| m.version.clone()).unwrap_or_else(|| "0.1.0".to_string());
    // El identifier por defecto sale del nombre (minúsculas, [a-z0-9-]): estable y único-ish.
    let bundle_id = id_arg.clone().unwrap_or_else(|| {
        let slug: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        format!("org.raylang.{}", slug.trim_matches('-'))
    });
    let mut exclude: Vec<String> = without_arg
        .as_deref()
        .map(|s| s.split(',').map(str::trim).filter(|p| !p.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();
    if let Some(m) = &manifest {
        for d in &m.native_without {
            if !exclude.contains(d) {
                exclude.push(d.clone());
            }
        }
    }
    // §80b: en móvil, `process` (fork/exec denegado en dispositivo y prohibido en las
    // stores) va excluido SIEMPRE; `audio` solo en iOS (CoreAudio de iOS sin validar) —
    // Android tiene backend AAudio desde M158.
    if ios || android {
        let forced: &[&str] = if ios { &["process", "audio"] } else { &["process"] };
        for sub in forced {
            if !exclude.iter().any(|d| d == sub) {
                exclude.push(sub.to_string());
            }
        }
    }
    // M183: la misma puerta por plataforma que `ray build` (en Windows ARM64 corosensei no tiene
    // backend: el bundle intentaba compilarlo y fallaba con un error de tipos en corosensei).
    let fibers = fibers_for_target(None, exclude.iter().any(|d| d == "fibers"));
    let embed = collect_embed(&path, None);
    if embed.is_empty() {
        eprintln!(
            "note: no embedded assets ([native] embed) — a bundled app launches with cwd=/ and \
             cannot read project files by relative path"
        );
    }

    // Compila el binario release a un temporal; build_native sale del proceso si algo falla.
    let (mut program, locate, multi) = load_and_locate(&path);
    check_or_exit(&mut program, &locate, multi);
    let work = std::env::temp_dir().join(format!("ray_bundle_{}", process::id()));
    let _ = fs::remove_dir_all(&work);
    if let Err(e) = fs::create_dir_all(&work) {
        eprintln!("bundle: could not create the work directory: {e}");
        process::exit(74);
    }
    let out_dir = out_arg.map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("."));
    if android {
        let abi = android_abi_arg.as_deref().unwrap_or("arm64");
        if !matches!(abi, "arm64" | "x86_64" | "all") {
            eprintln!("--android-abi must be 'arm64', 'x86_64' or 'all', not '{abi}'");
            process::exit(64);
        }
        let build_arm = abi != "x86_64";
        let build_x86 = abi != "arm64";
        let app_id = id_arg.clone().unwrap_or_else(|| {
            manifest
                .as_ref()
                .and_then(|m| m.android_application_id.clone())
                .unwrap_or_else(|| {
                    let slug: String = name
                        .to_lowercase()
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '.' })
                        .collect();
                    format!("org.raylang.{}", slug.trim_matches('.'))
                })
        });
        eprintln!("[bundle] Android shared libraries — abi: {abi} (a cold build compiles ring/mimalloc per target; needs the NDK and `rustup target add aarch64-linux-android`)");
        let arm_so = work.join("arm64.so");
        let x86_so = work.join("x86_64.so");
        if build_arm {
            eprintln!("[bundle] arm64-v8a (aarch64-linux-android)…");
            build_native(&path, arm_so.to_str(), true, &exclude, Some("aarch64-linux-android"), false, fibers, &embed, true);
        }
        if build_x86 {
            eprintln!("[bundle] x86_64 (x86_64-linux-android)…");
            build_native(&path, x86_so.to_str(), true, &exclude, Some("x86_64-linux-android"), false, fibers, &embed, true);
        }
        let proj = out_dir.join(format!("{name}-android"));
        // M160: los mipmaps del icono se generan ANTES de escribir el proyecto — el manifest
        // solo declara android:icon con los 5 PNG logrados (con el atributo y sin los PNG,
        // aapt rompe el build de Gradle).
        let mipmaps = icon.as_deref().and_then(|i| make_android_mipmaps(Path::new(i), &work));
        // Preservaciones (patrón M151/M155b): el .so del ABI NO construido, local.properties
        // y (M160) el material de firma de la raíz — keystore.properties y *.jks/*.keystore;
        // remove_dir_all borra el proyecto entero y regenerar NO debe destruir el keystore.
        let jni = proj.join("app/src/main/jniLibs");
        let kept_arm = (!build_arm).then(|| fs::read(jni.join("arm64-v8a/libray_app.so")).ok()).flatten();
        let kept_x86 = (!build_x86).then(|| fs::read(jni.join("x86_64/libray_app.so")).ok()).flatten();
        let kept_local = fs::read_to_string(proj.join("local.properties")).ok();
        let mut kept_signing: Vec<(String, Vec<u8>)> = Vec::new();
        if let Ok(rd) = fs::read_dir(&proj) {
            for entry in rd.flatten() {
                let file = entry.file_name().to_string_lossy().into_owned();
                if (file == "keystore.properties" || file.ends_with(".jks") || file.ends_with(".keystore"))
                    && let Ok(bytes) = fs::read(entry.path())
                {
                    kept_signing.push((file, bytes));
                }
            }
        }
        let _ = fs::remove_dir_all(&proj);
        let place = |built: bool, src_so: &Path, kept: &Option<Vec<u8>>, dir: std::path::PathBuf| {
            let dst = dir.join("libray_app.so");
            let r = fs::create_dir_all(&dir).and_then(|_| {
                if built {
                    fs::copy(src_so, &dst).map(|_| ())
                } else if let Some(bytes) = kept {
                    fs::write(&dst, bytes)
                } else {
                    Ok(())
                }
            });
            if let Err(e) = r {
                eprintln!("bundle: could not place the shared libraries: {e}");
                process::exit(74);
            }
        };
        place(build_arm, &arm_so, &kept_arm, jni.join("arm64-v8a"));
        place(build_x86, &x86_so, &kept_x86, jni.join("x86_64"));
        // abiFilters según lo PRESENTE (construido o preservado): un filtro de un ABI sin .so
        // daría un APK que no instala en ese ABI.
        let mut abis: Vec<&str> = Vec::new();
        if build_arm || kept_arm.is_some() {
            abis.push("'arm64-v8a'");
        }
        if build_x86 || kept_x86.is_some() {
            abis.push("'x86_64'");
        }
        let abis = abis.join(", ");
        if let Err(e) =
            crate::bundle_android::write_project(&proj, &name, &app_id, &version, &abis, mipmaps.is_some())
        {
            eprintln!("bundle: could not write the Gradle project: {e}");
            process::exit(74);
        }
        // M160: los PNG multi-densidad a res/mipmap-<d>/ic_launcher.png (el manifest ya los
        // declara — write_project recibió icon=true solo con los 5 logrados).
        if let Some(pngs) = &mipmaps {
            for (density, src) in pngs {
                let dir = proj.join(format!("app/src/main/res/mipmap-{density}"));
                let r = fs::create_dir_all(&dir)
                    .and_then(|_| fs::copy(src, dir.join("ic_launcher.png")).map(|_| ()));
                if let Err(e) = r {
                    eprintln!("bundle: could not place the launcher icon: {e}");
                    process::exit(74);
                }
            }
        }
        // M160: el material de firma preservado vuelve byte-idéntico a la raíz.
        for (file, bytes) in &kept_signing {
            if let Err(e) = fs::write(proj.join(file), bytes) {
                eprintln!("bundle: could not restore {file}: {e}");
                process::exit(74);
            }
        }
        // local.properties: preservado si existía; si no, sdk.dir detectado — Gradle lo exige.
        let local = match kept_local {
            Some(l) => Some(l),
            None => {
                let sdk = env::var("ANDROID_HOME").ok().or_else(|| {
                    env::var("HOME").ok().map(|h| format!("{h}/Library/Android/sdk"))
                });
                sdk.filter(|s| Path::new(s).is_dir()).map(|s| format!("sdk.dir={s}\n"))
            }
        };
        if let Some(l) = local {
            let _ = fs::write(proj.join("local.properties"), l);
        }
        let _ = fs::remove_dir_all(&work);
        println!("ok: Android project '{}'", proj.display());
        println!("  build:   cd {} && gradle assembleDebug", proj.display());
        println!("  install: adb install -r app/build/outputs/apk/debug/app-debug.apk");
        println!("  launch:  adb shell am start -n {app_id}/org.raylang.shell.MainActivity");
        println!("  logs:    adb logcat -s ray");
        return;
    }
    if ios {
        if !cfg!(target_os = "macos") {
            eprintln!("bundle --ios: an iOS app can only be built on macOS (Xcode)");
            process::exit(64);
        }
        // Los DOS staticlibs (dispositivo y simulador; ambos arm64 — el xcconfig elige por
        // SDK, jamás un lipo). Con el rustup target ausente, cargo lo dice y la pista es esta:
        let ios_target = ios_target_arg.as_deref().unwrap_or("both");
        if !matches!(ios_target, "device" | "sim" | "both") {
            eprintln!("--ios-target must be 'device', 'sim' or 'both', not '{ios_target}'");
            process::exit(64);
        }
        let build_dev = ios_target != "sim";
        let build_sim = ios_target != "device";
        eprintln!("[bundle] iOS static libraries — target: {ios_target} (a cold build compiles ring/mimalloc per target; needs `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`)");
        let dev_a = work.join("dev.a");
        let sim_a = work.join("sim.a");
        if build_dev {
            eprintln!("[bundle] device (aarch64-apple-ios)…");
            build_native(&path, dev_a.to_str(), true, &exclude, Some("aarch64-apple-ios"), false, fibers, &embed, true);
        }
        if build_sim {
            eprintln!("[bundle] simulator (aarch64-apple-ios-sim)…");
            build_native(&path, sim_a.to_str(), true, &exclude, Some("aarch64-apple-ios-sim"), false, fibers, &embed, true);
        }
        let proj = out_dir.join(format!("{name}-ios"));
        // Con un solo lado construido, el `.a` del OTRO lado del proyecto anterior se
        // conserva (como la firma): regenerar no debe dejar cojo lo que ya funcionaba.
        let kept_dev = (!build_dev).then(|| fs::read(proj.join("libs/libray_app.a")).ok()).flatten();
        let kept_sim = (!build_sim).then(|| fs::read(proj.join("libs-sim/libray_app.a")).ok()).flatten();
        // M151 (raydesk #9): la firma del xcconfig ANTERIOR se lee ANTES del borrado — cada
        // regeneración la pisaba (Xcode la escribe al elegir equipo) y había que reponerla a
        // mano tras cada bundle. `[ios] development_team` del ray.toml manda; lo preservado
        // rellena.
        let previous = fs::read_to_string(proj.join("App.xcconfig"))
            .map(|t| crate::bundle_ios::Signing::from_xcconfig(&t))
            .unwrap_or_default();
        let signing = crate::bundle_ios::Signing::resolve(
            manifest.as_ref().and_then(|m| m.ios_development_team.as_deref()),
            &previous,
        );
        let _ = fs::remove_dir_all(&proj);
        if let Err(e) = fs::create_dir_all(proj.join("libs")).and_then(|_| fs::create_dir_all(proj.join("libs-sim"))) {
            eprintln!("bundle: could not create '{}': {e}", proj.display());
            process::exit(74);
        }
        // Nombre FIJO del archive en ambos dirs (el `-lray_app` del xcconfig): la ruta decide.
        let place = |built: bool, src_a: &Path, kept: &Option<Vec<u8>>, dst: std::path::PathBuf| {
            let r = if built {
                fs::copy(src_a, &dst).map(|_| ())
            } else if let Some(bytes) = kept {
                fs::write(&dst, bytes)
            } else {
                Ok(()) // lado no construido y sin proyecto previo: el dir queda vacío
            };
            if let Err(e) = r {
                eprintln!("bundle: could not place the static libraries: {e}");
                process::exit(74);
            }
        };
        place(build_dev, &dev_a, &kept_dev, proj.join("libs/libray_app.a"));
        place(build_sim, &sim_a, &kept_sim, proj.join("libs-sim/libray_app.a"));
        if let Err(e) = crate::bundle_ios::write_project(&proj, &name, &bundle_id, &version, &signing) {
            eprintln!("bundle: could not write the Xcode project: {e}");
            process::exit(74);
        }
        if let Some(icon) = icon.as_deref() {
            write_ios_appicon(&proj, Path::new(icon));
        }
        let _ = fs::remove_dir_all(&work);
        println!("ok: iOS project '{}'", proj.display());
        println!("  simulator: xcodebuild -project {name}.xcodeproj -target {name} -sdk iphonesimulator -configuration Debug build CODE_SIGNING_ALLOWED=NO");
        match &signing.team {
            Some(team) => println!("  device:    signing team {team} already in App.xcconfig; open the project in Xcode and run"),
            None => println!("  device:    open the project in Xcode and pick your signing team (persist it with [ios] development_team in ray.toml)"),
        }
        return;
    }
    // M186: el binario que empaquetamos es el que el build ESCRIBIÓ (en Windows, `bin.exe`), no el
    // nombre que le pedimos.
    let tmp_bin = PathBuf::from(build_native(&path, work.join("bin").to_str(), true, &exclude, None, false, fibers, &embed, false));

    if cfg!(target_os = "macos") {
        bundle_macos(&out_dir, &name, &version, &bundle_id, icon.as_deref(), manifest.as_ref().and_then(|m| m.app_copyright.as_deref()), &tmp_bin);
    } else if cfg!(unix) {
        bundle_linux(&out_dir, &name, icon.as_deref(), &tmp_bin);
    } else if cfg!(windows) {
        // M180 (W7d): `<name><name>.exe` (subsistema WINDOWS + icono + VERSIONINFO como
        // recursos) y el acceso directo `<name>.lnk`.
        #[cfg(windows)]
        if let Err(e) = crate::bundle_windows::bundle(
            &out_dir,
            &name,
            &version,
            manifest.as_ref().and_then(|m| m.app_copyright.as_deref()),
            icon.as_deref().map(Path::new),
            &tmp_bin,
        ) {
            eprintln!("bundle: {e}");
            process::exit(74);
        }
    } else {
        eprintln!("bundle: no bundle format for this platform (macOS .app / Linux .desktop / Windows .exe)");
        process::exit(64);
    }
    let _ = fs::remove_dir_all(&work);
}

/// El `.app` de macOS: la estructura es un árbol de carpetas + un Info.plist mínimo. El icns es
/// best-effort (sips + iconutil, herramientas del sistema); el codesign ad-hoc también (mantiene
/// válida la firma que el linker de arm64 aplicó, tras mover el binario).
fn bundle_macos(out_dir: &Path, name: &str, version: &str, bundle_id: &str, icon: Option<&str>, copyright: Option<&str>, bin: &Path) {
    let app = out_dir.join(format!("{name}.app"));
    let _ = fs::remove_dir_all(&app);
    let macos_dir = app.join("Contents/MacOS");
    let resources = app.join("Contents/Resources");
    if let Err(e) = fs::create_dir_all(&macos_dir).and_then(|_| fs::create_dir_all(&resources)) {
        eprintln!("bundle: could not create '{}': {e}", app.display());
        process::exit(74);
    }
    let exe = macos_dir.join(name);
    if let Err(e) = fs::copy(bin, &exe) {
        eprintln!("bundle: could not place the binary: {e}");
        process::exit(74);
    }
    let mut icon_key = String::new();
    if let Some(icon) = icon {
        match make_icns(Path::new(icon), &resources.join("icon.icns")) {
            Ok(()) => icon_key = "  <key>CFBundleIconFile</key><string>icon</string>\n".to_string(),
            Err(e) => eprintln!("bundle: warning: could not build the icon ({e}); continuing without it"),
        }
    }
    // M155: el copyright del panel About sale de `[app] copyright` del ray.toml.
    let copyright_key = copyright
        .map(|c| format!("\x20 <key>NSHumanReadableCopyright</key><string>{c}</string>\n"))
        .unwrap_or_default();
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \x20 <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>\n\
         \x20 <key>CFBundlePackageType</key><string>APPL</string>\n\
         \x20 <key>CFBundleName</key><string>{name}</string>\n\
         \x20 <key>CFBundleExecutable</key><string>{name}</string>\n\
         \x20 <key>CFBundleIdentifier</key><string>{bundle_id}</string>\n\
         \x20 <key>CFBundleVersion</key><string>{version}</string>\n\
         \x20 <key>CFBundleShortVersionString</key><string>{version}</string>\n\
         \x20 <key>NSHighResolutionCapable</key><true/>\n\
         {icon_key}\
         {copyright_key}\
         \x20 <key>NSAppTransportSecurity</key><dict><key>NSAllowsLocalNetworking</key><true/></dict>\n\
         </dict>\n</plist>\n"
    );
    if let Err(e) = fs::write(app.join("Contents/Info.plist"), plist) {
        eprintln!("bundle: could not write Info.plist: {e}");
        process::exit(74);
    }
    // Firma ad-hoc best-effort: sin identidad (no distribuible firmado), pero deja el .app
    // internamente consistente en arm64 tras mover el binario.
    let _ = process::Command::new("codesign")
        .args(["--force", "--deep", "-s", "-"])
        .arg(&app)
        .output();
    println!("ok: bundle '{}'", app.display());
}

/// El "bundle" de Linux: un directorio con el binario + el lanzador `.desktop` (el `Exec=` va
/// ABSOLUTO — un .desktop con ruta relativa no funciona desde un lanzador; para instalarlo,
/// copiarlo a ~/.local/share/applications ajustando la ruta si se mueve el directorio).
fn bundle_linux(out_dir: &Path, name: &str, icon: Option<&str>, bin: &Path) {
    let dir = out_dir.join(name);
    let _ = fs::remove_dir_all(&dir);
    if let Err(e) = fs::create_dir_all(&dir) {
        eprintln!("bundle: could not create '{}': {e}", dir.display());
        process::exit(74);
    }
    let exe = dir.join(name);
    if let Err(e) = fs::copy(bin, &exe) {
        eprintln!("bundle: could not place the binary: {e}");
        process::exit(74);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&exe, fs::Permissions::from_mode(0o755));
    }
    let abs = dir.canonicalize().unwrap_or_else(|_| dir.clone());
    let mut icon_line = String::new();
    if let Some(icon) = icon {
        let dest = dir.join("icon.png");
        match fs::copy(icon, &dest) {
            Ok(_) => icon_line = format!("Icon={}/icon.png\n", abs.display()),
            Err(e) => eprintln!("bundle: warning: could not copy the icon ({e}); continuing without it"),
        }
    }
    let desktop = format!(
        "[Desktop Entry]\nType=Application\nName={name}\nExec={}/{name}\n{icon_line}Terminal=false\nCategories=Utility;\n",
        abs.display()
    );
    if let Err(e) = fs::write(dir.join(format!("{name}.desktop")), desktop) {
        eprintln!("bundle: could not write the .desktop launcher: {e}");
        process::exit(74);
    }
    println!("ok: bundle '{}'", dir.display());
}

/// §80b: el AppIcon del proyecto iOS — un appiconset de TAMAÑO ÚNICO (1024, "single size":
/// Xcode 14+ genera el resto), vía sips. Best-effort: sin icono válido, la app compila igual.
fn write_ios_appicon(proj: &Path, icon: &Path) {
    let set = proj.join("Shell/Assets.xcassets/AppIcon.appiconset");
    let ok = fs::create_dir_all(&set).is_ok()
        && process::Command::new("sips")
            .args(["-z", "1024", "1024"])
            .arg(icon)
            .arg("--out")
            .arg(set.join("icon_1024.png"))
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        && fs::write(
            set.join("Contents.json"),
            "{\n  \"images\": [{ \"filename\": \"icon_1024.png\", \"idiom\": \"universal\", \"platform\": \"ios\", \"size\": \"1024x1024\" }],\n  \"info\": { \"author\": \"xcode\", \"version\": 1 }\n}\n",
        )
        .is_ok()
        && fs::write(
            proj.join("Shell/Assets.xcassets/Contents.json"),
            "{\n  \"info\": { \"author\": \"xcode\", \"version\": 1 }\n}\n",
        )
        .is_ok();
    if !ok {
        eprintln!("bundle: warning: could not build the app icon; continuing without it");
    }
    // Nota: sin cablear el asset catalog en el pbxproj v1 (exigiría fase Resources +
    // ASSETCATALOG_COMPILER_APPICON_NAME); el catálogo queda listo para arrastrar en Xcode.
}

/// M160: los `ic_launcher.png` multi-densidad del proyecto Android, vía sips (precedente
/// make_icns). Best-effort con gate honesto: sin sips (host no-mac) o con cualquier resize
/// fallido devuelve None — el llamador NO declara `android:icon` (aapt rompería el build con
/// el mipmap ausente) y avisa. Icono legacy v1 (Android 8+ lo enmascara a círculo); el
/// adaptive de capas queda diferido (IDEAS §80b).
const ANDROID_DENSITIES: &[(u32, &str)] =
    &[(48, "mdpi"), (72, "hdpi"), (96, "xhdpi"), (144, "xxhdpi"), (192, "xxxhdpi")];

fn make_android_mipmaps(icon: &Path, work: &Path) -> Option<Vec<(&'static str, PathBuf)>> {
    if !icon.is_file() {
        eprintln!("bundle: warning: icon not found: {}; continuing without it", icon.display());
        return None;
    }
    let mut out = Vec::new();
    for (px, density) in ANDROID_DENSITIES {
        let dst = work.join(format!("ic_launcher_{density}.png"));
        let ok = process::Command::new("sips")
            .args(["-z", &px.to_string(), &px.to_string()])
            .arg(icon)
            .arg("--out")
            .arg(&dst)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("bundle: warning: could not build the launcher icon (needs sips, macOS); continuing without it");
            return None;
        }
        out.push((*density, dst));
    }
    Some(out)
}

/// Un `.icns` desde un PNG, vía las herramientas del sistema: `sips -z` genera el iconset (los
/// NOMBRES son exactos — iconutil los exige) e `iconutil` lo compila.
fn make_icns(icon: &Path, out_icns: &Path) -> Result<(), String> {
    if !icon.is_file() {
        return Err(format!("icon not found: {}", icon.display()));
    }
    let set = std::env::temp_dir().join(format!("ray_iconset_{}.iconset", process::id()));
    let _ = fs::remove_dir_all(&set);
    fs::create_dir_all(&set).map_err(|e| e.to_string())?;
    const SIZES: &[(u32, &str)] = &[
        (16, "icon_16x16.png"),
        (32, "icon_16x16@2x.png"),
        (32, "icon_32x32.png"),
        (64, "icon_32x32@2x.png"),
        (128, "icon_128x128.png"),
        (256, "icon_128x128@2x.png"),
        (256, "icon_256x256.png"),
        (512, "icon_256x256@2x.png"),
        (512, "icon_512x512.png"),
        (1024, "icon_512x512@2x.png"),
    ];
    for (px, file) in SIZES {
        let st = process::Command::new("sips")
            .args(["-z", &px.to_string(), &px.to_string()])
            .arg(icon)
            .arg("--out")
            .arg(set.join(file))
            .output()
            .map_err(|e| format!("sips: {e}"))?;
        if !st.status.success() {
            return Err(format!("sips failed on {file}"));
        }
    }
    let st = process::Command::new("iconutil")
        .args(["-c", "icns"])
        .arg(&set)
        .arg("-o")
        .arg(out_icns)
        .output()
        .map_err(|e| format!("iconutil: {e}"))?;
    let _ = fs::remove_dir_all(&set);
    if !st.status.success() {
        return Err(format!("iconutil failed: {}", String::from_utf8_lossy(&st.stderr).trim()));
    }
    Ok(())
}

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
    // `--lib` (§80b): emite una LIBRERÍA estática con la entrada C `ray_start()` en vez de un
    // binario — lo que un shell móvil (o cualquier host C) linkea. Exige --native.
    let lib_mode = args.iter().any(|a| a == "--lib");
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
    // `--embed <dirs>` (M147): directorios de assets a HORNEAR en el binario nativo (lista
    // separada por comas, relativa a la raíz del proyecto). Se UNE a `[native] embed` del
    // ray.toml (la política versionada); mismo rastreo de origen que `--without`.
    let embed_arg = args.iter().position(|a| a == "--embed").and_then(|i| args.get(i + 1)).cloned();
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
    // M146: `audio` faltaba desde M145 (el help/docs lo anunciaban y la validación lo rechazaba
    // con exit 64) — entra junto a `ui`.
    const RT_SUBSYSTEMS: &[&str] = &["crypto", "tls", "sqlite", "mimalloc", "ahash", "regex", "fibers", "process", "watch", "unicode", "audio", "ui"];
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
    // M173 excluía `watch` solo en targets Windows (el gate de M169 rechazaba cualquier programa
    // que importara `std/fs`); desde M181 `ray_runtime::watch` compila en Windows y ya no hace falta.
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
        // M168: sin `--target`, el target efectivo es el HOST. M182: en Windows las fibras corren
        // sobre el reactor WSAPoll — pero solo en x86_64: corosensei no tiene backend para
        // AArch64-Windows, y ahí se apagan solas, con aviso (hilo-por-tarea, el respaldo completo).
        fibers_for_target(target.as_deref(), without_fibers)
    };
    let file = args
        .iter()
        .find(|a| {
            !a.starts_with('-')
                && Some(a.as_str()) != output.as_deref()
                && Some(a.as_str()) != without_arg.as_deref()
                && Some(a.as_str()) != target.as_deref()
                && Some(a.as_str()) != embed_arg.as_deref()
        })
        .map(String::as_str);
    let path = resolve_entry(file, true);
    let (mut program, locate, multi) = load_and_locate(&path);
    check_or_exit(&mut program, &locate, multi);
    if native {
        let embed = collect_embed(&path, embed_arg.as_deref());
        build_native(&path, output.as_deref(), release, &exclude, target.as_deref(), fast, fibers, &embed, lib_mode);
        return;
    }
    if lib_mode {
        eprintln!("--lib requires --native (a static library is a native artifact)");
        process::exit(64);
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
#[allow(clippy::too_many_arguments)] // la firma refleja los flags de `ray build --native`
/// M169: los subsistemas del runtime nativo que no existen en Windows (sus módulos en
/// M182/M183: ¿van las fibras en este build? `without_fibers` las apaga siempre; si no, dependen
/// del target efectivo (`--target` o el host): en Windows solo x86_64 tiene backend de corrutinas
/// (corosensei), y en los demás Windows se apagan solas, con aviso — el hilo-por-tarea es el
/// respaldo completo. Compartida por `ray build --native` y `ray bundle`.
fn fibers_for_target(target: Option<&str>, without_fibers: bool) -> bool {
    let triple = target.map(str::to_string).unwrap_or_else(host_triple);
    let no_fibers_target = triple.contains("windows") && !triple.starts_with("x86_64");
    if no_fibers_target && !without_fibers {
        eprintln!("note: fibers are not available on {triple} (no coroutine backend for this architecture); building with the thread-per-task model");
    }
    !without_fibers && !no_fibers_target
}

/// El triple del HOST, tal como lo escribiría rustup (`aarch64-pc-windows-msvc`, `x86_64-apple-darwin`…):
/// el target efectivo de un build nativo sin `--target`. De él cuelgan las decisiones por SO y
/// arquitectura (M182: las fibras de Windows son solo x86_64; M183: clang para `ring` en ARM64).
///
/// **M184**: lo dice `rustc -vV`, no la arquitectura de ESTE proceso. Un `ray.exe` x86_64 corriendo
/// emulado en Windows ARM64 (hoy no publicamos asset arm64-msvc) creía compilar para x86_64 mientras
/// el `rustc` nativo compilaba para `aarch64-pc-windows-msvc`: las comprobaciones previas miraban el
/// target equivocado y el build moría minutos después dentro de un build script. Se consulta una vez
/// (el proceso no cambia de rustc a media ejecución) y el cálculo por `env::consts` queda de respaldo.
pub fn host_triple() -> String {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HOST.get_or_init(|| rustc_host_triple().unwrap_or_else(fallback_host_triple)).clone()
}

/// El triple que reporta el `rustc` con el que se compilará (RAY_RUSTC → PATH → toolchain privada).
fn rustc_host_triple() -> Option<String> {
    let out = crate::toolchain::command("rustc")?.arg("-vV").output().ok()?;
    out.status.success().then(|| parse_rustc_host(&String::from_utf8_lossy(&out.stdout)))?
}

/// La línea `host: <triple>` de `rustc -vV`. Pura para testearla sin rustc.
fn parse_rustc_host(vv: &str) -> Option<String> {
    vv.lines()
        .find_map(|l| l.strip_prefix("host:"))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Respaldo si no hay rustc a mano: SO de este binario + arquitectura de la MÁQUINA.
fn fallback_host_triple() -> String {
    let (arch, os) = (crate::toolchain::machine_arch(), std::env::consts::OS);
    match os {
        "windows" => format!("{arch}-pc-windows-msvc"),
        "macos" => format!("{arch}-apple-darwin"),
        "ios" => format!("{arch}-apple-ios"),
        "android" => format!("{arch}-linux-android"),
        _ => format!("{arch}-unknown-{os}"),
    }
}

/// `ray-runtime` son `cfg(unix)`), con el mismo mensaje que la VM devuelve en runtime. Recibe las
/// features que el transpilador pidió y devuelve las que no compilarían. Pura para testearla.
fn native_unsupported_on_windows(rt_features: &[&str]) -> Vec<&'static str> {
    // M175: `process` compila en Windows (ray_runtime::process tiene su variante); ya no es hueco.
    // M177: `ui` compila en Windows (headless; las ventanas reales devuelven el `Err` de la VM).
    // M178: `audio` compila en Windows (WASAPI). M181: `watch` también (notify sobre
    // ReadDirectoryChangesW). La lista queda VACÍA: se conserva como red (y su test) por si un
    // subsistema futuro volviera a ser solo-unix.
    const GAPS: &[(&str, &str)] = &[];
    GAPS.iter()
        .filter(|(feature, _)| rt_features.contains(feature))
        .map(|(_, message)| *message)
        .collect()
}

/// Devuelve la ruta del artefacto REALMENTE escrito: en Windows no coincide con lo pedido (M186 le
/// añade la extensión que el SO exige), y `ray bundle` necesita el nombre de verdad para empaquetar.
fn build_native(path: &str, output: Option<&str>, release: bool, exclude: &[String], target: Option<&str>, fast: bool, fibers: bool, embed: &[(String, String)], lib_mode: bool) -> String {
    let (mut program, locate, multi) = load_and_locate(path);
    check_or_exit(&mut program, &locate, multi);
    let transpiled = match crate::transpile::transpile_entry(&program, exclude, fast, fibers, embed, lib_mode) {
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
    // M169 (docs/windows.md W2): en Windows, los subsistemas cuyo módulo de ray-runtime es
    // `cfg(unix)` no COMPILAN — antes el usuario veía un backtrace de rustc en vez del mensaje que la
    // VM da en runtime. Se comprueba aquí, antes de generar nada, con el target EFECTIVO (host si no
    // hay `--target`).
    let for_windows = target.map_or(cfg!(windows), |t| t.contains("windows"));
    if for_windows {
        let gaps = native_unsupported_on_windows(&transpiled.rt_features);
        if !gaps.is_empty() {
            eprintln!(
                "native build: this program uses subsystems that are not available on Windows yet \
                 (the VM reports the same error at runtime):"
            );
            for gap in gaps {
                eprintln!("  · {gap}");
            }
            eprintln!("(see docs/windows.md for the status of each subsystem)");
            process::exit(69); // EX_UNAVAILABLE
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
    // M186: en Windows el nombre pedido no basta — sin `.exe` el archivo no se ejecuta. Se decide
    // por el target EFECTIVO, así que cruzar a Windows desde macOS/Linux también produce un `.exe`.
    let out_bin = ensure_windows_extension(out_bin, for_windows, lib_mode);
    // **Bifurcación bajo demanda** (P2.b, docs/transpilador-nativo.md §4.5): sin features de `ray-runtime`
    // → `rustc` pelado, rápido y sin red. Con features → un proyecto Cargo generado que enlaza `ray-runtime`
    // (mismo código que la VM). N1/N2: como `mimalloc` y `ahash` van POR DEFECTO, el camino común hoy es el
    // Cargo (con la caché compartida ~/.ray/native-cache, se compilan una vez por máquina); `--without
    // mimalloc,ahash` (sin otros subsistemas) recupera el rustc pelado.
    // §80b: el modo lib va SIEMPRE por Cargo (el [lib] crate-type vive en el manifiesto
    // generado; un staticlib por rustc pelado no amortiza otra rama).
    if transpiled.rt_features.is_empty() && !lib_mode {
        build_native_rustc(&transpiled.source, stem, &out_bin, release, target);
    } else {
        build_native_cargo(&transpiled.source, &transpiled.rt_features, path, stem, &out_bin, release, target, lib_mode);
    }
    out_bin
}

/// M186: la extensión que Windows EXIGE en el nombre de salida — `.exe` para un binario, `.lib` para
/// el staticlib de `--lib`. Sin ella el archivo no es ejecutable: el Explorador abre el diálogo de
/// "elegir con qué abrir" y la consola no lo lanza. El nombre por defecto (el `name` del ray.toml o
/// el stem del fuente) nunca la lleva, y un `-o` casi nunca. Fuera de un target Windows no toca nada
/// (ahí la extensión no significa nada y `hello` es el nombre idiomático). Pura para testearla.
fn ensure_windows_extension(out: String, for_windows: bool, lib_mode: bool) -> String {
    let want = if lib_mode { "lib" } else { "exe" };
    let already = Path::new(&out).extension().is_some_and(|e| e.eq_ignore_ascii_case(want));
    if !for_windows || already {
        return out;
    }
    format!("{out}.{want}")
}

/// M147: reúne la tabla de assets a hornear: el `[native] embed` del ray.toml del ENTRY (no
/// del cwd; es el mismo ancla que usa la resolución en runtime) unido al `--embed` de la CLI.
/// Cada dir configurado debe EXISTIR (fail-fast nombrando el origen, como los typos de
/// `--without`); las claves salen del walker compartido con la VM (mismo espacio y orden).
fn collect_embed(entry: &str, embed_arg: Option<&str>) -> Vec<(String, String)> {
    // Canonicalizar ANTES de buscar el manifiesto: los ancestros de una ruta RELATIVA terminan
    // en "" (una raíz vacía que produce include_bytes! relativos — irresolubles desde el
    // proyecto Cargo generado en /tmp).
    let entry_dir = match Path::new(entry).parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let entry_dir = entry_dir.canonicalize().unwrap_or(entry_dir);
    let manifest = Manifest::load(&entry_dir).ok().flatten();
    let mut dirs: Vec<(String, &'static str)> = embed_arg
        .map(|s| s.split(',').map(str::trim).filter(|p| !p.is_empty()).map(|p| (p.to_string(), "--embed")).collect())
        .unwrap_or_default();
    if let Some(m) = &manifest {
        for d in &m.native_embed {
            if !dirs.iter().any(|(x, _)| x == d) {
                dirs.push((d.clone(), "ray.toml"));
            }
        }
    }
    if dirs.is_empty() {
        return Vec::new();
    }
    let root = manifest.map(|m| m.root).unwrap_or(entry_dir);
    let root = root.canonicalize().unwrap_or(root);
    for (d, origin) in &dirs {
        if !root.join(d).is_dir() {
            eprintln!("embed directory in {origin} does not exist: '{d}' (relative to '{}')", root.display());
            process::exit(64);
        }
    }
    let dir_names: Vec<String> = dirs.into_iter().map(|(d, _)| d).collect();
    crate::builtins::embed_walk(&root, &dir_names)
        .into_iter()
        .map(|(key, p)| (key, p.to_string_lossy().into_owned()))
        .collect()
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
/// Coloca el binario recién construido en `out_bin` reemplazando el inode (`rename`) en vez de
/// sobrescribirlo in-place. Si el rename falla, el `.tmp` se limpia para no dejar artefactos.
fn replace_output_binary(tmp_bin: &str, out_bin: &str) -> std::io::Result<()> {
    std::fs::rename(tmp_bin, out_bin).inspect_err(|_| {
        let _ = std::fs::remove_file(tmp_bin);
    })
}

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
    // M171: `rustc` resuelto por el orden RAY_RUSTC → PATH → toolchain privada (`src/toolchain.rs`).
    let Some(mut cmd) = crate::toolchain::command("rustc") else {
        eprintln!("native build: rustc not found (RAY_RUSTC, PATH, {})", crate::toolchain::home().display());
        eprintln!("{}", crate::toolchain::missing_hint("rustc"));
        process::exit(65);
    };
    cmd.args(&flags);
    // MISMA edition que el proyecto Cargo generado (edition 2024 en su Cargo.toml). Sin el flag, rustc
    // pelado caía a la 2015 → los dos caminos compilaban el MISMO Rust generado bajo reglas distintas
    // (p. ej. `gen` es keyword solo en 2024: un identificador podía pasar por un tier y romper por el otro).
    cmd.arg("--edition").arg("2024");
    if let Some(t) = target {
        cmd.arg("--target").arg(t);
    }
    // Se compila a `<out>.tmp` y se renombra (IDEAS §77): sobrescribir IN-PLACE un binario existente
    // (mismo inode) invalida en macOS la caché de firma del kernel y el nuevo binario muere con SIGKILL
    // al exec. `rename` reemplaza el inode; y si el build falla, el binario anterior sigue intacto.
    let tmp_bin = format!("{out_bin}.tmp");
    let status = cmd.arg(&rs_path).arg("-o").arg(&tmp_bin).status();
    match status {
        Ok(s) if s.success() => {
            let _ = std::fs::remove_file(&rs_path); // build ok → no dejar el `.rs` temporal (sin fugas)
            if let Err(e) = replace_output_binary(&tmp_bin, out_bin) {
                eprintln!("native build: could not place the binary at '{out_bin}': {e}");
                process::exit(65);
            }
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
            eprintln!("native build: could not run rustc ({}): {e}", cmd.get_program().to_string_lossy());
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
const RT_AUDIO_RS: &str = include_str!("../crates/ray-runtime/src/audio.rs");
const RT_UI_RS: &str = include_str!("../crates/ray-runtime/src/ui.rs");
const RT_X509_RS: &str = include_str!("../crates/ray-runtime/src/x509.rs");
const RT_UNICODE_RS: &str = include_str!("../crates/ray-runtime/src/unicode.rs");

/// Camino Cargo: el programa usa un subsistema con crate externo (cripto/…). Se genera un proyecto Cargo
/// temporal (`src/main.rs` + una copia de `ray-runtime` con las fuentes incrustadas) y se compila con
/// `cargo build`, activando SOLO las features detectadas. Un `CARGO_TARGET_DIR` compartido compila los
/// crates (ring…) una vez por máquina; builds siguientes solo recompilan `main.rs`.
/// M156: el directorio bin del toolchain LLVM del NDK. Orden: ANDROID_NDK_HOME →
/// $ANDROID_HOME/ndk/<mayor versión> → ~/Library/Android/sdk/ndk/<mayor>. El dir prebuilt es
/// `darwin-x86_64` también en Apple Silicon (binarios universales desde r25) y
/// `linux-x86_64` en Linux.
fn android_ndk_bin() -> Option<std::path::PathBuf> {
    let host = if cfg!(target_os = "macos") { "darwin-x86_64" } else { "linux-x86_64" };
    let from_root = |root: &Path| -> Option<std::path::PathBuf> {
        let bin = root.join("toolchains/llvm/prebuilt").join(host).join("bin");
        bin.is_dir().then_some(bin)
    };
    if let Ok(home) = env::var("ANDROID_NDK_HOME")
        && let Some(b) = from_root(Path::new(&home))
    {
        return Some(b);
    }
    let sdk = env::var("ANDROID_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = env::var("HOME").unwrap_or_default();
            Path::new(&home).join("Library/Android/sdk")
        });
    let ndk_dir = sdk.join("ndk");
    let mut versions: Vec<std::path::PathBuf> =
        std::fs::read_dir(&ndk_dir).ok()?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    versions.sort();
    versions.into_iter().rev().find_map(|v| from_root(&v))
}

#[allow(clippy::too_many_arguments)] // la firma refleja los flags de `ray build --native`
fn build_native_cargo(rust: &str, rt_features: &[&str], src_path: &str, stem: &str, out_bin: &str, release: bool, target: Option<&str>, lib_mode: bool) {
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
    // M183 (RayDesk en Windows): las dependencias con build script del runtime — `mimalloc` (por
    // defecto), `ring` (tls/crypto) y `sqlite` — necesitan un compilador de C, y en Windows ARM64
    // `ring` compila su ensamblador con clang. Sin él, cargo fallaba tras MINUTOS de compilación
    // con un "failed to find tool clang" enterrado en el log: se comprueba antes, con el remedio.
    // M184: `triple` es el target EFECTIVO (`--target` o el host según rustc) — antes se deducía de
    // la arquitectura del propio `ray`, y un `ray` emulado saltaba esta comprobación entera.
    let triple = target.map(str::to_string).unwrap_or_else(host_triple);
    let needs_clang = {
        let c_features: Vec<&str> = rt_features.iter().copied().filter(|f| matches!(*f, "mimalloc" | "tls" | "crypto" | "sqlite")).collect();
        let needs_clang = triple.starts_with("aarch64")
            && triple.contains("windows")
            && c_features.iter().any(|f| matches!(*f, "tls" | "crypto"));
        if !c_features.is_empty() {
            let mut checks = vec![crate::toolchain::c_compiler(&triple, false)];
            if needs_clang {
                checks.push(crate::toolchain::c_compiler(&triple, true));
            }
            for r in checks {
                if let Err(how) = r {
                    eprintln!(
                        "native build: the runtime features [{}] need a C compiler for {triple} — {how}\n  (or leave them out: --without {})",
                        c_features.join(", "),
                        c_features.join(",")
                    );
                    process::exit(69);
                }
            }
        }
        needs_clang
    };
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
    // §80b (modo lib): [lib] staticlib con la fuente en src/lib.rs — el artefacto pasa a ser
    // `lib<pkg con -→_>.a` (un archive que un shell C/ObjC linkea).
    // M156: Android carga la lib con System.loadLibrary → cdylib (.so); el resto (iOS/host)
    // sigue en staticlib (.a que el shell linkea).
    let android = target.is_some_and(|t| t.contains("android"));
    let lib_section = if lib_mode {
        let crate_type = if android { "cdylib" } else { "staticlib" };
        format!("[lib]\nname = \"{pkg}\"\ncrate-type = [\"{crate_type}\"]\npath = \"src/lib.rs\"\n\n")
    } else {
        String::new()
    };
    let cargo_toml = format!(
        "[package]\nname = \"{pkg}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n\n\
         {lib_section}\
         [dependencies]\nray-runtime = {{ path = \"ray-runtime\", default-features = false, features = [{feats}] }}\n\n\
         [profile.dev]\nopt-level = 2\n\n[profile.release]\nopt-level = 3\nlto = \"fat\"\ncodegen-units = 1\n"
    );
    let src_rel = if lib_mode { "src/lib.rs" } else { "src/main.rs" };
    let files = [
        ("Cargo.toml", cargo_toml.as_str()),
        (src_rel, rust),
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
        ("ray-runtime/src/audio.rs", RT_AUDIO_RS), // ídem: solo con la feature `audio` (M145)
        ("ray-runtime/src/ui.rs", RT_UI_RS), // ídem: solo con la feature `ui` (M146)
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
    // M171: con el vendor de la release instalado (`ray toolchain install`), el proyecto toma sus
    // crates del vendor y usa SU `Cargo.lock` (el que se resolvió al vendorizar: las versiones que
    // hay en el directorio) → build sin red. El lock cacheado solo aplica sin vendor: podría fijar
    // versiones que el vendor no contiene.
    let vendor = crate::toolchain::installed_vendor();
    if let Some(v) = &vendor {
        if let Err(e) = write(".cargo/config.toml", &crate::toolchain::vendor_cargo_config(v)) {
            eprintln!("native build: could not write the Cargo project (.cargo/config.toml): {e}");
            process::exit(65);
        }
        let _ = std::fs::copy(v.join("Cargo.lock"), proj.join("Cargo.lock"));
    } else if cached_lock.is_file() {
        let _ = std::fs::copy(&cached_lock, proj.join("Cargo.lock")); // proj ya existe (files escritos arriba)
    }
    // M171: `cargo` resuelto por el orden RAY_CARGO → PATH → toolchain privada (`src/toolchain.rs`).
    let Some(mut cmd) = crate::toolchain::command("cargo") else {
        eprintln!("native build: cargo not found (RAY_CARGO, PATH, {})", crate::toolchain::home().display());
        eprintln!("{}", crate::toolchain::missing_hint("cargo"));
        // N1/N2: mimalloc+ahash-por-defecto traen el camino Cargo al caso común; sin cargo aún se
        // puede compilar con rustc pelado excluyendo las features siempre-on (las de USO no: el
        // programa las necesita de verdad).
        if rt_features.iter().all(|f| *f == "mimalloc" || *f == "ahash" || *f == "fibers") {
            let list = rt_features.join(",");
            eprintln!("hint: or build without cargo (plain rustc) with: ray build --native --without {list}");
        }
        process::exit(65);
    };
    cmd.arg("build").current_dir(&proj).env("CARGO_TARGET_DIR", &target_dir);
    if let Some(t) = target {
        cmd.arg("--target").arg(t);
    }
    // M156: toolchain del NDK inyectado por env — SIN cargo-ndk: el linker del target y el
    // CC/AR que usan los build scripts (ring/rusqlite/mimalloc) apuntan al clang/llvm-ar del
    // NDK. Android 15+ exige .so alineados a 16KB → flag de linker explícito.
    let mut extra_rustflags = String::new();
    if android {
        let Some(ndk_bin) = android_ndk_bin() else {
            eprintln!(
                "native build: Android NDK not found — set ANDROID_NDK_HOME (or install it \
                 under $ANDROID_HOME/ndk/, e.g. `sdkmanager \"ndk;27.2.12479018\"`)"
            );
            process::exit(64);
        };
        let t = target.unwrap_or_default();
        let arch = if t.starts_with("x86_64") { "x86_64" } else { "aarch64" };
        let clang = ndk_bin.join(format!("{arch}-linux-android24-clang"));
        let env_arch = format!("{arch}_linux_android");
        cmd.env(
            format!("CARGO_TARGET_{}_LINUX_ANDROID_LINKER", arch.to_uppercase()),
            &clang,
        );
        cmd.env(format!("CC_{env_arch}"), &clang);
        cmd.env(format!("AR_{env_arch}"), ndk_bin.join("llvm-ar"));
        extra_rustflags.push_str(" -C link-arg=-Wl,-z,max-page-size=16384");
    }
    // `target-cpu=native` solo en release SIN cross-compile (con `--target` sería la CPU del host → no
    // portable al target).
    if release && target.is_none() {
        cmd.arg("--release").env("RUSTFLAGS", format!("-C target-cpu=native -A warnings{extra_rustflags}"));
    } else if release {
        cmd.arg("--release").env("RUSTFLAGS", format!("-A warnings{extra_rustflags}"));
    } else {
        cmd.env("RUSTFLAGS", format!("-A warnings{extra_rustflags}"));
    }
    // M184: si el build necesita clang y está instalado fuera del PATH, se lo damos al hijo en vez
    // de rendirnos (`cc` lo busca por PATH; rustc localiza `link.exe` igual de solo).
    if let Some(bin) = crate::toolchain::augment_build_path(&mut cmd, needs_clang) {
        eprintln!("note: clang is off the PATH — adding {} for this build", bin.display());
    }
    match run_cargo_teeing(&mut cmd) {
        Ok((s, _log)) if s.success() => {
            let sub = if release { "release" } else { "debug" };
            // Con `--target`, cargo pone el artefacto en `target/<triple>/<profile>/…`. El
            // staticlib se llama `lib<pkg con -→_>.a` (calculado, jamás un glob: pkg lleva hash).
            // M168: el target efectivo decide la extensión — en Windows (host o `--target`) el
            // binario es `<pkg>.exe` y el staticlib `<pkg>.lib` (sin prefijo `lib`, convención msvc).
            let for_windows = target.map_or(cfg!(windows), |t| t.contains("windows"));
            let artifact = if lib_mode {
                if android {
                    format!("lib{}.so", pkg.replace('-', "_"))
                } else if for_windows {
                    format!("{}.lib", pkg.replace('-', "_"))
                } else {
                    format!("lib{}.a", pkg.replace('-', "_"))
                }
            } else if for_windows {
                format!("{pkg}.exe")
            } else {
                pkg.clone()
            };
            let produced = match target {
                Some(t) => target_dir.join(t).join(sub).join(&artifact),
                None => target_dir.join(sub).join(&artifact),
            };
            // Copia a `<out>.tmp` + rename, nunca in-place (IDEAS §77: SIGKILL en macOS al sobrescribir
            // el inode de un binario firmado).
            let tmp_bin = format!("{out_bin}.tmp");
            let copied = std::fs::copy(&produced, &tmp_bin).and_then(|_| replace_output_binary(&tmp_bin, out_bin));
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
        Ok((s, log)) => {
            // Falló: se CONSERVA el proyecto y se nombra su ruta, para inspeccionar el Rust generado.
            eprintln!(
                "native build: cargo failed (code {}); project at {}",
                s.code().unwrap_or(-1),
                proj.display()
            );
            // M184: red de seguridad de la comprobación previa — si el log delata una herramienta
            // que cargo no encontró, el remedio se dice AQUÍ, no se deja enterrado 200 líneas atrás.
            if let Some(hint) = native_failure_hint(&log, &triple) {
                eprintln!("hint: {hint}");
            }
            process::exit(65);
        }
        Err(e) => {
            eprintln!("native build: could not run cargo ({}): {e}", cmd.get_program().to_string_lossy());
            process::exit(65);
        }
    }
}

/// M184: lanza cargo **retransmitiendo su salida en vivo** y guardando a la vez su cola, para poder
/// traducir el fallo a un remedio sin que el usuario pierda el progreso. Con stderr por tubería cargo
/// apaga el color solo: se lo pedimos de vuelta cuando NUESTRO stderr sí es un terminal.
fn run_cargo_teeing(cmd: &mut process::Command) -> std::io::Result<(process::ExitStatus, String)> {
    use std::io::{BufRead, IsTerminal, Write};
    /// Líneas de cola que se guardan: de sobra para el error final de cargo y sus `Caused by`.
    const TAIL: usize = 600;
    if std::io::stderr().is_terminal() {
        cmd.arg("--color=always");
    }
    cmd.stderr(process::Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut kept: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    if let Some(err) = child.stderr.take() {
        let mut reader = std::io::BufReader::new(err);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let mut out = std::io::stderr();
            let _ = out.write_all(&line);
            let _ = out.flush();
            kept.push_back(String::from_utf8_lossy(&line).into_owned());
            if kept.len() > TAIL {
                kept.pop_front();
            }
        }
    }
    let status = child.wait()?;
    Ok((status, kept.into_iter().collect()))
}

/// M184: traduce el log de un build nativo fallido al remedio, cuando lo que falló fue una
/// herramienta ausente y no el programa del usuario. La comprobación previa (arriba) cubre lo que
/// sabemos enumerar; esto cubre lo que no. Pura —recibe log y target— para testearla.
fn native_failure_hint(log: &str, triple: &str) -> Option<String> {
    let missing = |tool: &str| log.contains(&format!("failed to find tool \"{tool}\""));
    if missing("clang") || missing("clang-cl") {
        return Some(format!(
            "cargo could not find clang (`ring` compiles its assembly with clang on {triple}): install \
             LLVM (`winget install LLVM.LLVM`); `ray toolchain status` shows what ray sees"
        ));
    }
    if missing("link") || log.contains("linker `link.exe` not found") {
        return Some(
            "cargo could not find the MSVC linker: install the Visual Studio Build Tools (C++ workload)"
                .to_string(),
        );
    }
    if missing("cl") || missing("cc") || log.contains("linker `cc` not found") {
        return Some(format!(
            "cargo could not find a C compiler for {triple} (the runtime's C dependencies — mimalloc, \
             ring, sqlite — need one); `ray toolchain status` shows what ray sees"
        ));
    }
    None
}

/// `ray test [archivo] [filtro]`: corre las funciones `@test` (a nivel proyecto, M101).
/// Sin archivo explícito, las suites son la **entrada del proyecto** (sus `@test` y las de todos
/// los módulos que importa) más cada **`tests/*.ray`** junto al `ray.toml` (pruebas de
/// integración: importan los módulos del proyecto porque la raíz de la entrada va como raíz
/// extra del loader). Un primer argumento que no termina en `.ray` se toma como filtro.
fn cmd_test_sub(args: &[String]) {
    let (watch, args) = take_flag_bool(args, "--watch");
    if watch {
        cmd_test_watch(&args);
    }
    let (explicit, filter) = split_test_args(&args);
    let (suites, roots) = test_suites_and_roots(&explicit);
    process::exit(test_runner::run(&suites, &roots, filter.as_deref()));
}

/// Separa los argumentos de `ray test`: los `.ray` iniciales son suites explícitas (una o
/// VARIAS, M141 — la vía del watch selectivo) y el primer argumento que no termina en `.ray`
/// es el filtro por nombre.
fn split_test_args(args: &[String]) -> (Vec<String>, Option<String>) {
    let mut explicit = Vec::new();
    let mut idx = 0;
    while let Some(a) = args.get(idx) {
        if !a.ends_with(".ray") {
            break;
        }
        explicit.push(a.clone());
        idx += 1;
    }
    (explicit, args.get(idx).cloned())
}

/// Las suites y raíces de una invocación de `ray test`: las explícitas tal cual o, sin
/// explícitas, la entrada del proyecto más cada `tests/*.ray`. La raíz de cada entrada
/// implicada va como raíz extra del loader (un `tests/*.ray` resuelve `import m;` contra
/// `src/`).
fn test_suites_and_roots(explicit: &[String]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut suites: Vec<PathBuf> = Vec::new();
    if explicit.is_empty() {
        let entry = resolve_entry(None, false);
        suites.push(PathBuf::from(&entry));
        let root = load_manifest().map(|m| m.root).unwrap_or_else(|| PathBuf::from("."));
        suites.extend(discover_test_files(&root.join("tests")));
    } else {
        for f in explicit {
            suites.push(PathBuf::from(resolve_entry(Some(f), false)));
        }
    }
    let mut roots = dependency_roots();
    // Solo las entradas-ANCLA aportan raíz: la del proyecto (modo implícito) o cada explícita —
    // los `tests/*.ray` descubiertos no (su raíz útil es la de la entrada, no `tests/`).
    let anchors = if explicit.is_empty() { 1 } else { suites.len() };
    for s in suites.iter().take(anchors) {
        if let Some(parent) = s.parent().map(Path::to_path_buf)
            && !roots.contains(&parent)
        {
            roots.push(parent);
        }
    }
    (suites, roots)
}

/// `ray test --watch [args…]` (M140): el bucle de dev aplicado al runner — re-corre la suite ante
/// cada cambio en los fuentes del proyecto (misma detección por eventos de kernel, debounce y
/// confirmación por hash que `ray dev`). Diferencias deliberadas con dev: SIN check-before-restart
/// (no hay servidor que proteger — la propia corrida muestra el diagnóstico de compilación) y SIN
/// hub de live-reload ni socket-activation. Un cambio a MITAD de corrida la corta y re-corre.
/// Entre corridas, `q` sale (tecla única, como en dev).
fn cmd_test_watch(args: &[String]) -> ! {
    let exe = env::current_exe().unwrap_or_else(|_| PathBuf::from("ray"));
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = Manifest::find(&cwd)
        .and_then(|toml| toml.parent().map(Path::to_path_buf))
        .or_else(|| {
            args.iter()
                .find(|a| a.ends_with(".ray"))
                .and_then(|a| Path::new(a).parent().map(Path::to_path_buf))
                .filter(|p| !p.as_os_str().is_empty())
        })
        .unwrap_or(cwd);
    eprintln!("[watch] watching {} (.ray, .ray.html, ray.toml); Ctrl-C to exit", root.display());
    install_cleanup_on_death();

    let (explicit, filter) = split_test_args(args);
    // Baseline del termios (como en dev): si una corrida cortada dejó el terminal cambiado
    // (un test en crudo matado a mitad), se repone antes de re-correr.
    let baseline_tty = crate::builtins::term_attrs_fingerprint();
    let mut snapshot = scan_sources(&root);
    let mut hashes = content_hashes(&snapshot);
    let mut watcher = DevWatcher::new(&root);
    let mut first = true;
    // Qué correr en la PRÓXIMA corrida (M141): `None` = todo (los args originales); `Some(sel)`
    // = solo las suites afectadas por el último cambio. La primera corrida siempre es completa.
    let mut selection: Option<Vec<PathBuf>> = None;
    loop {
        // Entre corridas, la pantalla se limpia (convención de los watch de tests) — solo en un
        // terminal: bajo un pipe/CI el scroll completo es el registro.
        if !first && std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            print!("\x1b[H\x1b[2J");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
        first = false;
        let run_args: Vec<String> = match &selection {
            Some(subset) => {
                subset.iter().map(|p| p.display().to_string()).chain(filter.clone()).collect()
            }
            None => args.to_vec(),
        };
        let mut cmd = process::Command::new(&exe);
        cmd.arg("test").args(&run_args);
        let mut child = spawn_supervised(cmd, "[watch] could not launch the tests");
        // La corrida en marcha: termina sola, o un cambio la corta para re-correr ya. El reap
        // (`try_wait`) va PRIMERO: un cambio que llega justo cuando la corrida acaba de terminar
        // debe tratarse por la vía de espera (con su gate de hash), no como corte a mitad —
        // cortar re-corre sin gate (la corrida quedó trunca) y un guardado idéntico re-correría.
        let interrupted = loop {
            if let Ok(Some(_)) = child.try_wait() {
                DEV_CHILD.store(0, std::sync::atomic::Ordering::SeqCst);
                break None;
            }
            if let Some(c) = watcher.wait_change(&root, &mut snapshot) {
                break Some(c);
            }
        };
        if let Some((path, label)) = interrupted {
            terminate_gracefully(&mut child);
            if let (Some(base), Some(now)) =
                (baseline_tty.as_ref(), crate::builtins::term_attrs_fingerprint())
                && now != *base
            {
                let _ = crate::builtins::term_attrs_restore(base);
            }
            DEV_CHILD.store(0, std::sync::atomic::Ordering::SeqCst);
            watcher.debounce(&root, &mut snapshot);
            hashes = content_hashes(&snapshot);
            selection = affected_test_suites(&explicit, &path);
            announce_rerun(&label, &selection, &explicit);
            continue;
        }
        // Corrida terminada (el runner ya imprimió su resumen): esperar el próximo cambio o `q`.
        let tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
        if tty {
            eprintln!("\r[watch] waiting for changes… (press q to exit)");
        } else {
            eprintln!("\r[watch] waiting for changes… (q⏎ or Ctrl-C exits)");
        }
        let mut keys_armed = tty && crate::builtins::term_raw_on().is_ok();
        loop {
            let (path, change) = loop {
                if let Some(c) = watcher.wait_change(&root, &mut snapshot) {
                    break c;
                }
                if keys_armed && dev_raw_key_quit() {
                    let _ = crate::builtins::term_raw_off();
                    eprintln!("\r[watch] bye");
                    process::exit(0);
                }
                if !keys_armed && dev_stdin_quit() {
                    eprintln!("\r[watch] bye");
                    process::exit(0);
                }
            };
            if keys_armed {
                let _ = crate::builtins::term_raw_off();
                keys_armed = false;
            }
            watcher.debounce(&root, &mut snapshot);
            let current = content_hashes(&snapshot);
            if current == hashes {
                // Un mtime tocado con los mismos bytes (guardado sin editar, formateador
                // idempotente): ni re-corre ni sale del estado de espera.
                eprintln!("\r[watch] change in {change}: contents unchanged — ignoring");
                keys_armed = tty && crate::builtins::term_raw_on().is_ok();
                continue;
            }
            hashes = current;
            selection = affected_test_suites(&explicit, &path);
            announce_rerun(&change, &selection, &explicit);
            break;
        }
    }
}

/// Anuncia la re-corrida: completa, o selectiva con su conteo (M141).
fn announce_rerun(label: &str, selection: &Option<Vec<PathBuf>>, explicit: &[String]) {
    match selection {
        Some(subset) => {
            let total = test_suites_and_roots(explicit).0.len();
            eprintln!("\r[watch] change in {label}: re-running {} of {total} suite(s)…", subset.len());
        }
        None => eprintln!("\r[watch] change in {label}: re-running…"),
    }
}

/// Las suites afectadas por un cambio en `changed` (M141, la selección del watch): las que lo
/// contienen en su grafo de imports — calculado FRESCO con el loader, así un import recién
/// añadido ya cuenta — más las que NO cargan (import roto a medio editar: siguen corriendo y
/// mostrando su diagnóstico hasta sanar). `None` = correr todo, la vía segura: `ray.toml` (o un
/// manifiesto roto a medio editar), un archivo borrado, uno que ningún grafo conoce (módulo
/// nuevo a medio cablear), una sola suite (la selección no aporta), o todas afectadas.
fn affected_test_suites(explicit: &[String], changed: &Path) -> Option<Vec<PathBuf>> {
    if changed.file_name().is_some_and(|n| n == "ray.toml") {
        return None;
    }
    // Un manifiesto ilegible mataría al supervisor dentro de `test_suites_and_roots`
    // (`load_manifest` aborta): mejor correr todo y que la corrida muestre el error.
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if Manifest::load(&cwd).is_err() {
        return None;
    }
    let canon = fs::canonicalize(changed).ok()?; // borrado → todo
    let (suites, roots) = test_suites_and_roots(explicit);
    if suites.len() <= 1 {
        return None;
    }
    let mut affected = Vec::new();
    let mut known_hit = false;
    for suite in &suites {
        match crate::loader::load_with_deps(suite, &roots) {
            Ok(loaded) => {
                let hit = loaded
                    .modules
                    .iter()
                    .any(|m| fs::canonicalize(&m.path).is_ok_and(|p| p == canon));
                if hit {
                    known_hit = true;
                    affected.push(suite.clone());
                }
            }
            Err(_) => affected.push(suite.clone()),
        }
    }
    if !known_hit || affected.len() == suites.len() {
        return None;
    }
    Some(affected)
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
    // M169: en Windows el directorio del usuario es USERPROFILE (HOME no suele existir).
    let home = env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
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
/// M147: fija la config de `std/embed` (raíz + dirs de `[native] embed`) desde el proyecto del
/// ENTRY — no del cwd: `ray run otra/app/src/main.ray` resuelve contra SU raíz. Sin manifiesto
/// o sin `[native] embed`, no se fija nada (std/embed responde Err de sin-config).
pub(crate) fn configure_embed(entry: &str) {
    // Canonicalizar primero: los ancestros de una ruta relativa terminan en "" (raíz vacía).
    let dir = match Path::new(entry).parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let dir = dir.canonicalize().unwrap_or(dir);
    if let Ok(Some(m)) = Manifest::load(&dir)
        && !m.native_embed.is_empty()
    {
        let root = m.root.canonicalize().unwrap_or_else(|_| m.root.clone());
        crate::builtins::set_embed_config(root, m.native_embed);
    }
}

fn run_file(path: &str, prog_args: Vec<String>, use_interp: bool, fuel: Option<u64>, heap: Option<usize>) {
    if (fuel.is_some() || heap.is_some()) && use_interp {
        eprintln!("--fuel/--heap are VM limits (product engine); they do not apply with --interp");
        process::exit(64);
    }
    runtime::set_program_args(prog_args);
    configure_embed(path);
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

    #[cfg(all(feature = "watch", any(unix, windows)))]
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
        assert!(!is_watched_source(&root, &root.join("src/.!92067!main.ray")));
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
        assert!(w.wait_change(&dir, &mut snapshot).is_none(), "sin cambios no hay cambio");
        fs::write(dir.join("main.ray"), "fn main() { print(1); }\n").unwrap();
        let change = w.wait_change(&dir, &mut snapshot);
        assert!(
            change.is_some_and(|(p, _)| p.ends_with("main.ray")),
            "el cambio debe detectarse"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn host_triple_names_the_host_arch_and_os() {
        // M182: la puerta de las fibras decide por arquitectura y SO del target efectivo.
        // M184: el triple sale de `rustc -vV` — y estos tests los compila ese mismo rustc, así que
        // su `host:` es la arquitectura con la que se construyó este binario de test.
        let t = host_triple();
        assert!(t.starts_with(std::env::consts::ARCH), "{t}");
        assert!(t.contains(if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "darwin" } else { std::env::consts::OS }), "{t}");
    }

    #[test]
    fn rustc_host_line_is_the_source_of_truth() {
        // M184: `rustc -vV` es la autoridad sobre el target efectivo, no `env::consts::ARCH` (un
        // `ray.exe` x86_64 emulado en Windows ARM64 decía x86_64 mientras rustc compilaba aarch64).
        let vv = "rustc 1.98.1 (48a229cea 2026-09-01)\nbinary: rustc\ncommit-hash: 48a2\n\
                  host: aarch64-pc-windows-msvc\nrelease: 1.98.1\n";
        assert_eq!(parse_rustc_host(vv).as_deref(), Some("aarch64-pc-windows-msvc"));
        assert_eq!(parse_rustc_host("rustc 1.98.1\nrelease: 1.98.1\n"), None, "sin línea host");
        assert_eq!(parse_rustc_host("host:\n"), None, "host vacío no es un triple");
        // El respaldo sigue nombrando arquitectura y SO.
        let f = fallback_host_triple();
        assert!(f.contains(if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "darwin" } else { std::env::consts::OS }), "{f}");
    }

    #[test]
    fn windows_binaries_always_carry_their_extension() {
        // M186: en Windows un archivo sin `.exe` no es ejecutable desde el Explorador ni la consola
        // (CreateProcess sí lo lanza con ruta completa — por eso los tests no lo cazaron).
        assert_eq!(ensure_windows_extension("raydesk".into(), true, false), "raydesk.exe");
        assert_eq!(ensure_windows_extension("dist/app".into(), true, false), "dist/app.exe");
        // Ya la lleva (en cualquier caja): se respeta tal cual, sin duplicarla.
        assert_eq!(ensure_windows_extension("app.exe".into(), true, false), "app.exe");
        assert_eq!(ensure_windows_extension("app.EXE".into(), true, false), "app.EXE");
        // Otra extensión NO basta en Windows: `app.bin` tampoco arranca → se añade la que vale.
        assert_eq!(ensure_windows_extension("app.bin".into(), true, false), "app.bin.exe");
        // `--lib` produce el staticlib de MSVC.
        assert_eq!(ensure_windows_extension("prog".into(), true, true), "prog.lib");
        assert_eq!(ensure_windows_extension("prog.lib".into(), true, true), "prog.lib");
        // Fuera de un target Windows no se toca nada: `hello` es el nombre idiomático.
        assert_eq!(ensure_windows_extension("hello".into(), false, false), "hello");
        assert_eq!(ensure_windows_extension("libprog.a".into(), false, true), "libprog.a");
    }

    #[test]
    fn native_failure_hint_translates_missing_tools() {
        // M184: el fallo real de RayDesk en Windows ARM64 — clang ausente, enterrado en el log.
        let log = "  error occurred in cc-rs: failed to find tool \"clang\": program not found\n\
                   error: failed to run custom build command for `ring v0.17.14`\n";
        let hint = native_failure_hint(log, "aarch64-pc-windows-msvc").expect("debe reconocer clang");
        assert!(hint.contains("clang") && hint.contains("LLVM"), "{hint}");
        // El linker de MSVC y el compilador de C tienen su propio remedio.
        assert!(native_failure_hint("error: linker `link.exe` not found\n", "x86_64-pc-windows-msvc")
            .is_some_and(|h| h.contains("Build Tools")));
        assert!(native_failure_hint("error: linker `cc` not found\n", "x86_64-unknown-linux-gnu")
            .is_some_and(|h| h.contains("C compiler")));
        // Un error del PROGRAMA no se disfraza de problema de herramientas.
        assert_eq!(native_failure_hint("error[E0308]: mismatched types\n", "x86_64-apple-darwin"), None);
    }

    #[test]
    fn native_windows_gaps_are_named_from_the_runtime_features() {
        // M169: solo los subsistemas `cfg(unix)` de ray-runtime; el resto (tls, crypto, sqlite,
        // regex, mimalloc, señales desde M168, procesos desde M175) compila en Windows.
        // M181: `watch` también compila en Windows — la lista de huecos está vacía.
        assert!(native_unsupported_on_windows(&["tls", "crypto", "sqlite", "regex", "mimalloc", "process", "ui", "ui-shell", "audio", "watch"]).is_empty());
        assert!(native_unsupported_on_windows(&[]).is_empty());
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
        // Windows va por zip; M185: también en ARM64 (antes se instalaba la x86_64 emulada).
        assert_eq!(
            upgrade_asset("windows", "x86_64").as_deref(),
            Some("raylang-x86_64-pc-windows-msvc.zip")
        );
        assert_eq!(
            upgrade_asset("windows", "aarch64").as_deref(),
            Some("raylang-aarch64-pc-windows-msvc.zip")
        );
        // Una arquitectura desconocida no tiene asset.
        assert_eq!(upgrade_asset("linux", "riscv64"), None);
        assert_eq!(upgrade_asset("windows", "riscv64"), None);
    }

    #[test]
    fn an_emulated_build_is_told_about_its_native_twin() {
        // M187: el caso real — un `ray` x86_64 corriendo en una máquina Windows ARM64.
        let note = emulated_arch_note("windows", "x86_64", "aarch64").expect("hay build nativa");
        assert!(note.contains("raylang-aarch64-pc-windows-msvc.zip"), "{note}");
        assert!(note.contains("install.ps1"), "en Windows se reinstala con install.ps1\n{note}");
        assert!(note.contains("keeps the current architecture"), "dice qué NO hace upgrade\n{note}");
        // En unix el remedio es el otro instalador.
        assert!(
            emulated_arch_note("macos", "x86_64", "aarch64").is_some_and(|n| n.contains("install.sh")),
            "en unix se reinstala con install.sh"
        );
        // Sin emulación no hay nada que contar, y tampoco si la máquina no tiene asset.
        assert_eq!(emulated_arch_note("windows", "aarch64", "aarch64"), None);
        assert_eq!(emulated_arch_note("linux", "x86_64", "riscv64"), None, "sin build nativa, sin aviso");
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
