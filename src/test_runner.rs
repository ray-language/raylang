//! Runner de pruebas `@test` (M10.1, ampliado en M13.2b; a nivel proyecto en M101).
//!
//! Un **cliente externo** del front-end + VM, en el espíritu del REPL (M8.2): usa solo la API
//! pública y **no toca** el checker ni los motores. Una función marcada con `@test` debe tener
//! firma `() -> bool` o `() -> unit` (la valida el checker):
//!   - `() -> bool`: pasa si devuelve `true`.
//!   - `() -> unit`: pasa si **no dispara** ningún `assert`/`panic` (M13.2a).
//!
//! **A nivel proyecto (M101)**: cada *suite* (archivo de entrada) se carga por el **loader**
//! (`load_with_deps`), así los `import` resuelven igual que bajo `ray run`, y las `@test` se
//! recolectan de **todos** los módulos fusionados (las de un módulo importado se reportan con su
//! nombre calificado, `math.double_ok`). Como el nombre global de una función de módulo
//! (`math::double_ok`) no lexea como fuente, el `main` sintético se **construye como AST** — lo
//! que además esquiva la visibilidad `pub` (el resolver del loader ya corrió: una `@test` privada
//! de su módulo sigue siendo llamable por su nombre global).
//!
//! **Aislamiento por prueba (M13.2b)**: cada test se ejecuta en su **propia** ejecución de la VM,
//! sintetizando un `main` que llama solo a esa prueba. Así un `panic`/aserción que falle aborta
//! *esa* ejecución y no la batería entera, y se captura su mensaje — con **ubicación** (M101):
//! `at módulo:línea:col`, reposicionada al primer marco de usuario (estilo M79c) para que un
//! `assert` fallido apunte al assert del usuario y no al `panic` del prelude.
//!
//! El código de salida (M101): **0** si todo pasó, **1** si hubo fallos, **65** si alguna suite no
//! compila. (Antes era el número de fallos: 256 fallos → exit 0 → un falso verde en CI.)

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::ast::{Block, Expr, ExprKind, Function, Program, Stmt, StmtKind, Type};
use crate::loader::{self, Loaded};
use crate::runtime::{TraceFrame, Value};
use crate::{checker, diagnostic};

/// Cómo se decide si una prueba pasa, según su tipo de retorno.
enum Kind {
    /// `() -> bool`: pasa si devuelve `true`.
    Bool,
    /// `() -> unit`: pasa si no dispara `assert`/`panic`.
    Unit,
}

/// Una prueba recolectada: su nombre global en el programa fusionado (`math::double_ok`), su
/// nombre de cara al usuario (`math.double_ok`) y su modo de veredicto.
struct Test {
    global: String,
    display: String,
    kind: Kind,
}

/// Una suite cargada: un archivo de entrada (el del proyecto, o un `tests/*.ray`) con su programa
/// fusionado y sus pruebas ya filtradas.
struct Suite {
    display: String,
    loaded: Loaded,
    tests: Vec<Test>,
}

/// Ejecuta las pruebas de las suites y devuelve el código de salida: **0** todo verde, **1** hubo
/// fallos, **65** alguna suite no compila (las demás corren igual). Cada suite se carga con el
/// loader contra `dep_roots` (la caché de dependencias + la raíz del proyecto, para que un
/// `tests/*.ray` importe los módulos de `src/`). La **primera** suite es la del proyecto y
/// recolecta las `@test` de todos sus módulos (pruebas unitarias); las demás (`tests/*.ray`)
/// recolectan **solo las propias** — si importan un módulo del proyecto, sus pruebas inline ya
/// corren en la suite del proyecto y repetirlas duplicaría el informe. `filter`, si está,
/// selecciona las pruebas cuyo nombre de cara al usuario lo **contiene** (subcadena). Imprime el
/// informe y los errores.
pub fn run(suite_paths: &[PathBuf], dep_roots: &[PathBuf], filter: Option<&str>) -> i32 {
    // M146: una suite no abre ventanas reales por defecto — std/ui corre headless bajo el
    // runner (RAY_UI_BACKEND en el entorno sigue mandando si el usuario lo puso).
    #[cfg(all(feature = "ui", unix, not(target_arch = "wasm32")))]
    ray_runtime::ui::default_headless();
    let mut frontend_failed = false;
    let mut suites: Vec<Suite> = Vec::new();
    for (i, path) in suite_paths.iter().enumerate() {
        match loader::load_with_deps(path, dep_roots) {
            Ok(loaded) => {
                let tests = collect_tests(&loaded.program, filter, i > 0);
                suites.push(Suite { display: display_path(path), loaded, tests });
            }
            Err(e) => {
                eprintln!("{}", e.message);
                frontend_failed = true;
            }
        }
    }

    let total: usize = suites.iter().map(|s| s.tests.len()).sum();
    if total == 0 {
        if !frontend_failed {
            match filter {
                Some(p) => println!("no tests (@test) containing '{}'", p),
                None => println!("no tests (@test) in the project"),
            }
        }
        return if frontend_failed { 65 } else { 0 };
    }

    println!("running {} test(s)\n", total);
    let multi_suite = suites.iter().filter(|s| !s.tests.is_empty()).count() > 1;
    let mut failures = 0;
    let mut ran = 0;
    for suite in &suites {
        if suite.tests.is_empty() {
            continue;
        }
        if multi_suite {
            println!("-- {}", suite.display);
        }
        // Chequeo único de la suite: un `main` que llama a TODAS sus pruebas, para surfacing de
        // errores de compilación una sola vez (no por prueba).
        if let Err(e) = check_suite(suite) {
            eprintln!("{}", e);
            frontend_failed = true;
            continue;
        }
        for test in &suite.tests {
            ran += 1;
            let started = Instant::now();
            match run_one(suite, test) {
                Ok(()) => println!("ok    {} ({} ms)", test.display, started.elapsed().as_millis()),
                Err(reason) => {
                    println!("FAIL  {}", test.display);
                    for line in reason {
                        println!("        {}", line);
                    }
                    failures += 1;
                }
            }
        }
        if multi_suite {
            println!();
        }
    }

    if !multi_suite {
        println!();
    }
    // El resumen cuenta lo EJECUTADO: una suite que no compila deja fuera sus pruebas (y el
    // código de salida 65 ya lo delata).
    if failures == 0 {
        println!("result: {} test(s), all passed ✓", ran);
    } else {
        println!("result: {} of {} test(s) failed ✗", failures, ran);
    }
    if frontend_failed {
        65
    } else if failures > 0 {
        1
    } else {
        0
    }
}

/// Recolecta las funciones `@test` del programa fusionado, en orden de declaración, aplicando el
/// filtro por subcadena sobre el nombre de cara al usuario (`math.double_ok`). Con `own_only`,
/// solo las del módulo de entrada de la suite (las importadas llevan nombre namespaceado `::`).
fn collect_tests(program: &Program, filter: Option<&str>, own_only: bool) -> Vec<Test> {
    program
        .functions
        .iter()
        .filter(|f| f.annotations.iter().any(|a| a.name == "test"))
        .filter(|f| !(own_only && f.name.contains("::")))
        .map(|f| Test {
            global: f.name.clone(),
            // El separador interno `::` del loader vuelve al `.` que escribe el usuario.
            display: f.name.replace("::", "."),
            kind: if f.return_type == Type::Bool { Kind::Bool } else { Kind::Unit },
        })
        .filter(|t| filter.is_none_or(|p| t.display.contains(p)))
        .collect()
}

/// Chequea la suite completa (un `main` sintético que llama a todas sus pruebas). Un error se
/// devuelve ya renderizado contra su módulo y línea local.
fn check_suite(suite: &Suite) -> Result<(), String> {
    let statements = suite.tests.iter().map(|t| stmt_call(&t.global)).collect();
    let body = Block { statements, tail: Some(Box::new(expr(ExprKind::Int(0, crate::token::Radix::Dec)))), line: 1, col: 1, end_line: 1 };
    let mut program = swap_main(suite.loaded.program.clone(), synth_main(body));
    checker::check(&mut program).map_err(|mut e| {
        let (module, source, local, col, len) = suite.loaded.locate(e.line, e.col, e.len);
        e.line = local;
        e.col = col;
        let head = if suite.loaded.multi_module() { format!("[{}] {}", module, e) } else { e.to_string() };
        diagnostic::render(source, local, col, len, &head)
    })
}

/// Ejecuta una sola prueba en aislamiento sobre la VM (el motor de producto — M35; una prueba
/// puede usar concurrencia). Clona el programa fusionado, le pone un `main` que llama solo a esa
/// prueba, lo verifica (el check también **baja** el programa: enums, UFCS, dicts…) y lo corre.
/// `Ok(())` = pasó; `Err(líneas)` = falló, con el motivo y —si la hay— la ubicación.
fn run_one(suite: &Suite, test: &Test) -> Result<(), Vec<String>> {
    let call = call_expr(&test.global);
    let body = match test.kind {
        // bool: `fn main() -> int { if (t()) { 0 } else { 1 } }`.
        Kind::Bool => Block {
            statements: vec![],
            tail: Some(Box::new(expr(ExprKind::If {
                cond: Box::new(call),
                then_branch: int_block(0),
                else_branch: Some(Box::new(expr(ExprKind::Block(int_block(1))))),
            }))),
            line: 1,
            col: 1,
            end_line: 1,
        },
        // unit: `fn main() -> int { t(); 0 }` — un panic/aserción aborta con error.
        Kind::Unit => Block {
            statements: vec![stmt_call(&test.global)],
            tail: Some(Box::new(expr(ExprKind::Int(0, crate::token::Radix::Dec)))),
            line: 1,
            col: 1,
            end_line: 1,
        },
    };
    let mut program = swap_main(suite.loaded.program.clone(), synth_main(body));
    if let Err(e) = checker::check(&mut program) {
        // No debería pasar (el chequeo de la suite ya corrió), pero el check es obligatorio
        // igualmente porque baja el programa antes de ejecutar.
        return Err(vec![format!("compilation error: {}", e)]);
    }
    let outcome = crate::run_on_vm(&program);
    // M129: aísla de verdad — descarta los handles del SO que el test dejó vivos (listeners
    // incluidos: antes sobrevivían y aceptaban conexiones que nadie atendía en el siguiente test).
    crate::builtins::close_all_handles();
    match outcome {
        Ok(Value::Int(0)) => Ok(()),
        Ok(Value::Int(_)) => Err(vec!["the test returned false".into()]),
        Ok(_) => Ok(()),
        Err(mut e) => {
            let trace = std::mem::take(&mut e.trace);
            let mut lines = vec![e.msg.clone()];
            // M101: la ubicación del fallo, reposicionada al primer marco de USUARIO (estilo
            // M79c): un assert fallido apunta al `assert(...)` del usuario, no al `panic` del
            // prelude. Sin marco de usuario ni posición en banda, no se imprime ubicación.
            let (line, col) = first_user_frame(&trace, &suite.loaded).unwrap_or((e.line, e.col));
            if line > 0 {
                let (module, source, local, col, _len) = suite.loaded.locate(line, col, 1);
                if local <= source.lines().count() {
                    lines.push(format!("at {}:{}:{}", module, local, col));
                }
            }
            Err(lines)
        }
    }
}

/// El primer marco de la traza que es código de usuario: en banda (no el prelude, el único fuente
/// inyectado sin banda propia) y fuera de la std embebida (`std`/`std/…`). Posición **global**.
fn first_user_frame(trace: &[TraceFrame], loaded: &Loaded) -> Option<(usize, usize)> {
    trace.iter().find_map(|f| {
        let (module, source, local, _col, _len) = loaded.locate(f.line, f.col, 1);
        let in_band = local <= source.lines().count();
        let is_std = module == "std" || module.starts_with("std/");
        if in_band && !is_std { Some((f.line, f.col)) } else { None }
    })
}

/// Sustituye el `main` del programa por el sintético.
fn swap_main(mut program: Program, main_fn: Function) -> Program {
    program.functions.retain(|f| f.name != "main");
    program.functions.push(main_fn);
    program
}

/// Envuelve un cuerpo en un `fn main() -> int` sintético. Se construye como **AST** (no como
/// fuente): el nombre global de una prueba de módulo (`math::t`) contiene `::`, ilegal en el
/// léxico de usuario, así que no puede pasar por el lexer.
fn synth_main(body: Block) -> Function {
    Function {
        annotations: vec![],
        is_pub: false,
        name: "main".into(),
        type_params: vec![],
        bounds: vec![],
        params: vec![],
        return_type: Type::Int,
        body,
        line: 1,
        col: 1,
    }
}

/// La ruta de una suite de cara al usuario: relativa al directorio actual si vive bajo él.
fn display_path(path: &Path) -> String {
    let relative = std::env::current_dir().ok().and_then(|cwd| path.strip_prefix(cwd).ok());
    relative.unwrap_or(path).display().to_string()
}

fn expr(kind: ExprKind) -> Expr {
    Expr { kind, line: 1, col: 1 }
}

/// `nombre()` como expresión.
fn call_expr(name: &str) -> Expr {
    expr(ExprKind::Call { callee: Box::new(expr(ExprKind::Ident(name.to_string()))), args: vec![] })
}

/// `nombre();` como sentencia.
fn stmt_call(name: &str) -> Stmt {
    Stmt { kind: StmtKind::Expr(call_expr(name)), line: 1, col: 1 }
}

/// Un bloque cuyo valor es el literal entero `v`.
fn int_block(v: i64) -> Block {
    Block { statements: vec![], tail: Some(Box::new(expr(ExprKind::Int(v, crate::token::Radix::Dec)))), line: 1, col: 1, end_line: 1 }
}
