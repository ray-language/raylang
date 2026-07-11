//! M88.1b — apagado ordenado del webserver (`serve_shutdown`/`serve_graceful`). Un proyecto
//! temporal con `ray.toml` (dep de ruta al paquete `net`, como `trace_cli`); el servidor imprime
//! su puerto efímero ("escuchando en el puerto N") y el harness interactúa por TCP crudo. La
//! señal real (SIGTERM) sigue el molde de `signals_cli`. Unix + VM (concurrencia).

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

/// Crea el proyecto temporal (ray.toml → packages/net) y lanza `ray run`; devuelve el hijo y el
/// puerto que anunció por stdout.
fn lanzar(nombre: &str, main: &str) -> (Child, u16) {
    let base = std::env::temp_dir().join(format!("ray_shutdown_{nombre}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).expect("crea el dir temporal");
    let repo = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"shut\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{repo}/packages/net\"\n"),
    )
    .unwrap();
    std::fs::write(base.join("src/main.ray"), main).unwrap();
    let mut child = Command::new(BIN)
        .arg("run")
        .current_dir(&base)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanza ray run");
    let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
    let mut linea = String::new();
    reader.read_line(&mut linea).expect("lee el puerto");
    let port: u16 = linea.trim().rsplit(' ').next().and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("puerto inválido: {linea:?}"));
    child.stdout = Some(reader.into_inner());
    (child, port)
}

/// GET crudo con `Connection: close`; devuelve la respuesta completa (hasta EOF).
fn pedir(port: u16, path: &str) -> std::io::Result<String> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(10))).ok();
    write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")?;
    let mut resp = String::new();
    s.read_to_string(&mut resp)?;
    Ok(resp)
}

/// Espera al hijo y devuelve (stdout restante, stderr, código).
fn esperar(mut child: Child) -> (String, String, i32) {
    let out = child.wait_with_output().expect("espera al hijo");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// `serve_shutdown`: sirve con normalidad; un valor en el canal `stop` (aquí lo envía el propio
/// handler de `/stop`) apaga: la petición en curso SE RESPONDE, el servidor deja de aceptar,
/// `serve_shutdown` devuelve y main sigue (imprime "servidor apagado" y sale 0).
#[test]
fn stop_por_canal_apaga_y_devuelve() {
    let (child, port) = lanzar(
        "canal",
        r#"import net/webserver;

fn main() -> int {
    let stop: Channel<int> = Channel.new();
    let r = webserver.serve_shutdown("127.0.0.1", 0, stop, 2000, fn(req: webserver.Request) -> webserver.Response {
        if (req.path == "/stop") {
            send(stop, 1);
            webserver.ok("bye")
        } else {
            webserver.ok("hola")
        }
    });
    print("servidor apagado");
    0
}
"#,
    );
    let r1 = pedir(port, "/").expect("primera petición");
    assert!(r1.contains("hola"), "sirve con normalidad: {r1}");
    let r2 = pedir(port, "/stop").expect("petición de stop");
    assert!(r2.contains("bye"), "la petición que apaga se responde: {r2}");
    let (out, err, code) = esperar(child);
    assert_eq!(code, 0, "sale limpio\n{out}\n{err}");
    assert!(out.contains("apagando"), "anuncia el apagado: {out}");
    assert!(out.contains("servidor apagado"), "serve_shutdown devolvió a main: {out}");
    // Ya apagado: un cliente nuevo no conecta (el listener está cerrado).
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err(), "el puerto quedó cerrado");
}

/// `serve_graceful` + SIGTERM: la petición EN VUELO (handler lento) se drena —el cliente recibe
/// su respuesta completa aunque la señal llegue a mitad— y el proceso sale 0.
#[test]
fn sigterm_drena_la_peticion_en_vuelo() {
    let (child, port) = lanzar(
        "sigterm",
        r#"import net/webserver;
import std/time;

fn main() -> int {
    let r = webserver.serve_graceful("127.0.0.1", 0, 5000, fn(req: webserver.Request) -> webserver.Response {
        time.sleep(500);
        webserver.ok("lento ok")
    });
    print("servidor apagado");
    0
}
"#,
    );
    // La petición lenta, en un hilo; la señal la mandamos con el handler a mitad del sleep.
    let cliente = std::thread::spawn(move || pedir(port, "/lento"));
    std::thread::sleep(Duration::from_millis(150));
    let st = Command::new("kill").arg("-TERM").arg(child.id().to_string()).status().unwrap();
    assert!(st.success(), "kill -TERM");
    let resp = cliente.join().expect("hilo cliente").expect("respuesta");
    assert!(resp.contains("lento ok"), "la petición en vuelo se drenó: {resp}");
    let (out, _err, code) = esperar(child);
    assert_eq!(code, 0, "sale limpio tras drenar\n{out}");
    assert!(out.contains("apagando: 1 conexión(es) en vuelo"), "drenó exactamente la en vuelo: {out}");
}

/// El plazo del drenaje: un handler que tarda MÁS que `drain_ms` no retiene el apagado — el
/// servidor se rinde (lo anuncia por stderr) y sale 0 igualmente.
#[test]
fn el_plazo_del_drenaje_no_espera_para_siempre() {
    let (child, port) = lanzar(
        "plazo",
        r#"import net/webserver;
import std/time;

fn main() -> int {
    let stop: Channel<int> = Channel.new();
    let r = webserver.serve_shutdown("127.0.0.1", 0, stop, 200, fn(req: webserver.Request) -> webserver.Response {
        if (req.path == "/stop") {
            send(stop, 1);
            webserver.ok("bye")
        } else {
            time.sleep(3000);
            webserver.ok("tarde")
        }
    });
    print("servidor apagado");
    0
}
"#,
    );
    // La petición eterna en un hilo (no esperamos su respuesta: el proceso saldrá antes).
    let _lenta = std::thread::spawn(move || pedir(port, "/eterna"));
    std::thread::sleep(Duration::from_millis(100));
    let r2 = pedir(port, "/stop").expect("petición de stop");
    assert!(r2.contains("bye"), "el stop se responde: {r2}");
    let inicio = std::time::Instant::now();
    let (out, err, code) = esperar(child);
    assert_eq!(code, 0, "sale limpio pese a la conexión eterna\n{out}\n{err}");
    assert!(inicio.elapsed() < Duration::from_secs(2), "no esperó los 3 s del handler");
    assert!(err.contains("al vencer el plazo"), "anuncia la rendición del plazo: {err}");
    assert!(out.contains("servidor apagado"), "{out}");
}
