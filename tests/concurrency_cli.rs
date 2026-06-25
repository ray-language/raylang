//! Pruebas de **concurrencia** (M12.1, CSP sobre la VM) sobre el binario.
//!
//! La concurrencia vive SOLO en la VM (el intérprete da un error limpio). El scheduler es **cooperativo
//! M:1 y determinista** (cola FIFO, único punto de yield = `recv` bloqueante), así que la salida de un
//! programa concurrente es fija → se compara contra **salida esperada exacta** (no hay oráculo cruzado
//! VM↔intérprete para programas concurrentes). Se prueba por subproceso con `raylang --vm <archivo>`.
//! Ver DESIGN §21.2.

use std::io::Write;
use std::process::Command;

/// Escribe `src` en un `.ray` temporal, ejecuta `raylang [--vm] <archivo>` y devuelve (stdout, stderr, código).
fn run(name: &str, src: &str, vm: bool) -> (String, String, i32) {
    let mut path = std::env::temp_dir();
    path.push(format!("{name}.ray"));
    std::fs::File::create(&path).expect("crea").write_all(src.as_bytes()).expect("escribe");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    if vm {
        cmd.arg("--vm");
    }
    let out = cmd.arg(&path).output().expect("lanza raylang");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn productor_consumidor() {
    // Una fibra produce 5 valores y cierra; main los consume vía recv -> Option, sumando.
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = channel();
    spawn(fn() {
        var i = 1;
        while (i <= 5) { send(ch, i * 10); i = i + 1; }
        close(ch);
    });
    var total = 0;
    var seguir = true;
    while (seguir) {
        match (recv(ch)) {
            Option.Some(v) => { print(v); total = total + v; },
            Option.None => { seguir = false; },
        }
    }
    print(total);
    total
}
"#;
    let (out, _err, code) = run("conc_prodcons", src, true);
    assert_eq!(out, "10\n20\n30\n40\n50\n150\n");
    assert_eq!(code, 150);
}

#[test]
fn orden_determinista_dos_productores() {
    // Dos productores + un consumidor: el scheduler determinista fija el orden de entrega.
    let src = r#"
fn productor(ch: Channel<int>, base: int) {
    var i = 0;
    while (i < 3) { send(ch, base + i); i = i + 1; }
}
fn main() -> int {
    let ch: Channel<int> = channel();
    spawn(fn() { productor(ch, 100); });
    spawn(fn() { productor(ch, 200); });
    var n = 0;
    while (n < 6) {
        match (recv(ch)) { Option.Some(v) => print(v), Option.None => {} }
        n = n + 1;
    }
    0
}
"#;
    let (out, _err, code) = run("conc_orden", src, true);
    assert_eq!(out, "100\n101\n102\n200\n201\n202\n");
    assert_eq!(code, 0);
}

#[test]
fn closure_captura_en_fibra() {
    // La función spawneada es una closure que captura una variable de main (por celda compartida).
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = channel();
    let factor = 7;
    spawn(fn() {
        var i = 1;
        while (i <= 3) { send(ch, i * factor); i = i + 1; }
        close(ch);
    });
    var seguir = true;
    while (seguir) {
        match (recv(ch)) { Option.Some(v) => print(v), Option.None => { seguir = false; } }
    }
    0
}
"#;
    let (out, _err, code) = run("conc_closure", src, true);
    assert_eq!(out, "7\n14\n21\n");
    assert_eq!(code, 0);
}

#[test]
fn close_da_none() {
    // recv sobre un canal cerrado y vacío devuelve None (sin bloquear).
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = channel();
    close(ch);
    match (recv(ch)) {
        Option.Some(_) => print("algo"),
        Option.None => print("cerrado"),
    }
    0
}
"#;
    let (out, _err, code) = run("conc_close", src, true);
    assert_eq!(out, "cerrado\n");
    assert_eq!(code, 0);
}

#[test]
fn deadlock_detectado() {
    // recv sobre un canal vacío que nadie alimentará ni cerrará → deadlock (error de ejecución limpio).
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = channel();
    match (recv(ch)) { Option.Some(v) => print(v), Option.None => print(0 - 1) }
    0
}
"#;
    let (out, err, code) = run("conc_deadlock", src, true);
    assert_eq!(out, "");
    assert!(err.contains("deadlock"), "stderr no menciona deadlock: {err}");
    assert_eq!(code, 70);
}

#[test]
fn send_a_canal_cerrado_es_error() {
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = channel();
    close(ch);
    send(ch, 1);
    0
}
"#;
    let (_out, err, code) = run("conc_send_cerrado", src, true);
    assert!(err.contains("canal cerrado"), "stderr no menciona canal cerrado: {err}");
    assert_eq!(code, 70);
}

#[test]
fn interprete_da_error_limpio() {
    // La concurrencia requiere la VM: el intérprete (sin --vm) da un error de ejecución claro, no un panic.
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = channel();
    send(ch, 1);
    0
}
"#;
    let (_out, err, code) = run("conc_interp", src, false);
    assert!(err.contains("requiere la VM"), "stderr no pide la VM: {err}");
    assert_eq!(code, 70);
}

#[test]
fn pipeline_de_fibras() {
    // Pipeline CSP: generador -> cuadrador -> main. Cada etapa es una fibra; comunican por canales.
    let src = r#"
fn main() -> int {
    let nums: Channel<int> = channel();
    let sqrs: Channel<int> = channel();
    // Generador: 1..4 -> nums
    spawn(fn() {
        var i = 1;
        while (i <= 4) { send(nums, i); i = i + 1; }
        close(nums);
    });
    // Cuadrador: nums -> sqrs
    spawn(fn() {
        var seguir = true;
        while (seguir) {
            match (recv(nums)) {
                Option.Some(v) => send(sqrs, v * v),
                Option.None => { close(sqrs); seguir = false; },
            }
        }
    });
    var total = 0;
    var seguir = true;
    while (seguir) {
        match (recv(sqrs)) {
            Option.Some(v) => { print(v); total = total + v; },
            Option.None => { seguir = false; },
        }
    }
    total
}
"#;
    let (out, _err, code) = run("conc_pipeline", src, true);
    assert_eq!(out, "1\n4\n9\n16\n");
    assert_eq!(code, 30);
}
