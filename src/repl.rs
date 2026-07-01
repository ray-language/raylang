//! REPL — *read-eval-print loop* interactivo (M8.2).
//!
//! ## Un cliente externo, sin tocar el core
//!
//! El REPL es **puramente un cliente** del front-end y el intérprete: usa solo su API
//! pública (`lexer::lex`, `parser::parse`, `checker::check`, `interpreter::run` y el
//! builtin `print`). No hay ni una línea del REPL en el checker ni en el intérprete.
//!
//! ## Estrategia: re-ejecutar el preámbulo acumulado
//!
//! No mantiene un entorno vivo entre líneas. En su lugar **acumula** lo escrito y, en
//! cada entrada, **reconstruye un programa completo** y lo verifica y ejecuta:
//!
//! ```text
//!   <definiciones acumuladas (fn/struct/enum)>
//!   fn main() { <sentencias acumuladas>  print(<entrada>); }
//! ```
//!
//! El valor se muestra haciendo que el `main` sintetizado **imprima** la entrada con el
//! builtin `print` (de ahí que el REPL muestre el *valor*, no el tipo: obtener el tipo
//! exigiría una API del checker que no queremos añadir solo para esto). Si la entrada es
//! de tipo `unit` (p. ej. `print(x)` o un `while`), `print(...)` no tiparía: se reintenta
//! ejecutándola como sentencia, sin envolver.
//!
//! El estado entre líneas "vive" en el historial de fuente que se re-ejecuta. El coste es
//! recomputarlo en cada entrada (aceptable en un REPL pedagógico). Una entrada que no
//! verifica/ejecuta se **descarta** (no contamina el estado).

use std::io::{self, BufRead, Write};

use crate::ast::{ExprKind, Program, StmtKind};
use crate::{checker, diagnostic, interpreter, lexer, parser};

/// Arranca el bucle interactivo leyendo de la entrada estándar.
pub fn run() {
    println!("raylang REPL — escribe una expresión, un let/var, o una definición (fn/struct/enum).");
    println!("  :help  ayuda     :quit  salir");
    let mut session = Session::new();
    let stdin = io::stdin();
    let mut handle = stdin.lock();
    loop {
        print!("> ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        match handle.read_line(&mut line) {
            Ok(0) => {
                println!();
                break;
            } // EOF (Ctrl-D)
            Ok(_) => {}
            Err(e) => {
                eprintln!("error de entrada: {}", e);
                break;
            }
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match line {
            ":quit" | ":exit" | ":q" => break,
            ":help" | ":h" => {
                print_help();
                continue;
            }
            _ => {}
        }
        match session.eval(line) {
            Ok(Outcome::Defined(name)) => println!("definida '{}'", name),
            // El valor (si lo hubo) ya lo imprimió el intérprete vía `print`.
            Ok(_) => {}
            Err(e) => eprintln!("{}", e),
        }
    }
}

fn print_help() {
    println!("Entradas posibles:");
    println!("  - una expresión:        1 + 2          -> imprime  3");
    println!("  - un let/var:           let x = 3      -> imprime  3");
    println!("  - una asignación:       x = 4          (requiere 'var')");
    println!("  - una definición:       fn f(n: int) -> int {{ n + 1 }}");
    println!("El estado persiste entre líneas. ':quit' para salir.");
}

/// Una definición de nivel superior acumulada (fn/struct/enum), por nombre y fuente.
struct Item {
    name: String,
    src: String,
}

/// Cómo clasificamos una línea del REPL tras parsearla.
enum Entry {
    Def { name: String, src: String },
    Bind { name: String, src: String },
    Assign { show: Option<String>, src: String },
    Expr { src: String },
}

/// Resultado de procesar una línea (lo que el bucle debe mostrar él mismo).
#[derive(Debug, PartialEq)]
enum Outcome {
    /// El intérprete ya imprimió el valor (vía `print`).
    Printed,
    /// Una definición; el bucle imprime la confirmación.
    Defined(String),
    /// Nada que mostrar (entrada de tipo unit, o asignación a índice/campo).
    Quiet,
}

/// El estado acumulado de una sesión de REPL.
pub struct Session {
    items: Vec<Item>,
    stmts: Vec<String>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Session { items: Vec::new(), stmts: Vec::new() }
    }

    /// Procesa una línea. El estado solo se modifica si la entrada verifica y ejecuta.
    fn eval(&mut self, line: &str) -> Result<Outcome, String> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(Outcome::Quiet);
        }
        let entry = self.classify(line)?;
        let snap_items: Vec<Item> = self.items.iter().map(|i| Item { name: i.name.clone(), src: i.src.clone() }).collect();
        let snap_stmts = self.stmts.clone();
        let result = self.apply(entry);
        if result.is_err() {
            self.items = snap_items;
            self.stmts = snap_stmts;
        }
        result
    }

    fn apply(&mut self, entry: Entry) -> Result<Outcome, String> {
        match entry {
            Entry::Def { name, src } => {
                self.items.retain(|it| it.name != name); // redefinir reemplaza
                self.items.push(Item { name: name.clone(), src });
                // Validar (solo verificar) reconstruyendo el programa con un main vacío.
                self.check_program(&self.synth(None))?;
                Ok(Outcome::Defined(name))
            }
            Entry::Bind { name, src } => {
                self.stmts.push(src);
                self.show(&name)
            }
            Entry::Assign { show, src } => {
                self.stmts.push(src);
                match show {
                    Some(n) => self.show(&n),
                    None => {
                        // Asignación a índice/campo: ejecutar el historial, sin eco.
                        let src = self.synth(None);
                        let prog = self.check_program(&src)?;
                        run_prog(&prog, &src)?;
                        Ok(Outcome::Quiet)
                    }
                }
            }
            Entry::Expr { src } => self.show(&src),
        }
    }

    /// Evalúa y muestra `expr`: intenta imprimirlo con `print(expr)`; si eso no tipa
    /// (p. ej. `expr` es de tipo unit), lo ejecuta como sentencia sin imprimir.
    fn show(&self, expr: &str) -> Result<Outcome, String> {
        let with_print = self.synth(Some(&format!("print({});", expr)));
        match self.check_program(&with_print) {
            Ok(prog) => {
                run_prog(&prog, &with_print)?;
                Ok(Outcome::Printed)
            }
            // No tipó con `print(...)`: quizá `expr` es unit. Reintentar como sentencia;
            // si tampoco tipa, ese error (sin el envoltorio `print`) es el de verdad.
            Err(_) => {
                let as_stmt = self.synth(Some(&format!("{};", expr)));
                let prog = self.check_program(&as_stmt)?;
                run_prog(&prog, &as_stmt)?;
                Ok(Outcome::Quiet)
            }
        }
    }

    /// Construye la fuente del programa: las definiciones, y un `main` con el historial
    /// de sentencias y, opcionalmente, una sentencia de cola (`tail`).
    fn synth(&self, tail: Option<&str>) -> String {
        let items: String = self.items.iter().map(|i| i.src.as_str()).collect::<Vec<_>>().join("\n");
        let mut body = self.stmts.join("\n");
        if let Some(t) = tail {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(t);
        }
        format!("{}\nfn main() {{\n{}\n}}\n", items, body)
    }

    fn check_program(&self, src: &str) -> Result<Program, String> {
        // Errores renderizados con su contexto de fuente (M8.3), contra la fuente
        // sintetizada `src`: el `^` apunta al token ofensor (que contiene el código del
        // usuario). El número de línea es el de la fuente sintetizada.
        let tokens = lexer::lex(src).map_err(|e| diagnostic::render(src, e.line, e.col, e.len, &e.to_string()))?;
        let mut prog = parser::parse(tokens).map_err(|e| diagnostic::render(src, e.line, e.col, e.len, &e.to_string()))?;
        checker::check(&mut prog).map_err(|e| diagnostic::render(src, e.line, e.col, e.len, &e.to_string()))?;
        Ok(prog)
    }

    /// Clasifica la línea parseándola de forma aislada.
    fn classify(&self, line: &str) -> Result<Entry, String> {
        if starts_with_word(line, "fn") || starts_with_word(line, "struct") || starts_with_word(line, "enum") {
            let prog = parse_src(line)?;
            let name = def_name(&prog).ok_or_else(|| "no se reconoció la definición".to_string())?;
            return Ok(Entry::Def { name, src: line.to_string() });
        }
        // Resto: envolver en una función y mirar el cuerpo. Se intenta tal cual y, si no
        // parsea, con un ';' añadido (para aceptar `let x = 3` sin punto y coma).
        let prog = match parse_src(&wrap(line)) {
            Ok(p) => p,
            Err(_) => parse_src(&wrap(&format!("{};", line)))?,
        };
        let body = &prog.functions[0].body;
        if let Some(stmt) = body.statements.first() {
            match &stmt.kind {
                StmtKind::Let { name, .. } => Ok(Entry::Bind { name: name.clone(), src: ensure_semi(line) }),
                // M27.1: desestructuración de tupla — se muestra el valor del primer nombre ligado.
                StmtKind::LetTuple { names, .. } => Ok(Entry::Bind {
                    name: names.iter().flatten().next().cloned().unwrap_or_else(|| "_".to_string()),
                    src: ensure_semi(line),
                }),
                // M27.2: un `for` es una sentencia con efectos; se conserva sin mostrar valor.
                StmtKind::For { .. } => Ok(Entry::Assign { show: None, src: ensure_semi(line) }),
                StmtKind::Assign { target, .. } => {
                    let show = match &target.kind {
                        ExprKind::Ident(n) => Some(n.clone()),
                        _ => None,
                    };
                    Ok(Entry::Assign { show, src: ensure_semi(line) })
                }
                StmtKind::Expr(_) => Ok(Entry::Expr { src: strip_semi(line) }),
                StmtKind::Return { .. } => Err("'return' no tiene sentido en el REPL".to_string()),
            }
        } else {
            Ok(Entry::Expr { src: strip_semi(line) })
        }
    }
}

/// Ejecuta el programa y renderiza un posible error de ejecución con su contexto.
fn run_prog(prog: &Program, src: &str) -> Result<(), String> {
    interpreter::run(prog)
        .map(|_| ())
        .map_err(|e| diagnostic::render(src, e.line, e.col, 1, &e.to_string()))
}

fn wrap(line: &str) -> String {
    format!("fn __c__() {{ {} }}", line)
}

fn parse_src(src: &str) -> Result<Program, String> {
    let tokens = lexer::lex(src).map_err(|e| e.to_string())?;
    parser::parse(tokens).map_err(|e| e.to_string())
}

fn def_name(p: &Program) -> Option<String> {
    if let Some(f) = p.functions.first() {
        return Some(f.name.clone());
    }
    if let Some(s) = p.structs.first() {
        return Some(s.name.clone());
    }
    if let Some(e) = p.enums.first() {
        return Some(e.name.clone());
    }
    None
}

/// ¿`line` empieza por la palabra `kw` (y no por un identificador más largo)?
fn starts_with_word(line: &str, kw: &str) -> bool {
    match line.strip_prefix(kw) {
        Some(rest) => rest.is_empty() || rest.starts_with(|c: char| !c.is_alphanumeric() && c != '_'),
        None => false,
    }
}

fn ensure_semi(s: &str) -> String {
    let s = s.trim_end();
    if s.ends_with(';') {
        s.to_string()
    } else {
        format!("{};", s)
    }
}

fn strip_semi(s: &str) -> String {
    s.trim_end().strip_suffix(';').unwrap_or(s.trim_end()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // El valor se imprime por stdout (vía el builtin `print`), así que aquí asertamos
    // el *resultado* (Ok/Err y la variante de Outcome) y el estado de la sesión. El
    // texto exacto impreso se prueba por subproceso en `tests/repl_cli.rs`.

    #[test]
    fn expresion_imprime() {
        let mut s = Session::new();
        assert_eq!(s.eval("1 + 2").unwrap(), Outcome::Printed);
        assert_eq!(s.eval("[1, 2, 3]").unwrap(), Outcome::Printed);
    }

    #[test]
    fn let_persiste_entre_lineas() {
        let mut s = Session::new();
        assert_eq!(s.eval("let x = 3").unwrap(), Outcome::Printed);
        assert_eq!(s.eval("x + 1").unwrap(), Outcome::Printed);
        // Si 'x' no hubiera persistido, esto sería Err.
        assert!(s.eval("x * x").is_ok());
    }

    #[test]
    fn var_y_asignacion_persisten() {
        let mut s = Session::new();
        assert_eq!(s.eval("var t = 0").unwrap(), Outcome::Printed);
        assert!(s.eval("t = t + 5").is_ok());
        assert!(s.eval("t = t + 5").is_ok());
        // El valor (10) se imprime; aquí basta con que no falle.
        assert_eq!(s.eval("t").unwrap(), Outcome::Printed);
    }

    #[test]
    fn definiciones_y_su_uso() {
        let mut s = Session::new();
        assert_eq!(s.eval("fn doble(n: int) -> int { n * 2 }").unwrap(), Outcome::Defined("doble".into()));
        assert!(s.eval("let x = 21").is_ok());
        assert!(s.eval("doble(x)").is_ok()); // llamada normal
        assert!(s.eval("x.doble()").is_ok()); // UFCS
        assert!(s.eval("x |> doble").is_ok()); // pipeline
    }

    #[test]
    fn struct_y_enum_en_repl() {
        let mut s = Session::new();
        assert_eq!(s.eval("struct Punto { x: int, y: int }").unwrap(), Outcome::Defined("Punto".into()));
        assert!(s.eval("let p = Punto { x: 7, y: 6 }").is_ok());
        assert!(s.eval("p.x + p.y").is_ok());
        assert_eq!(s.eval("enum Caja<T> { Llena(T), Vacia }").unwrap(), Outcome::Defined("Caja".into()));
        assert!(s.eval("Caja.Llena(5)").is_ok());
    }

    #[test]
    fn expresion_unit_no_falla() {
        // `print(x)` es de tipo unit: `print(print(x))` no tiparía, pero el REPL lo
        // reintenta como sentencia y lo ejecuta (Quiet).
        let mut s = Session::new();
        s.eval("let x = 3").unwrap();
        assert_eq!(s.eval("print(x)").unwrap(), Outcome::Quiet);
    }

    #[test]
    fn redefinir_una_variable() {
        let mut s = Session::new();
        s.eval("let x = 3").unwrap();
        assert!(s.eval("let x = true").is_ok()); // la última gana (shadowing al re-ejecutar)
        assert!(s.eval("x").is_ok());
    }

    #[test]
    fn una_entrada_con_error_no_contamina_el_estado() {
        let mut s = Session::new();
        s.eval("let x = 3").unwrap();
        assert!(s.eval("let y = x + true").is_err()); // error de tipos
        assert!(s.eval("y").is_err()); // 'y' no se añadió
        assert!(s.eval("x").is_ok()); // 'x' sigue intacto
    }

    #[test]
    fn lo_indeterminado_pide_anotacion() {
        let mut s = Session::new();
        let e = s.eval("let xs = []").unwrap_err();
        assert!(e.contains("no se puede inferir"), "mensaje: {}", e);
    }
}
