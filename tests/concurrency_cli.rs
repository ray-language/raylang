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
fn producer_consumer() {
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
    var follow = true;
    while (follow) {
        match (recv(ch)) {
            Option.Some(v) => { print(v); total = total + v; },
            Option.None => { follow = false; },
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
fn deterministic_order_two_producers() {
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
    let (out, _err, code) = run("conc_order", src, true);
    assert_eq!(out, "100\n101\n102\n200\n201\n202\n");
    assert_eq!(code, 0);
}

#[test]
fn closure_capture_en_fiber() {
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
    var follow = true;
    while (follow) {
        match (recv(ch)) { Option.Some(v) => print(v), Option.None => { follow = false; } }
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
        Option.None => print("closed"),
    }
    0
}
"#;
    let (out, _err, code) = run("conc_close", src, true);
    assert_eq!(out, "closed\n");
    assert_eq!(code, 0);
}

#[test]
fn deadlock_detected() {
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
fn send_to_closed_channel_is_error() {
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = Channel.new();
    close(ch);
    send(ch, 1);
    0
}
"#;
    let (_out, err, code) = run("conc_send_closed", src, true);
    assert!(err.contains("closed channel"), "stderr no menciona canal closed: {err}");
    assert_eq!(code, 70);
}

#[test]
fn interpreter_da_error_clean() {
    // La concurrencia requiere la VM: el intérprete (sin --vm) da un error de ejecución claro, no un panic.
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = Channel.new();
    send(ch, 1);
    0
}
"#;
    let (_out, err, code) = run("conc_interp", src, false);
    assert!(err.contains("requires the VM"), "stderr no asks la VM: {err}");
    assert_eq!(code, 70);
}

#[test]
fn fiber_pipeline() {
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
        var follow = true;
        while (follow) {
            match (recv(nums)) {
                Option.Some(v) => send(sqrs, v * v),
                Option.None => { close(sqrs); follow = false; },
            }
        }
    });
    var total = 0;
    var follow = true;
    while (follow) {
        match (recv(sqrs)) {
            Option.Some(v) => { print(v); total = total + v; },
            Option.None => { follow = false; },
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
fn backpressure_canal_bounded() {
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
    var follow = true;
    while (follow) {
        match (recv(ch)) {
            Option.Some(v) => { print(v); total = total + v; },
            Option.None => { follow = false; },
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
fn rendezvous_zero_capacity() {
    // Canal de capacidad 0 (síncrono): cada `send` se completa solo cuando hay un `recv` esperando.
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = Channel.bounded(0);
    spawn(fn() {
        send(ch, 10); send(ch, 20); send(ch, 30);
        close(ch);
    });
    var total = 0;
    var follow = true;
    while (follow) {
        match (recv(ch)) {
            Option.Some(v) => { print(v); total = total + v; },
            Option.None => { follow = false; },
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
fn close_with_blocked_sender_is_error() {
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
    assert!(err.contains("blocked sender"), "stderr no menciona emisor bloqueado: {err}");
    assert_eq!(code, 70);
}

#[test]
fn deadlock_from_blocked_sender() {
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
fn scope_join_returns_value() {
    // spawn devuelve Task<int>; join bloquea y da su valor; el scope devuelve el valor del cuerpo.
    let src = r#"
fn square(n: int) -> int { n * n }
fn main() -> int {
    let total = scope(fn() -> int {
        let a: Task<int> = spawn(fn() -> int { square(3) });
        let b: Task<int> = spawn(fn() -> int { square(4) });
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
fn scope_joins_unjoined_tasks() {
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
fn join_propagates_panic_from_task() {
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
    assert!(err.contains("boom"), "stderr no propagates el panic: {err}");
    assert_eq!(code, 70);
}

#[test]
fn scope_propagates_panic_from_one_child() {
    // Una tarea lanzada dentro del scope hace panic: el scope lo propaga al unir al salir.
    let src = r#"
fn main() -> int {
    scope(fn() -> int {
        spawn(fn() -> int { panic("la hija failure") });
        0
    });
    print(999);
    0
}
"#;
    let (out, err, code) = run("conc_scope_panic", src, true);
    assert_eq!(out, "");
    assert!(err.contains("la hija failure"), "stderr no propagates el panic de la hija: {err}");
    assert_eq!(code, 70);
}

#[test]
fn scope_with_various_tasks() {
    // Varias tareas en un scope, unidas en orden: ejercita el scheduler y el GC multi-raíz.
    let src = r#"
fn main() -> int {
    let sum = scope(fn() -> int {
        var tasks: [Task<int>] = [];
        var i = 1;
        while (i <= 5) {
            let n = i;
            tasks.push(spawn(fn() -> int { n * n }));
            i = i + 1;
        }
        var total = 0;
        var j = 0;
        while (j < tasks.len()) {
            total = total + join(tasks[j]);
            j = j + 1;
        }
        total
    });
    print(sum);
    sum
}
"#;
    let (out, _err, code) = run("conc_scope_various", src, true);
    assert_eq!(out, "55\n"); // 1+4+9+16+25
    assert_eq!(code, 55);
}

// --- M12.4: select sobre varios canales ---

#[test]
fn select_multiplexes_two_channels() {
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
fn select_detects_closed_channel() {
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
    let (out, _err, code) = run("conc_select_closed", src, true);
    assert_eq!(out, "-7\n");
    assert_eq!(code, 0);
}

#[test]
fn select_blocks_until_a_channel_is_ready() {
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
fn select_without_source_is_deadlock() {
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
fn scope_cancels_siblings_when_one_fails() {
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
    assert!(err.contains("boom"), "stderr no propagates el failure original: {err}");
    assert!(!err.contains("deadlock"), "no debería haber deadlock: {err}");
    assert_eq!(code, 70);
}

#[test]
fn scope_sees_failure_even_when_waiting_on_other_sibling() {
    // Regresión (17 jul 2026, destapada por el port nativo H21-N3): `ScopeEnd` aparca sobre la PRIMERA
    // hija pendiente; si la que falla es OTRA (aquí la bloqueada se registra primero), nadie despertaba
    // al scope para re-escanear → deadlock en vez de propagar. Ahora un fallo despierta a TODOS los
    // Join-waiters (despertar espurio seguro: re-escanean y se re-aparcan).
    let src = r#"
fn main() -> int {
    scope(fn() -> int {
        spawn(fn() -> int {
            let ch: Channel<int> = Channel.new();
            recv(ch);
            print(777);
            0
        });
        spawn(fn() -> int { panic("boom") });
        0
    });
    print(999);
    0
}
"#;
    let (out, err, code) = run("conc_cancel_hermana_orden_inverso", src, true);
    assert_eq!(out, ""); // ni 777 (cancelada) ni 999 (el scope propagó)
    assert!(err.contains("boom"), "stderr no propagates el failure original: {err}");
    assert!(!err.contains("deadlock"), "no debería haber deadlock: {err}");
    assert_eq!(code, 70);
}

#[test]
fn fiber_that_fails_cancels_its_own_tasks() {
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
            panic("externa fails")
        })
    });
    let v = join(t);
    print(v);
    0
}
"#;
    let (out, err, code) = run("conc_cancel_subtareas", src, true);
    assert_eq!(out, ""); // ni 555 (sub-tarea cancelada) ni v
    assert!(err.contains("externa fails"), "stderr no propagates el failure: {err}");
    assert_eq!(code, 70);
}

#[test]
fn try_join_observes_failure_without_rethrowing() {
    // M56.5: try_join une una tarea devolviendo su desenlace como VALOR — Ok(valor) si terminó,
    // Err(mensaje del panic) si falló — a diferencia de join, que re-lanza el fallo. El programa
    // sigue vivo tras observar un fallo (base del webserver: un handler que panica no tumba nada).
    let src = r#"
fn mala() -> int {
    panic("kaboom");
    0
}

fn main() -> int {
    let a: Task<int> = spawn(fn() -> int { 40 + 2 });
    match (try_join(a)) {
        Result.Ok(v) => print("ok " + to_string(v)),
        Result.Err(e) => print("err " + e),
    }
    let b: Task<int> = spawn(fn() -> int { mala() });
    match (try_join(b)) {
        Result.Ok(v) => print("ok " + to_string(v)),
        Result.Err(e) => print("err " + e),
    }
    let c = spawn(fn() { print("efecto") });
    match (try_join(c)) {
        Result.Ok(_) => print("unit ok"),
        Result.Err(e) => print("unit err " + e),
    }
    print("sigo live");
    0
}
"#;
    let (stdout, stderr, code) = run("try_join_failure", src, true);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "ok 42\nerr kaboom\nefecto\nunit ok\nsigo live\n");
}

#[test]
fn try_join_inside_scope_counts_as_handled() {
    // M97.1 (semántica FIJADA con el usuario): un fallo OBSERVADO con try_join dentro de un scope
    // cuenta como MANEJADO — el ScopeEnd lo trata como tarea terminada: NO cancela a las hermanas
    // ni re-lanza el fallo (antes: try_join observaba pero ScopeEnd re-lanzaba igual → try_join
    // era inútil dentro de scopes). `join` en cambio SIGUE re-lanzando (observar ≠ unir); y un
    // fallo NO observado conserva la cancelación de M12.5 (test scope_cancela_a_las_hermanas...).
    let src = r#"
fn main() -> int {
    let r = scope(fn() -> int {
        let bad = spawn(fn() -> int { panic("boom"); 0 });
        let good = spawn(fn() -> int { 42 });
        match (try_join(bad)) {
            Result.Ok(v) => print("bad ok: " + to_string(v)),
            Result.Err(msg) => print("bad manejado: " + msg),
        }
        print("good: " + to_string(join(good)));
        7
    });
    print("scope devolvio: " + to_string(r));
    0
}
"#;
    let (stdout, stderr, code) = run("try_join_en_scope", src, true);
    assert_eq!(code, 0, "el scope NO debe re-lanzar el fallo observado; stderr: {stderr}");
    assert_eq!(stdout, "bad manejado: boom\ngood: 42\nscope devolvio: 7\n");
}

// --- M98.1: el almacén de tareas libera (join/try_join consumen; el scope consume a sus hijas) ---

#[test]
fn join_consumes_task_and_double_join_is_error() {
    // M98.1 (semántica FIJADA): una tarea es de UN solo consumidor — `join`/`try_join` la toman
    // (liberan su slot y el heap del resultado; antes quedaba retenida para siempre: la fuga de
    // ~1 KB/request del webserver). Un segundo join sobre el mismo handle → error claro.
    let src = r#"
fn main() -> int {
    let t = spawn(fn() -> int { 42 });
    print(join(t));
    print(join(t));
    0
}
"#;
    let (out, err, code) = run("doble_join", src, true);
    assert_eq!(out, "42\n", "el primer join funciona; el segundo no imprime");
    assert!(err.contains("task already consumed"), "stderr debe explicar el doble join: {err}");
    assert_eq!(code, 70);
}

#[test]
fn spawn_join_in_loop_reuses_slots_without_corrupting() {
    // M98.1: el free-list reusa slots liberados. 10k ciclos spawn+join con valores distintos:
    // si el reuso confundiera generaciones (ABA) o mezclara heaps, la suma no cuadraría.
    let src = r#"
fn main() -> int {
    var acc = 0;
    var i = 0;
    while (i < 10000) {
        let t = spawn(fn() -> int { i * 2 });
        acc = acc + join(t);
        i = i + 1;
    }
    print(acc);
    0
}
"#;
    let (out, err, code) = run("churn_reusa_slots", src, true);
    assert_eq!(code, 0, "stderr: {stderr}", stderr = err);
    assert_eq!(out, "99990000\n"); // 2 * (0 + 1 + … + 9999)
}

#[test]
fn freed_channel_behaves_as_closed_and_empty() {
    // M98.3: un canal cerrado y drenado se LIBERA (su slot se reusa; antes quedaba retenido para
    // siempre, ~450 B/canal). La liberación es INVISIBLE: el handle stale responde exactamente como
    // un canal cerrado y vacío — recv → None (una y otra vez), close → no-op (idempotente como
    // siempre), select → listo (el gotcha documentado del canal cerrado), send → el error de cerrado.
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = Channel.new();
    send(ch, 7);
    close(ch);
    match (recv(ch)) {
        Option.Some(v) => print("drenado: " + to_string(v)),
        Option.None => print("vacio"),
    }
    // A partir de aquí el canal está liberado: todo debe responder como cerrado+vacío.
    match (recv(ch)) {
        Option.Some(v) => print("? " + to_string(v)),
        Option.None => print("recv tras drenar: None"),
    }
    match (recv(ch)) {
        Option.Some(v) => print("? " + to_string(v)),
        Option.None => print("recv de nuevo: None"),
    }
    close(ch); // doble close: no-op, como siempre
    print("doble close ok");
    let listo = select([ch]); // cerrado → listo para siempre (gotcha documentado)
    print("select: " + to_string(listo));
    send(ch, 8); // send sobre cerrado → error de ejecución
    0
}
"#;
    let (out, err, code) = run("canal_liberado", src, true);
    assert_eq!(
        out,
        "drenado: 7\nrecv tras drenar: None\nrecv de nuevo: None\ndoble close ok\nselect: 0\n"
    );
    assert!(err.contains("send on a closed channel"), "stderr: {err}");
    assert_eq!(code, 70);
}

#[test]
fn children_do_not_survive_the_scope() {
    // M98.1: el scope CONSUME a sus hijas al cerrar (es su dueño). Un handle que escapa del scope
    // y se une después → el mismo error del doble join (la tarea ya fue consumida).
    let src = r#"
fn main() -> int {
    var fuera: Task<int> = spawn(fn() -> int { 0 });
    join(fuera);
    scope(fn() {
        fuera = spawn(fn() -> int { 7 });
    });
    print(join(fuera));
    0
}
"#;
    let (out, err, code) = run("hija_no_sobrevive", src, true);
    assert_eq!(out, "", "el join tras el scope no debe producir valor");
    assert!(err.contains("task already consumed"), "stderr: {err}");
    assert_eq!(code, 70);
}

#[test]
fn sleep_cede_la_fiber() {
    // M57.2: `time.sleep` es cooperativo en la VM — aparca la fibra con deadline (sin fd) y las
    // demás siguen corriendo. Antes bloqueaba el worker entero (en M:1, TODAS las fibras): la
    // hija no habría corrido hasta el join. Márgenes anchos (20 vs 200 ms) → orden estable.
    let src = r#"
import std/time;

fn main() -> int {
    let t = spawn(fn() {
        print("hija antes");
        time.sleep(20);
        print("hija después");
    });
    print("main 1");
    time.sleep(200);
    print("main 2");
    join(t);
    0
}
"#;
    let (stdout, stderr, code) = run("sleep_cede", src, true);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "main 1\nhija antes\nhija después\nmain 2\n");
}

#[test]
fn spawned_captured_local_cell_lives_in_child_heap() {
    // Regresión: las celdas de los locales CAPTURADOS de la fibra hija se alojaban en el heap del
    // SPAWNER (heap-por-fibra, M38.1b-2). Si la hija llegaba a un punto seguro del GC antes del
    // `InitLocal` que estrena la celda propia, ese handle ajeno se marcaba contra la tabla de slots
    // de la hija: objeto equivocado si el índice cabía, `index out of bounds` en `Heap::mark` si no
    // (el caso observado: heap del spawner con cientos de objetos vivos, heap joven con 64 slots).
    // El programa fuerza justo eso: main deja 400 objetos vivos y la hija asigna —y recolecta—
    // antes de declarar su `var` capturado.
    let src = r#"
fn main() -> int {
    var seed: [[int]] = [];
    var i = 0;
    while (i < 400) {
        seed.push([i, i + 1]);
        i = i + 1;
    }
    let t: Task<int> = spawn(fn() -> int {
        var junk: [[int]] = [];
        var j = 0;
        while (j < 300) {
            junk.push([j]);
            j = j + 1;
        }
        var acc = 0;
        let bump = fn(x: int) -> int { acc = acc + x; acc };
        bump(junk.len())
    });
    print(join(t));
    0
}
"#;
    let (out, err, code) = run("conc_spawn_captured_cell", src, true);
    assert_eq!(code, 0, "stderr: {err}");
    assert_eq!(out, "300\n");
    assert!(!err.contains("panicked"), "stderr: {err}");
}

#[test]
fn try_recv_non_blocking_three_states() {
    // M116: try_recv distingue Got/Empty/Closed sin bloquear. Empty (abierto y vacío), Got tras un
    // send, Empty de nuevo, Closed tras cerrar, y Got de un emisor bloqueado en rendezvous.
    let src = r#"
import std/time;
fn describe(r: Received<int>) -> string {
    match (r) {
        Received.Got(v) => "got " + to_string(v),
        Received.Empty => "empty",
        Received.Closed => "closed",
    }
}
fn main() -> int {
    let ch: Channel<int> = Channel.bounded(4);
    print(describe(try_recv(ch)));
    send(ch, 42);
    print(describe(try_recv(ch)));
    print(describe(try_recv(ch)));
    close(ch);
    print(describe(try_recv(ch)));
    let r: Channel<int> = Channel.new();
    spawn(fn() { send(r, 7); });
    time.sleep(50);
    print(describe(try_recv(r)));
    0
}
"#;
    let (out, _err, code) = run("conc_try_recv", src, true);
    assert_eq!(out, "empty\ngot 42\nempty\nclosed\ngot 7\n", "try_recv (vm): {out}");
    assert_eq!(code, 0);
}

#[test]
fn try_recv_requires_the_vm() {
    // Como recv/select, try_recv es concurrencia → el intérprete da un error claro, no un panic.
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = Channel.new();
    match (try_recv(ch)) { Received.Got(v) => print(v), Received.Empty => print(0), Received.Closed => print(0) }
    0
}
"#;
    let (_out, err, code) = run("conc_try_recv_interp", src, false);
    assert!(err.contains("requires the VM"), "stderr no pide la VM: {err}");
    assert_eq!(code, 70);
}

#[test]
fn select_timeout_ready_wake_and_deadline() {
    // M116.1: select_timeout — plazo vencido (None), valor ya listo (Some inmediato), despertar por
    // canal ANTES del plazo (event-driven), y poll no bloqueante (ms=0). Determinista con --deterministic.
    let src = r#"
import std/time;
fn main() -> int {
    let a: Channel<int> = Channel.new();
    let b: Channel<int> = Channel.new();
    match (select_timeout([a, b], 100)) {
        Option.Some(i) => print("ready " + to_string(i)),
        Option.None => print("timeout"),
    };
    send(b, 99);
    match (select_timeout([a, b], 100)) {
        Option.Some(i) => print("ready " + to_string(i)),
        Option.None => print("timeout"),
    };
    let _ = recv(b);
    spawn(fn() { time.sleep(50); send(a, 7); });
    match (select_timeout([a, b], 2000)) {
        Option.Some(i) => print("ready " + to_string(i) + " val " + to_string(recv(a).unwrap_or(0 - 1))),
        Option.None => print("timeout"),
    };
    match (select_timeout([a, b], 0)) {
        Option.Some(i) => print("ready " + to_string(i)),
        Option.None => print("poll empty"),
    };
    0
}
"#;
    let (out, _err, code) = run("conc_select_timeout", src, true);
    assert_eq!(out, "timeout\nready 1\nready 0 val 7\npoll empty\n", "select_timeout (vm): {out}");
    assert_eq!(code, 0);
}

#[test]
fn select_timeout_requires_the_vm() {
    let src = r#"
fn main() -> int {
    let a: Channel<int> = Channel.new();
    match (select_timeout([a], 10)) { Option.Some(i) => print(i), Option.None => print(0 - 1) }
    0
}
"#;
    let (_out, err, code) = run("conc_select_timeout_interp", src, false);
    assert!(err.contains("requires the VM"), "stderr no pide la VM: {err}");
    assert_eq!(code, 70);
}
