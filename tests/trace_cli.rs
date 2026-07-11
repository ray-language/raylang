//! M88.3 — tracing distribuido (`net/trace` + `webserver.trace_of` + `http.request_traced` +
//! `log.with_trace`). Un proyecto temporal con `ray.toml` (dependencia de ruta al paquete `net`,
//! como en `cli_cli.rs`). Dos pruebas: el golden puro (parse/format/child/from_headers/log, ambos
//! motores; PRNG sembrado y aserciones RELACIONALES —los bools— para no fijar los ids) y el
//! end-to-end por sockets (solo VM): el `traceparent` VIAJA de `request_traced` a `trace_of`.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

/// Crea un proyecto temporal con `ray.toml` (dep de ruta a `packages/net`) y `src/main.ray`,
/// y lo corre con `ray run [--interp]`.
fn correr(nombre: &str, main: &str, interp: bool) -> (String, String, i32) {
    let base = std::env::temp_dir().join(format!("ray_trace_{nombre}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).expect("crea el dir temporal");
    let repo = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"traced\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{repo}/packages/net\"\n"),
    )
    .unwrap();
    std::fs::write(base.join("src/main.ray"), main).unwrap();
    let mut args = vec!["run"];
    if interp {
        args.push("--interp");
    }
    let out = Command::new(BIN).args(&args).current_dir(&base).output().expect("lanza ray");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

const GOLDEN: &str = r#"import net/trace;
import net/log;
import std/random;

// ¿`parse_traceparent(s)` rechaza? (true = inválida)
fn rechaza(s: string) -> bool {
    match (trace.parse_traceparent(s)) {
        Option.Some(t) => false,
        Option.None => true,
    }
}

fn main() -> int {
    random.seed(7);
    let valida = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    // Parse de una cabecera válida: campos exactos + round-trip.
    match (trace.parse_traceparent(valida)) {
        Option.Some(t) => {
            print(t.trace_id + " " + t.span_id + " " + t.flags);
            print(trace.traceparent(t) == valida);
        },
        Option.None => print("no parsea"),
    }
    // Inválidas: versión ff, hex en mayúscula, trace-id todo ceros, span corto, basura.
    print(rechaza("ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"));
    print(rechaza("00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01"));
    print(rechaza("00-00000000000000000000000000000000-00f067aa0ba902b7-01"));
    print(rechaza("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba9-01"));
    print(rechaza("nada"));

    // new_trace: forma correcta (largos, flags, re-parseable) sin fijar los ids del PRNG.
    let raiz = trace.new_trace();
    print(to_string(raiz.trace_id.len()) + " " + to_string(raiz.span_id.len()) + " " + raiz.flags);
    print(rechaza(trace.traceparent(raiz)) == false);

    // child: mismo trace_id, span nuevo.
    let hijo = trace.child(raiz);
    print(hijo.trace_id == raiz.trace_id);
    print(hijo.span_id != raiz.span_id);
    print(hijo.flags == raiz.flags);

    // from_headers: adopta la entrante; sin cabecera (o malformada) arranca una nueva.
    var h: Map<string, string> = Map.new();
    h.insert("traceparent", valida);
    let adoptada = trace.from_headers(h);
    print(adoptada.trace_id == "4bf92f3577b34da6a3ce929d0e0e4736");
    var h2: Map<string, string> = Map.new();
    let nueva = trace.from_headers(h2);
    print(nueva.trace_id != adoptada.trace_id);
    h2.insert("traceparent", "basura");
    print(trace.from_headers(h2).trace_id.len() == 32);

    // log.with_trace: la línea lleva trace_id (tras service, antes de msg); sin él, no.
    let lg = log.logger("svc");
    print(log.render(log.info(lg, "hola"), "TS"));
    let lgt = log.with_trace(lg, adoptada.trace_id);
    print(log.render(log.field_int(log.info(lgt, "hola"), "n", 3), "TS"));
    // with_level conserva el trace y filtra de verdad.
    let lgw = log.with_level(lgt, 2);
    print(log.render(log.error(lgw, "boom"), "TS"));
    0
}
"#;

const GOLDEN_ESPERADO: &str = "4bf92f3577b34da6a3ce929d0e0e4736 00f067aa0ba902b7 01\n\
true\n\
true\ntrue\ntrue\ntrue\ntrue\n\
32 16 01\n\
true\n\
true\ntrue\ntrue\n\
true\ntrue\ntrue\n\
{\"ts\":\"TS\",\"level\":\"INFO\",\"service\":\"svc\",\"msg\":\"hola\"}\n\
{\"ts\":\"TS\",\"level\":\"INFO\",\"service\":\"svc\",\"trace_id\":\"4bf92f3577b34da6a3ce929d0e0e4736\",\"msg\":\"hola\",\"n\":3}\n\
{\"ts\":\"TS\",\"level\":\"ERROR\",\"service\":\"svc\",\"trace_id\":\"4bf92f3577b34da6a3ce929d0e0e4736\",\"msg\":\"boom\"}\n";

#[test]
fn trace_golden_ambos_motores() {
    let (o_vm, e_vm, c_vm) = correr("golden_vm", GOLDEN, false);
    assert_eq!(c_vm, 0, "vm sale 0\n{e_vm}\n{o_vm}");
    assert_eq!(o_vm, GOLDEN_ESPERADO, "golden vm");
    let (o_in, e_in, c_in) = correr("golden_interp", GOLDEN, true);
    assert_eq!(c_in, 0, "intérprete sale 0\n{e_in}\n{o_in}");
    assert_eq!(o_in, GOLDEN_ESPERADO, "golden intérprete");
}

// El e2e: un servidor de UNA conexión (tcp_listen + read_request + trace_of) y un cliente
// `request_traced` en el mismo programa (fibras). Verifica que el `traceparent` VIAJA: el
// servidor ve el mismo trace_id que la raíz del cliente y un span_id hijo (fresco).
const E2E: &str = r#"import net/http;
import net/webserver;
import net/trace;
import std/net;
import std/random;

fn main() -> int {
    random.seed(11);
    let l = match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(h) => h,
        Result.Err(e) => { print("listen err " + e); return 1; },
    };
    let port = net.local_port(l);
    let srv = spawn(fn() -> string {
        match (net.tcp_accept(l)) {
            Result.Ok(conn) => {
                match (webserver.read_request(conn)) {
                    Result.Ok(req) => {
                        let t = webserver.trace_of(req);
                        let _ = webserver.send_response(conn, webserver.ok("hola"));
                        let _ = close(conn);
                        t.trace_id + "|" + t.span_id
                    },
                    Result.Err(e) => "req err " + e,
                }
            },
            Result.Err(e) => "accept err " + e,
        }
    });
    let raiz = trace.new_trace();
    let url = "http://127.0.0.1:" + to_string(port) + "/";
    match (http.request_traced("GET", url, "", Map.new(), raiz)) {
        Result.Ok(r) => print(r.status),
        Result.Err(e) => print("http err " + e),
    }
    let visto = join(srv);
    let partes = visto.split("|");
    if (partes.len() != 2) { print("servidor: " + visto); return 1; }
    print(partes[0] == raiz.trace_id);   // mismo trace de punta a punta
    print(partes[1] != raiz.span_id);    // pero un span HIJO (fresco por salto)
    print(partes[1].len() == 16);
    0
}
"#;

#[test]
fn traceparent_viaja_del_cliente_al_servidor() {
    let (out, err, code) = correr("e2e", E2E, false);
    assert_eq!(code, 0, "e2e sale 0\n{err}\n{out}");
    assert_eq!(out, "200\ntrue\ntrue\ntrue\n", "el trace viaja y el span es hijo\n{out}");
}
