//! IDEAS §117: un peer que cierra el TCP sin `close_notify` es un fin de flujo limpio solo TRAS el
//! handshake TLS (servidores HTTP con `Connection: close` lo hacen); si cierra DURANTE el handshake
//! es un error con nombre, en la VM y en el nativo — antes ambos lo daban como «EOF limpio» y una
//! conexión que murió a medio saludo pasaba por una respuesta vacía.

use std::process::Command;

const PROG: &str = r##"import std/net;

fn main() {
    // Un «servidor» que acepta, lee el ClientHello y cierra sin hablar TLS. Se LEE antes de
    // cerrar: en Linux, cerrar con datos sin leer manda RST (ECONNRESET) en vez de FIN.
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(l);
    let _ = spawn(fn() -> int {
        let c = net.tcp_accept(l).unwrap();
        let _ = net.socket_read_bytes(c);
        close(c);
        0
    });
    let c = net.tcp_connect("127.0.0.1", port).unwrap();
    let outcome = match (net.tls_upgrade(c, "localhost")) {
        Result.Ok(t) => match (net.socket_read_bytes(t)) {
            Result.Ok(b) => "read ok ${b.len()}",
            Result.Err(e) => "read err: " + e,
        },
        Result.Err(e) => "upgrade err: " + e,
    };
    print(outcome);
}
"##;

#[test]
fn a_peer_that_closes_during_the_handshake_is_an_error_not_a_clean_eof() {
    let dir = std::env::temp_dir().join(format!("ray_tls_hs_eof_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("prog.ray"), PROG).unwrap();
    let check = |out: &str, engine: &str| {
        assert!(
            out.contains("err: ") && out.contains("tls: the peer closed the connection during the handshake"),
            "{engine}: {out}"
        );
    };
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).current_dir(&dir).output().unwrap();
    check(&String::from_utf8_lossy(&out.stdout), "vm");
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = dir.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin).output().unwrap();
        check(&String::from_utf8_lossy(&out.stdout), "nativo");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
