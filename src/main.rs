//! CLI de raylang.
//!
//! Uso: `raylang <archivo.ray>`
//!
//! En esta fase (parser) el CLI tokeniza y parsea el archivo, y vuelca el AST
//! resultante con el Debug "bonito". Es una herramienta para ver el front-end.

use std::env;
use std::fs;
use std::process;

use raylang::{lexer, parser};

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

    // Fase 1: lexer (texto → tokens).
    let tokens = match lexer::lex(&src) {
        Ok(tokens) => tokens,
        Err(e) => {
            eprintln!("{}", e);
            process::exit(65);
        }
    };

    // Fase 2: parser (tokens → AST).
    match parser::parse(tokens) {
        Ok(program) => println!("{:#?}", program),
        Err(e) => {
            eprintln!("{}", e);
            process::exit(65);
        }
    }
}
