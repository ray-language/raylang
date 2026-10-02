//! `packages/llm`: hablar con modelos de lenguaje. Dos frentes, sin red ni clave:
//!
//! - Las piezas puras de los dos dialectos (Anthropic y el de OpenAI): el cuerpo de la
//!   petición, la lectura de una respuesta entera y el ensamblado de un flujo, con texto fijo.
//! - De punta a punta contra un proveedor simulado dentro del propio programa de prueba, en un
//!   puerto efímero: el bucle de conversación con una herramienta, entero y por partes, en los
//!   dos dialectos; un 429 con `Retry-After`, la corrección de `max_tokens` cuando el
//!   proveedor lo rechaza, los errores con su estado y el listado de modelos.
//!
//! Corre sobre la VM, como los demás tests de paquetes.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// Un proyecto temporal que depende de `packages/llm` por ruta, con `main` como programa.
fn project(tag: &str, main: &str) -> std::path::PathBuf {
    let package = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/llm");
    let app = std::env::temp_dir().join(format!("ray_llm_package_{tag}")).join("app");
    let _ = std::fs::remove_dir_all(&app);
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nllm = \"path:{}\"\n",
            package.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

/// Corre el proyecto en la VM y devuelve su stdout (sin la línea del puerto del proveedor
/// simulado, que cambia en cada ejecución), su stderr y su código de salida.
fn run(app: &std::path::Path) -> (String, String, i32) {
    let out = Command::new(BIN).arg("run").current_dir(app).output().expect("ejecuta raylang");
    let stdout: String = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.starts_with("listening on port "))
        .map(|l| format!("{l}\n"))
        .collect();
    (stdout, String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

const DIALECTS_MAIN: &str = r##"import std/json;
import llm/llm;
import llm/config;
import llm/message;
import llm/anthropic;
import llm/openai;
from llm/message import Message, Reply, Tool;

fn show(label: string, r: Reply) {
    print("${label}: stop=${r.stop_reason} model=${r.model} text=[${r.message.text}]");
    for call in r.message.tool_calls {
        print("  call ${call.id} ${call.name} ${call.arguments}");
    }
    print("  usage in=${r.usage.input_tokens} out=${r.usage.output_tokens} cached=${r.usage.cached_tokens} measured=${r.usage.measured}");
    if (r.message.raw != "") {
        print("  raw ${r.message.raw}");
    }
}

// Una conversación con todo: una pregunta, un turno del asistente que pide dos herramientas,
// sus dos resultados y otra pregunta.
fn conversation() -> [Message] {
    [
        message.user("What is 19 + 23?"),
        message.assistant(
            "Let me add.",
            [
                message.ToolCall { id: "call_1", name: "add", arguments: `{"a": 19, "b": 23}` },
                message.ToolCall { id: "call_2", name: "now", arguments: "" }
            ]
        ),
        message.tool_result("call_1", "42"),
        message.tool_result("call_2", "noon"),
        message.user("Thanks.")
    ]
}

fn main() -> int {
    let tools = [
        message.tool("add", "Adds two integers", `{"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}}}`).unwrap(),
        message.tool("now", "The time", "").unwrap()
    ];
    print(message.tool("bad", "", "{nope").is_err());

    // ---- Anthropic: el cuerpo de la petición.
    var a = llm.for_anthropic("claude-opus-5-5", "k-ant");
    a.system = "Be brief.";
    a.max_tokens = 1000;
    a.effort = "medium";
    a.extra.insert("fallbacks", json.parse(`"default"`).unwrap());
    a.headers.insert("anthropic-beta", "server-side-fallback-2026-07-01");
    print(config.chat_url(a));
    print(config.models_url(a));
    print(anthropic.build_body(a, tools, conversation(), false));
    a.cache = false;
    a.effort = "";
    a.temperature = 0.5;
    let none: [Tool] = [];
    print(anthropic.build_body(a, none, [message.user("hi")], true));
    for (k, v) in anthropic.headers(a) {
        print("${k}: ${v}");
    }
    // Un turno del asistente con sus bloques originales se reenvía tal cual.
    let replayed = message.assistant_raw("ignored", [], `[{"type":"thinking","thinking":"hm","signature":"s1"},{"type":"text","text":"Hi."}]`);
    print(anthropic.build_body(a, none, [message.user("hi"), replayed], false));

    // ---- Anthropic: leer una respuesta entera.
    show("anthropic", anthropic.parse_reply(
        `{"id":"msg_1","type":"message","role":"assistant","model":"claude-opus-5-5","stop_reason":"tool_use","content":[{"type":"thinking","thinking":"hm","signature":"s1"},{"type":"text","text":"Let me add."},{"type":"tool_use","id":"toolu_1","name":"add","input":{"a":19,"b":23}}],"usage":{"input_tokens":30,"output_tokens":12,"cache_read_input_tokens":20}}`
    ).unwrap());
    print(anthropic.parse_reply(`{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}`));
    print(anthropic.parse_reply(`{"type":"message"}`));
    print(anthropic.parse_reply("not json").is_err());

    // ---- Anthropic: ensamblar un flujo.
    let events = [
        `{"type":"message_start","message":{"id":"msg_1","model":"claude-opus-5-5","usage":{"input_tokens":25,"output_tokens":1,"cache_read_input_tokens":10}}}`,
        `{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}`,
        `{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think."}}`,
        `{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig123"}}`,
        `{"type":"content_block_stop","index":0}`,
        `{"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"opaque"}}`,
        `{"type":"content_block_start","index":2,"content_block":{"type":"text","text":""}}`,
        `{"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"Hel"}}`,
        `{"type":"ping"}`,
        `{"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"lo"}}`,
        `{"type":"content_block_start","index":3,"content_block":{"type":"tool_use","id":"toolu_1","name":"add","input":{}}}`,
        `{"type":"content_block_delta","index":3,"delta":{"type":"input_json_delta","partial_json":"{\\"a\\": 1,"}}`,
        `{"type":"content_block_delta","index":3,"delta":{"type":"input_json_delta","partial_json":" \\"b\\": 2}"}}`,
        "not json at all",
        `{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":42}}`,
        `{"type":"message_stop"}`
    ];
    let assembling = anthropic.assembly();
    var pieces: [string] = [];
    for e in events {
        let piece = anthropic.absorb(assembling, e);
        if (piece != "") {
            pieces.push(piece);
        }
    }
    print("pieces: ${pieces.join("|")} done=${assembling.done} error=[${assembling.error}]");
    show("anthropic stream", anthropic.assembled(assembling));
    let broken = anthropic.assembly();
    let _ = anthropic.absorb(broken, `{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}`);
    print("error=[${broken.error}]");

    // ---- OpenAI: el cuerpo de la petición.
    var o = llm.for_openai("gpt-x", "k-oai");
    o.system = "Be brief.";
    o.max_tokens = 1000;
    print(config.chat_url(o));
    print(config.models_url(o));
    print(o.max_tokens_field);
    print(openai.build_body(o, tools, conversation(), false));
    var local = llm.for_endpoint("http://127.0.0.1:11434/v1/", "llama3.2", "");
    local.temperature = 0.2;
    local.effort = "low";
    print(config.chat_url(local));
    print(local.max_tokens_field);
    print(openai.build_body(local, none, [message.user("hi")], true));
    for (k, v) in openai.headers(o) {
        print("${k}: ${v}");
    }
    print(openai.headers(local).contains_key("Authorization"));

    // ---- OpenAI: leer una respuesta entera.
    show("openai", openai.parse_reply(
        `{"id":"c1","model":"gpt-x","choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":"Let me add.","tool_calls":[{"id":"call_9","type":"function","function":{"name":"add","arguments":"{\\"a\\":19,\\"b\\":23}"}}]}}],"usage":{"prompt_tokens":30,"completion_tokens":12,"prompt_tokens_details":{"cached_tokens":8}}}`
    ).unwrap());
    show("openai, no usage", openai.parse_reply(`{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"Hi."}}]}`).unwrap());
    print(openai.parse_reply(`{"error":{"message":"bad key"}}`));
    print(openai.parse_reply(`{"error":"plain"}`));
    print(openai.parse_reply(`{"choices":[]}`));

    // ---- OpenAI: ensamblar un flujo.
    let chunks = [
        `{"model":"gpt-x","choices":[{"index":0,"delta":{"role":"assistant","content":"Hel"}}]}`,
        `{"model":"gpt-x","choices":[{"index":0,"delta":{"content":"lo"}}]}`,
        `{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_9","type":"function","function":{"name":"add","arguments":"{\\"a\\":"}}]}}]}`,
        `{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":" 1}"}}]}}]}`,
        `{"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"name":"now"}}]}}]}`,
        `{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}`,
        `{"choices":[],"usage":{"prompt_tokens":25,"completion_tokens":7}}`,
        "[DONE]"
    ];
    let gathering = openai.assembly();
    var parts: [string] = [];
    for c in chunks {
        let piece = openai.absorb(gathering, c);
        if (piece != "") {
            parts.push(piece);
        }
    }
    print("pieces: ${parts.join("|")} done=${gathering.done} error=[${gathering.error}]");
    show("openai stream", openai.assembled(gathering));

    // ---- Presets, modelos y lo demás.
    var names: [string] = [];
    for p in config.presets() {
        names.push(p.id);
    }
    print(names.join(" "));
    let groq = llm.for_preset("groq", "m", "k").unwrap();
    print("${groq.dialect} ${config.chat_url(groq)}");
    print(llm.for_preset("nope", "m", "k").is_none());
    print(llm.parse_models(`{"data":[{"id":"a"},{"id":"b"}]}`));
    print(llm.parse_models(`{"models":[{"name":"local"}]}`));
    print(llm.parse_models("{}").is_err());
    print("${message.was_truncated("max_tokens")} ${message.was_truncated("length")} ${message.was_truncated("end_turn")}");
    print("${message.was_refused("refusal")} ${message.was_refused("content_filter")} ${message.was_refused("stop")}");
    var h: Map<string, string> = Map.new();
    h.insert("Retry-After", "2");
    print(llm.retry_after_ms(h, ""));
    let no_header: Map<string, string> = Map.new();
    print(llm.retry_after_ms(no_header, "Rate limit reached. Please try again in 9.882s."));
    print(llm.retry_after_ms(no_header, "try again in 7200s"));
    print(llm.retry_after_ms(no_header, "no hint here"));
    print("[${llm.error_message(`{"error":{"message":"bad key"}}`)}] [${llm.error_message(`{"error":"plain"}`)}] [${llm.error_message("<html>")}]");
    0
}
"##;

const DIALECTS_EXPECTED: &str = r##"true
https://api.anthropic.com/v1/messages
https://api.anthropic.com/v1/models
{"fallbacks":"default","max_tokens":1000,"messages":[{"content":[{"text":"What is 19 + 23?","type":"text"}],"role":"user"},{"content":[{"text":"Let me add.","type":"text"},{"id":"call_1","input":{"a":19,"b":23},"name":"add","type":"tool_use"},{"id":"call_2","input":{},"name":"now","type":"tool_use"}],"role":"assistant"},{"content":[{"content":"42","tool_use_id":"call_1","type":"tool_result"},{"content":"noon","tool_use_id":"call_2","type":"tool_result"}],"role":"user"},{"content":[{"text":"Thanks.","type":"text"}],"role":"user"}],"model":"claude-opus-5-5","output_config":{"effort":"medium"},"system":[{"cache_control":{"type":"ephemeral"},"text":"Be brief.","type":"text"}],"tools":[{"description":"Adds two integers","input_schema":{"properties":{"a":{"type":"integer"},"b":{"type":"integer"}},"type":"object"},"name":"add"},{"cache_control":{"type":"ephemeral"},"description":"The time","input_schema":{"properties":{},"type":"object"},"name":"now"}]}
{"fallbacks":"default","max_tokens":1000,"messages":[{"content":[{"text":"hi","type":"text"}],"role":"user"}],"model":"claude-opus-5-5","stream":true,"system":[{"text":"Be brief.","type":"text"}],"temperature":0.5}
Accept: application/json
Content-Type: application/json
anthropic-beta: server-side-fallback-2026-07-01
anthropic-version: 2023-06-01
x-api-key: k-ant
{"fallbacks":"default","max_tokens":1000,"messages":[{"content":[{"text":"hi","type":"text"}],"role":"user"},{"content":[{"signature":"s1","thinking":"hm","type":"thinking"},{"text":"Hi.","type":"text"}],"role":"assistant"}],"model":"claude-opus-5-5","system":[{"text":"Be brief.","type":"text"}],"temperature":0.5}
anthropic: stop=tool_use model=claude-opus-5-5 text=[Let me add.]
  call toolu_1 add {"a":19,"b":23}
  usage in=30 out=12 cached=20 measured=true
  raw [{"signature":"s1","thinking":"hm","type":"thinking"},{"text":"Let me add.","type":"text"},{"id":"toolu_1","input":{"a":19,"b":23},"name":"add","type":"tool_use"}]
Result.Err(provider error: Overloaded)
Result.Err(malformed response: no 'content' array)
true
pieces: Hel|lo done=true error=[]
anthropic stream: stop=tool_use model=claude-opus-5-5 text=[Hello]
  call toolu_1 add {"a": 1, "b": 2}
  usage in=25 out=42 cached=10 measured=true
  raw [{"signature":"sig123","thinking":"Let me think.","type":"thinking"},{"data":"opaque","type":"redacted_thinking"},{"text":"Hello","type":"text"},{"id":"toolu_1","input":{"a":1,"b":2},"name":"add","type":"tool_use"}]
error=[Overloaded]
https://api.openai.com/v1/chat/completions
https://api.openai.com/v1/models
max_completion_tokens
{"max_completion_tokens":1000,"messages":[{"content":"Be brief.","role":"system"},{"content":"What is 19 + 23?","role":"user"},{"content":"Let me add.","role":"assistant","tool_calls":[{"function":{"arguments":"{\"a\": 19, \"b\": 23}","name":"add"},"id":"call_1","type":"function"},{"function":{"arguments":"{}","name":"now"},"id":"call_2","type":"function"}]},{"content":"42","role":"tool","tool_call_id":"call_1"},{"content":"noon","role":"tool","tool_call_id":"call_2"},{"content":"Thanks.","role":"user"}],"model":"gpt-x","stream":false,"tool_choice":"auto","tools":[{"function":{"description":"Adds two integers","name":"add","parameters":{"properties":{"a":{"type":"integer"},"b":{"type":"integer"}},"type":"object"}},"type":"function"},{"function":{"description":"The time","name":"now","parameters":{"properties":{},"type":"object"}},"type":"function"}]}
http://127.0.0.1:11434/v1/chat/completions
max_tokens
{"max_tokens":4096,"messages":[{"content":"hi","role":"user"}],"model":"llama3.2","reasoning_effort":"low","stream":true,"stream_options":{"include_usage":true},"temperature":0.2}
Accept: application/json
Authorization: Bearer k-oai
Content-Type: application/json
false
openai: stop=tool_calls model=gpt-x text=[Let me add.]
  call call_9 add {"a":19,"b":23}
  usage in=30 out=12 cached=8 measured=true
openai, no usage: stop=stop model= text=[Hi.]
  usage in=0 out=0 cached=0 measured=false
Result.Err(provider error: bad key)
Result.Err(provider error: plain)
Result.Err(malformed response: empty 'choices')
pieces: Hel|lo done=true error=[]
openai stream: stop=tool_calls model=gpt-x text=[Hello]
  call call_9 add {"a": 1}
  call call_1 now {}
  usage in=25 out=7 cached=0 measured=true
anthropic openai openrouter groq deepseek mistral xai gemini ollama lmstudio
openai https://api.groq.com/openai/v1/chat/completions
true
Result.Ok([a, b])
Result.Ok([local])
true
true true false
true true false
2500
10382
60000
0
[bad key] [plain] []
"##;

#[test]
fn both_dialects_build_requests_and_read_replies() {
    let app = project("dialects", DIALECTS_MAIN);
    let (stdout, stderr, code) = run(&app);
    assert_eq!(code, 0, "el programa sale 0\n{stdout}\n{stderr}");
    assert_eq!(stdout, DIALECTS_EXPECTED, "la salida esperada\n{stderr}");
}

const CONVERSATION_MAIN: &str = r##"import std/json;
import std/net;
import net/webserver;
import llm/llm;
import llm/config;
from std/json import Json;
from net/webserver import Request, Response;
from llm/config import Config;
from llm/message import Message, Tool;

fn json_status(status: int, body: string) -> Response {
    let r = webserver.json_response(body);
    r.status = status;
    r
}

fn event_stream(body: string) -> Response {
    let r = webserver.text(200, body);
    r.headers.insert("Content-Type", "text/event-stream");
    r
}

// El lado Anthropic del proveedor simulado: pide la herramienta `add` y, cuando le llega su
// resultado, contesta. `busy` es un canal con una ficha: quien la saca recibe un 429.
fn anthropic_side(req: Request, body: string, busy: Channel<int>) -> Response {
    if (req.headers.get("x-api-key").unwrap_or("") != "k-ant") {
        return json_status(401, `{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}`);
    }
    match (try_recv(busy)) {
        Received.Got(_) => {
            print("server: 429, retry after 1 s");
            let r = json_status(429, `{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}`);
            r.headers.insert("Retry-After", "1");
            return r;
        },
        _ => { },
    }
    let streaming = body.contains(`"stream":true`);
    if (body.contains("tool_result")) {
        if (streaming) {
            return event_stream(`event: message_start
data: {"type":"message_start","message":{"model":"claude-test","usage":{"input_tokens":40,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"The sum "}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"is 42."}}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":9}}

event: message_stop
data: {"type":"message_stop"}

`);
        }
        return webserver.json_response(
            `{"type":"message","model":"claude-test","stop_reason":"end_turn","content":[{"type":"text","text":"The sum is 42."}],"usage":{"input_tokens":40,"output_tokens":9}}`
        );
    }
    if (streaming) {
        return event_stream(`event: message_start
data: {"type":"message_start","message":{"model":"claude-test","usage":{"input_tokens":20,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Let me add."}}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"add","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\\"a\\": 19, \\"b\\": 23}"}}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":15}}

event: message_stop
data: {"type":"message_stop"}

`);
    }
    webserver.json_response(
        `{"type":"message","model":"claude-test","stop_reason":"tool_use","content":[{"type":"text","text":"Let me add."},{"type":"tool_use","id":"toolu_1","name":"add","input":{"a":19,"b":23}}],"usage":{"input_tokens":20,"output_tokens":15}}`
    )
}

// El lado OpenAI: lo mismo en su dialecto, y rechaza `max_tokens` como hacen sus modelos
// nuevos.
fn openai_side(req: Request, body: string) -> Response {
    if (req.headers.get("authorization").unwrap_or("") != "Bearer k-oai") {
        return json_status(401, `{"error":{"message":"Incorrect API key provided"}}`);
    }
    if (body.contains(`"max_tokens"`)) {
        print("server: 400, max_tokens is not accepted");
        return json_status(
            400,
            `{"error":{"message":"Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead."}}`
        );
    }
    let streaming = body.contains(`"stream":true`);
    if (body.contains(`"role":"tool"`)) {
        if (streaming) {
            return event_stream(`data: {"model":"gpt-test","choices":[{"index":0,"delta":{"role":"assistant","content":"The sum "}}]}

data: {"model":"gpt-test","choices":[{"index":0,"delta":{"content":"is 42."}}]}

data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: {"choices":[],"usage":{"prompt_tokens":40,"completion_tokens":9}}

data: [DONE]

`);
        }
        return webserver.json_response(
            `{"model":"gpt-test","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"The sum is 42."}}],"usage":{"prompt_tokens":40,"completion_tokens":9}}`
        );
    }
    if (streaming) {
        return event_stream(`data: {"model":"gpt-test","choices":[{"index":0,"delta":{"role":"assistant","content":"Let me add."}}]}

data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"add","arguments":"{\\"a\\": 19,"}}]}}]}

data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":" \\"b\\": 23}"}}]}}]}

data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}

data: {"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":15}}

data: [DONE]

`);
    }
    webserver.json_response(
        `{"model":"gpt-test","choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":"Let me add.","tool_calls":[{"id":"call_1","type":"function","function":{"name":"add","arguments":"{\\"a\\": 19, \\"b\\": 23}"}}]}}],"usage":{"prompt_tokens":20,"completion_tokens":15}}`
    )
}

fn provider(req: Request, busy: Channel<int>) -> Response {
    let body = webserver.request_text(req).unwrap_or("");
    if (req.path == "/ant/v1/messages") {
        return anthropic_side(req, body, busy);
    }
    if (req.path == "/oai/v1/chat/completions") {
        return openai_side(req, body);
    }
    if (req.path == "/oai/v1/models") {
        return webserver.json_response(`{"data":[{"id":"gpt-test"},{"id":"gpt-mini"}]}`);
    }
    if (req.path == "/down/v1/chat/completions") {
        print("server: 503");
        return json_status(503, `{"error":{"message":"The engine is currently overloaded"}}`);
    }
    webserver.text(404, "no such endpoint")
}

// El bucle de quien usa el paquete: preguntar, ejecutar las herramientas que pida el modelo,
// devolverle los resultados y volver a preguntar.
fn converse(label: string, c: Config, tools: [Tool], streaming: bool) {
    print("== ${label}");
    var history = [llm.user("What is 19 + 23?")];
    var step = 0;
    while (step < 4) {
        step = step + 1;
        let outcome = if (streaming) {
            llm.send_stream(c, tools, history, fn(piece: string) {
                print("  piece [${piece}]");
            })
        } else {
            llm.send(c, tools, history)
        };
        let reply = match (outcome) {
            Result.Ok(r) => r,
            Result.Err(e) => {
                print("failed: ${e}");
                return;
            },
        };
        print("${reply.stop_reason} | ${reply.model} | ${reply.message.text} | in=${reply.usage.input_tokens} out=${reply.usage.output_tokens}");
        history.push(reply.message);
        let calls = llm.tool_calls(reply);
        if (calls.len() == 0) {
            return;
        }
        for call in calls {
            print("  call ${call.id} ${call.name} ${call.arguments}");
            let args = json.parse(call.arguments).unwrap_or(Json.JNull);
            let sum = json.get_int(args, "a").unwrap_or(0) + json.get_int(args, "b").unwrap_or(0);
            history.push(llm.tool_result(call.id, to_string(sum)));
        }
    }
}

fn main() -> int {
    let listener = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(listener);
    let busy: Channel<int> = Channel.new();
    send(busy, 1);
    spawn(fn() {
        let _ = webserver.serve_on(listener, fn(req: Request) -> Response { provider(req, busy) });
    });
    let base = "http://127.0.0.1:${port}";
    let tools = [llm.tool("add", "Adds two integers", `{"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}}}`).unwrap()];

    var a = llm.for_anthropic("claude-test", "k-ant");
    a.base_url = base + "/ant";
    // La primera petición cae en el 429: se espera lo que pide el servidor y se reintenta.
    converse("anthropic", a, tools, false);
    converse("anthropic, streamed", a, tools, true);

    // Un endpoint compatible: se le habla con `max_tokens`, lo rechaza, y la petición se
    // corrige sola.
    let o = llm.for_endpoint(base + "/oai/v1", "gpt-test", "k-oai");
    print(o.max_tokens_field);
    converse("openai dialect", o, tools, false);
    print(o.max_tokens_field);
    converse("openai dialect, streamed", o, tools, true);

    print("== failures");
    let wrong = llm.for_endpoint(base + "/oai/v1", "gpt-test", "nope");
    match (llm.send(wrong, tools, [llm.user("hi")])) {
        Result.Ok(_) => print("unexpected"),
        Result.Err(e) => print(e.replace(":${port}", ":PORT")),
    }
    var bad_key = llm.for_anthropic("claude-test", "nope");
    bad_key.base_url = base + "/ant";
    match (llm.send_stream(bad_key, tools, [llm.user("hi")], fn(piece: string) { })) {
        Result.Ok(_) => print("unexpected"),
        Result.Err(e) => print(e.replace(":${port}", ":PORT")),
    }
    // Un 503 se reintenta; al agotar los intentos, el error dice cuántos fueron.
    var down = llm.for_endpoint(base + "/down/v1", "gpt-test", "k-oai");
    down.max_attempts = 2;
    match (llm.send(down, tools, [llm.user("hi")])) {
        Result.Ok(_) => print("unexpected"),
        Result.Err(e) => print(e.replace(":${port}", ":PORT")),
    }
    print("== models");
    print(llm.models(o));
    0
}
"##;

const CONVERSATION_EXPECTED: &str = r##"== anthropic
server: 429, retry after 1 s
tool_use | claude-test | Let me add. | in=20 out=15
  call toolu_1 add {"a":19,"b":23}
end_turn | claude-test | The sum is 42. | in=40 out=9
== anthropic, streamed
  piece [Let me add.]
tool_use | claude-test | Let me add. | in=20 out=15
  call toolu_1 add {"a": 19, "b": 23}
  piece [The sum ]
  piece [is 42.]
end_turn | claude-test | The sum is 42. | in=40 out=9
max_tokens
== openai dialect
server: 400, max_tokens is not accepted
tool_calls | gpt-test | Let me add. | in=20 out=15
  call call_1 add {"a": 19, "b": 23}
stop | gpt-test | The sum is 42. | in=40 out=9
max_completion_tokens
== openai dialect, streamed
  piece [Let me add.]
tool_calls | gpt-test | Let me add. | in=20 out=15
  call call_1 add {"a": 19, "b": 23}
  piece [The sum ]
  piece [is 42.]
stop | gpt-test | The sum is 42. | in=40 out=9
== failures
HTTP 401 from http://127.0.0.1:PORT/oai/v1/chat/completions: Incorrect API key provided
HTTP 401 from http://127.0.0.1:PORT/ant/v1/messages: invalid x-api-key
server: 503
server: 503
HTTP 503 from http://127.0.0.1:PORT/down/v1/chat/completions: The engine is currently overloaded (after 2 attempts)
== models
Result.Ok([gpt-test, gpt-mini])
"##;

#[test]
fn conversation_with_tools_against_a_fake_provider() {
    let app = project("conversation", CONVERSATION_MAIN);
    let (stdout, stderr, code) = run(&app);
    assert_eq!(code, 0, "el programa sale 0\n{stdout}\n{stderr}");
    assert_eq!(stdout, CONVERSATION_EXPECTED, "la salida esperada\n{stderr}");
}
