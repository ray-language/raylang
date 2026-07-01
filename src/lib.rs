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
pub mod compiler;
pub mod diagnostic;
pub mod fmt;
pub mod gc;
pub mod interpreter;
pub mod lexer;
pub mod loader;
pub mod lsp;
pub mod parser;
pub mod poll;
pub mod prelude;
pub mod repl;
pub mod test_runner;
pub mod token;
pub mod vm;

/// Tamaño de pila del hilo worker (M13.3a): 256 MiB, muy por encima de los ~8 MiB
/// por defecto del hilo principal. El parser (descenso recursivo en Rust) y el
/// intérprete (`eval_*` recurre sobre la pila de Rust) recurren con la profundidad
/// del programa; sin esto, una entrada muy anidada o una recursión profunda
/// desbordarían la pila y el proceso moriría con SIGSEGV en vez de dar un error.
/// Con esta pila, el techo efectivo lo pone `interpreter::MAX_CALL_DEPTH` (error
/// limpio), que se alcanza holgadamente dentro de 256 MiB. La **VM no lo necesita**
/// (sus marcos viven en el heap), pero correrla también aquí no cuesta nada.
const STACK_SIZE: usize = 256 * 1024 * 1024;

/// Ejecuta `f` en un hilo dedicado con una pila grande (`STACK_SIZE`) y devuelve su
/// resultado (M13.3a). Lo usa el binario para todo el trabajo del pipeline; así la
/// recursión profunda da un error limpio en vez de un segfault. Si `f` llama a
/// `process::exit`, el proceso termina y este `join` no retorna —es lo normal—.
pub fn with_big_stack<F, T>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(f)
        .expect("no se pudo crear el hilo worker con pila grande")
        .join()
        .expect("el hilo worker entró en pánico")
}
