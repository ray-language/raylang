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
    if test_mode {
        let src = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("no se pudo leer '{}': {}", path, e);
                process::exit(66); // EX_NOINPUT
            }
        };
        process::exit(test_runner::run(&src));
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
    if let Err(mut e) = checker::check(&mut program) {
        let (source, name, local) = locate(e.line);
        e.line = local; // que el `Display` del error muestre la línea local, no la global
        let head = if multi { format!("[{}] {}", name, e) } else { e.to_string() };
        eprintln!("{}", diagnostic::render(&source, local, e.col, &head));
        process::exit(65);
    }

    // Backend: intérprete (M1) o VM (M2).
    let result = if use_vm {
        match compiler::compile_program(&program) {
            Ok(compiled) => vm::run_program(&compiled),
            Err(mut e) => {
                let (source, name, local) = locate(e.line);
                e.line = local;
                let head = if multi { format!("[{}] {}", name, e) } else { e.to_string() };
                eprintln!("{}", diagnostic::render(&source, local, e.col, &head));
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
            eprintln!("{}", diagnostic::render(&source, local, e.col, &head));
            process::exit(70); // EX_SOFTWARE
        }
    }
}
