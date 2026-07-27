//! Carga `plaintext` — Rust con hyper sobre tokio multi-hilo.
//!
//! Esta implementación NO es un rival: es el TECHO. Dice cuánto da la máquina cuando el
//! servidor es prácticamente el syscall, y por tanto cuánto hardware está dejando en la mesa
//! cada una de las demás. Un resultado de raylang solo se interpreta como fracción de este.

use std::convert::Infallible;
use std::net::SocketAddr;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

async fn handle(_req: Request<hyper::body::Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    Ok(Response::builder()
        .header("Content-Type", "text/plain")
        .body(Full::new(Bytes::from_static(b"Hello, World!")))
        .unwrap())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut argv = std::env::args().skip(1);
    let (host, port) = match (argv.next(), argv.next().and_then(|p| p.parse::<u16>().ok())) {
        (Some(h), Some(p)) => (h, p),
        _ => {
            eprintln!("uso: plaintext-hyper <host> <puerto>");
            std::process::exit(2);
        }
    };

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = TcpListener::bind(addr).await?;

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        tokio::task::spawn(async move {
            // `keep_alive(true)` es el default de http1::Builder; se deja explícito porque el
            // banco mide SIEMPRE con keep-alive (es lo que hace un cliente HTTP real y lo que
            // hacen las otras tres implementaciones).
            let _ = http1::Builder::new()
                .keep_alive(true)
                .serve_connection(io, service_fn(handle))
                .await;
        });
    }
}
