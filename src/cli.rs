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
        Some("fmt") => cmd_fmt(&rest[1..]),
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
  run [archivo]     ejecuta (por defecto src/main.ray) [--interp] [args...]
  build [archivo]   chequea y compila sin ejecutar (0 ok / 65 error)
  test [archivo]    corre las funciones @test [filtro]
  fmt <archivo>     imprime la versión canónica por stdout
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
    let Some(nombre) = args.first() else {
        eprintln!("uso: ray new <nombre>");
        process::exit(64);
    };
    let raiz = Path::new(nombre);
    if raiz.exists() {
        eprintln!("'{nombre}' ya existe");
        process::exit(65);
    }
    let manifiesto = format!(
        "[package]\nname = \"{nombre}\"\nversion = \"0.1.0\"\n\n[dependencies]\n"
    );
    let main_ray = format!("fn main() -> int {{\n    print(\"hola desde {nombre}\");\n    0\n}}\n");
    let gitignore = "# dependencias descargadas por el gestor de paquetes (M39c)\n.ray-deps/\n";
    let escribir = |ruta: std::path::PathBuf, contenido: &str| {
        if let Some(padre) = ruta.parent()
            && let Err(e) = fs::create_dir_all(padre)
        {
            eprintln!("no se pudo crear '{}': {e}", padre.display());
            process::exit(73); // EX_CANTCREAT
        }
        if let Err(e) = fs::write(&ruta, contenido) {
            eprintln!("no se pudo escribir '{}': {e}", ruta.display());
            process::exit(73);
        }
    };
    escribir(raiz.join("ray.toml"), &manifiesto);
    escribir(raiz.join("src/main.ray"), &main_ray);
    escribir(raiz.join(".gitignore"), gitignore);
    println!("proyecto '{nombre}' creado. Para correrlo:\n  cd {nombre} && ray run");
}

/// `ray run [--interp] [archivo] [args...]`: ejecuta el programa. Sin archivo usa
/// `src/main.ray` (convención de proyecto). Los args tras el archivo van a `args()`.
fn cmd_run(args: &[String]) {
    let (use_interp, resto) = tomar_interp(args);
    let (explicito, prog_args) = match resto.split_first() {
        Some((p, rest)) => (Some(p.as_str()), rest.to_vec()),
        None => (None, Vec::new()),
    };
    let path = resolver_entrada(explicito, false);
    ejecutar(&path, prog_args, use_interp);
}

/// `ray build [archivo]`: chequea y **compila** el programa sin ejecutarlo (útil para CI y
/// para validar antes de publicar). Sale 0 si compila, 65 si hay errores de compilación.
fn cmd_build(args: &[String]) {
    let path = resolver_entrada(args.first().map(String::as_str), true);
    let (mut program, locate, multi) = cargar_y_localizar(&path);
    verificar_o_salir(&mut program, &locate, multi);
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
    let path = resolver_entrada(args.first().map(String::as_str), false);
    let filtro = args.get(1).map(String::as_str);
    ejecutar_tests(&path, filtro);
}

/// `ray fmt <archivo>`: imprime la versión canónica por stdout.
fn cmd_fmt(args: &[String]) {
    let Some(path) = args.first() else {
        eprintln!("uso: ray fmt <archivo>");
        process::exit(64);
    };
    formatear(path);
}

// ── Modo legado (compatibilidad con la interfaz por flags) ───────────────────────────

fn legacy(rest: &[String]) {
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
        formatear(&rest[1]);
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
        ejecutar_tests(&path, rest.get(idx + 1).map(String::as_str));
    } else {
        ejecutar(&path, rest[idx + 1..].to_vec(), use_interp);
    }
}

// ── Piezas compartidas ───────────────────────────────────────────────────────────────

/// Resuelve el archivo a procesar (run/build/test) y el contexto de proyecto (M39b).
/// `explicito`: el archivo dado en la línea de comandos, si lo hay. `banner`: imprime
/// "compilando <nombre> v<versión>" (para `build`). Prioridad: (1) el archivo explícito;
/// (2) la entrada del manifiesto (`ray.toml` subiendo desde el cwd); (3) `src/main.ray` en
/// el cwd; si nada, error de uso. Avisa —una vez— si el manifiesto declara dependencias
/// (aún no se resuelven, M39c).
fn resolver_entrada(explicito: Option<&str>, banner: bool) -> String {
    let manifiesto = cargar_manifiesto();
    if let Some(m) = &manifiesto {
        if banner {
            eprintln!("compilando {} v{}", m.name, m.version);
        }
        if !m.dependencies.is_empty() {
            eprintln!(
                "aviso: '{}' declara {} dependencia(s), pero su resolución llega en M39c; se ignoran.",
                m.name,
                m.dependencies.len()
            );
        }
    }
    if let Some(p) = explicito {
        return p.to_string();
    }
    if let Some(m) = &manifiesto {
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

/// Carga el manifiesto del proyecto que contiene el directorio actual. `None` si no hay
/// proyecto; un `ray.toml` mal formado aborta con 65 (error de compilación de la config).
fn cargar_manifiesto() -> Option<Manifest> {
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
fn tomar_interp(args: &[String]) -> (bool, Vec<String>) {
    match args.split_first() {
        Some((f, rest)) if f == "--interp" => (true, rest.to_vec()),
        _ => (false, args.to_vec()),
    }
}

/// Lee el fuente de un archivo o aborta con el código de E/S adecuado.
fn leer_fuente(path: &str) -> String {
    match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("no se pudo leer '{}': {}", path, e);
            process::exit(66); // EX_NOINPUT
        }
    }
}

/// Runner de `@test` (M10.1): sale con el número de fallos como código.
fn ejecutar_tests(path: &str, filtro: Option<&str>) {
    process::exit(test_runner::run(&leer_fuente(path), filtro));
}

/// Formateador (M29.2): imprime la versión canónica o aborta con el error.
fn formatear(path: &str) {
    match crate::fmt::format_source(&leer_fuente(path)) {
        Ok(out) => print!("{}", out),
        Err(e) => {
            eprintln!("error de formato: {}", e);
            process::exit(65);
        }
    }
}

/// Un localizador de líneas globales→(fuente del módulo, nombre, línea local), para
/// renderizar errores contra el archivo correcto en programas multi-módulo (L3).
type Locate = Box<dyn Fn(usize) -> (String, String, usize)>;

/// Carga el archivo de entrada y sus imports (loader, M11.3), devolviendo el programa
/// fusionado, un localizador de líneas y si hay más de un módulo.
fn cargar_y_localizar(path: &str) -> (crate::ast::Program, Locate, bool) {
    let loaded = match loader::load(Path::new(path)) {
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
        (m.source.clone(), m.name.clone(), gline - m.start_line + 1)
    });
    (loaded.program, locate, multi)
}

/// Chequea el programa; si falla, re-corre la variante acumuladora y muestra TODOS los
/// errores (M33c) contra su módulo, y sale con 65.
fn verificar_o_salir(program: &mut crate::ast::Program, locate: &Locate, multi: bool) {
    let backup = program.clone();
    if checker::check(program).is_err() {
        let mut copia = backup;
        for mut e in checker::check_all(&mut copia) {
            let (source, name, local) = locate(e.line);
            e.line = local;
            let head = if multi { format!("[{}] {}", name, e) } else { e.to_string() };
            eprintln!("{}", diagnostic::render(&source, local, e.col, e.len, &head));
        }
        process::exit(65);
    }
}

/// Carga, chequea y ejecuta un archivo (VM por defecto, `--interp` para el intérprete).
fn ejecutar(path: &str, prog_args: Vec<String>, use_interp: bool) {
    runtime::set_program_args(prog_args);
    let (mut program, locate, multi) = cargar_y_localizar(path);
    verificar_o_salir(&mut program, &locate, multi);

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
            Ok(compiled) => vm::run_program(&compiled),
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
