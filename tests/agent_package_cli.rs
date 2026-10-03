//! `packages/agent`: el bucle de un agente. De punta a punta, sin red externa ni clave, contra
//! un proveedor simulado dentro del propio programa de prueba (dialecto de OpenAI, en un puerto
//! efímero) que obedece un guion escrito en la pregunta: qué herramientas pedir, repetir sin
//! fin, negarse, cortarse o tardar.
//!
//! Cubre: el registro de herramientas, los tres riesgos frente a los tres niveles de autonomía,
//! la aprobación (sí, siempre, no, y nadie a quien preguntar), lo que puede salir mal dentro
//! de una herramienta, dos llamadas en una respuesta, el presupuesto de pasos y el historial
//! que deja, los otros finales, la nota de autonomía en el prompt, la respuesta por partes, la
//! cancelación, y las herramientas de un servidor MCP real (`ray mcp`).
//!
//! Corre sobre la VM, como los demás tests de paquetes.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// Un proyecto temporal que depende de `packages/agent` por ruta, con `main` como programa.
fn project(tag: &str, main: &str) -> std::path::PathBuf {
    let package = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/agent");
    let app = std::env::temp_dir().join(format!("ray_agent_package_{tag}")).join("app");
    let _ = std::fs::remove_dir_all(&app);
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nagent = \"path:{}\"\n",
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

const LOOP_MAIN: &str = r##"import std/json;
import std/net;
import std/time;
import net/webserver;
import agent/agent;
import llm/llm;
import mcp/mcp;
from std/json import Json;
from net/webserver import Request, Response;
from agent/agent import Agent, Decision, Event;
from llm/message import ToolCall;

// ---------------------------------------------------------------- el proveedor simulado
//
// Habla el dialecto de OpenAI y obedece un guion escrito en la pregunta del usuario:
//   call <tool> <json>   pide esa herramienta (una línea por llamada)
//   forever              tras los resultados, vuelve a pedir lo mismo
//   echo system          contesta con el prompt de sistema que recibió
//   refuse / truncate    termina con ese motivo
//   slow                 tarda 1,5 s en contestar
// Sin nada de eso contesta "plain answer"; tras unos resultados, los repite.

fn text_of(m: Json) -> string {
    json.get_string(m, "content").unwrap_or("")
}

fn role_of(m: Json) -> string {
    json.get_string(m, "role").unwrap_or("")
}

fn quoted(s: string) -> string {
    json.stringify(Json.JStr(s))
}

fn usage_json() -> string {
    `{"prompt_tokens":10,"completion_tokens":5}`
}

// Las llamadas que pide un texto: una por cada línea `call <tool> <json>`.
fn directives(prompt: string) -> [(string, string)] {
    var out: [(string, string)] = [];
    for line in prompt.split("\n") {
        if (line.starts_with("call ")) {
            let rest = line.substring(5, line.len());
            match (rest.index_of(" ")) {
                Option.Some(i) => out.push((rest.substring(0, i), rest.substring(i + 1, rest.len()))),
                Option.None => out.push((rest, "")),
            }
        }
    }
    out
}

fn say(reply: string, reason: string, streaming: bool) -> Response {
    if (streaming) {
        let half = reply.len() / 2;
        let body = `data: {"model":"fake","choices":[{"index":0,"delta":{"role":"assistant","content":${quoted(reply.substring(0, half))}}}]}

data: {"choices":[{"index":0,"delta":{"content":${quoted(reply.substring(half, reply.len()))}}}]}

data: {"choices":[{"index":0,"delta":{},"finish_reason":${quoted(reason)}}]}

data: {"choices":[],"usage":${usage_json()}}

data: [DONE]

`;
        let r = webserver.text(200, body);
        r.headers.insert("Content-Type", "text/event-stream");
        return r;
    }
    webserver.json_response(
        `{"model":"fake","choices":[{"index":0,"finish_reason":${quoted(reason)},"message":{"role":"assistant","content":${quoted(reply)}}}],"usage":${usage_json()}}`
    )
}

fn ask_for(calls: [(string, string)], streaming: bool) -> Response {
    var whole: [string] = [];
    var events: [string] = [];
    var i = 0;
    for call in calls {
        let id = "call_${i + 1}";
        whole.push(
            `{"id":${quoted(id)},"type":"function","function":{"name":${quoted(call.0)},"arguments":${quoted(call.1)}}}`
        );
        events.push(
            `data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":${i},"id":${quoted(id)},"type":"function","function":{"name":${quoted(call.0)},"arguments":${quoted(call.1)}}}]}}]}`
        );
        i = i + 1;
    }
    if (streaming) {
        let body = events.join("\n\n") + `

data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}

data: {"choices":[],"usage":${usage_json()}}

data: [DONE]

`;
        let r = webserver.text(200, body);
        r.headers.insert("Content-Type", "text/event-stream");
        return r;
    }
    webserver.json_response(
        `{"model":"fake","choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":"","tool_calls":[${whole.join(",")}]}}],"usage":${usage_json()}}`
    )
}

fn provider(req: Request) -> Response {
    let body = json.parse(webserver.request_text(req).unwrap_or("")).unwrap_or(Json.JNull);
    let streaming = json.get_bool(body, "stream").unwrap_or(false);
    let messages = json.get_array(body, "messages").unwrap_or([]);
    var system = "";
    var prompt = "";
    var results: [string] = [];
    for m in messages {
        let role = role_of(m);
        if (role == "system") {
            system = text_of(m);
        }
        if (role == "user") {
            prompt = text_of(m);
            results = [];
        }
        if (role == "tool") {
            results.push(text_of(m));
        }
    }
    let last = if (messages.len() == 0) { "" } else { role_of(messages[messages.len() - 1]) };
    if (prompt.contains("slow")) {
        time.sleep(1500);
    }
    if (last == "tool" && !prompt.contains("forever")) {
        return say("results: " + results.join(" | "), "stop", streaming);
    }
    if (prompt.contains("echo system")) {
        return say(system, "stop", streaming);
    }
    if (prompt.contains("refuse")) {
        return say("", "content_filter", streaming);
    }
    if (prompt.contains("truncate")) {
        return say("cut", "length", streaming);
    }
    let calls = directives(prompt);
    if (calls.len() > 0) {
        return ask_for(calls, streaming);
    }
    say("plain answer", "stop", streaming)
}

// ---------------------------------------------------------------- el agente a prueba

fn show(label: string, outcome: Result<agent.Outcome, string>) {
    match (outcome) {
        Result.Ok(o) => print("${label}: ${o.stop} in ${o.steps} step(s): ${o.text}"),
        Result.Err(e) => print("${label}: FAILED ${e}"),
    }
}

fn describe(e: Event) -> string {
    match (e) {
        Event.Thinking(step) => "thinking ${step}",
        Event.Text(piece) => "text [${piece}]",
        Event.Said(said) => "said [${said}]",
        Event.Calling(call, risk) => "calling ${call.name} (${risk}) ${call.arguments}",
        Event.Returned(call, output, failed) => "returned ${call.name} failed=${failed} [${output}]",
        Event.Declined(call, why) => "declined ${call.name}: ${why}",
    }
}

// Las herramientas de la prueba, una de cada riesgo, y tres que salen mal de formas distintas.
fn tooled(base: string) -> Agent {
    let a = agent.new(llm.for_endpoint(base, "fake", ""));
    a.config.system = "You are a test agent.";
    agent.tool(a, "read_note", "Reads a note", `{"type":"object","properties":{"id":{"type":"integer"}}}`, agent.READ, fn(args: Json) -> Result<string, string> {
        Result.Ok("note ${json.get_int(args, "id").unwrap_or(0)}")
    });
    agent.tool(a, "write_note", "Writes a note", "", agent.WRITE, fn(args: Json) -> Result<string, string> {
        Result.Ok("written")
    });
    agent.tool(a, "run_command", "Runs a command", "", agent.EXEC, fn(args: Json) -> Result<string, string> {
        Result.Ok("ran")
    });
    agent.tool(a, "fails", "Always fails", "", agent.READ, fn(args: Json) -> Result<string, string> {
        Result.Err("disk on fire")
    });
    agent.tool(a, "crashes", "Always aborts", "", agent.READ, fn(args: Json) -> Result<string, string> {
        panic("boom");
        Result.Ok("unreachable")
    });
    a
}

fn roles(a: Agent) -> string {
    var out: [string] = [];
    for m in a.history {
        out.push(if (m.role == "assistant" && m.tool_calls.len() > 0) { "assistant+${m.tool_calls.len()}" } else { m.role });
    }
    out.join(" ")
}

fn main() -> int {
    let listener = net.tcp_listen("127.0.0.1", 0).unwrap();
    let base = "http://127.0.0.1:${net.local_port(listener)}/v1";
    spawn(fn() {
        let _ = webserver.serve_on(listener, provider);
    });

    print("== registering tools");
    let a = tooled(base);
    // Un nombre repetido, un riesgo desconocido o un esquema que no es JSON abortan el programa
    // al arrancar: son errores de programación.
    print(try_call(fn() { agent.tool(a, "read_note", "again", "", agent.READ, fn(args: Json) -> Result<string, string> { Result.Ok("") }) }));
    print(try_call(fn() { agent.tool(a, "odd", "bad risk", "", "maybe", fn(args: Json) -> Result<string, string> { Result.Ok("") }) }));
    print(try_call(fn() { agent.tool(a, "bad", "bad schema", "{nope", agent.READ, fn(args: Json) -> Result<string, string> { Result.Ok("") }) }));
    var names: [string] = [];
    for t in agent.offered(a) {
        names.push("${t.name}:${agent.risk_of(a, t.name)}");
    }
    print(names.join(" "));
    print("${agent.unattended(agent.ASK, agent.READ)} ${agent.unattended(agent.ASK, agent.WRITE)} ${agent.unattended(agent.EDITS, agent.WRITE)} ${agent.unattended(agent.EDITS, agent.EXEC)} ${agent.unattended(agent.AUTO, agent.EXEC)}");

    print("== a plain turn, with events");
    agent.on_event(a, fn(e: Event) {
        print("  " + describe(e));
    });
    show("plain", agent.run(a, "hello"));

    print("== a read tool runs unasked");
    show("read", agent.run(a, `call read_note {"id": 7}`));

    print("== at ASK, a write tool needs a yes and there is nobody to ask");
    show("nobody", agent.run(a, "call write_note {}"));

    print("== with someone to ask: yes, always, no");
    // Las respuestas del usuario, por orden; lo que se le preguntó queda anotado.
    let answers: Channel<Decision> = Channel.new();
    agent.on_approve(a, fn(call: ToolCall, risk: string) -> Decision {
        print("  asked about ${call.name} (${risk})");
        recv(answers).unwrap_or(Decision.No)
    });
    send(answers, Decision.Yes);
    show("yes", agent.run(a, "call write_note {}"));
    send(answers, Decision.Always);
    show("always", agent.run(a, "call write_note {}"));
    show("not asked again", agent.run(a, "call write_note {}"));
    send(answers, Decision.No);
    show("no", agent.run(a, "call run_command {}"));
    print("allowed: ${a.allowed.join(" ")}");

    print("== autonomy levels");
    agent.on_event(a, fn(e: Event) { });
    a.allowed = [];
    a.autonomy = agent.EDITS;
    show("edits, write", agent.run(a, "call write_note {}"));
    send(answers, Decision.No);
    show("edits, exec", agent.run(a, "call run_command {}"));
    a.autonomy = agent.AUTO;
    show("auto, exec", agent.run(a, "call run_command {}"));

    print("== what can go wrong inside a tool");
    show("err", agent.run(a, "call fails {}"));
    show("panic", agent.run(a, "call crashes {}"));
    show("unknown", agent.run(a, "call no_such_tool {}"));
    show("bad arguments", agent.run(a, "call read_note {oops"));

    print("== two calls in one reply");
    show("two", agent.run(a, "call read_note {\"id\": 1}\ncall read_note {\"id\": 2}"));

    print("== the step budget");
    agent.reset(a);
    a.max_steps = 3;
    show("forever", agent.run(a, "forever\ncall read_note {\"id\": 1}"));
    print(roles(a));
    a.max_steps = 20;

    print("== other endings");
    show("refuse", agent.run(a, "refuse"));
    show("truncate", agent.run(a, "truncate"));

    print("== the system prompt carries the autonomy note");
    a.autonomy = agent.ASK;
    show("ask", agent.run(a, "echo system"));
    a.autonomy = agent.AUTO;
    print(agent.run(a, "echo system").unwrap().text.contains("without waiting for the user"));
    print(a.config.system);

    print("== streamed");
    agent.reset(a);
    a.stream = true;
    agent.on_event(a, fn(e: Event) {
        print("  " + describe(e));
    });
    show("streamed", agent.run(a, `call read_note {"id": 3}`));
    agent.on_event(a, fn(e: Event) { });

    print("== cancellable");
    let cancel: Channel<int> = Channel.new();
    show("not cancelled", agent.run_cancellable(a, "hello", cancel));
    send(cancel, 1);
    let before = a.history.len();
    show("cancelled", agent.run_cancellable(a, "slow", cancel));
    print("history grew by ${a.history.len() - before}");
    a.stream = false;

    print("== the tools of an MCP server");
    let session = mcp.connect(mcp.stdio_server("ray", env("RAY_BIN").unwrap(), ["mcp"])).unwrap();
    let b = agent.new(llm.for_endpoint(base, "fake", ""));
    print(agent.connect(b, "ray", session));
    print("${agent.risk_of(b, "mcp__ray__ray_doc")} ${agent.risk_of(b, "mcp__ray__ray_run")}");
    print(agent.connect(b, "ray", session).is_err());
    print(agent.run(b, `call mcp__ray__ray_doc {"symbol": "len"}`).unwrap().text.contains("len"));
    show("mcp exec", agent.run(b, `call mcp__ray__ray_run {"code": "fn main() { print(6 * 7); }"}`));
    b.autonomy = agent.AUTO;
    print(agent.run(b, `call mcp__ray__ray_run {"code": "fn main() { print(6 * 7); }"}`).unwrap().text.contains("42"));
    print(agent.run(b, "echo system").unwrap().text.contains("Instructions from the 'ray' tools:"));
    mcp.close(session);

    print("== usage adds up");
    print("${a.usage.input_tokens > 0} ${a.usage.output_tokens > 0} ${a.usage.measured}");
    0
}
"##;

const LOOP_EXPECTED: &str = r##"== registering tools
Result.Err(agent: tool 'read_note' is already registered)
Result.Err(agent: the risk of tool 'odd' must be read, write or exec, not 'maybe')
Result.Err(llm: the schema of tool 'bad' is not valid JSON: expected a string key)
read_note:read write_note:write run_command:exec fails:read crashes:read
true false true false true
== a plain turn, with events
  thinking 1
  said [plain answer]
plain: done in 1 step(s): plain answer
== a read tool runs unasked
  thinking 1
  calling read_note (read) {"id": 7}
  returned read_note failed=false [note 7]
  thinking 2
  said [results: note 7]
read: done in 2 step(s): results: note 7
== at ASK, a write tool needs a yes and there is nobody to ask
  thinking 1
  declined write_note: not run: this tool needs the user's approval and there is nobody to ask here.
  thinking 2
  said [results: not run: this tool needs the user's approval and there is nobody to ask here.]
nobody: done in 2 step(s): results: not run: this tool needs the user's approval and there is nobody to ask here.
== with someone to ask: yes, always, no
  thinking 1
  asked about write_note (write)
  calling write_note (write) {}
  returned write_note failed=false [written]
  thinking 2
  said [results: written]
yes: done in 2 step(s): results: written
  thinking 1
  asked about write_note (write)
  calling write_note (write) {}
  returned write_note failed=false [written]
  thinking 2
  said [results: written]
always: done in 2 step(s): results: written
  thinking 1
  calling write_note (write) {}
  returned write_note failed=false [written]
  thinking 2
  said [results: written]
not asked again: done in 2 step(s): results: written
  thinking 1
  asked about run_command (exec)
  declined run_command: the user did not allow this call. Do not try it again; say what you needed it for.
  thinking 2
  said [results: the user did not allow this call. Do not try it again; say what you needed it for.]
no: done in 2 step(s): results: the user did not allow this call. Do not try it again; say what you needed it for.
allowed: write_note
== autonomy levels
edits, write: done in 2 step(s): results: written
  asked about run_command (exec)
edits, exec: done in 2 step(s): results: the user did not allow this call. Do not try it again; say what you needed it for.
auto, exec: done in 2 step(s): results: ran
== what can go wrong inside a tool
err: done in 2 step(s): results: error: disk on fire
panic: done in 2 step(s): results: error: the tool failed: boom
unknown: done in 2 step(s): results: error: there is no tool called 'no_such_tool'
bad arguments: done in 2 step(s): results: error: the arguments are not valid JSON: expected a string key
== two calls in one reply
two: done in 2 step(s): results: note 1 | note 2
== the step budget
forever: max_steps in 3 step(s): 
user assistant+1 tool assistant+1 tool assistant+1 tool
== other endings
refuse: refused in 1 step(s): 
truncate: truncated in 1 step(s): cut
== the system prompt carries the autonomy note
ask: done in 1 step(s): You are a test agent.
true
You are a test agent.
== streamed
  thinking 1
  calling read_note (read) {"id": 3}
  returned read_note failed=false [note 3]
  thinking 2
  text [results]
  text [: note 3]
  said [results: note 3]
streamed: done in 2 step(s): results: note 3
== cancellable
not cancelled: done in 1 step(s): plain answer
cancelled: cancelled in 1 step(s): 
history grew by 0
== the tools of an MCP server
Result.Ok(5)
read exec
true
true
mcp exec: done in 2 step(s): results: not run: this tool needs the user's approval and there is nobody to ask here.
true
true
== usage adds up
true true true
"##;

#[test]
fn the_loop_runs_tools_within_its_rules() {
    let app = project("loop", LOOP_MAIN);
    let (stdout, stderr, code) = run(&app);
    assert_eq!(code, 0, "el programa sale 0\n{stdout}\n{stderr}");
    assert_eq!(stdout, LOOP_EXPECTED, "la salida esperada\n{stderr}");
}
