//! Conformidad de FRAMING HTTP/1.1 del paquete `net` (RFC 9112), dirigida por tabla.
//!
//! De dónde sale: el repaso de los hallazgos históricos (nov 2026) contó diez bugs del mismo corte
//! en `net` — cabeceras duplicadas, `Content-Length` contra `chunked`, tamaños de chunk, líneas
//! plegadas, pipelining (#81, #229, #230, #238, #239, #243, #244, #275, #277, #283) — y cada uno se
//! arregló con su test suelto. Aquí la clase entera va en una tabla: octetos crudos contra un
//! socket, y lo que la RFC manda hacer con ellos.
//!
//! Dos direcciones:
//! - **Servidor** (`net/webserver`): un servidor raylang en el puerto 0 con un handler que devuelve
//!   lo que VIO (método, ruta, cuerpo, una cabecera); el test le escribe peticiones crudas.
//! - **Cliente** (`net/http`): un servidor falso de Rust que contesta octetos crudos; un programa
//!   raylang hace la petición e imprime lo que entendió.
//!
//! Cada fila usa su propia conexión y todas corren en paralelo. Lo que se exige de una petición
//! mal formada es lo que evita el *request smuggling*: un 4xx/5xx, la conexión cerrada y que el
//! handler NO la vea.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_ray");

fn project(name: &str, main: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("ray_http_conformance_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).unwrap();
    let repo = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"probe\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{repo}/packages/net\"\n"),
    )
    .unwrap();
    std::fs::write(base.join("src/main.ray"), main).unwrap();
    base
}

// ---------------------------------------------------------------------------------------------
// Servidor
// ---------------------------------------------------------------------------------------------

/// El handler devuelve lo que vio: `<MÉTODO ruta-cruda q=query b=cuerpo x=cabecera-x-test>`.
/// `/reflect?v=…` copia `v` (ya descodificado) a una cabecera de la respuesta.
const ECHO_SERVER: &str = r#"import std/net;
import net/webserver;
from net/webserver import Request, Response;

fn handle(req: Request) -> Response {
    let body = webserver.request_text(req).unwrap_or("?");
    let x = get(req.headers, "x-test").unwrap_or("-");
    var r = webserver.text(200, "<${req.method} ${req.raw_path} q=${req.query} b=${body} x=${x}>");
    if (req.path == "/reflect") {
        r.headers.insert("x-reflected", get(webserver.query_params(req), "v").unwrap_or(""));
    }
    r
}

fn main() -> int {
    let listener = net.tcp_listen("127.0.0.1", 0).unwrap();
    print("port ${net.local_port(listener)}");
    let _ = webserver.serve_on(listener, handle);
    0
}
"#;

struct Server {
    child: Child,
    port: u16,
    dir: std::path::PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        // Es NUESTRO hijo (el `Child` que lanzó este test): se termina por su manejador.
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn launch_echo_server() -> Server {
    // Un directorio por servidor: los tests corren en paralelo y cada uno lanza el suyo.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = project(&format!("server{n}"), ECHO_SERVER);
    let mut child = Command::new(BIN).arg("run").current_dir(&dir).stdout(Stdio::piped()).stderr(std::fs::File::create(dir.join("stderr.txt")).unwrap()).spawn().expect("lanza el servidor");
    let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
    let mut line = String::new();
    reader.read_line(&mut line).expect("lee el puerto");
    let port: u16 = line.trim().rsplit(' ').next().and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("puerto inválido: {line:?}"));
    // El resto del stdout se DRENA hasta que el servidor termina: soltar el lector cerraría la
    // tubería y el servidor moriría (código 141) al escribir su siguiente línea de registro.
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = reader.read_to_end(&mut sink);
    });
    Server { child, port, dir }
}

/// Lo que volvió por una conexión: las respuestas completas (estado, cabeceras crudas, cuerpo) y
/// si el servidor cerró.
struct Outcome {
    responses: Vec<(u16, String, String)>,
    closed: bool,
    raw: String,
}

/// Escribe `raw` y lee hasta que el servidor cierra o calla `idle` (tope total: 8 s).
fn exchange(port: u16, raw: &[u8], idle: Duration) -> Outcome {
    // Sin servidor (murió) no hay nada que leer: la fila falla con su veredicto y el test enseña
    // el stderr del servidor, en vez de un pánico por fila que lo tapa.
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return Outcome { responses: Vec::new(), closed: true, raw: "(no se pudo conectar)".to_string() };
    };
    stream.set_read_timeout(Some(idle)).ok();
    // El servidor puede cerrar a mitad de un envío grande: no es un fallo del test.
    let _ = stream.write_all(raw);
    let mut got = Vec::new();
    let mut closed = false;
    let start = Instant::now();
    let mut buf = [0u8; 8192];
    while start.elapsed() < Duration::from_secs(8) {
        match stream.read(&mut buf) {
            Ok(0) => {
                closed = true;
                break;
            }
            Ok(n) => got.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => break,
            Err(_) => {
                closed = true; // reset: el servidor soltó la conexión
                break;
            }
        }
    }
    Outcome { responses: parse_responses(&got), closed, raw: String::from_utf8_lossy(&got).into_owned() }
}

/// Parte un flujo en respuestas HTTP/1.1 (Content-Length o chunked). Lo que no cuadra se ignora.
fn parse_responses(mut data: &[u8]) -> Vec<(u16, String, String)> {
    let mut out = Vec::new();
    while let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
        let head = String::from_utf8_lossy(&data[..end]).into_owned();
        let Some(status) = head.strip_prefix("HTTP/1.").and_then(|r| r.get(2..5)).and_then(|s| s.parse::<u16>().ok()) else { break };
        let rest = &data[end + 4..];
        let lower = head.to_ascii_lowercase();
        let header = |name: &str| lower.lines().skip(1).find_map(|l| l.strip_prefix(name).map(|v| v.trim().to_string()));
        let (body, used) = if header("transfer-encoding:").is_some_and(|v| v.contains("chunked")) {
            let mut body = Vec::new();
            let mut at = 0;
            loop {
                let Some(nl) = rest[at..].windows(2).position(|w| w == b"\r\n") else { break };
                let Ok(size) = usize::from_str_radix(String::from_utf8_lossy(&rest[at..at + nl]).trim(), 16) else { break };
                at += nl + 2;
                if size == 0 {
                    at = (at + 2).min(rest.len());
                    break;
                }
                if at + size + 2 > rest.len() {
                    at = rest.len();
                    break;
                }
                body.extend_from_slice(&rest[at..at + size]);
                at += size + 2;
            }
            (body, at)
        } else {
            let len = header("content-length:").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0).min(rest.len());
            (rest[..len].to_vec(), len)
        };
        if (100..200).contains(&status) {
            data = rest; // una respuesta interina no lleva cuerpo
            out.push((status, head, String::new()));
            continue;
        }
        out.push((status, head, String::from_utf8_lossy(&body).into_owned()));
        data = &rest[used..];
    }
    out
}

/// Lo que la RFC manda hacer con una fila.
enum Expect {
    /// Exactamente estas respuestas 200, cada una con su texto dentro del cuerpo.
    Ok(&'static [&'static str]),
    /// Un 4xx/5xx, la conexión cerrada y NINGÚN 200: el handler no vio nada.
    Reject,
    /// Un envío ENORME que el servidor corta a medias: un rechazo, o la conexión cerrada sin
    /// respuesta (si el servidor cierra mientras el cliente aún escribe, el sistema puede
    /// descartar el 400 que ya iba de camino). Nunca un 200.
    RejectOrDrop,
    /// La RFC deja elegir: rechazar, o servirla como UNA petición con este cuerpo.
    RejectOrOk(&'static str),
    /// Como `RejectOrOk`, o quedarse esperando sin contestar nada.
    RejectOrOkOrWait(&'static str),
        /// `Content-Length` junto a `chunked`: rechazar, o tratarla como chunked Y cerrar después.
    RejectOrChunkedThenClose(&'static str),
    /// Petición incompleta: el servidor espera; nada de 200.
    NoOk,
}

struct Case {
    name: &'static str,
    request: Vec<u8>,
    expect: Expect,
}

fn case(name: &'static str, request: &str, expect: Expect) -> Case {
    Case { name, request: request.as_bytes().to_vec(), expect }
}

fn is_rejection(o: &Outcome) -> bool {
    let no_ok = o.responses.iter().all(|r| r.0 != 200);
    let refused = o.responses.first().is_some_and(|r| (400..=505).contains(&r.0));
    no_ok && refused && o.closed
}

fn verdict(c: &Case, o: &Outcome) -> Result<(), String> {
    let seen = || {
        let list: Vec<String> = o.responses.iter().map(|r| format!("{} {:?}", r.0, r.2)).collect();
        format!("respuestas: [{}], cerrada: {}", list.join(", "), o.closed)
    };
    let one_ok = |text: &str| o.responses.len() == 1 && o.responses[0].0 == 200 && o.responses[0].2.contains(text);
    match &c.expect {
        Expect::Ok(texts) => {
            let finals: Vec<&(u16, String, String)> = o.responses.iter().filter(|r| r.0 >= 200).collect();
            if finals.len() == texts.len() && finals.iter().zip(texts.iter()).all(|(r, t)| r.0 == 200 && r.2.contains(t)) {
                Ok(())
            } else {
                Err(format!("se esperaban {} respuestas 200 con {:?}; {}", texts.len(), texts, seen()))
            }
        }
        Expect::Reject => {
            if is_rejection(o) {
                Ok(())
            } else {
                Err(format!("se esperaba un 4xx/5xx con cierre y ningún 200; {}", seen()))
            }
        }
        Expect::RejectOrDrop => {
            if is_rejection(o) || (o.responses.is_empty() && o.closed) {
                Ok(())
            } else {
                Err(format!("se esperaba un rechazo o un cierre sin respuesta; {}", seen()))
            }
        }
        Expect::RejectOrOk(text) => {
            if is_rejection(o) || one_ok(text) {
                Ok(())
            } else {
                Err(format!("se esperaba un rechazo o UN 200 con {text:?}; {}", seen()))
            }
        }
        Expect::RejectOrOkOrWait(text) => {
            if is_rejection(o) || one_ok(text) || o.responses.is_empty() {
                Ok(())
            } else {
                Err(format!("se esperaba un rechazo, UN 200 con {text:?} o silencio; {}", seen()))
            }
        }
        Expect::RejectOrChunkedThenClose(text) => {
            if is_rejection(o) || (one_ok(text) && o.closed) {
                Ok(())
            } else {
                Err(format!("se esperaba un rechazo, o UN 200 con {text:?} y la conexión cerrada; {}", seen()))
            }
        }
        Expect::NoOk => {
            if o.responses.iter().all(|r| r.0 != 200) {
                Ok(())
            } else {
                Err(format!("petición incompleta servida como buena; {}", seen()))
            }
        }
    }
}

fn server_cases() -> Vec<Case> {
    use Expect::*;
    let mut cases = vec![
        // ---- Lo válido: debe servirse tal cual ----
        case("get_simple", "GET /a?x=1 HTTP/1.1\r\nHost: h\r\n\r\n", Ok(&["<GET /a q=x=1 b= x=->"])),
        case("post_content_length", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\n\r\nhello", Ok(&["b=hello"])),
        case(
            "post_chunked_with_extension_and_trailer",
            "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n5;ext=1\r\nhello\r\n6\r\n world\r\n0\r\nX-Trailer: t\r\n\r\n",
            Ok(&["b=hello world"]),
        ),
        case("chunked_hex_uppercase_and_leading_zeros", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n00A\r\n0123456789\r\n0\r\n\r\n", Ok(&["b=0123456789"])),
        case("transfer_encoding_is_case_insensitive", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: Chunked\r\n\r\n2\r\nok\r\n0\r\n\r\n", Ok(&["b=ok"])),
        case("pipelined_gets_in_one_write", "GET /1 HTTP/1.1\r\nHost: h\r\n\r\nGET /2 HTTP/1.1\r\nHost: h\r\n\r\n", Ok(&["<GET /1 ", "<GET /2 "])),
        case(
            "pipelined_post_then_get",
            "POST /1 HTTP/1.1\r\nHost: h\r\nContent-Length: 3\r\n\r\nabcGET /2 HTTP/1.1\r\nHost: h\r\n\r\n",
            Ok(&["<POST /1 q= b=abc", "<GET /2 q= b= "]),
        ),
        case(
            "pipelined_chunked_then_get",
            "POST /1 HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\nGET /2 HTTP/1.1\r\nHost: h\r\n\r\n",
            Ok(&["<POST /1 q= b=abc", "<GET /2 q= b= "]),
        ),
        case(
            "crlf_after_a_body_before_the_next_request",
            "POST /1 HTTP/1.1\r\nHost: h\r\nContent-Length: 3\r\n\r\nabc\r\nGET /2 HTTP/1.1\r\nHost: h\r\n\r\n",
            Ok(&["<POST /1 q= b=abc", "<GET /2 q= b= "]),
        ),
        case("header_names_are_case_insensitive_and_values_trimmed", "GET / HTTP/1.1\r\nhOsT: h\r\nX-TEST:   padded value \t\r\n\r\n", Ok(&["x=padded value>"])),
        case("empty_header_value", "GET / HTTP/1.1\r\nHost: h\r\nX-Test:\r\n\r\n", Ok(&["x=>"])),
        case("absolute_form_target", "GET http://example.test/abs?k=v HTTP/1.1\r\nHost: example.test\r\n\r\n", Ok(&["q=k=v"])),
        case("http_1_0_without_host", "GET /old HTTP/1.0\r\n\r\n", Ok(&["<GET /old "])),
        case("content_length_zero", "POST /z HTTP/1.1\r\nHost: h\r\nContent-Length: 0\r\n\r\n", Ok(&["<POST /z q= b= "])),
        case("percent_encoded_target_is_kept_raw", "GET /a%20b%2Fc HTTP/1.1\r\nHost: h\r\n\r\n", Ok(&["<GET /a%20b%2Fc "])),
        // ---- Lo que la RFC deja elegir ----
        case("identical_content_lengths", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\nok", RejectOrOk("b=ok")),
        case("content_length_list_of_equal_values", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: 2, 2\r\n\r\nok", RejectOrOk("b=ok")),
        case("leading_empty_line", "\r\nGET /lead HTTP/1.1\r\nHost: h\r\n\r\n", RejectOrOk("<GET /lead ")),
        // Solo LF: la RFC deja reconocerlo; este servidor espera el CRLF (y vence el plazo).
        case("bare_lf_line_endings", "GET /lf HTTP/1.1\nHost: h\n\n", RejectOrOkOrWait("<GET /lf ")),
        case("fragment_in_target", "GET /f#frag HTTP/1.1\r\nHost: h\r\n\r\n", RejectOrOk("<GET /f")),
        case(
            "content_length_with_chunked",
            "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
            RejectOrChunkedThenClose("b=abc"),
        ),
        case(
            "chunked_with_content_length_smuggled_prefix",
            "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\nContent-Length: 3\r\n\r\n0\r\n\r\nGET /smuggled HTTP/1.1\r\nHost: h\r\n\r\n",
            RejectOrChunkedThenClose("<POST /p q= b= "),
        ),
        // ---- Content-Length mal formado ----
        case("two_different_content_lengths", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: 2\r\nContent-Length: 5\r\n\r\nokGET ", Reject),
        case("content_length_list_of_different_values", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: 2, 5\r\n\r\nok", Reject),
        case("content_length_negative", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: -1\r\n\r\n", Reject),
        case("content_length_with_plus_sign", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: +2\r\n\r\nok", Reject),
        case("content_length_hex", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: 0x2\r\n\r\nok", Reject),
        case("content_length_trailing_garbage", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: 2x\r\n\r\nok", Reject),
        case("content_length_empty", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length:\r\n\r\n", Reject),
        case("content_length_with_inner_space", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: 1 0\r\n\r\n0123456789", Reject),
        case("content_length_overflows_64_bits", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: 18446744073709551618\r\n\r\nok", Reject),
        case("content_length_decimal_point", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: 2.0\r\n\r\nok", Reject),
        // ---- Transfer-Encoding mal formado ----
        case("transfer_encoding_unknown", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: xchunked\r\n\r\n2\r\nok\r\n0\r\n\r\n", Reject),
        case("transfer_encoding_chunked_not_last", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked, gzip\r\n\r\n2\r\nok\r\n0\r\n\r\n", Reject),
        case("transfer_encoding_chunked_twice", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked, chunked\r\n\r\n2\r\nok\r\n0\r\n\r\n", Reject),
        case(
            "transfer_encoding_split_in_two_headers",
            "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nok\r\n0\r\n\r\n",
            Reject,
        ),
        case("transfer_encoding_in_http_1_0", "POST /p HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nok\r\n0\r\n\r\n", RejectOrChunkedThenClose("b=ok")),
        case("transfer_encoding_with_a_vertical_tab", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: \x0bchunked\r\n\r\n2\r\nok\r\n0\r\n\r\n", Reject),
        case("transfer_encoding_with_a_trailing_cr", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\r\n\r\n2\r\nok\r\n0\r\n\r\n", Reject),
        case("content_length_with_a_form_feed", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: \x0c2\r\n\r\nok", Reject),
        // ---- Chunks mal formados ----
        case("chunk_size_17_hex_digits", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n10000000000000005\r\nhello\r\n0\r\n\r\n", Reject),
        case("chunk_size_not_hex", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nhello\r\n0\r\n\r\n", Reject),
        case("chunk_size_negative", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n-5\r\nhello\r\n0\r\n\r\n", Reject),
        case("chunk_size_with_0x_prefix", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n0x5\r\nhello\r\n0\r\n\r\n", Reject),
        case("chunk_size_empty", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n\r\nhello\r\n0\r\n\r\n", Reject),
        case("chunk_data_longer_than_its_size", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nhello\r\n0\r\n\r\n", Reject),
        case("chunk_data_without_crlf", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello0\r\n\r\n", Reject),
        // ---- Cabeceras mal formadas ----
        case("space_before_colon", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length : 2\r\n\r\nok", Reject),
        case("space_before_colon_on_transfer_encoding", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding : chunked\r\n\r\n2\r\nok\r\n0\r\n\r\n", Reject),
        case("tab_before_colon", "GET / HTTP/1.1\r\nHost: h\r\nX-Test\t: v\r\n\r\n", Reject),
        case("folded_header_line", "GET / HTTP/1.1\r\nHost: h\r\nX-Test: a\r\n b\r\n\r\n", Reject),
        case("folded_header_hiding_a_content_length", "POST /p HTTP/1.1\r\nHost: h\r\nX-Test: a\r\n Content-Length: 2\r\n\r\nok", Reject),
        case("header_line_without_colon", "GET / HTTP/1.1\r\nHost: h\r\nNoColonHere\r\n\r\n", Reject),
        case("empty_header_name", "GET / HTTP/1.1\r\nHost: h\r\n: v\r\n\r\n", Reject),
        case("space_inside_header_name", "GET / HTTP/1.1\r\nHost: h\r\nX Test: v\r\n\r\n", Reject),
        case("header_name_with_a_control_character", "GET / HTTP/1.1\r\nHost: h\r\nX-\x01Test: v\r\n\r\n", Reject),
        case("nul_in_header_value", "GET / HTTP/1.1\r\nHost: h\r\nX-Test: a\0b\r\n\r\n", Reject),
        case("bare_cr_in_header_value", "GET / HTTP/1.1\r\nHost: h\r\nX-Test: a\rb\r\n\r\n", Reject),
        case("missing_host_in_http_1_1", "GET /nohost HTTP/1.1\r\n\r\n", Reject),
        case("two_host_headers", "GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n", Reject),
        // ---- Línea de petición mal formada ----
        case("request_line_without_version", "GET /\r\nHost: h\r\n\r\n", Reject),
        case("request_line_with_two_spaces", "GET  / HTTP/1.1\r\nHost: h\r\n\r\n", Reject),
        case("request_line_with_tabs", "GET\t/\tHTTP/1.1\r\nHost: h\r\n\r\n", Reject),
        case("request_line_with_trailing_token", "GET / HTTP/1.1 extra\r\nHost: h\r\n\r\n", Reject),
        case("version_with_trailing_garbage", "GET / HTTP/1.1x\r\nHost: h\r\n\r\n", Reject),
        case("version_lowercase", "GET / http/1.1\r\nHost: h\r\n\r\n", Reject),
        case("version_2_0_on_the_wire", "GET / HTTP/2.0\r\nHost: h\r\n\r\n", Reject),
        case("method_with_invalid_character", "G(ET / HTTP/1.1\r\nHost: h\r\n\r\n", Reject),
        case("target_with_a_raw_space", "GET /a b HTTP/1.1\r\nHost: h\r\n\r\n", Reject),
        case("target_with_a_control_character", "GET /a\x01b HTTP/1.1\r\nHost: h\r\n\r\n", Reject),
        case("empty_request_line_only", "\r\n\r\n\r\nGARBAGE\r\n\r\n", Reject),
        // ---- Incompletas: el servidor espera, no inventa ----
        case("body_shorter_than_content_length", "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: 10\r\n\r\nshort", NoOk),
        case("chunked_without_last_chunk", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n", NoOk),
        case("last_chunk_without_final_crlf", "POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n", NoOk),
        case("headers_never_finished", "GET / HTTP/1.1\r\nHost: h\r\nX-Test: v\r\n", NoOk),
    ];
    // Una extensión de chunk enorme (#239): no se acumula sin tope.
    let mut huge_ext = b"POST /p HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n5;".to_vec();
    huge_ext.extend(std::iter::repeat_n(b'a', 200_000));
    huge_ext.extend_from_slice(b"\r\nhello\r\n0\r\n\r\n");
    cases.push(Case { name: "chunk_extension_of_200_kb", request: huge_ext, expect: RejectOrDrop });
    // Un valor casi todo ESPACIOS, dentro del límite: se recorta en una pasada (revisión previa a
    // 1.27.113: quitar un carácter por vuelta era cuadrático sobre entrada del cliente).
    let mut padded = b"GET / HTTP/1.1\r\nHost: h\r\nX-Test: ".to_vec();
    padded.extend(std::iter::repeat_n(b' ', 60_000));
    padded.extend_from_slice(b"end\r\n\r\n");
    cases.push(Case { name: "header_value_padded_with_60_kb_of_spaces", request: padded, expect: Ok(&["x=end>"]) });
    // Una línea de cabecera por encima del límite de cabeceras (64 KiB).
    let mut huge_header = b"GET / HTTP/1.1\r\nHost: h\r\nX-Test: ".to_vec();
    huge_header.extend(std::iter::repeat_n(b'a', 100_000));
    huge_header.extend_from_slice(b"\r\n\r\n");
    cases.push(Case { name: "header_line_of_100_kb", request: huge_header, expect: RejectOrDrop });
    cases
}

fn run_server_cases(port: u16, cases: &[Case]) -> Vec<String> {
    let results: Vec<Option<String>> = std::thread::scope(|s| {
        let handles: Vec<_> = cases
            .iter()
            .map(|c| {
                s.spawn(move || {
                    // Una petición incompleta deja al servidor esperando: basta un silencio corto.
                    let idle = if matches!(c.expect, Expect::NoOk) { Duration::from_millis(900) } else { Duration::from_millis(4000) };
                    let o = exchange(port, &c.request, idle);
                    verdict(c, &o).err().map(|e| format!("{}: {e}\n      crudo: {:?}", c.name, o.raw.chars().take(160).collect::<String>()))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("hilo de la fila")).collect()
    });
    results.into_iter().flatten().collect()
}

#[test]
fn the_webserver_frames_requests_as_rfc_9112_says() {
    let server = launch_echo_server();
    let cases = server_cases();
    let failures = run_server_cases(server.port, &cases);
    // Un error de ejecución del servidor tumba todas las filas a la vez: su stderr lo explica.
    let mut server = server;
    let stderr = format!(
        "{}\n(estado del proceso: {:?})",
        std::fs::read_to_string(server.dir.join("stderr.txt")).unwrap_or_default(),
        server.child.try_wait()
    );
    assert!(failures.is_empty(), "{} de {} filas fallan:\n  {}\nstderr del servidor: {stderr}", failures.len(), cases.len(), failures.join("\n  "));
}

/// Un valor que el handler copia a una cabecera de la respuesta no puede partir la respuesta
/// (#243): ni CR ni LF llegan al cable.
#[test]
fn the_webserver_never_writes_a_line_break_inside_a_response_header() {
    let server = launch_echo_server();
    let o = exchange(server.port, b"GET /reflect?v=a%0d%0aInjected:%201 HTTP/1.1\r\nHost: h\r\n\r\n", Duration::from_millis(1500));
    assert!(!o.raw.contains("\r\nInjected: 1"), "la respuesta se partió: {:?}", o.raw);
    assert!(!o.raw.to_ascii_lowercase().contains("\ninjected:"), "la respuesta se partió: {:?}", o.raw);
}

// ---------------------------------------------------------------------------------------------
// Cliente
// ---------------------------------------------------------------------------------------------

/// El cliente hace las peticiones que le dicta `PLAN` (una por línea: `MÉTODO ruta`) por UNA
/// conexión reutilizable e imprime lo que entendió de cada una.
const CLIENT: &str = r#"import net/http;

// El tercer lector del cliente: cabeza ya, cuerpo a trozos.
fn streamed(url: string) -> string {
    let s = match (http.stream("GET", url, b"", Map.new())) {
        Result.Ok(st) => st,
        Result.Err(_e) => { return "err"; },
    };
    var parts: [bytes] = [];
    while (true) {
        match (http.stream_read(s)) {
            Result.Ok(Option.Some(piece)) => { parts.push(piece); },
            Result.Ok(Option.None) => { break; },
            Result.Err(_e) => { return "err"; },
        }
    }
    "ok ${s.status} [${from_utf8(bytes_concat(parts)).unwrap_or("?")}]"
}

fn main() -> int {
    let port = env("PORT").unwrap_or("0");
    let plan = env("PLAN").unwrap_or("GET /");
    let conn = match (http.connect("http://127.0.0.1:${port}")) {
        Result.Ok(c) => c,
        Result.Err(_e) => {
            print("err");
            return 1;
        },
    };
    for step in plan.split(";") {
        let parts = step.split(" ");
        if (parts[0] == "STREAM") {
            print(streamed("http://127.0.0.1:${port}${parts[1]}"));
            continue;
        }
        match (http.conn_request(conn, parts[0], parts[1], "", Map.new())) {
            Result.Ok(r) => print("ok ${r.status} [${http.body_text(r).unwrap_or("?")}]"),
            Result.Err(_e) => print("err"),
        }
    }
    http.conn_close(conn);
    0
}
"#;

/// Un servidor falso: acepta conexiones y a la petición N-ésima de cada una contesta
/// `replies[N]` (octetos crudos); después de la última cierra si `close_after`.
fn fake_server(replies: Vec<Vec<u8>>, close_after: bool) -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("puerto 0");
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut served = 0usize;
        while Instant::now() < deadline && served < replies.len() {
            let Ok((mut conn, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            conn.set_nonblocking(false).unwrap();
            conn.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let mut pending = Vec::new();
            let mut buf = [0u8; 4096];
            while served < replies.len() {
                // Una petición sin cuerpo termina en la línea en blanco.
                let Some(end) = pending.windows(4).position(|w| w == b"\r\n\r\n") else {
                    match conn.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => pending.extend_from_slice(&buf[..n]),
                    }
                    continue;
                };
                pending.drain(..end + 4);
                if conn.write_all(&replies[served]).is_err() {
                    break;
                }
                served += 1;
            }
            if !close_after {
                // Deja la conexión abierta un momento: el cliente no debe depender del cierre.
                std::thread::sleep(Duration::from_millis(1500));
            }
        }
    });
    (port, handle)
}

/// Lanza el cliente contra un servidor falso y devuelve sus líneas (tope: 15 s).
fn run_client(dir: &std::path::Path, port: u16, plan: &str) -> Vec<String> {
    let mut child = Command::new(BIN)
        .arg("run")
        .current_dir(dir)
        .env("PORT", port.to_string())
        .env("PLAN", plan)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("lanza el cliente");
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > Duration::from_secs(15) => {
                let _ = child.kill(); // nuestro hijo, colgado
                let _ = child.wait();
                return vec!["hang".to_string()];
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break,
        }
    }
    let mut out = String::new();
    child.stdout.take().unwrap().read_to_string(&mut out).ok();
    out.lines().map(str::to_string).collect()
}

struct ClientCase {
    name: &'static str,
    plan: &'static str,
    replies: Vec<&'static str>,
    close_after: bool,
    /// Lo que el cliente debe imprimir, línea a línea; `"err"` = la petición falla.
    want: &'static [&'static str],
}

#[test]
fn the_http_client_frames_responses_as_rfc_9112_says() {
    let dir = project("client", CLIENT);
    let cases = vec![
        ClientCase { name: "content_length_body", plan: "GET /", replies: vec!["HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello"], close_after: false, want: &["ok 200 [hello]"] },
        ClientCase {
            name: "chunked_body_with_extension_and_trailer",
            plan: "GET /",
            replies: vec!["HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;e=1\r\nhello\r\n1\r\n!\r\n0\r\nX-T: 1\r\n\r\n"],
            close_after: false,
            want: &["ok 200 [hello!]"],
        },
        ClientCase { name: "body_until_close", plan: "GET /", replies: vec!["HTTP/1.1 200 OK\r\n\r\nuntil close"], close_after: true, want: &["ok 200 [until close]"] },
        ClientCase { name: "head_response_has_no_body", plan: "HEAD /", replies: vec!["HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n"], close_after: false, want: &["ok 200 []"] },
        ClientCase { name: "no_content_has_no_body", plan: "GET /", replies: vec!["HTTP/1.1 204 No Content\r\n\r\n"], close_after: false, want: &["ok 204 []"] },
        ClientCase { name: "not_modified_has_no_body", plan: "GET /", replies: vec!["HTTP/1.1 304 Not Modified\r\nContent-Length: 5\r\n\r\n"], close_after: false, want: &["ok 304 []"] },
        ClientCase {
            name: "interim_100_is_skipped",
            plan: "GET /",
            replies: vec!["HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"],
            close_after: false,
            want: &["ok 200 [ok]"],
        },
        ClientCase { name: "content_length_with_chunked_is_refused", plan: "GET /", replies: vec!["HTTP/1.1 200 OK\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n"], close_after: true, want: &["err"] },
        ClientCase { name: "two_different_content_lengths_are_refused", plan: "GET /", replies: vec!["HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 5\r\n\r\nhello"], close_after: true, want: &["err"] },
        ClientCase { name: "content_length_not_a_number_is_refused", plan: "GET /", replies: vec!["HTTP/1.1 200 OK\r\nContent-Length: 2x\r\n\r\nok"], close_after: true, want: &["err"] },
        ClientCase { name: "negative_content_length_is_refused", plan: "GET /", replies: vec!["HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n"], close_after: true, want: &["err"] },
        ClientCase { name: "chunk_size_not_hex_is_refused", plan: "GET /", replies: vec!["HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nhello\r\n0\r\n\r\n"], close_after: true, want: &["err"] },
        ClientCase { name: "chunk_size_overflow_is_refused", plan: "GET /", replies: vec!["HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n10000000000000005\r\nhello\r\n0\r\n\r\n"], close_after: true, want: &["err"] },
        ClientCase { name: "body_cut_short_is_an_error", plan: "GET /", replies: vec!["HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort"], close_after: true, want: &["err"] },
        ClientCase { name: "chunked_cut_short_is_an_error", plan: "GET /", replies: vec!["HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel"], close_after: true, want: &["err"] },
        ClientCase { name: "status_line_garbage_is_an_error", plan: "GET /", replies: vec!["NOT-HTTP 200\r\n\r\n"], close_after: true, want: &["err"] },
        ClientCase {
            name: "bytes_after_a_response_are_not_the_next_response",
            plan: "GET /a;GET /b",
            replies: vec!["HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nokHTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nfake", "HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\ntwo"],
            close_after: false,
            want: &["ok 200 [ok]", "ok 200 [two]|err"],
        },
        ClientCase {
            name: "keep_alive_reuses_the_connection",
            plan: "GET /a;GET /b",
            replies: vec!["HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\none", "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\ntwo\r\n0\r\n\r\n"],
            close_after: false,
            want: &["ok 200 [one]", "ok 200 [two]"],
        },
        ClientCase { name: "transfer_encoding_with_a_vertical_tab_is_refused", plan: "GET /", replies: vec!["HTTP/1.1 200 OK\r\nTransfer-Encoding: \x0bchunked\r\n\r\n2\r\nok\r\n0\r\n\r\n"], close_after: true, want: &["err"] },
        ClientCase { name: "content_length_with_a_form_feed_is_refused", plan: "GET /", replies: vec!["HTTP/1.1 200 OK\r\nContent-Length: 2\x0c\r\n\r\nok"], close_after: true, want: &["err"] },
        ClientCase { name: "chunk_size_followed_by_a_space", plan: "GET /", replies: vec!["HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5 \r\nhello\r\n1 ;e\r\n!\r\n0\r\n\r\n"], close_after: false, want: &["ok 200 [hello!]"] },
        ClientCase { name: "stream_content_length_body", plan: "STREAM /", replies: vec!["HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello"], close_after: true, want: &["ok 200 [hello]"] },
        ClientCase { name: "stream_chunked_body", plan: "STREAM /", replies: vec!["HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n1 \r\n!\r\n0\r\n\r\n"], close_after: true, want: &["ok 200 [hello!]"] },
        ClientCase {
            name: "stream_interim_100_is_skipped",
            plan: "STREAM /",
            replies: vec!["HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"],
            close_after: true,
            want: &["ok 200 [ok]"],
        },
        ClientCase { name: "stream_chunk_size_not_hex_is_refused", plan: "STREAM /", replies: vec!["HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nhello\r\n0\r\n\r\n"], close_after: true, want: &["err"] },
        ClientCase { name: "stream_unknown_transfer_encoding_is_refused", plan: "STREAM /", replies: vec!["HTTP/1.1 200 OK\r\nTransfer-Encoding: xchunked\r\n\r\n2\r\nok\r\n0\r\n\r\n"], close_after: true, want: &["err"] },
        ClientCase { name: "stream_negative_content_length_is_refused", plan: "STREAM /", replies: vec!["HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n"], close_after: true, want: &["err"] },
        ClientCase { name: "folded_header_is_not_a_new_header", plan: "GET /", replies: vec!["HTTP/1.1 200 OK\r\nX-A: 1\r\n Content-Length: 99\r\nContent-Length: 2\r\n\r\nok"], close_after: true, want: &["ok 200 [ok]|err"] },
    ];
    let failures: Vec<String> = std::thread::scope(|s| {
        let handles: Vec<_> = cases
            .iter()
            .map(|c| {
                let dir = &dir;
                s.spawn(move || {
                    let (port, server) = fake_server(c.replies.iter().map(|r| r.as_bytes().to_vec()).collect(), c.close_after);
                    let got = run_client(dir, port, c.plan);
                    let _ = server.join();
                    let matches = got.len() == c.want.len() && got.iter().zip(c.want.iter()).all(|(g, w)| w.split('|').any(|alt| alt == g));
                    if matches { None } else { Some(format!("{}: se esperaba {:?}, el cliente imprimió {:?}", c.name, c.want, got)) }
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().expect("hilo de la fila")).collect()
    });
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failures.is_empty(), "{} de {} filas fallan:\n  {}", failures.len(), cases.len(), failures.join("\n  "));
}
