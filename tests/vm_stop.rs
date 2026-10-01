//! D1 del arco de hot reload móvil (findings #50/#106): **parada cooperativa** de la VM desde
//! fuera del programa (`vm::stop::request_stop`) y reset del runtime para correr OTRO programa
//! en el mismo proceso. Archivo propio (no `src/vm/tests.rs`): la bandera de parada es global
//! al proceso, y aquí cada test toma un mutex para no parar el programa de otro.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use raylang::runtime::Value;

/// Cómo acabó un programa: `Stopped` (el error distinguido de la parada), `Failed` (otro error)
/// u `Ok` con el valor mostrado. Los valores de la VM llevan `Rc`, así que cruzan el hilo como
/// texto.
#[derive(Debug, PartialEq)]
enum Outcome {
    Stopped,
    Failed(String),
    Ok(String),
}

fn serial() -> std::sync::MutexGuard<'static, ()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
}

/// Escribe el programa en un directorio temporal propio (el loader trabaja sobre rutas: así
/// resuelven los módulos de la stdlib embebida, `net`, etc.), lo carga, chequea y compila.
fn compile(src: &str) -> raylang::bytecode::CompiledProgram {
    let dir = std::env::temp_dir().join(format!("ray-vm-stop-{}-{}", std::process::id(), rand_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    let entry = dir.join("main.ray");
    std::fs::write(&entry, src).unwrap();
    let mut loaded = raylang::loader::load(&entry).unwrap_or_else(|e| panic!("load: {}", e.message));
    raylang::checker::check(&mut loaded.program).unwrap_or_else(|e| panic!("check at {}:{}: {}", e.line, e.col, e.msg));
    raylang::compiler::compile_program(&loaded.program).unwrap_or_else(|e| panic!("compile: {e}"))
}

fn rand_suffix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64
}

/// Corre el programa en un hilo, pide la parada a los `after_ms` y devuelve (resultado, duración
/// desde la petición hasta que la VM volvió).
fn run_and_stop(src: &str, after_ms: u64) -> (Outcome, Duration) {
    let src = src.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let compiled = compile(&src); // `CompiledProgram` lleva `Rc`: se compila en este hilo
        let r = run_outcome(&compiled);
        tx.send(Instant::now()).unwrap();
        r
    });
    std::thread::sleep(Duration::from_millis(after_ms));
    let asked = Instant::now();
    raylang::vm::stop::request_stop();
    let result = handle.join().expect("the program thread should not panic");
    let ended = rx.recv().unwrap();
    (result, ended.saturating_duration_since(asked))
}

fn run_outcome(compiled: &raylang::bytecode::CompiledProgram) -> Outcome {
    match raylang::vm::run_program(compiled) {
        Ok(v) => Outcome::Ok(format!("{v:?}")),
        Err(e) if e.is_stop() => Outcome::Stopped,
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

fn assert_stopped_fast(result: &Outcome, took: Duration, what: &str) {
    assert_eq!(*result, Outcome::Stopped, "{what}: expected the stop error");
    assert!(took < Duration::from_millis(1500), "{what}: stopping took {took:?}");
}

#[test]
fn a_loop_that_never_yields_stops_within_the_instruction_budget() {
    let _g = serial();
    let (r, took) = run_and_stop("fn main() { var i = 0; while (true) { i = i + 1; } }", 100);
    assert_stopped_fast(&r, took, "tight loop");
}

#[test]
fn a_sleeping_fiber_wakes_up_to_stop() {
    let _g = serial();
    let (r, took) = run_and_stop("import std/time;\nfn main() { time.sleep(60000); }", 100);
    assert_stopped_fast(&r, took, "sleep");
}

#[test]
fn a_fiber_parked_on_the_reactor_wakes_up_to_stop() {
    let _g = serial();
    // Un listener en puerto 0 aparcado en `accept`: la única fibra espera E/S sin plazo.
    let src = r#"
import std/net;
fn main() -> int {
    match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(srv) => {
            match (net.tcp_accept(srv)) {
                Result.Ok(c) => { close(c); 1 },
                Result.Err(_) => 2,
            }
        },
        Result.Err(_) => 3,
    }
}
"#;
    let (r, took) = run_and_stop(src, 100);
    assert_stopped_fast(&r, took, "accept");
}

#[test]
fn a_spinning_task_on_another_worker_stops_too() {
    let _g = serial();
    // Con `spawn` la VM lanza varios workers: la tarea gira en uno y main espera en otro.
    let src = r#"
fn main() {
    let t = spawn(fn() -> int { var i = 0; while (true) { i = i + 1; } });
    let _ = join(t);
}
"#;
    let (r, took) = run_and_stop(src, 100);
    assert_stopped_fast(&r, took, "spawned spinner");
}

#[test]
fn after_a_stop_and_a_reset_another_program_runs_in_the_same_process() {
    let _g = serial();
    // El primero se queda escuchando y lo paramos; el reset debe soltar su listener y dejar el
    // runtime limpio para el segundo, que corre hasta el final con normalidad.
    let src = r#"
import std/net;
fn main() {
    match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(srv) => { let _ = net.tcp_accept(srv); },
        Result.Err(_) => {},
    }
}
"#;
    let (r, took) = run_and_stop(src, 100);
    assert_stopped_fast(&r, took, "first program");
    raylang::builtins::runtime_reset();
    let second = compile("fn main() -> int { var s = 0; var i = 0; while (i < 1000) { s = s + i; i = i + 1; } s }");
    let v = raylang::vm::run_program(&second).expect("the second program should run to completion");
    assert_eq!(v, Value::Int(499500));
}

#[test]
fn a_stop_requested_with_no_program_running_does_not_affect_the_next_one() {
    let _g = serial();
    raylang::vm::stop::request_stop();
    let p = compile("fn main() -> int { 40 + 2 }");
    assert_eq!(raylang::vm::run_program(&p).expect("runs"), Value::Int(42));
}
