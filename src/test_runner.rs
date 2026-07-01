//! Runner de pruebas `@test` (M10.1, ampliado en M13.2b).
//!
//! Un **cliente externo** del front-end + intérprete, en el espíritu del REPL (M8.2): usa
//! solo la API pública y **no toca** el checker ni el intérprete. Una función marcada con
//! `@test` debe tener firma `() -> bool` o `() -> unit` (la valida el checker):
//!   - `() -> bool`: pasa si devuelve `true`.
//!   - `() -> unit`: pasa si **no dispara** ningún `assert`/`panic` (M13.2a).
//!
//! **Aislamiento por prueba (M13.2b)**: cada test se ejecuta en su **propia** ejecución del
//! intérprete, sintetizando un `main` que llama solo a esa prueba. Así un `panic`/aserción que
//! falle aborta *esa* ejecución y no la batería entera, y se captura su mensaje. Como no hay forma
//! pública de "ejecutar la función N", se **reescribe `main`** (igual que el REPL): se clona el
//! programa base, se sustituye su `main` y se verifica y ejecuta el resultado.
//!
//! El código de salida es el **número de fallos** (0 = todas pasaron).

use crate::ast::Type;
use crate::interpreter::Value;
use crate::{checker, diagnostic, interpreter, lexer, parser};

/// Cómo se decide si una prueba pasa, según su tipo de retorno.
enum Kind {
    /// `() -> bool`: pasa si devuelve `true`.
    Bool,
    /// `() -> unit`: pasa si no dispara `assert`/`panic`.
    Unit,
}

/// Ejecuta las pruebas del fuente y devuelve el código de salida (número de fallos, o un
/// código de error si el front-end falla). `filtro`, si está, selecciona solo las pruebas cuyo
/// nombre lo **contiene** (subcadena). Imprime el informe y los errores con contexto.
pub fn run(src: &str, filtro: Option<&str>) -> i32 {
    let tokens = match lexer::lex(src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", diagnostic::render(src, e.line, e.col, e.len, &e.to_string()));
            return 65;
        }
    };
    let pristine = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", diagnostic::render(src, e.line, e.col, e.len, &e.to_string()));
            return 65;
        }
    };

    // Recolectar las funciones `@test`, en orden de declaración, con su tipo (bool/unit).
    let mut tests: Vec<(String, Kind)> = pristine
        .functions
        .iter()
        .filter(|f| f.annotations.iter().any(|a| a.name == "test"))
        .map(|f| {
            let kind = if f.return_type == Type::Bool { Kind::Bool } else { Kind::Unit };
            (f.name.clone(), kind)
        })
        .collect();

    // Filtro por nombre (subcadena), si se pidió.
    if let Some(pat) = filtro {
        tests.retain(|(n, _)| n.contains(pat));
    }

    if tests.is_empty() {
        match filtro {
            Some(p) => println!("no hay pruebas (@test) que contengan '{}'", p),
            None => println!("no hay pruebas (@test) en el archivo"),
        }
        return 0;
    }

    // Chequeo único del programa completo: sintetiza un `main` que llama a TODAS las pruebas
    // seleccionadas, para **surfacing** de errores de compilación una sola vez (no por prueba).
    {
        let nombres: Vec<&str> = tests.iter().map(|(n, _)| n.as_str()).collect();
        let mut prog = swap_main(pristine.clone(), &synth_main_all(&nombres));
        if let Err(e) = checker::check(&mut prog) {
            eprintln!("{}", diagnostic::render(src, e.line, e.col, e.len, &e.to_string()));
            return 65;
        }
    }

    println!("corriendo {} prueba(s)\n", tests.len());
    let mut fallos = 0;
    for (name, kind) in &tests {
        match ejecutar_una(&pristine, name, kind) {
            Ok(()) => println!("ok    {}", name),
            Err(motivo) => {
                println!("FALLO {}", name);
                println!("        {}", motivo);
                fallos += 1;
            }
        }
    }

    println!();
    if fallos == 0 {
        println!("resultado: {} prueba(s), todas pasaron ✓", tests.len());
    } else {
        println!("resultado: {} de {} prueba(s) fallaron ✗", fallos, tests.len());
    }
    fallos & 0xFF
}

/// Ejecuta una sola prueba en aislamiento. Clona el programa base, le pone un `main` que llama
/// solo a esa prueba, lo verifica y lo corre. `Ok(())` = pasó; `Err(motivo)` = falló (devolvió
/// `false`, o disparó un `assert`/`panic` cuyo mensaje se devuelve).
fn ejecutar_una(pristine: &crate::ast::Program, name: &str, kind: &Kind) -> Result<(), String> {
    let main_src = match kind {
        // bool: 0 si pasó (true), 1 si devolvió false.
        Kind::Bool => format!("fn main() -> int {{ if ({name}()) {{ 0 }} else {{ 1 }} }}"),
        // unit: la llama y devuelve 0; un panic/aserción aborta con error.
        Kind::Unit => format!("fn main() -> int {{ {name}(); 0 }}"),
    };
    let mut prog = swap_main(pristine.clone(), &main_src);
    // El chequeo no debería fallar (el chequeo global ya pasó), pero también **baja** el programa
    // (resuelve enums, UFCS, dicts…) antes de ejecutar, así que es obligatorio.
    if let Err(e) = checker::check(&mut prog) {
        return Err(format!("error de compilación: {}", e));
    }
    match interpreter::run(&prog) {
        Ok(Value::Int(0)) => Ok(()),
        Ok(Value::Int(_)) => Err("la prueba devolvió false".into()),
        Ok(_) => Ok(()),
        Err(e) => Err(e.msg),
    }
}

/// Sustituye el `main` del programa por el `main` descrito en `main_src` (lo parsea y extrae).
/// El parser no resuelve nombres, así que un `main` que llama a las pruebas parsea aunque estas
/// vivan en el otro programa.
fn swap_main(mut program: crate::ast::Program, main_src: &str) -> crate::ast::Program {
    let main_fn = {
        let toks = lexer::lex(main_src).expect("el main sintetizado lexea");
        let mut prog = parser::parse(toks).expect("el main sintetizado parsea");
        prog.functions.remove(0)
    };
    program.functions.retain(|f| f.name != "main");
    program.functions.push(main_fn);
    program
}

/// `main` que llama a todas las pruebas (solo para el chequeo global; descarta sus valores).
fn synth_main_all(tests: &[&str]) -> String {
    let mut body = String::new();
    for t in tests {
        body.push_str(&format!("    {t}();\n"));
    }
    format!("fn main() -> int {{\n{body}    0\n}}")
}
