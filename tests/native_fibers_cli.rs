//! F2 del arco de concurrencia nativa — paridad del modo `--fibers` (`ray build --native --fibers`).
//!
//! Con `--fibers`, la concurrencia del binario nativo corre sobre el scheduler M:N de
//! `ray_runtime::fibers` (corrutinas corosensei + reactor kqueue/epoll) en vez de hilo-de-SO por
//! tarea. Estos tests fijan el nivel 2 de paridad (salida byte a byte contra la VM) sobre los
//! ejes que el cambio toca: spawn/join, canales (rendezvous y acotados), scope/cancelación,
//! `try_call` (la profundidad viaja en el ctx de la fibra), sleep, y un servidor TCP que se habla
//! a sí mismo (accept/read/write/close aparcando fibras de verdad).
//!
//! Los programas son DETERMINISTAS por causalidad (todo orden observable pasa por join/canales),
//! así que la comparación es byte a byte aunque ambos motores sean multicore.

use std::path::PathBuf;
use std::process::Command;

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Compila `src` con `--native --fibers`, lo ejecuta, y exige stdout+exit idénticos a la VM.
fn assert_fibers_matches_vm(name: &str, src: &str) {
    if !has_rustc() {
        eprintln!("saltando {name}: rustc no disponible");
        return;
    }
    let dir = std::env::temp_dir().join(format!("ray_fibers_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("crea el dir del test");
    let src_path = dir.join(format!("{name}.ray"));
    std::fs::write(&src_path, src).expect("escribe el fuente");

    let vm = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(&src_path)
        .output()
        .expect("corre la VM");

    let bin: PathBuf = dir.join(format!("{name}_bin"));
    let built = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", src_path.to_str().unwrap(), "--native", "--fibers", "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza ray build --native --fibers");
    assert!(
        built.status.success(),
        "build --native --fibers falla para {name}: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let native = Command::new(&bin).output().expect("corre el binario de fibras");

    assert_eq!(
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&vm.stdout),
        "stdout de fibras ≡ VM en {name} (stderr nativo: {})",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(native.status.code(), vm.status.code(), "código de salida de fibras ≡ VM en {name}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn spawn_join_tree_is_byte_identical() {
    // Árbol de tareas con resultados agregados por join: el orden observable es 100% causal.
    assert_fibers_matches_vm(
        "spawn_tree",
        r#"
fn work(n: int) -> int {
    var acc: int = 0;
    var i: int = 0;
    while (i < n) { acc = acc + i * i; i = i + 1; }
    acc
}
fn main() -> int {
    var tasks: [Task<int>] = [];
    var i: int = 1;
    while (i <= 8) {
        let n = i * 100;
        tasks.push(spawn(fn() -> int { work(n) }));
        i = i + 1;
    }
    var total: int = 0;
    for t in tasks { total = total + join(t); }
    print(total);
    0
}
"#,
    );
}

#[test]
fn channels_bounded_and_rendezvous_are_byte_identical() {
    // Productor/consumidor por canal acotado + rendezvous (cap 0): la espera del emisor sobre el
    // canal lleno ejercita el __ray_cv_wait de fibras (ceder, no bloquear el worker).
    assert_fibers_matches_vm(
        "channels",
        r#"
fn main() -> int {
    let ch: Channel<int> = Channel.bounded(2);
    let done: Channel<int> = Channel.new();
    let producer = spawn(fn() {
        var i: int = 0;
        while (i < 50) { send(ch, i); i = i + 1; }
        close(ch);
    });
    let consumer = spawn(fn() -> int {
        var sum: int = 0;
        var go: bool = true;
        while (go) {
            match (recv(ch)) {
                Option.Some(v) => { sum = sum + v; },
                Option.None => { go = false; },
            }
        }
        sum
    });
    join(producer);
    print(join(consumer));
    // Rendezvous: el emisor no sigue hasta que SU valor se consume.
    let rz: Channel<string> = Channel.bounded(0);
    let t = spawn(fn() { send(rz, "handshake"); });
    match (recv(rz)) {
        Option.Some(s) => print(s),
        Option.None => print("closed"),
    }
    join(t);
    print("done");
    0
}
"#,
    );
}

#[test]
fn scope_failure_cancels_siblings_byte_identically() {
    // Un scope cuya hija falla: el fallo se propaga al salir y se recupera con try_call — junta
    // scopes (ctx de fibra) + cancelación + try_call en el mismo programa.
    assert_fibers_matches_vm(
        "scope_cancel",
        r#"
fn main() -> int {
    let r = try_call(fn() {
        scope(fn() {
            spawn(fn() { panic("child failed"); });
        });
    });
    match (r) {
        Result.Ok(_) => print("no failure"),
        Result.Err(e) => print("caught: " + e),
    }
    print("alive");
    0
}
"#,
    );
}

#[test]
fn try_call_depth_travels_with_the_fiber() {
    // try_call anidado DENTRO de tareas: la profundidad in_try vive en el ctx por-fibra. Si se
    // quedara en un thread-local, el hook de panic se silenciaría/quejaría en el hilo equivocado
    // (divergencia observable en stderr y, en el peor caso, doble contabilidad).
    assert_fibers_matches_vm(
        "try_in_fibers",
        r#"
fn main() -> int {
    var tasks: [Task<string>] = [];
    var i: int = 0;
    while (i < 6) {
        let n = i;
        tasks.push(spawn(fn() -> string {
            match (try_call(fn() -> int {
                if (n % 2 == 0) { panic("even " + to_string(n)); }
                n * 10
            })) {
                Result.Ok(v) => "ok " + to_string(v),
                Result.Err(e) => "err " + e,
            }
        }));
        i = i + 1;
    }
    for t in tasks { print(join(t)); }
    0
}
"#,
    );
}

#[test]
fn sleeping_fibers_and_causal_prints_are_byte_identical() {
    // sleep en fibra (timer del reactor, no thread::sleep) con orden fijado por joins.
    assert_fibers_matches_vm(
        "fiber_sleep",
        r#"
import std/time;

fn main() -> int {
    let a = spawn(fn() -> int { time.sleep(30); 1 });
    let b = spawn(fn() -> int { time.sleep(10); 2 });
    print(join(b));
    print(join(a));
    print("end");
    0
}
"#,
    );
}

#[test]
fn a_tcp_server_talking_to_itself_parks_fibers_end_to_end() {
    // El eje central de F2: accept/read/write/close de sockets NO-bloqueantes aparcando fibras.
    // Un servidor de eco atiende K clientes secuenciales dentro del mismo proceso; el orden es
    // causal (cada cliente espera su eco antes de imprimir).
    assert_fibers_matches_vm(
        "tcp_self_talk",
        r#"
import std/net;

fn serve_one(srv: int) {
    match (net.tcp_accept(srv)) {
        Result.Ok(c) => {
            match (net.socket_read(c)) {
                Result.Ok(data) => { net.socket_write(c, "echo:" + data); },
                Result.Err(_) => {},
            }
            close(c);
        },
        Result.Err(e) => print("accept error: " + e),
    }
}
fn main() -> int {
    match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Err(e) => { print("listen: " + e); 1 },
        Result.Ok(srv) => {
            let port = net.local_port(srv);
            let server = spawn(fn() {
                var k: int = 0;
                while (k < 5) { serve_one(srv); k = k + 1; }
            });
            var k: int = 0;
            while (k < 5) {
                match (net.tcp_connect("127.0.0.1", port)) {
                    Result.Ok(c) => {
                        net.socket_write(c, "msg" + to_string(k));
                        match (net.socket_read(c)) {
                            Result.Ok(reply) => print(reply),
                            Result.Err(e) => print("read: " + e),
                        }
                        close(c);
                    },
                    Result.Err(e) => print("connect: " + e),
                }
                k = k + 1;
            }
            join(server);
            close(srv);
            print("served all");
            0
        },
    }
}
"#,
    );
}

#[test]
fn a_read_timeout_fires_byte_identically_under_fibers() {
    // El timeout de lectura (M56.4) del lado fibras: en no-bloqueante vive en el park con
    // deadline, y el vencimiento debe dar el MISMO "read timeout" que la VM.
    assert_fibers_matches_vm(
        "read_timeout",
        r#"
import std/net;
import std/time;

fn main() -> int {
    match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Err(e) => { print("listen: " + e); 1 },
        Result.Ok(srv) => {
            let port = net.local_port(srv);
            let server = spawn(fn() {
                match (net.tcp_accept(srv)) {
                    Result.Ok(c) => {
                        // No escribe nada: el cliente debe vencer su plazo de lectura.
                        time.sleep(300);
                        close(c);
                    },
                    Result.Err(_) => {},
                }
            });
            match (net.tcp_connect("127.0.0.1", port)) {
                Result.Ok(c) => {
                    net.set_read_timeout(c, 50);
                    match (net.socket_read(c)) {
                        Result.Ok(_) => print("unexpected data"),
                        Result.Err(e) => print("err: " + e),
                    }
                    close(c);
                },
                Result.Err(e) => print("connect: " + e),
            }
            join(server);
            close(srv);
            0
        },
    }
}
"#,
    );
}
