//! CLI de raylang.
//!
//! Uso: `raylang <archivo.ray>`
//!
//! Corre el pipeline completo de M1: lexer → parser → checker → intérprete. Si el
//! programa es válido, lo ejecuta. El código de salida del proceso es el entero
//! que devuelve `main` (como en C); si `main` devuelve unit, es 0.

use std::env;
use std::fs;
use std::process;

use raylang::interpreter::Value;
use raylang::{checker, interpreter, lexer, parser};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("uso: raylang <archivo.ray>");
        process::exit(64); // EX_USAGE
    }

    let path = &args[1];
    let src = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("no se pudo leer '{}': {}", path, e);
            process::exit(66); // EX_NOINPUT
        }
    };

    // Fase 1: lexer (texto → tokens).
    let tokens = match lexer::lex(&src) {
        Ok(tokens) => tokens,
        Err(e) => {
            eprintln!("{}", e);
            process::exit(65); // EX_DATAERR
        }
    };

    // Fase 2: parser (tokens → AST).
    let program = match parser::parse(tokens) {
        Ok(program) => program,
        Err(e) => {
            eprintln!("{}", e);
            process::exit(65);
        }
    };

    // Fase 3: checker (análisis semántico / tipos).
    if let Err(e) = checker::check(&program) {
        eprintln!("{}", e);
        process::exit(65);
    }

    // Fase 4: intérprete (ejecuta). La salida de los `print` aparece en stdout.
    match interpreter::run(&program) {
        Ok(Value::Int(code)) => process::exit((code & 0xFF) as i32),
        Ok(_) => process::exit(0),
        Err(e) => {
            eprintln!("{}", e);
            process::exit(70); // EX_SOFTWARE
        }
    }
}
