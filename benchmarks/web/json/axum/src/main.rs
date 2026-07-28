//! Carga `json` — escalón de FRAMEWORK: Rust con axum (hyper + router).
//!
//! Es el TECHO de este escalón, igual que hyper lo es del pelado: dice cuánto cuesta enrutar y
//! responder cuando el framework es prácticamente el syscall más un match de rutas. Cada
//! resultado se lee como fracción de él.

use axum::{extract::Path, response::IntoResponse, routing::get, Router};
use std::net::SocketAddr;

async fn user(Path(id): Path<String>) -> impl IntoResponse {
    (
        [("content-type", "application/json")],
        format!(r#"{{"id":"{id}","name":"Ada"}}"#),
    )
}

async fn empty() -> impl IntoResponse {
    ([("content-type", "application/json")], "{}")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut argv = std::env::args().skip(1);
    let (host, port) = match (argv.next(), argv.next().and_then(|p| p.parse::<u16>().ok())) {
        (Some(h), Some(p)) => (h, p),
        _ => {
            eprintln!("uso: json-axum <host> <puerto>");
            std::process::exit(2);
        }
    };

    // Las mismas 10 rutas que las otras tres implementaciones: el coste de emparejar depende del
    // tamaño de la tabla, así que igualarlo es parte de que la comparación signifique algo.
    let mut app = Router::new().route("/users/{id}", get(user));
    for p in ["/", "/health", "/version", "/items", "/items/{id}",
              "/orders", "/orders/{id}", "/posts", "/posts/{id}"] {
        app = app.route(p, get(empty));
    }

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
