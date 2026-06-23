//! Runner de pruebas `@test` (M10.1).
//!
//! Un **cliente externo** del front-end + intérprete, en el espíritu del REPL (M8.2): usa
//! solo la API pública y **no toca** el checker ni el intérprete. Una función marcada con
//! `@test` debe tener firma `() -> bool` (la valida el checker); aquí se **recolectan** y se
//! ejecutan sintetizando un `main` que las llama e informa `ok`/`FALLO` por cada una.
//!
//! Estrategia (como el REPL): no hay forma pública de "ejecutar la función N", así que se
//! **reescribe `main`**. Se descarta el `main` del usuario (si lo hay), se sintetiza uno que
//! recorre las pruebas, y se verifica y ejecuta el programa resultante. El código de salida
//! es el **número de fallos** (0 = todas pasaron).

use crate::{checker, diagnostic, interpreter, lexer, parser};

/// Ejecuta las pruebas del fuente y devuelve el código de salida (número de fallos, o un
/// código de error si el front-end falla). Imprime el informe y los errores con contexto.
pub fn run(src: &str) -> i32 {
    let tokens = match lexer::lex(src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", diagnostic::render(src, e.line, e.col, &e.to_string()));
            return 65;
        }
    };
    let mut program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", diagnostic::render(src, e.line, e.col, &e.to_string()));
            return 65;
        }
    };

    // Recolectar las funciones marcadas con `@test`, en orden de declaración.
    let tests: Vec<String> = program
        .functions
        .iter()
        .filter(|f| f.annotations.iter().any(|a| a.name == "test"))
        .map(|f| f.name.clone())
        .collect();

    if tests.is_empty() {
        println!("no hay pruebas (@test) en el archivo");
        return 0;
    }

    // Sintetizar un `main` que corre cada prueba. Se parsea como fuente y se extrae la
    // función (el parser no resuelve nombres, así que un `main` que llama a las pruebas
    // parsea aunque estas vivan en el otro programa); luego se inserta en lugar del `main`
    // del usuario.
    let main_src = synth_main(&tests);
    let main_fn = {
        let toks = lexer::lex(&main_src).expect("el main sintetizado lexea");
        let mut prog = parser::parse(toks).expect("el main sintetizado parsea");
        prog.functions.remove(0)
    };
    program.functions.retain(|f| f.name != "main");
    program.functions.push(main_fn);

    if let Err(e) = checker::check(&mut program) {
        eprintln!("{}", diagnostic::render(src, e.line, e.col, &e.to_string()));
        return 65;
    }

    println!("corriendo {} prueba(s)\n", tests.len());
    match interpreter::run(&program) {
        Ok(interpreter::Value::Int(fallos)) => {
            let fallos = fallos.max(0);
            println!();
            if fallos == 0 {
                println!("resultado: todas pasaron ✓");
            } else {
                println!("resultado: {} fallo(s) ✗", fallos);
            }
            (fallos & 0xFF) as i32
        }
        Ok(_) => 0,
        Err(e) => {
            eprintln!("{}", diagnostic::render(src, e.line, e.col, &e.to_string()));
            70
        }
    }
}

/// Construye el fuente de un `main` que ejecuta las pruebas y devuelve el número de fallos.
fn synth_main(tests: &[String]) -> String {
    let mut body = String::from("    var fallos: int = 0;\n");
    for t in tests {
        body.push_str(&format!(
            "    if ({t}()) {{ print(\"ok    {t}\"); }} else {{ print(\"FALLO {t}\"); fallos = fallos + 1; }}\n"
        ));
    }
    format!("fn main() -> int {{\n{body}    fallos\n}}")
}
