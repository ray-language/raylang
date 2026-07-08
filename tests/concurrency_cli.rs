//! Pruebas de **concurrencia** (M12.1 slice CSP + M12.2 acotados/backpressure + M12.3 structured
//! concurrency + M12.4 `select` + M12.5 cancelación de hermanas) sobre el binario.
//!
//! La concurrencia vive SOLO en la VM (el intérprete da un error limpio). El scheduler es **cooperativo
//! M:1 y determinista** (cola FIFO, puntos de yield = `recv` bloqueante y, desde M12.2, `send` sobre un
//! canal lleno), así que la salida de un programa concurrente es fija → se compara contra **salida
//! esperada exacta** (no hay oráculo cruzado VM↔intérprete para programas concurrentes). Se prueba por
//! subproceso con `raylang --vm <archivo>`. Ver DESIGN §21.2/§21.3.

use std::io::Write;
use std::process::Command;

/// Escribe `src` en un `.ray` temporal, ejecuta raylang y devuelve (stdout, stderr, código).
/// `vm = true` corre en la VM (el default de M35; se pasa `--vm`, redundante pero explícito);
/// `vm = false` fuerza el **intérprete** (`--interp`) para verificar su error limpio ante la
/// concurrencia — que ya no es el default, así que hay que pedirlo.
fn run(name: &str, src: &str, vm: bool) -> (String, String, i32) {
    let mut path = std::env::temp_dir();
    path.push(format!("{name}.ray"));
    std::fs::File::create(&path).expect("crea").write_all(src.as_bytes()).expect("escribe");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    cmd.arg(if vm { "--vm" } else { "--interp" });
    // M38.4: el default de la VM es multicore (orden de fibras NO determinista); estas pruebas comparan
    // contra salida EXACTA (orden FIFO), así que fuerzan el scheduler M:1 reproducible con `--deterministic`.
    if vm {
        cmd.arg("--deterministic");
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
    let ch: Channel<int> = Channel.new();
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
    let ch: Channel<int> = Channel.new();
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
    let ch: Channel<int> = Channel.new();
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
    let ch: Channel<int> = Channel.new();
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
    let ch: Channel<int> = Channel.new();
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
    let ch: Channel<int> = Channel.new();
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
    let ch: Channel<int> = Channel.new();
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
    let nums: Channel<int> = Channel.new();
    let sqrs: Channel<int> = Channel.new();
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

// --- M12.2: canales acotados / backpressure ---

#[test]
fn backpressure_canal_acotado() {
    // Canal acotado a 2: el productor genera 1..5 más rápido de lo que el consumidor lee. Con backpressure
    // el `send` se bloquea al llenarse la cola (no se desborda); el consumidor recibe todo en orden.
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = Channel.bounded(2);
    spawn(fn() {
        var i = 1;
        while (i <= 5) { send(ch, i); i = i + 1; }
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
    let (out, _err, code) = run("conc_backpressure", src, true);
    assert_eq!(out, "1\n2\n3\n4\n5\n15\n");
    assert_eq!(code, 15);
}

#[test]
fn rendezvous_capacidad_cero() {
    // Canal de capacidad 0 (síncrono): cada `send` se completa solo cuando hay un `recv` esperando.
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = Channel.bounded(0);
    spawn(fn() {
        send(ch, 10); send(ch, 20); send(ch, 30);
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
    total
}
"#;
    let (out, _err, code) = run("conc_rendezvous", src, true);
    assert_eq!(out, "10\n20\n30\n");
    assert_eq!(code, 60);
}

#[test]
fn close_con_emisor_bloqueado_es_error() {
    // Cerrar un canal del que un emisor todavía espera enviar es un error de programa, detectado de forma
    // determinista en el sitio del `close`.
    let src = r#"
fn productor(ch: Channel<int>) { send(ch, 1); send(ch, 2); send(ch, 3); }
fn main() -> int {
    let ch: Channel<int> = Channel.bounded(1);
    spawn(fn() { productor(ch); });
    let primero: Option<int> = recv(ch);  // recibe 1; el productor bufferiza 2 y se bloquea en send(3)
    close(ch);                            // emisor bloqueado -> error
    0
}
"#;
    let (_out, err, code) = run("conc_close_emisor", src, true);
    assert!(err.contains("emisor bloqueado"), "stderr no menciona emisor bloqueado: {err}");
    assert_eq!(code, 70);
}

#[test]
fn deadlock_por_emisor_bloqueado() {
    // Un `send` sobre un canal síncrono que nadie recibirá bloquea al emisor para siempre → deadlock.
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = Channel.bounded(0);
    send(ch, 1);
    0
}
"#;
    let (_out, err, code) = run("conc_deadlock_emisor", src, true);
    assert!(err.contains("deadlock"), "stderr no menciona deadlock: {err}");
    assert_eq!(code, 70);
}

// --- M12.3: structured concurrency (Task<T> + join + scope) ---

#[test]
fn scope_join_valor_de_retorno() {
    // spawn devuelve Task<int>; join bloquea y da su valor; el scope devuelve el valor del cuerpo.
    let src = r#"
fn cuadrado(n: int) -> int { n * n }
fn main() -> int {
    let total = scope(fn() -> int {
        let a: Task<int> = spawn(fn() -> int { cuadrado(3) });
        let b: Task<int> = spawn(fn() -> int { cuadrado(4) });
        join(a) + join(b)
    });
    print(total);
    total
}
"#;
    let (out, _err, code) = run("conc_scope_join", src, true);
    assert_eq!(out, "25\n");
    assert_eq!(code, 25);
}

#[test]
fn scope_une_tareas_no_unidas() {
    // El scope espera a una tarea aunque no se la una explícitamente: al salir, ya terminó.
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = Channel.new();
    scope(fn() {
        spawn(fn() { send(ch, 42); });
    });
    match (recv(ch)) {
        Option.Some(v) => print(v),
        Option.None => print(0 - 1),
    }
    0
}
"#;
    let (out, _err, code) = run("conc_scope_autojoin", src, true);
    assert_eq!(out, "42\n");
    assert_eq!(code, 0);
}

#[test]
fn join_propaga_el_panic_de_la_tarea() {
    // Una tarea que hace panic: join re-lanza ese fallo en el sitio del join (propagación).
    let src = r#"
fn main() -> int {
    let t: Task<int> = spawn(fn() -> int { panic("boom") });
    let v = join(t);
    print(v);
    0
}
"#;
    let (_out, err, code) = run("conc_join_panic", src, true);
    assert!(err.contains("boom"), "stderr no propaga el panic: {err}");
    assert_eq!(code, 70);
}

#[test]
fn scope_propaga_el_panic_de_una_hija() {
    // Una tarea lanzada dentro del scope hace panic: el scope lo propaga al unir al salir.
    let src = r#"
fn main() -> int {
    scope(fn() -> int {
        spawn(fn() -> int { panic("la hija fallo") });
        0
    });
    print(999);
    0
}
"#;
    let (out, err, code) = run("conc_scope_panic", src, true);
    assert_eq!(out, "");
    assert!(err.contains("la hija fallo"), "stderr no propaga el panic de la hija: {err}");
    assert_eq!(code, 70);
}

#[test]
fn scope_de_varias_tareas() {
    // Varias tareas en un scope, unidas en orden: ejercita el scheduler y el GC multi-raíz.
    let src = r#"
fn main() -> int {
    let suma = scope(fn() -> int {
        var tareas: [Task<int>] = [];
        var i = 1;
        while (i <= 5) {
            let n = i;
            tareas.push(spawn(fn() -> int { n * n }));
            i = i + 1;
        }
        var total = 0;
        var j = 0;
        while (j < tareas.len()) {
            total = total + join(tareas[j]);
            j = j + 1;
        }
        total
    });
    print(suma);
    suma
}
"#;
    let (out, _err, code) = run("conc_scope_varias", src, true);
    assert_eq!(out, "55\n"); // 1+4+9+16+25
    assert_eq!(code, 55);
}

// --- M12.4: select sobre varios canales ---

#[test]
fn select_multiplexa_dos_canales() {
    // Dos productores en canales distintos; main multiplexa con select (devuelve el índice listo).
    let src = r#"
fn main() -> int {
    let a: Channel<int> = Channel.new();
    let b: Channel<int> = Channel.new();
    spawn(fn() { send(a, 10); });
    spawn(fn() { send(b, 20); });
    let chs: [Channel<int>] = [a, b];
    var total = 0;
    var n = 0;
    while (n < 2) {
        let i = select(chs);
        match (recv(chs[i])) {
            Option.Some(v) => { print(v); total = total + v; },
            Option.None => {},
        }
        n = n + 1;
    }
    print(total);
    total
}
"#;
    let (out, _err, code) = run("conc_select_mux", src, true);
    assert_eq!(out, "10\n20\n30\n");
    assert_eq!(code, 30);
}

#[test]
fn select_detecta_canal_cerrado() {
    // Un canal cerrado cuenta como "listo": select lo devuelve y el recv da None.
    let src = r#"
fn main() -> int {
    let a: Channel<int> = Channel.new();
    spawn(fn() { close(a); });
    let chs: [Channel<int>] = [a];
    let i = select(chs);
    match (recv(chs[i])) {
        Option.Some(v) => print(v),
        Option.None => print(0 - 7),
    }
    0
}
"#;
    let (out, _err, code) = run("conc_select_cerrado", src, true);
    assert_eq!(out, "-7\n");
    assert_eq!(code, 0);
}

#[test]
fn select_bloquea_hasta_que_un_canal_este_listo() {
    // Un canal nunca recibe valor; select bloquea hasta que el otro lo tiene, y devuelve su índice.
    let src = r#"
fn productor(ch: Channel<int>) {
    var i = 0;
    while (i < 3) { i = i + 1; }
    send(ch, 99);
}
fn main() -> int {
    let a: Channel<int> = Channel.new();
    let b: Channel<int> = Channel.new();
    spawn(fn() { productor(b); });
    let chs: [Channel<int>] = [a, b];
    let i = select(chs);
    print(i);
    match (recv(chs[i])) { Option.Some(v) => print(v), Option.None => {} }
    0
}
"#;
    let (out, _err, code) = run("conc_select_bloquea", src, true);
    assert_eq!(out, "1\n99\n");
    assert_eq!(code, 0);
}

#[test]
fn select_sin_fuente_es_deadlock() {
    // select sobre un canal que nadie alimenta ni cierra → deadlock (error de ejecución limpio).
    let src = r#"
fn main() -> int {
    let a: Channel<int> = Channel.new();
    let chs: [Channel<int>] = [a];
    let i = select(chs);
    print(i);
    0
}
"#;
    let (_out, err, code) = run("conc_select_deadlock", src, true);
    assert!(err.contains("deadlock"), "stderr no menciona deadlock: {err}");
    assert_eq!(code, 70);
}

// --- M12.5: cancelación de hermanas ---

#[test]
fn scope_cancela_a_las_hermanas_cuando_una_falla() {
    // La hija 0 hace panic; la hija 1 se bloquearía para siempre e imprimiría al final. El scope cancela a
    // la hija 1 (no llega a imprimir) y propaga el panic ORIGINAL (no un deadlock por esperarla).
    let src = r#"
fn main() -> int {
    scope(fn() -> int {
        spawn(fn() -> int { panic("boom") });
        spawn(fn() -> int {
            let ch: Channel<int> = Channel.new();
            recv(ch);
            print(777);
            0
        });
        0
    });
    print(999);
    0
}
"#;
    let (out, err, code) = run("conc_cancel_hermana", src, true);
    assert_eq!(out, ""); // ni 777 (cancelada) ni 999 (el scope propagó)
    assert!(err.contains("boom"), "stderr no propaga el fallo original: {err}");
    assert!(!err.contains("deadlock"), "no debería haber deadlock: {err}");
    assert_eq!(code, 70);
}

#[test]
fn fibra_que_falla_cancela_sus_propias_tareas() {
    // Una tarea externa abre su scope; su cuerpo hace panic con una sub-tarea en vuelo. Al fallar, esa
    // fibra cancela sus hijos (la sub-tarea no llega a imprimir), y join re-lanza el panic.
    let src = r#"
fn main() -> int {
    let t: Task<int> = spawn(fn() -> int {
        scope(fn() -> int {
            spawn(fn() -> int {
                let ch: Channel<int> = Channel.new();
                recv(ch);
                print(555);
                0
            });
            panic("externa falla")
        })
    });
    let v = join(t);
    print(v);
    0
}
"#;
    let (out, err, code) = run("conc_cancel_subtareas", src, true);
    assert_eq!(out, ""); // ni 555 (sub-tarea cancelada) ni v
    assert!(err.contains("externa falla"), "stderr no propaga el fallo: {err}");
    assert_eq!(code, 70);
}
