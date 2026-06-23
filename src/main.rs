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
use raylang::{checker, compiler, diagnostic, interpreter, lexer, lsp, parser, repl, test_runner, vm};

fn main() {
    let args: Vec<String> = env::args().collect();
    let (use_vm, test_mode, path) = match args.len() {
        // Sin archivo: REPL interactivo (M8.2).
        1 => {
            repl::run();
            return;
        }
        2 if args[1] == "--repl" => {
            repl::run();
            return;
        }
        // Language Server (M10.2): habla LSP por stdin/stdout hasta `exit`.
        2 if args[1] == "--lsp" => {
            lsp::run();
            return;
        }
        2 => (false, false, args[1].clone()),
        3 if args[1] == "--vm" => (true, false, args[2].clone()),
        3 if args[1] == "--test" => (false, true, args[2].clone()),
        _ => {
            eprintln!("uso: raylang [--vm | --test] <archivo.ray>   |   raylang [--repl | --lsp]");
            process::exit(64); // EX_USAGE
        }
    };

    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("no se pudo leer '{}': {}", path, e);
            process::exit(66); // EX_NOINPUT
        }
    };

    // Modo prueba (M10.1): corre las funciones `@test` vía un cliente externo.
    if test_mode {
        process::exit(test_runner::run(&src));
    }

    // Front-end: lexer → parser → checker. Cada error se muestra con su contexto de
    // fuente (M8.3): la línea y un `^` bajo la posición.
    let tokens = match lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", diagnostic::render(&src, e.line, e.col, &e.to_string()));
            process::exit(65);
        }
    };
    let mut program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", diagnostic::render(&src, e.line, e.col, &e.to_string()));
            process::exit(65);
        }
    };
    // El checker resuelve la construcción de enums sobre el AST (lo muta), así que
    // el intérprete y la VM reciben un programa ya resuelto.
    if let Err(e) = checker::check(&mut program) {
        eprintln!("{}", diagnostic::render(&src, e.line, e.col, &e.to_string()));
        process::exit(65);
    }

    // Backend: intérprete (M1) o VM (M2).
    let result = if use_vm {
        match compiler::compile_program(&program) {
            Ok(compiled) => vm::run_program(&compiled),
            Err(e) => {
                eprintln!("{}", diagnostic::render(&src, e.line, e.col, &e.to_string()));
                process::exit(65);
            }
        }
    } else {
        interpreter::run(&program)
    };

    match result {
        Ok(Value::Int(code)) => process::exit((code & 0xFF) as i32),
        Ok(_) => process::exit(0),
        Err(e) => {
            eprintln!("{}", diagnostic::render(&src, e.line, e.col, &e.to_string()));
            process::exit(70); // EX_SOFTWARE
        }
    }
}
