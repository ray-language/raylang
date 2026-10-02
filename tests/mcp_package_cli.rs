//! `packages/mcp`: el cliente del Model Context Protocol. Tres frentes: las piezas puras del
//! protocolo (mensajes y lectura de respuestas, con texto fijo), el transporte por stdio contra
//! el servidor MCP real de la toolchain (`ray mcp`), y el transporte HTTP contra un servidor
//! mínimo dentro del propio programa de prueba, en un puerto efímero.
//!
//! Corre sobre la VM: el intérprete no tiene fibras ni procesos con stdin abierto.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// Un proyecto temporal que depende de `packages/mcp` por ruta, con `main` como programa.
fn project(tag: &str, main: &str) -> std::path::PathBuf {
    let package = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/mcp");
    let app = std::env::temp_dir().join(format!("ray_mcp_package_{tag}")).join("app");
    let _ = std::fs::remove_dir_all(&app);
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nmcp = \"path:{}\"\n",
            package.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

/// Corre el proyecto en la VM y devuelve su stdout (sin la línea del puerto del servidor de
/// prueba, que cambia en cada ejecución), su stderr y su código de salida.
fn run(app: &std::path::Path) -> (String, String, i32) {
    let out = Command::new(BIN)
        .arg("run")
        .current_dir(app)
        .env("RAY_BIN", BIN)
        .output()
        .expect("ejecuta raylang");
    let stdout: String = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.starts_with("listening on port "))
        .map(|l| format!("{l}\n"))
        .collect();
    (stdout, String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

const PROTOCOL_MAIN: &str = r##"import std/json;
import mcp/protocol;
from std/json import Json;

fn show(r: Option<Result<Json, string>>) -> string {
    match (r) {
        Option.None => "not the reply",
        Option.Some(Result.Ok(j)) => "ok " + json.stringify(j),
        Option.Some(Result.Err(e)) => "err " + e,
    }
}

fn main() -> int {
    // Mensajes que salen.
    print(protocol.request(7, "tools/list", protocol.empty_object()));
    print(protocol.notification("notifications/initialized"));
    print(protocol.initialize("app", "1.2.3"));
    // El resultado de `initialize`.
    let init = json.parse(
        `{"capabilities":{"tools":{},"resources":{"subscribe":true}},"instructions":"  Use wisely.\n"}`
    ).unwrap();
    print("[${protocol.instructions_of(init)}]");
    print("tools=${protocol.offers(init, "tools")} resources=${protocol.offers(init, "resources")} prompts=${protocol.offers(init, "prompts")}");
    print("[${protocol.instructions_of(Json.JNull)}] ${protocol.offers(Json.JNull, "tools")}");
    // Qué línea contesta a qué petición.
    print(show(protocol.reply_with_id(`{"jsonrpc":"2.0","id":3,"result":{"ok":true}}`, 3)));
    print(show(protocol.reply_with_id(`{"jsonrpc":"2.0","id":4,"result":{}}`, 3)));
    print(show(protocol.reply_with_id(`{"jsonrpc":"2.0","method":"notifications/progress"}`, 3)));
    print(show(protocol.reply_with_id("a log line, not JSON", 3)));
    print(show(protocol.reply_with_id(`{"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"nope"}}`, 3)));
    print(show(protocol.reply_with_id(`{"jsonrpc":"2.0","id":3}`, 3)));
    // tools/list
    let listed = json.parse(
        `{"tools":[{"name":"read","description":"Reads","inputSchema":{"type":"object","required":["path"]},"annotations":{"readOnlyHint":true}},{"name":"bare"}]}`
    ).unwrap();
    for t in protocol.parse_tools(listed).unwrap() {
        print("${t.name} | ${t.description} | ${json.stringify(t.schema)} | ${t.read_only}");
    }
    print(protocol.parse_tools(json.parse(`{"tools":[{"description":"nameless"}]}`).unwrap()).is_err());
    print(protocol.parse_tools(json.parse("{}").unwrap()).is_err());
    // resources/list
    let published = json.parse(
        `{"resources":[{"uri":"doc://a","name":"A","description":"First","mimeType":"text/plain"},{"uri":"doc://b"}]}`
    ).unwrap();
    for r in protocol.parse_resources(published).unwrap() {
        print("${r.uri} | ${r.name} | ${r.description} | ${r.mime}");
    }
    print(protocol.parse_resources(json.parse(`{"resources":[{"name":"no uri"}]}`).unwrap()).is_err());
    // tools/call
    print(protocol.call_text(json.parse(
        `{"content":[{"type":"text","text":"line one"},{"type":"image","data":"AAAAAAAAAAAAAAAA"},{"type":"text","text":"line two"}]}`
    ).unwrap()));
    print(protocol.call_text(json.parse(`{"content":[{"type":"text","text":"it broke"}],"isError":true}`).unwrap()));
    print(protocol.call_text(json.parse(`{"content":[],"isError":true}`).unwrap()));
    print(protocol.call_text(json.parse(`{"structuredContent":{"n":1}}`).unwrap()));
    // resources/read
    print(protocol.resource_text(json.parse(
        `{"contents":[{"uri":"doc://a","text":"hello"},{"uri":"doc://b","blob":"AAAA","mimeType":"image/png"}]}`
    ).unwrap()));
    print(protocol.resource_text(json.parse("{}").unwrap()).is_err());
    // El nombre que ve el modelo.
    print(protocol.tool_name("files", "read"));
    print(protocol.is_remote("mcp__files__read"));
    print(protocol.is_remote("read_file"));
    print(protocol.split_name("mcp__files__read"));
    print(protocol.split_name("mcp__files__read__deep"));
    print(protocol.split_name("mcp__files"));
    print(protocol.split_name("mcp____read"));
    print(protocol.split_name("read_file"));
    0
}
"##;

const PROTOCOL_EXPECTED: &str = r##"{"id":7,"jsonrpc":"2.0","method":"tools/list","params":{}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"id":1,"jsonrpc":"2.0","method":"initialize","params":{"capabilities":{},"clientInfo":{"name":"app","version":"1.2.3"},"protocolVersion":"2024-11-05"}}
[Use wisely.]
tools=true resources=true prompts=false
[] false
ok {"ok":true}
not the reply
not the reply
not the reply
err the server answered an error: nope (code -32601)
err the reply has neither "result" nor "error"
read | Reads | {"required":["path"],"type":"object"} | true
bare |  | {"properties":{},"type":"object"} | false
true
true
doc://a | A | First | text/plain
doc://b |  |  | 
true
Result.Ok(line one
[image 0 KiB]
line two)
Result.Err(it broke)
Result.Err(the tool reported an error)
Result.Ok({"n":1})
Result.Ok(hello
[blob 0 KiB image/png])
true
mcp__files__read
true
false
Option.Some((files, read))
Option.Some((files, read__deep))
Option.None
Option.None
Option.None
"##;

#[test]
fn protocol_messages_and_replies() {
    let app = project("protocol", PROTOCOL_MAIN);
    let (stdout, stderr, code) = run(&app);
    assert_eq!(code, 0, "el programa sale 0\n{stdout}\n{stderr}");
    assert_eq!(stdout, PROTOCOL_EXPECTED, "la salida esperada\n{stderr}");
}

const SLEEPER: &str = "import std/time;\n\nfn main() -> int {\n    time.sleep(60000);\n    0\n}\n";

const STDIO_MAIN: &str = r##"import std/json;
import mcp/mcp;

fn failure(server: mcp.Server) -> string {
    match (mcp.connect(server)) {
        Result.Ok(c) => {
            mcp.close(c);
            "connected"
        },
        Result.Err(e) => e,
    }
}

fn main() -> int {
    let ray = env("RAY_BIN").unwrap();
    let c = match (mcp.connect(mcp.stdio_server("ray", ray, ["mcp"]))) {
        Result.Ok(c) => c,
        Result.Err(e) => {
            print("connect: " + e);
            return 1;
        },
    };
    print("instructions: ${mcp.instructions(c).len() > 0}");
    print("offers tools=${mcp.offers(c, "tools")} resources=${mcp.offers(c, "resources")}");
    var names: [string] = [];
    for t in mcp.tools(c).unwrap() {
        names.push(t.name);
    }
    print(names.join(" "));
    print(mcp.call_json(c, "ray_run", `{"code": "fn main() { print(6 * 7); }"}`).unwrap().contains("42"));
    print(mcp.call(c, "ray_run", json.parse(`{"code": "fn main() { print(1 + 1); }"}`).unwrap()).unwrap().contains("2"));
    print(mcp.call_json(c, "no_such_tool", "{}"));
    print(mcp.call_json(c, "ray_run", "{not json"));
    let published = mcp.resources(c).unwrap();
    print(published.len() > 0);
    print(mcp.read_resource(c, published[0].uri).unwrap().len() > 0);
    print(mcp.read_resource(c, "raylang://no-such").is_err());
    // Una copia de la sesión en otra fibra habla con el mismo servidor.
    let other = spawn(fn() -> int { mcp.tools(c).unwrap().len() });
    print(join(other));
    mcp.close(c);
    mcp.close(c);
    print(mcp.tools(c));
    // Lo que puede ir mal al conectar.
    print(failure(mcp.stdio_server("ghost", "no-such-program-xyz", [])).starts_with("cannot start 'no-such-program-xyz': "));
    print(failure(mcp.stdio_server("quitter", ray, ["version"])).starts_with("initialize failed: "));
    var silent = mcp.stdio_server("silent", ray, ["run", "src/sleeper.ray"]);
    silent.start_timeout_ms = 500;
    print(failure(silent));
    0
}
"##;

const STDIO_EXPECTED: &str = r##"instructions: true
offers tools=true resources=true
ray_check ray_run ray_test ray_fmt ray_doc
true
true
Result.Err(unknown tool: no_such_tool)
Result.Err(the arguments of 'ray_run' are not valid JSON: expected a string key)
true
true
true
5
Result.Err(the session with 'ray' is closed)
true
true
initialize failed: no reply after 500ms; the server was killed
"##;

#[test]
fn stdio_session_against_the_toolchain_server() {
    let app = project("stdio", STDIO_MAIN);
    std::fs::write(app.join("src/sleeper.ray"), SLEEPER).unwrap();
    let (stdout, stderr, code) = run(&app);
    assert_eq!(code, 0, "el programa sale 0\n{stdout}\n{stderr}");
    assert_eq!(stdout, STDIO_EXPECTED, "la salida esperada\n{stderr}");
}

const HTTP_MAIN: &str = r##"import std/json;
import std/net;
import net/webserver;
import mcp/mcp;
from std/json import Json;
from net/webserver import Request, Response;

// Un servidor MCP mínimo por HTTP. No guarda estado en variables (cada conexión corre en su
// fibra, con memoria propia): `expire` es un canal con UNA ficha, y quien la saca responde 404
// a esa llamada, como un servidor que ha olvidado la sesión.
fn fake(req: Request, expire: Channel<int>) -> Response {
    let msg = json.parse(webserver.request_text(req).unwrap_or("")).unwrap_or(Json.JNull);
    let method = json.get_string(msg, "method").unwrap_or("");
    let id = json.get_int(msg, "id").unwrap_or(0 - 1);
    let session = req.headers.get("mcp-session-id").unwrap_or("");
    if (req.method == "DELETE") {
        print("server: DELETE session=${session}");
        return webserver.text(200, "");
    }
    if (method == "initialize") {
        print("server: initialize");
        let r = webserver.json_response(
            `{"jsonrpc":"2.0","id":${id},"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"instructions":" Be kind. "}}`
        );
        r.headers.insert("Mcp-Session-Id", "s-1");
        return r;
    }
    if (method == "notifications/initialized") {
        return webserver.text(202, "");
    }
    if (session != "s-1") {
        return webserver.text(400, "missing session");
    }
    if (req.headers.get("authorization").unwrap_or("") != "Bearer t0ken") {
        return webserver.text(401, "who are you");
    }
    if (method == "tools/list") {
        // Como flujo SSE: primero un evento que no es la respuesta, luego la respuesta.
        let body = `event: message
data: {"jsonrpc":"2.0","method":"notifications/progress","params":{}}

event: message
data: {"jsonrpc":"2.0","id":${id},"result":{"tools":[{"name":"echo","description":"Says it back","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}},{"name":"bare"}]}}

`;
        let r = webserver.text(200, body);
        r.headers.insert("Content-Type", "text/event-stream");
        return r;
    }
    if (method == "tools/call") {
        match (try_recv(expire)) {
            Received.Got(_) => {
                print("server: 404, the session expired");
                return webserver.text(404, "no such session");
            },
            _ => { },
        }
        let params = json.member(msg, "params").unwrap_or(Json.JNull);
        let name = json.get_string(params, "name").unwrap_or("");
        if (name == "fail") {
            return webserver.json_response(
                `{"jsonrpc":"2.0","id":${id},"result":{"content":[{"type":"text","text":"it broke"}],"isError":true}}`
            );
        }
        let args = json.stringify(json.member(params, "arguments").unwrap_or(Json.JNull));
        return webserver.json_response(
            `{"jsonrpc":"2.0","id":${id},"result":{"content":[{"type":"text","text":${json.stringify(Json.JStr(args))}},{"type":"image","data":"AAAA"}]}}`
        );
    }
    webserver.json_response(
        `{"jsonrpc":"2.0","id":${id},"error":{"code":-32601,"message":"method not found"}}`
    )
}

fn main() -> int {
    let listener = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(listener);
    let expire: Channel<int> = Channel.new();
    send(expire, 1);
    spawn(fn() {
        let _ = webserver.serve_on(listener, fn(req: Request) -> Response { fake(req, expire) });
    });
    var server = mcp.http_server("fake", "http://127.0.0.1:${port}/mcp");
    server.headers.insert("Authorization", "Bearer t0ken");
    let c = match (mcp.connect(server)) {
        Result.Ok(c) => c,
        Result.Err(e) => {
            print("connect: " + e);
            return 1;
        },
    };
    print("instructions: [${mcp.instructions(c)}]");
    print("offers tools=${mcp.offers(c, "tools")} resources=${mcp.offers(c, "resources")}");
    for t in mcp.tools(c).unwrap() {
        print("tool ${t.name} read_only=${t.read_only} schema=${json.stringify(t.schema)}");
    }
    // La primera llamada cae en la sesión caducada: el cliente la reabre y reintenta.
    print(mcp.call_json(c, "echo", `{"text": "hola"}`));
    print(mcp.call_json(c, "echo", ""));
    print(mcp.call_json(c, "fail", "{}"));
    print(mcp.resources(c).unwrap().len());
    print(mcp.request(c, "prompts/list", json.parse("{}").unwrap()).unwrap_or(Json.JNull) == Json.JNull);
    mcp.close(c);
    print(mcp.tools(c).is_err());
    // Sin la cabecera Authorization, el servidor rechaza y el error lleva su respuesta.
    let anonymous = mcp.connect(mcp.http_server("fake", "http://127.0.0.1:${port}/mcp")).unwrap();
    print(mcp.tools(anonymous));
    0
}
"##;

#[test]
fn http_session_with_sse_and_an_expired_session() {
    let app = project("http", HTTP_MAIN);
    let (stdout, stderr, code) = run(&app);
    assert_eq!(code, 0, "el programa sale 0\n{stdout}\n{stderr}");
    assert_eq!(stdout, HTTP_EXPECTED, "la salida esperada\n{stderr}");
}

const HTTP_EXPECTED: &str = r##"server: initialize
instructions: [Be kind.]
offers tools=true resources=false
tool echo read_only=true schema={"type":"object"}
tool bare read_only=false schema={"properties":{},"type":"object"}
server: 404, the session expired
server: initialize
Result.Ok({"text":"hola"}
[image 0 KiB])
Result.Ok({}
[image 0 KiB])
Result.Err(it broke)
0
true
server: DELETE session=s-1
true
server: initialize
Result.Err(HTTP 401: who are you)
"##;
