//! CLI de raylang.
//!
//! Uso: `raylang <archivo.ray>`
//!
//! En esta fase (checker) el CLI tokeniza, parsea y verifica los tipos del
//! archivo, e informa si el programa es válido.

use std::env;
use std::fs;
use std::process;

use raylang::{checker, lexer, parser};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("uso: raylang <archivo.ray>");
        process::exit(64);
    }

    let path = &args[1];
    let src = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("no se pudo leer '{}': {}", path, e);
            process::exit(66);
        }
    };

    // Fase 1: lexer.
    let tokens = match lexer::lex(&src) {
        Ok(tokens) => tokens,
        Err(e) => {
            eprintln!("{}", e);
            process::exit(65);
        }
    };

    // Fase 2: parser.
    let program = match parser::parse(tokens) {
        Ok(program) => program,
        Err(e) => {
            eprintln!("{}", e);
            process::exit(65);
        }
    };

    // Fase 3: checker (análisis semántico / tipos).
    match checker::check(&program) {
        Ok(()) => {
            println!("✓ {}: {} función(es), tipos verificados", path, program.functions.len());
        }
        Err(e) => {
            eprintln!("{}", e);
            process::exit(65);
        }
    }
}
