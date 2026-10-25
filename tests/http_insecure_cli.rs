//! rayreq #250: `http.Options.insecure_skip_verify` acepta un certificado que no verifica, solo en
//! la forma explícita `request_opts`; la petición normal falla con una pista que sugiere primero
//! confiar en la CA con `SSL_CERT_FILE`, y con ella la verificación pasa. Servidor TLS mínimo en
//! el puerto 0 con el certificado de `tests/fixtures` (firmado por su CA de prueba). VM y nativo
//! (en nativo, un peer que cierra sin `close_notify` es EOF, como en la VM).

use std::path::PathBuf;
use std::process::Command;

const PROG: &str = r##"import std/fs;
import std/net;
import net/http;

fn tls_server(cert: string, key: string) -> int {
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(l);
    let _ = spawn(fn() -> int {
        while (true) {
            let c = net.tcp_accept(l).unwrap();
            match (net.tls_accept(c, cert, key)) {
                Result.Ok(t) => {
                    let _ = net.socket_read_bytes(t);
                    let _ = net.socket_write(t, "HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\nsecure hi");
                    close(t);
                },
                Result.Err(_) => { close(c); },
            }
        }
        0
    });
    port
}

fn main() {
    let port = tls_server(fs.read_file("cert.pem").unwrap(), fs.read_file("key.pem").unwrap());
    let url = "https://localhost:${port}/";
    match (http.request_pairs("GET", url, b"", [], 5000)) {
        Result.Ok(r) => print("verified ${r.status}"),
        Result.Err(e) => print("verified err: " + e),
    }
    var o = http.options();
    o.insecure_skip_verify = true;
    match (http.request_opts("GET", url, b"", [], o)) {
        Result.Ok(r) => print("insecure ${r.status} " + http.body_text(r).unwrap_or("?")),
        Result.Err(e) => print("insecure err: " + e),
    }
}
"##;

fn project() -> PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let app = std::env::temp_dir().join(format!("ray_tls_insecure_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app);
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!("[package]\nname = \"t\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{}\"\n", root.join("packages/net").display().to_string().replace('\\', "/")),
    )
    .unwrap();
    std::fs::write(app.join("src/main.ray"), PROG).unwrap();
    std::fs::copy(root.join("tests/fixtures/tls_cert.pem"), app.join("cert.pem")).unwrap();
    std::fs::copy(root.join("tests/fixtures/tls_key.pem"), app.join("key.pem")).unwrap();
    app
}

fn check(out: &str, trusted: bool) {
    let mut lines = out.lines();
    let first = lines.next().unwrap_or("");
    if trusted {
        assert_eq!(first, "verified 200", "{out}");
    } else {
        assert!(first.starts_with("verified err: ") && first.contains("certificate"), "{out}");
        assert!(first.contains("SSL_CERT_FILE=ca.pem"), "la pista sugiere confiar en la CA\n{out}");
    }
    assert_eq!(lines.next(), Some("insecure 200 secure hi"), "{out}");
}

#[test]
fn insecure_skip_verify_is_explicit_and_the_error_suggests_the_ca_first() {
    let app = project();
    let ca = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tls_ca.pem");
    for (trusted, env) in [(false, None), (true, Some(ca.clone()))] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ray"));
        cmd.arg("run").current_dir(&app).env_remove("SSL_CERT_FILE");
        if let Some(p) = &env {
            cmd.env("SSL_CERT_FILE", p);
        }
        let out = cmd.output().unwrap();
        check(&String::from_utf8_lossy(&out.stdout), trusted);
    }
    // Nativo, sin confiar en la CA.
    let bin = app.join("nat");
    let b = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", "--native", "src/main.ray", "-o"]).arg(&bin).current_dir(&app).output().unwrap();
    assert_eq!(b.status.code(), Some(0), "{}", String::from_utf8_lossy(&b.stderr));
    let out = Command::new(&bin).current_dir(&app).env_remove("SSL_CERT_FILE").output().unwrap();
    check(&String::from_utf8_lossy(&out.stdout), false);
}
