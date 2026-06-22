//! REPL — *read-eval-print loop* interactivo (M8.2).
//!
//! ## Estrategia: re-ejecutar el preámbulo acumulado
//!
//! El REPL no mantiene un entorno vivo del intérprete entre líneas. En su lugar,
//! **acumula** lo escrito y, en cada entrada, **reconstruye un programa completo** y lo
//! verifica y ejecuta con el mismo `checker` e `interpreter` de siempre:
//!
//! ```text
//!   <definiciones acumuladas (fn/struct/enum)>
//!   fn __repl__() { <sentencias acumuladas (let/var/asignación)>  <entrada como cola> }
//! ```
//!
//! La **cola** del cuerpo es lo que se muestra como `valor : tipo`. El tipo viene del
//! checker (`check_repl`, que devuelve el tipo de esa cola); el valor, del intérprete
//! (`run_named`, que ejecuta `__repl__`).
//!
//! Es una estrategia deliberadamente simple: reusa todo el pipeline sin tocarlo, y el
//! estado entre líneas "vive" en el historial de fuente que se re-ejecuta. El coste es
//! recomputar el historial en cada entrada (aceptable en un REPL pedagógico) y que un
//! efecto secundario dentro de un inicializador (`let x = print(1);`) se repita. Una
//! entrada que no verifica/ejecuta se **descarta** (no contamina el historial).

use std::io::{self, BufRead, Write};

use crate::ast::{ExprKind, Program, StmtKind};
use crate::{checker, interpreter, lexer, parser};

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
            Ok(out) if !out.is_empty() => println!("{}", out),
            Ok(_) => {}
            Err(e) => eprintln!("{}", e),
        }
    }
}

fn print_help() {
    println!("Entradas posibles:");
    println!("  - una expresión:        1 + 2          -> muestra  3 : int");
    println!("  - un let/var:           let x = 3      -> muestra  x : int");
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
    /// `fn`/`struct`/`enum`: definición de nivel superior.
    Def { name: String, src: String },
    /// `let`/`var x = e;`: liga una variable (se persiste; se muestra `x : tipo`).
    Bind { name: String, src: String },
    /// `lhs = e;`: asignación (se persiste). `show` es el nombre a mostrar si es simple.
    Assign { show: Option<String>, src: String },
    /// Una expresión suelta: se evalúa una vez y se muestra `valor : tipo`.
    Expr { src: String },
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

    /// Procesa una línea y devuelve el texto a mostrar (`valor : tipo`, `x : tipo`,
    /// `definida 'f'`…) o un mensaje de error. El estado solo se modifica si la entrada
    /// verifica y ejecuta sin error.
    pub fn eval(&mut self, line: &str) -> Result<String, String> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(String::new());
        }
        let entry = self.classify(line)?;
        // Instantánea para deshacer si la entrada no verifica/ejecuta.
        let snap_items: Vec<Item> = self.items.iter().map(|i| Item { name: i.name.clone(), src: i.src.clone() }).collect();
        let snap_stmts = self.stmts.clone();
        let result = self.apply(entry);
        if result.is_err() {
            self.items = snap_items;
            self.stmts = snap_stmts;
        }
        result
    }

    fn apply(&mut self, entry: Entry) -> Result<String, String> {
        match entry {
            Entry::Def { name, src } => {
                // Redefinir: reemplaza una definición homónima previa.
                self.items.retain(|it| it.name != name);
                self.items.push(Item { name: name.clone(), src });
                self.run_entry("0")?; // valida con una cola trivial
                Ok(format!("definida '{}'", name))
            }
            Entry::Bind { name, src } => {
                self.stmts.push(src);
                let (_, ty) = self.run_entry(&name)?;
                Ok(format!("{} : {}", name, ty))
            }
            Entry::Assign { show, src } => {
                self.stmts.push(src);
                match show {
                    Some(n) => {
                        let (_, ty) = self.run_entry(&n)?;
                        Ok(format!("{} : {}", n, ty))
                    }
                    None => {
                        self.run_entry("0")?;
                        Ok("ok".to_string())
                    }
                }
            }
            Entry::Expr { src } => {
                let (val, ty) = self.run_entry(&src)?;
                Ok(format!("{} : {}", val, ty))
            }
        }
    }

    /// Reconstruye el programa (preámbulo + `tail` como cola de `__repl__`), lo verifica
    /// y lo ejecuta, devolviendo `(valor, tipo)`.
    fn run_entry(&self, tail: &str) -> Result<(String, String), String> {
        let items: String = self.items.iter().map(|i| i.src.as_str()).collect::<Vec<_>>().join("\n");
        let mut body = self.stmts.join("\n");
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(tail);
        let src = format!("{}\nfn __repl__() {{\n{}\n}}\n", items, body);

        let mut prog = parse_src(&src)?;
        let ty = checker::check_repl(&mut prog, "__repl__").map_err(|e| e.to_string())?;
        let val = interpreter::run_named(&prog, "__repl__").map_err(|e| e.to_string())?;
        Ok((val.to_string(), ty.to_string()))
    }

    /// Decide qué clase de entrada es la línea, parseándola de forma aislada.
    fn classify(&self, line: &str) -> Result<Entry, String> {
        // Definición: empieza por una palabra clave de declaración. Una definición sola
        // es un programa válido, así que se parsea directamente.
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
            // Sin sentencias: el cuerpo es solo una expresión (la cola).
            Ok(Entry::Expr { src: strip_semi(line) })
        }
    }
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

/// ¿`line` empieza por la palabra `kw` seguida de un separador (no es un identificador
/// más largo como `function` o `enumeracion`)?
fn starts_with_word(line: &str, kw: &str) -> bool {
    let rest = match line.strip_prefix(kw) {
        Some(r) => r,
        None => return false,
    };
    rest.is_empty() || rest.starts_with(|c: char| !c.is_alphanumeric() && c != '_')
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

    #[test]
    fn expresiones_muestran_valor_y_tipo() {
        let mut s = Session::new();
        assert_eq!(s.eval("1 + 2").unwrap(), "3 : int");
        assert_eq!(s.eval("[1, 2, 3]").unwrap(), "[1, 2, 3] : [int]");
        assert_eq!(s.eval("3 > 1").unwrap(), "true : bool");
    }

    #[test]
    fn let_persiste_entre_lineas() {
        let mut s = Session::new();
        assert_eq!(s.eval("let x = 3").unwrap(), "x : int");
        assert_eq!(s.eval("x + 1").unwrap(), "4 : int");
        assert_eq!(s.eval("x * x").unwrap(), "9 : int");
    }

    #[test]
    fn var_y_asignacion_persisten() {
        let mut s = Session::new();
        assert_eq!(s.eval("var t = 0").unwrap(), "t : int");
        assert_eq!(s.eval("t = t + 5").unwrap(), "t : int");
        assert_eq!(s.eval("t = t + 5").unwrap(), "t : int");
        assert_eq!(s.eval("t").unwrap(), "10 : int");
    }

    #[test]
    fn definiciones_y_su_uso() {
        let mut s = Session::new();
        assert_eq!(s.eval("fn doble(n: int) -> int { n * 2 }").unwrap(), "definida 'doble'");
        assert_eq!(s.eval("let x = 21").unwrap(), "x : int");
        assert_eq!(s.eval("doble(x)").unwrap(), "42 : int");
        // UFCS y pipeline también funcionan en el REPL.
        assert_eq!(s.eval("x.doble()").unwrap(), "42 : int");
        assert_eq!(s.eval("x |> doble").unwrap(), "42 : int");
    }

    #[test]
    fn struct_y_enum_en_repl() {
        let mut s = Session::new();
        assert_eq!(s.eval("struct Punto { x: int, y: int }").unwrap(), "definida 'Punto'");
        assert_eq!(s.eval("let p = Punto { x: 7, y: 6 }").unwrap(), "p : Punto");
        assert_eq!(s.eval("p.x + p.y").unwrap(), "13 : int");
        assert_eq!(s.eval("enum Caja<T> { Llena(T), Vacia }").unwrap(), "definida 'Caja'");
        assert_eq!(s.eval("Caja.Llena(5)").unwrap(), "Caja.Llena(5) : Caja<int>");
    }

    #[test]
    fn redefinir_una_variable() {
        let mut s = Session::new();
        s.eval("let x = 3").unwrap();
        // Redefinir con otro tipo: la última gana (shadowing al re-ejecutar).
        assert_eq!(s.eval("let x = true").unwrap(), "x : bool");
        assert_eq!(s.eval("x").unwrap(), "true : bool");
    }

    #[test]
    fn una_entrada_con_error_no_contamina_el_estado() {
        let mut s = Session::new();
        s.eval("let x = 3").unwrap();
        // Tipo mal: error, y NO se añade al historial.
        assert!(s.eval("let y = x + true").is_err());
        // 'y' no existe; 'x' sigue intacto.
        assert!(s.eval("y").is_err());
        assert_eq!(s.eval("x").unwrap(), "3 : int");
    }

    #[test]
    fn lo_indeterminado_pide_anotacion() {
        let mut s = Session::new();
        let e = s.eval("let xs = []").unwrap_err();
        assert!(e.contains("no se puede inferir"), "mensaje: {}", e);
    }
}
