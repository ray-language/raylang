//! CLI de raylang.
//!
//! Uso: `raylang [--vm] <archivo.ray>`
//!
//! Corre el pipeline: lexer → parser → checker, y luego ejecuta el programa con el
//! **intérprete** (por defecto) o con la **máquina virtual** (`--vm`). El código de
//! salida del proceso es el entero que devuelve `main` (0 si es unit).

use std::env;
use std::fs;
use std::process;

use raylang::interpreter::Value;
use raylang::{checker, compiler, interpreter, lexer, parser, vm};

fn main() {
    let args: Vec<String> = env::args().collect();
    let (use_vm, path) = match args.len() {
        2 => (false, args[1].clone()),
        3 if args[1] == "--vm" => (true, args[2].clone()),
        _ => {
            eprintln!("uso: raylang [--vm] <archivo.ray>");
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

    // Front-end: lexer → parser → checker.
    let tokens = match lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            process::exit(65);
        }
    };
    let program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e);
            process::exit(65);
        }
    };
    if let Err(e) = checker::check(&program) {
        eprintln!("{}", e);
        process::exit(65);
    }

    // Backend: intérprete (M1) o VM (M2).
    let result = if use_vm {
        match compiler::compile_program(&program) {
            Ok(compiled) => vm::run_program(&compiled),
            Err(e) => {
                eprintln!("{}", e);
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
            eprintln!("{}", e);
            process::exit(70); // EX_SOFTWARE
        }
    }
}
