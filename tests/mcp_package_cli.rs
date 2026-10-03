//! `packages/mcp`: el Model Context Protocol, cliente y servidor.
//!
//! El cliente (`mcp/mcp`), en tres frentes: las piezas puras del protocolo (mensajes y lectura
//! de respuestas, con texto fijo), el transporte por stdio contra el servidor MCP real de la
//! toolchain (`ray mcp`), y el transporte HTTP contra un servidor mínimo dentro del propio
//! programa de prueba, en un puerto efímero.
//!
//! El servidor (`mcp/serve`), en dos: `handle` con mensajes fijos, y de punta a punta con el
//! cliente del mismo paquete, por stdio (el programa se lanza a sí mismo como servidor) y por
//! HTTP (lo que rechaza, lo que acepta sin cuerpo y el token).
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

const HANDLE_MAIN: &str = r##"import std/json;
import mcp/serve;
from std/json import Json;
from mcp/serve import Provider;

fn build() -> Provider {
    var p = serve.provider("demo", "1.0.0");
    p.instructions = "A demo server.";
    serve.read_only_tool(
        p,
        "shout",
        "Upper-cases a text",
        `{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}`,
        fn(args: Json) -> Result<string, string> {
            match (json.get_string(args, "text")) {
                Option.Some(t) => Result.Ok(t.to_upper()),
                Option.None => Result.Err("'text' is required"),
            }
        }
    );
    serve.tool(p, "crash", "Always aborts", "", fn(args: Json) -> Result<string, string> {
        panic("boom");
        Result.Ok("unreachable")
    });
    serve.resource(
        p,
        "demo://greeting",
        "Greeting",
        "",
        "text/plain",
        fn() -> Result<string, string> { Result.Ok("hello") }
    );
    serve.resource(
        p,
        "demo://broken",
        "Broken",
        "Cannot be read",
        "",
        fn() -> Result<string, string> { Result.Err("disk on fire") }
    );
    p
}

fn show(p: Provider, line: string) {
    match (serve.handle(p, line)) {
        Option.Some(out) => print(out),
        Option.None => print("(no reply)"),
    }
}

fn main() -> int {
    let p = build();
    // Registrar dos veces lo mismo, o con un esquema que no es JSON, aborta el programa al
    // arrancar: son errores de programación, y el mensaje dice cuál.
    print(try_call(fn() { serve.tool(p, "shout", "again", "", fn(args: Json) -> Result<string, string> { Result.Ok("") }) }));
    print(try_call(fn() { serve.tool(p, "bad", "bad schema", "{not json", fn(args: Json) -> Result<string, string> { Result.Ok("") }) }));
    print(try_call(fn() { serve.resource(p, "demo://greeting", "again", "", "", fn() -> Result<string, string> { Result.Ok("") }) }));
    show(p, `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}`);
    show(p, `{"jsonrpc":"2.0","id":"a","method":"initialize","params":{}}`);
    show(p, `{"jsonrpc":"2.0","method":"notifications/initialized"}`);
    show(p, `{"jsonrpc":"2.0","id":2,"method":"ping"}`);
    show(p, `{"jsonrpc":"2.0","id":3,"method":"tools/list"}`);
    show(p, `{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"shout","arguments":{"text":"hola"}}}`);
    show(p, `{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"shout"}}`);
    show(p, `{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"crash"}}`);
    show(p, `{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"nope"}}`);
    show(p, `{"jsonrpc":"2.0","id":8,"method":"resources/list"}`);
    show(p, `{"jsonrpc":"2.0","id":9,"method":"resources/read","params":{"uri":"demo://greeting"}}`);
    show(p, `{"jsonrpc":"2.0","id":10,"method":"resources/read","params":{"uri":"demo://broken"}}`);
    show(p, `{"jsonrpc":"2.0","id":11,"method":"resources/read","params":{"uri":"demo://nope"}}`);
    show(p, `{"jsonrpc":"2.0","id":12,"method":"prompts/list"}`);
    show(p, `{"jsonrpc":"2.0","id":13}`);
    show(p, `{"jsonrpc":"2.0","id":14,"result":{}}`);
    show(p, "not json");
    // Un servidor sin recursos no los anuncia.
    show(serve.provider("bare", "0.0.1"), `{"jsonrpc":"2.0","id":15,"method":"initialize"}`);
    0
}
"##;

const HANDLE_EXPECTED: &str = r##"Result.Err(mcp/serve: tool 'shout' is already registered)
Result.Err(mcp/serve: the schema of tool 'bad' is not valid JSON: expected a string key)
Result.Err(mcp/serve: resource 'demo://greeting' is already registered)
{"id":1,"jsonrpc":"2.0","result":{"capabilities":{"resources":{},"tools":{}},"instructions":"A demo server.","protocolVersion":"2025-06-18","serverInfo":{"name":"demo","version":"1.0.0"}}}
{"id":"a","jsonrpc":"2.0","result":{"capabilities":{"resources":{},"tools":{}},"instructions":"A demo server.","protocolVersion":"2024-11-05","serverInfo":{"name":"demo","version":"1.0.0"}}}
(no reply)
{"id":2,"jsonrpc":"2.0","result":{}}
{"id":3,"jsonrpc":"2.0","result":{"tools":[{"annotations":{"readOnlyHint":true},"description":"Upper-cases a text","inputSchema":{"properties":{"text":{"type":"string"}},"required":["text"],"type":"object"},"name":"shout"},{"description":"Always aborts","inputSchema":{"properties":{},"type":"object"},"name":"crash"}]}}
{"id":4,"jsonrpc":"2.0","result":{"content":[{"text":"HOLA","type":"text"}],"isError":false}}
{"id":5,"jsonrpc":"2.0","result":{"content":[{"text":"'text' is required","type":"text"}],"isError":true}}
{"id":6,"jsonrpc":"2.0","result":{"content":[{"text":"the tool failed: boom","type":"text"}],"isError":true}}
{"error":{"code":-32602,"message":"unknown tool: nope"},"id":7,"jsonrpc":"2.0"}
{"id":8,"jsonrpc":"2.0","result":{"resources":[{"mimeType":"text/plain","name":"Greeting","uri":"demo://greeting"},{"description":"Cannot be read","name":"Broken","uri":"demo://broken"}]}}
{"id":9,"jsonrpc":"2.0","result":{"contents":[{"mimeType":"text/plain","text":"hello","uri":"demo://greeting"}]}}
{"error":{"code":-32603,"message":"cannot read demo://broken: disk on fire"},"id":10,"jsonrpc":"2.0"}
{"error":{"code":-32002,"message":"unknown resource: demo://nope"},"id":11,"jsonrpc":"2.0"}
{"error":{"code":-32601,"message":"method not found: prompts/list"},"id":12,"jsonrpc":"2.0"}
{"error":{"code":-32600,"message":"invalid request: no method"},"id":13,"jsonrpc":"2.0"}
(no reply)
{"error":{"code":-32700,"message":"parse error: expected 'null'"},"id":null,"jsonrpc":"2.0"}
{"id":15,"jsonrpc":"2.0","result":{"capabilities":{"tools":{}},"protocolVersion":"2024-11-05","serverInfo":{"name":"bare","version":"0.0.1"}}}
"##;

#[test]
fn server_answers_each_protocol_message() {
    let app = project("handle", HANDLE_MAIN);
    let (stdout, stderr, code) = run(&app);
    assert_eq!(code, 0, "el programa sale 0\n{stdout}\n{stderr}");
    assert_eq!(stdout, HANDLE_EXPECTED, "la salida esperada\n{stderr}");
}

const SERVE_MAIN: &str = r##"import std/json;
import std/net;
import net/http;
import mcp/mcp;
import mcp/serve;
from std/json import Json;
from mcp/serve import Provider;

fn number(args: Json, key: string) -> Result<int, string> {
    match (json.get_int(args, key)) {
        Option.Some(n) => Result.Ok(n),
        Option.None => Result.Err("'${key}' must be an integer"),
    }
}

// Lo que ofrece el servidor de prueba. Función de nivel superior: el transporte HTTP la llama
// en la fibra de cada conexión.
fn build() -> Provider {
    var p = serve.provider("demo", "1.0.0");
    p.instructions = "A demo server.";
    serve.read_only_tool(
        p,
        "add",
        "Adds two integers",
        `{"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}},"required":["a","b"]}`,
        fn(args: Json) -> Result<string, string> {
            let a = number(args, "a")?;
            let b = number(args, "b")?;
            Result.Ok(to_string(a + b))
        }
    );
    serve.tool(p, "crash", "Always aborts", "", fn(args: Json) -> Result<string, string> {
        panic("boom");
        Result.Ok("unreachable")
    });
    serve.resource(
        p,
        "demo://greeting",
        "Greeting",
        "A friendly line",
        "text/plain",
        fn() -> Result<string, string> { Result.Ok("hello from the server") }
    );
    p
}

// El mismo servidor, pero exige un token.
fn build_private() -> Provider {
    var p = build();
    p.token = "s3cret-t0ken";
    p
}

// El estado y el cuerpo de una petición HTTP cruda al servidor.
fn raw(method: string, url: string, body: string, name: string, value: string) -> string {
    var headers: Map<string, string> = Map.new();
    headers.insert("Content-Type", "application/json");
    if (name != "") {
        headers.insert(name, value);
    }
    match (http.request_bytes(method, url, body.to_bytes(), headers, 5000)) {
        Result.Ok(r) => "${r.status} ${http.body_text(r).unwrap_or("")}",
        Result.Err(e) => "failed: " + e,
    }
}

// Usa una sesión de punta a punta e imprime lo que ve el cliente.
fn exercise(label: string, c: mcp.Session) {
    print("== ${label}");
    print("instructions: [${mcp.instructions(c)}]");
    print("offers tools=${mcp.offers(c, "tools")} resources=${mcp.offers(c, "resources")}");
    for t in mcp.tools(c).unwrap() {
        print("tool ${t.name} | ${t.description} | read_only=${t.read_only} | ${json.stringify(t.schema)}");
    }
    print(mcp.call_json(c, "add", `{"a": 19, "b": 23}`));
    print(mcp.call_json(c, "add", `{"a": "x"}`));
    print(mcp.call_json(c, "crash", ""));
    print(mcp.call_json(c, "missing", ""));
    for r in mcp.resources(c).unwrap() {
        print("resource ${r.uri} | ${r.name} | ${r.description} | ${r.mime}");
    }
    print(mcp.read_resource(c, "demo://greeting"));
    print(mcp.read_resource(c, "demo://nope"));
    print(mcp.request(c, "ping", json.parse("{}").unwrap()).map(fn(j: Json) -> string { json.stringify(j) }));
    print(mcp.request(c, "prompts/list", json.parse("{}").unwrap()).is_err());
    mcp.close(c);
}

fn main() -> int {
    let argv = args();
    if (argv.len() > 0 && argv[0] == "server") {
        return serve.stdio(build());
    }
    // El mismo programa, lanzado como servidor por stdio.
    let ray = env("RAY_BIN").unwrap();
    exercise("stdio", mcp.connect(mcp.stdio_server("demo", ray, ["run", "src/main.ray", "server"])).unwrap());
    // Y por HTTP, en un puerto libre.
    let listener = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(listener);
    spawn(fn() {
        let _ = serve.http_on(build, listener);
    });
    exercise("http", mcp.connect_http("http://127.0.0.1:${port}/mcp").unwrap());
    // Lo que el transporte HTTP rechaza o acepta sin cuerpo.
    let url = "http://127.0.0.1:${port}/mcp";
    let ping = `{"jsonrpc":"2.0","id":9,"method":"ping"}`;
    print("== http, raw");
    print(raw("GET", url, "", "", ""));
    print(raw("POST", url, ping, "Origin", "https://evil.example"));
    print(raw("POST", url, ping, "Origin", "http://127.0.0.1:${port}"));
    print(raw("POST", url, `{"jsonrpc":"2.0","method":"notifications/initialized"}`, "", ""));
    print(raw("POST", url, "not json", "", ""));
    // Con token: sin él no se entra; con él, la sesión funciona.
    let private_listener = net.tcp_listen("127.0.0.1", 0).unwrap();
    let private_url = "http://127.0.0.1:${net.local_port(private_listener)}/mcp";
    spawn(fn() {
        let _ = serve.http_on(build_private, private_listener);
    });
    print("== http, token");
    print(raw("POST", private_url, ping, "", ""));
    print(raw("POST", private_url, ping, "Authorization", "Bearer wrong"));
    print(mcp.connect_http(private_url).is_err());
    var trusted = mcp.http_server("private", private_url);
    trusted.headers.insert("Authorization", "Bearer s3cret-t0ken");
    let c = mcp.connect(trusted).unwrap();
    print(mcp.call_json(c, "add", `{"a": 2, "b": 2}`));
    mcp.close(c);
    0
}
"##;

const SERVE_EXPECTED: &str = r##"== stdio
instructions: [A demo server.]
offers tools=true resources=true
tool add | Adds two integers | read_only=true | {"properties":{"a":{"type":"integer"},"b":{"type":"integer"}},"required":["a","b"],"type":"object"}
tool crash | Always aborts | read_only=false | {"properties":{},"type":"object"}
Result.Ok(42)
Result.Err('a' must be an integer)
Result.Err(the tool failed: boom)
Result.Err(the server answered an error: unknown tool: missing (code -32602))
resource demo://greeting | Greeting | A friendly line | text/plain
Result.Ok(hello from the server)
Result.Err(the server answered an error: unknown resource: demo://nope (code -32002))
Result.Ok({})
true
== http
instructions: [A demo server.]
offers tools=true resources=true
tool add | Adds two integers | read_only=true | {"properties":{"a":{"type":"integer"},"b":{"type":"integer"}},"required":["a","b"],"type":"object"}
tool crash | Always aborts | read_only=false | {"properties":{},"type":"object"}
Result.Ok(42)
Result.Err('a' must be an integer)
Result.Err(the tool failed: boom)
Result.Err(the server answered an error: unknown tool: missing (code -32602))
resource demo://greeting | Greeting | A friendly line | text/plain
Result.Ok(hello from the server)
Result.Err(the server answered an error: unknown resource: demo://nope (code -32002))
Result.Ok({})
true
== http, raw
405 use POST
403 cross-origin request refused
200 {"id":9,"jsonrpc":"2.0","result":{}}
202 
200 {"error":{"code":-32700,"message":"parse error: expected 'null'"},"id":null,"jsonrpc":"2.0"}
== http, token
401 missing or wrong bearer token
401 missing or wrong bearer token
true
Result.Ok(4)
"##;

#[test]
fn server_and_client_talk_over_stdio_and_http() {
    let app = project("serve", SERVE_MAIN);
    let (stdout, stderr, code) = run(&app);
    assert_eq!(code, 0, "el programa sale 0\n{stdout}\n{stderr}");
    assert_eq!(stdout, SERVE_EXPECTED, "la salida esperada\n{stderr}");
}
