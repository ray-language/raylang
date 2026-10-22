//! `net.set_write_timeout` (raysec #234): una escritura a un peer que no lee vence con el error
//! estable "write timeout" en los tres motores, y el plazo es de la escritura ENTERA (gotear no lo
//! renueva). El peer es un servidor de juguete en el propio Rust que acepta y NUNCA lee: el búfer de
//! recepción del kernel se llena y el emisor se queda esperando.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ray_wr_timeout_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Acepta UNA conexión y la retiene sin leer hasta que el cliente la cierre (o 30 s).
fn sink_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((s, _)) = listener.accept() {
            // Retener el socket sin leer: el emisor llena la ventana y se bloquea.
            std::thread::sleep(std::time::Duration::from_secs(30));
            drop(s);
        }
    });
    port
}

const PROG: &str = r#"import std/net;
import std/time;
fn main() -> int {
    let c = net.tcp_connect("127.0.0.1", PORT).unwrap();
    net.set_write_timeout(c, 400);
    let big = bytes_of([7]).repeat_bytes(33554432);
    let t0 = time.monotonic();
    let r = net.socket_write_bytes(c, big);
    let ms = time.monotonic() - t0;
    match (r) {
        Result.Ok(n) => print("ok ${n}"),
        Result.Err(e) => print("err " + e),
    }
    print(if (ms >= 350 && ms < 5000) { "timely" } else { "late ${ms}" });
    close(c);
    0
}
"#;

fn prog_with(port: u16) -> String {
    // `repeat_bytes` no existe: construye los 32 MiB doblando un `bytes`.
    PROG.replace("PORT", &port.to_string()).replace(
        "let big = bytes_of([7]).repeat_bytes(33554432);",
        "var big = bytes_of([7, 7, 7, 7, 7, 7, 7, 7]);\n    var i = 0;\n    while (i < 22) { big = big + big; i = i + 1; }",
    )
}

fn run(args: &[&str], dir: &PathBuf) -> (String, String, i32) {
    let out = Command::new(BIN).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn a_write_to_a_peer_that_never_reads_times_out_in_the_vm_and_the_interpreter() {
    for flags in [&[][..], &["--interp"][..]] {
        let d = tmp(if flags.is_empty() { "vm" } else { "interp" });
        let port = sink_server();
        std::fs::write(d.join("prog.ray"), prog_with(port)).unwrap();
        let mut args = vec!["run"];
        args.extend_from_slice(flags);
        args.push("prog.ray");
        let t0 = Instant::now();
        let (out, err, code) = run(&args, &d);
        assert_eq!(code, 0, "{flags:?}: {out}\n{err}");
        assert_eq!(out, "err write timeout\ntimely\n", "{flags:?}");
        assert!(t0.elapsed().as_secs() < 20, "{flags:?}: el proceso tardó {:?}", t0.elapsed());
    }
}

#[test]
fn a_write_to_a_peer_that_never_reads_times_out_in_a_native_binary() {
    if !has_rustc() {
        return;
    }
    let d = tmp("native");
    let port = sink_server();
    std::fs::write(d.join("prog.ray"), prog_with(port)).unwrap();
    let bin = d.join("prog_native");
    let (out, err, code) = run(&["build", "--native", "prog.ray", "-o", bin.to_str().unwrap()], &d);
    assert_eq!(code, 0, "build: {out}\n{err}");
    let o = Command::new(&bin).current_dir(&d).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout), "err write timeout\ntimely\n", "nativo: {}", String::from_utf8_lossy(&o.stderr));
}

/// Sin plazo, la misma escritura a un peer que SÍ lee termina bien (el plazo no cambia el camino
/// normal), y quitarlo con `ms <= 0` deja de acotar.
#[test]
fn a_write_to_a_reading_peer_completes_and_a_cleared_timeout_does_not_apply() {
    let d = tmp("reader");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::Read;
        if let Ok((mut s, _)) = listener.accept() {
            let mut sink = Vec::new();
            let _ = s.read_to_end(&mut sink);
        }
    });
    let src = format!(
        "import std/net;\nfn main() -> int {{\n    let c = net.tcp_connect(\"127.0.0.1\", {port}).unwrap();\n    net.set_write_timeout(c, 5);\n    net.set_write_timeout(c, 0);\n    var big = bytes_of([1, 2, 3, 4, 5, 6, 7, 8]);\n    var i = 0;\n    while (i < 20) {{ big = big + big; i = i + 1; }}\n    match (net.socket_write_bytes(c, big)) {{\n        Result.Ok(_) => print(\"ok\"),\n        Result.Err(e) => print(\"err \" + e),\n    }}\n    close(c);\n    0\n}}\n"
    );
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let (out, err, code) = run(&["run", "prog.ray"], &d);
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(out, "ok\n");
}
