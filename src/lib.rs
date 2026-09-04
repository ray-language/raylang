//! raylang — la librería del compilador/intérprete.
//!
//! Fases del pipeline (DESIGN.md §2). El front-end se comparte; el backend de
//! ejecución tiene dos caminos:
//!
//!   fuente → [lexer] → [parser] → [checker] → ┬─ [interpreter] ──────── (M1)
//!                                             └─ [compiler] → [vm] ───── (M2)
//!
//! El binario (`src/main.rs`) es un cliente delgado de esta librería.

/// Allocador global **mimalloc** (P0.4, perf). El malloc del sistema (macOS libmalloc en particular)
/// es lento para el *churn* de objetos pequeños del intérprete: `split`/`to_string`/arrays alocan
/// millones de `String` pequeños. mimalloc es 2–4× más rápido en ese patrón y acelera todos los
/// benchmarks **sin cambio semántico** (el oráculo VM↔intérprete no lo ve). No en wasm (no hay
/// allocador de sistema que reemplazar; mimalloc necesita C).
#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub mod ast;
pub mod builtins;
pub mod bundle_android;
pub mod bundle_ios;
pub mod bytecode;
pub mod checker;
pub mod cli;
pub mod compiler;
pub mod deps;
pub mod dev_host;
pub mod diagnostic;
pub mod editorconfig;
pub mod ffi;
pub mod fmt;
pub mod gc;
pub mod index;
#[cfg(feature = "interp")]
pub mod interpreter;
pub mod lexer;
pub mod loader;
pub mod lsp;
pub mod mcp;
pub mod raydoc;
pub mod manifest;
pub mod parser;
pub mod transpile;
pub mod poll;
pub mod prelude;
pub mod repl;
pub mod runtime;
pub mod semver;
pub mod sha256;
pub mod stdlib;
pub mod templ;
pub mod test_runner;
pub mod token;
pub mod toolchain;
pub mod vm;
/// M44a-3: el punto de entrada WebAssembly del playground (solo en `wasm32`).
#[cfg(target_arch = "wasm32")]
pub mod wasm;

/// M44a-3: enrutado de la salida de `print`/`eprint` (los opcodes `Print`/`EPrint` y el builtin del
/// intérprete pasan por aquí). En **nativo** va a stdout/stderr (idéntico al `println!`/`eprintln!` de
/// siempre); en el **playground web** (`wasm32`, sin stdout) se acumula en un buffer que el entry point
/// `wasm::run` devuelve. Así el mismo programa imprime en ambos entornos.
#[inline]
pub fn host_print(s: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let r = out.write_all(s.as_bytes()).and_then(|()| out.write_all(b"\n"));
        host_write_failed(r);
    }
    #[cfg(target_arch = "wasm32")]
    wasm::push_stdout(s);
}
#[inline]
pub fn host_eprint(s: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::Write;
        let mut err = std::io::stderr().lock();
        let r = err.write_all(s.as_bytes()).and_then(|()| err.write_all(b"\n"));
        host_write_failed(r);
    }
    #[cfg(target_arch = "wasm32")]
    wasm::push_stderr(s);
}

/// Maneja el fallo de escritura de `print`/`eprint` (que no tienen canal de error). Un **pipe
/// cerrado** (`programa | head`) sigue la convención Unix: el proceso termina EN SILENCIO con
/// código 141 (128+SIGPIPE) — antes reventaba con un pánico de Rust disfrazado de ICE (Rust
/// ignora SIGPIPE y `println!` paniquea con Broken pipe; C muere por la señal, Go re-arma la
/// señal para fd 1/2 — el 141 replica ese destino observable). Otros errores de escritura se
/// ignoran (mejor-esfuerzo: no hay a quién reportarlos). `io.write`/`io.flush` NO pasan por
/// aquí: tienen `Result` y el programa decide.
#[cfg(not(target_arch = "wasm32"))]
fn host_write_failed(r: std::io::Result<()>) {
    if matches!(r, Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe) {
        std::process::exit(141);
    }
}

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
        .map_err(|e| runtime::RuntimeError { msg: e.msg, line: e.line, col: e.col, trace: Vec::new() })?;
    vm::run_program(&compiled)
}

/// Lanza `f` en el hilo worker de pila grande y devuelve el resultado del `join`
/// (el pánico del worker, si lo hubo, viaja como `Err(payload)`).
fn spawn_big_stack<F, T>(f: F) -> std::thread::Result<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    raise_fd_limit();
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(f)
        .unwrap_or_else(|e| ice!("could not create the large-stack worker thread: {e}"))
        .join()
}

/// Sube el límite BLANDO de descriptores de archivo al duro (acotado) al arrancar — lo que hace
/// todo servidor de producción. El default de macOS es 256: un `wrk -c500` contra un webserver
/// raylang agotaba los fds ("Too many open files") sin que el programa tuviera culpa ni remedio.
/// Best-effort (si falla, se sigue con el límite heredado); también lo llama el preámbulo de los
/// binarios nativos (`ray build --native`).
pub fn raise_fd_limit() {
    #[cfg(unix)]
    {
        #[repr(C)]
        struct RLimit {
            cur: u64,
            max: u64,
        }
        // RLIMIT_NOFILE: 7 en Linux, 8 en macOS/BSD.
        #[cfg(target_os = "linux")]
        const RLIMIT_NOFILE: i32 = 7;
        #[cfg(not(target_os = "linux"))]
        const RLIMIT_NOFILE: i32 = 8;
        unsafe extern "C" {
            fn getrlimit(resource: i32, rlim: *mut RLimit) -> i32;
            fn setrlimit(resource: i32, rlim: *const RLimit) -> i32;
        }
        // Tope prudente: macOS rechaza cur > OPEN_MAX (10240) cuando el duro es "unlimited".
        let cap: u64 = if cfg!(target_os = "macos") { 10240 } else { 65536 };
        unsafe {
            let mut r = RLimit { cur: 0, max: 0 };
            if getrlimit(RLIMIT_NOFILE, &mut r) != 0 {
                return;
            }
            let target = r.max.min(cap);
            if target > r.cur {
                let new = RLimit { cur: target, max: r.max };
                let _ = setrlimit(RLIMIT_NOFILE, &new);
            }
        }
    }
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
    spawn_big_stack(f).expect("the worker thread panicked") // ice-ok: re-lanzar es deliberado (tests)
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
    // M112: el pánico de EPIPE de la libstd NO es un ICE. Rust ignora SIGPIPE al arrancar y
    // `println!`/`eprintln!` paniquean si su destino se cerró (`ray fmt x | head`, `ray doc |
    // less` y salir…). El programa del usuario ya no pasa por aquí (#126: `host_print` maneja el
    // EPIPE antes de paniquear), pero la salida del PROPIO CLI —fmt, doc, diagnósticos, REPL— usa
    // los macros de std. Dos piezas: el hook silencia la traza del pánico (corre en el hilo que
    // panica, ANTES de que el payload llegue aquí) y el catch convierte el payload en la
    // convención Unix: exit 141 (128+SIGPIPE), en silencio — el destino de un programa C.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !is_broken_pipe_panic_message(&panic_payload_text(info.payload())) {
            previous(info);
        }
    }));
    // M146 (std/ui): el hilo 1 del proceso —el ÚNICO que AppKit acepta— ya no se queda dentro
    // del `join()`: espera en un canal (worker terminó | una operación de UI pidió el hilo
    // principal). El join y su match de payload (141 broken-pipe / 101 ICE) se conservan
    // VERBATIM, en un hilo monitor — el comportamiento sin UI es idéntico al de siempre.
    enum GateMsg<T> {
        // En builds sin `ui` (o no-macOS) la variante no se construye — el brazo queda por
        // simetría y el canal solo lleva `Done`.
        #[allow(dead_code)]
        UiRequested,
        Done(T),
    }
    let (tx, rx) = std::sync::mpsc::channel::<GateMsg<T>>();
    #[cfg(all(feature = "ui", any(unix, windows), not(target_arch = "wasm32")))]
    {
        let tx_ui = tx.clone();
        ray_runtime::ui::set_main_thread_waker(Box::new(move || {
            let _ = tx_ui.send(GateMsg::UiRequested);
        }));
    }
    std::thread::spawn(move || match spawn_big_stack(f) {
        Ok(v) => {
            let _ = tx.send(GateMsg::Done(v));
        }
        Err(payload) => {
            let detail = panic_payload_text(payload.as_ref());
            if is_broken_pipe_panic_message(&detail) {
                std::process::exit(141);
            }
            eprintln!("{}", diagnostic::ice_banner(&detail));
            std::process::exit(101);
        }
    });
    loop {
        match rx.recv() {
            Ok(GateMsg::Done(v)) => return v,
            Ok(GateMsg::UiRequested) => {
                // Entra al loop de AppKit y NO retorna (la app vive lo que el proceso; el
                // worker sale por `process::exit`). Solo un fallo de inicialización (sin
                // sesión gráfica) vuelve: la operación que esperaba ya recibió su Err, y el
                // hilo 1 sigue esperando el final del worker.
                #[cfg(all(feature = "ui", any(unix, windows), not(target_arch = "wasm32")))]
                let _ = ray_runtime::ui::run_main_loop();
            }
            // Sin remitentes y sin `Done`: el monitor murió sin poder reportar (no ocurre — su
            // camino de pánico termina en `process::exit`). Nunca colgar el proceso por esto.
            Err(_) => std::process::exit(101),
        }
    }
}

/// El texto de un payload de pánico (`String` o `&str`; otra cosa → marcador).
fn panic_payload_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "panic without message".to_string())
}

/// ¿Es el pánico de la libstd por escribir en un stdout/stderr cerrado? (El texto es estable en
/// la práctica: `failed printing to stdout: Broken pipe (os error 32)` en unix; en Windows el
/// mismo caso es `The pipe is being closed. (os error 232)` o `The pipe has been ended. (os error
/// 109)` — M173. Si algún día cambiara, lo peor que pasa es volver al banner de ICE — nunca se
/// silencia un pánico que no sea este.)
fn is_broken_pipe_panic_message(detail: &str) -> bool {
    detail.starts_with("failed printing to")
        && (detail.contains("Broken pipe") || detail.contains("(os error 232)") || detail.contains("(os error 109)"))
}
