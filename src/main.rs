//! CLI de raylang.
//!
//! Uso: `raylang [--vm] <archivo.ray>`  — ejecuta un archivo.
//!      `raylang --test <archivo.ray>`  — corre las funciones `@test` (M10.1).
//!      `raylang --lsp`                  — arranca el Language Server (M10.2).
//!      `raylang`  (o `raylang --repl`)  — arranca el REPL interactivo (M8.2).
//!
//! Corre el pipeline: lexer → parser → checker, y luego ejecuta el programa con el
//! **intérprete** (por defecto) o con la **máquina virtual** (`--vm`). El código de
//! salida del proceso es el entero que devuelve `main` (0 si es unit).

use std::env;
use std::fs;
use std::process;

use raylang::interpreter::Value;
use raylang::{checker, compiler, diagnostic, interpreter, loader, lsp, repl, test_runner, vm};

fn main() {
    // M13.3a: todo el trabajo corre en un hilo con pila grande, para que la recursión
    // profunda (parser de descenso recursivo, intérprete tree-walking) dé un error
    // limpio (al tope de `MAX_CALL_DEPTH`) en vez de desbordar la pila y morir con
    // SIGSEGV. `run` siempre acaba en `process::exit`, así que el `join` no retorna.
    raylang::with_big_stack_or_ice(run);
}

fn run() {
    let args: Vec<String> = env::args().collect();
    let rest = &args[1..]; // sin el nombre del binario

    // Modos sin archivo: REPL interactivo (M8.2) y LSP (M10.2).
    if rest.is_empty() || (rest.len() == 1 && rest[0] == "--repl") {
        repl::run();
        return;
    }
    if rest.len() == 1 && rest[0] == "--lsp" {
        // Language Server (M10.2): habla LSP por stdin/stdout hasta `exit`.
        lsp::run();
        return;
    }

    // Formateador (M29.2): `raylang --fmt <archivo>` imprime la versión canónica por stdout.
    if rest.len() == 2 && rest[0] == "--fmt" {
        let path = &rest[1];
        let src = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("no se pudo leer '{}': {}", path, e);
                process::exit(66);
            }
        };
        match raylang::fmt::format_source(&src) {
            Ok(out) => {
                print!("{}", out);
                return;
            }
            Err(e) => {
                eprintln!("error de formato: {}", e);
                process::exit(65);
            }
        }
    }

    // Forma general: raylang [--vm | --test] <archivo.ray> [args del programa...].
    // Una flag opcional al principio, luego la ruta; todo lo que siga son los argumentos del
    // programa (M11.2b), accesibles desde raylang con el builtin `args()`.
    let mut idx = 0;
    let (mut use_vm, mut test_mode) = (false, false);
    match rest[0].as_str() {
        "--vm" => {
            use_vm = true;
            idx = 1;
        }
        "--test" => {
            test_mode = true;
            idx = 1;
        }
        _ => {}
    }
    if idx >= rest.len() {
        eprintln!("uso: raylang [--vm | --test] <archivo.ray> [args...]   |   raylang [--repl | --lsp]");
        process::exit(64); // EX_USAGE
    }
    let path = rest[idx].clone();
    // Los argumentos del programa son lo que sigue a la ruta; se dejan en el almacén de proceso.
    interpreter::set_program_args(rest[idx + 1..].to_vec());

    // Modo prueba (M10.1): corre las funciones `@test` vía un cliente externo (single-file).
    // M13.2b: un argumento tras la ruta filtra las pruebas por nombre (subcadena).
    if test_mode {
        let src = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("no se pudo leer '{}': {}", path, e);
                process::exit(66); // EX_NOINPUT
            }
        };
        let filtro = rest.get(idx + 1).map(|s| s.as_str());
        process::exit(test_runner::run(&src, filtro));
    }

    // M11.3: el loader carga el archivo de entrada y sus `import` (transitivos), y devuelve
    // el `Program` fusionado (módulos ya borrados). Los errores de carga vienen renderizados.
    let loaded = match loader::load(std::path::Path::new(&path)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}", e.message);
            process::exit(65);
        }
    };
    let mut program = loaded.program;
    let modules = loaded.modules;
    let multi = modules.len() > 1;
    // Localiza una línea **global** del programa fusionado: `(fuente del módulo, nombre, línea
    // local)`. Renumerar a la línea local hace que el error se dibuje contra el archivo correcto
    // con su número de línea real (L3).
    let locate = |gline: usize| -> (String, String, usize) {
        let m = modules.iter().rev().find(|m| m.start_line <= gline).unwrap_or(&modules[0]);
        (m.source.clone(), m.name.clone(), gline - m.start_line + 1)
    };

    // El checker resuelve la construcción de enums sobre el AST (lo muta), así que
    // el intérprete y la VM reciben un programa ya resuelto.
    // M33c: si la verificación falla, se re-corre la variante acumuladora sobre una copia
    // previa del programa y se muestran TODOS los errores (cada uno localizado contra su
    // módulo, L3), no solo el primero. El camino feliz sigue costando un solo `check`.
    let backup = program.clone();
    if checker::check(&mut program).is_err() {
        let mut copia = backup;
        for mut e in checker::check_all(&mut copia) {
            let (source, name, local) = locate(e.line);
            e.line = local; // que el `Display` del error muestre la línea local, no la global
            let head = if multi { format!("[{}] {}", name, e) } else { e.to_string() };
            eprintln!("{}", diagnostic::render(&source, local, e.col, e.len, &head));
        }
        process::exit(65);
    }
    drop(backup);

    // Backend: intérprete (M1) o VM (M2).
    let result = if use_vm {
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
    } else {
        interpreter::run(&program)
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
