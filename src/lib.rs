//! raylang — la librería del compilador/intérprete.
//!
//! Fases del pipeline (DESIGN.md §2). El front-end se comparte; el backend de
//! ejecución tiene dos caminos:
//!
//!   fuente → [lexer] → [parser] → [checker] → ┬─ [interpreter] ──────── (M1)
//!                                             └─ [compiler] → [vm] ───── (M2)
//!
//! El binario (`src/main.rs`) es un cliente delgado de esta librería.

pub mod ast;
pub mod builtins;
pub mod bytecode;
pub mod checker;
pub mod cli;
pub mod compiler;
pub mod deps;
pub mod diagnostic;
pub mod ffi;
pub mod fmt;
pub mod gc;
#[cfg(feature = "interp")]
pub mod interpreter;
pub mod lexer;
pub mod loader;
pub mod lsp;
pub mod raydoc;
pub mod manifest;
pub mod parser;
pub mod poll;
pub mod prelude;
pub mod repl;
pub mod runtime;
pub mod sha256;
pub mod stdlib;
pub mod test_runner;
pub mod token;
pub mod vm;

/// Tamaño de pila del hilo worker (M13.3a): 256 MiB, muy por encima de los ~8 MiB
/// por defecto del hilo principal. El parser (descenso recursivo en Rust) y el
/// intérprete (`eval_*` recurre sobre la pila de Rust) recurren con la profundidad
/// del programa; sin esto, una entrada muy anidada o una recursión profunda
/// desbordarían la pila y el proceso moriría con SIGSEGV en vez de dar un error.
/// Con esta pila, el techo efectivo lo pone `runtime::MAX_CALL_DEPTH` (error
/// limpio), que se alcanza holgadamente dentro de 256 MiB. La **VM no lo necesita**
/// (sus marcos viven en el heap), pero correrla también aquí no cuesta nada.
const STACK_SIZE: usize = 256 * 1024 * 1024;

/// Compila un programa **ya chequeado** (bajado por el checker) y lo ejecuta sobre la
/// **VM** — el motor de producto (M35). Devuelve el mismo `Value`/`RuntimeError` que el
/// intérprete (la VM convierte en la frontera), así que los clientes (REPL, runner de
/// `@test`, binario) son idénticos con uno u otro motor. Un error de **compilación** (raro
/// tras un check exitoso: es la bajada a bytecode) se presenta como error de ejecución.
pub fn run_on_vm(program: &ast::Program) -> Result<runtime::Value, runtime::RuntimeError> {
    let compiled = compiler::compile_program(program)
        .map_err(|e| runtime::RuntimeError { msg: e.msg, line: e.line, col: e.col })?;
    vm::run_program(&compiled)
}

/// Lanza `f` en el hilo worker de pila grande y devuelve el resultado del `join`
/// (el pánico del worker, si lo hubo, viaja como `Err(payload)`).
fn spawn_big_stack<F, T>(f: F) -> std::thread::Result<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(f)
        .unwrap_or_else(|e| ice!("no se pudo crear el hilo worker con pila grande: {e}"))
        .join()
}

/// Ejecuta `f` en un hilo dedicado con una pila grande (`STACK_SIZE`) y devuelve su
/// resultado (M13.3a); así la recursión profunda da un error limpio en vez de un
/// segfault. Un pánico del worker se **re-lanza** tal cual: esta variante la usan los
/// tests (un assert fallido dentro del closure debe fallar SU test, no el proceso).
/// El binario usa [`with_big_stack_or_ice`], que convierte el pánico en un ICE.
pub fn with_big_stack<F, T>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    spawn_big_stack(f).expect("el hilo worker entró en pánico") // ice-ok: re-lanzar es deliberado (tests)
}

/// **La red central de ICEs (M33b)**: como [`with_big_stack`], pero si el worker
/// **panica** —una invariante rota (`ice!`) o un pánico no auditado (un índice fuera
/// de rango futuro)— no se re-panica (doble traza fea): se imprime el banner de
/// `diagnostic::ice_banner` (que distingue el bug del compilador de un error del
/// usuario y pide el reporte) y se sale con el código **101** (la convención de Rust
/// para pánicos). El hook estándar ya imprimió archivo:línea, que el reporte necesita.
/// Es lo que usa el binario (`main.rs`): TODO pánico del pipeline acaba aquí.
pub fn with_big_stack_or_ice<F, T>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    match spawn_big_stack(f) {
        Ok(v) => v,
        Err(payload) => {
            let detalle = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "pánico sin mensaje".to_string());
            eprintln!("{}", diagnostic::ice_banner(&detalle));
            std::process::exit(101);
        }
    }
}
