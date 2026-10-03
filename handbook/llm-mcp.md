# LLM y MCP

Español · [English](llm-mcp.en.md)

Este capítulo tiene tres partes. La primera es **usar un asistente LLM para escribir raylang**:
qué darle para que el código que escribe compile. La segunda es **escribir un agente en
raylang**: un programa de terminal que habla con un modelo, usa herramientas y pregunta antes de
cambiar nada. La tercera es **ofrecer tus propias herramientas** a un asistente, con un servidor
MCP.

Las dos últimas se apoyan en tres paquetes: `llm` habla con el modelo, `mcp` con los servidores
de herramientas, y `agent` es el bucle que los une. Los proyectos están en
[`examples/apps/agent-cli`](../examples/apps/agent-cli/) y
[`examples/apps/notes-mcp`](../examples/apps/notes-mcp/), con sus tests. Los bloques de raylang
están copiados de ellos y el CI comprueba que sigan siéndolo.

```sh
agent-cli "escribe una función que invierta las palabras de un string"
```

```text
[tool] mcp__ray__ray_check
[tool] mcp__ray__ray_run
Aquí está, comprobada y ejecutada: …

? run write_file (write)? [y]es / [a]lways / [N]o
```

## 1. Un asistente que escribe raylang correcto

raylang es nuevo, y un modelo no lo conoce tan bien como Python o Rust. Dos piezas lo compensan.

**`llms.txt`.** Es el contexto destilado del lenguaje: en qué se diferencia de Rust, las formas
canónicas y los mensajes de error exactos. Está en la raíz del repositorio y `ray mcp` lo ofrece
como el recurso `raylang://llms.txt`. Basta una línea en el `CLAUDE.md` del proyecto:

```markdown
# CLAUDE.md
Este proyecto está escrito en raylang. Antes de escribir código, lee el recurso
raylang://llms.txt del servidor raylang, y compila y ejecuta todo con ray_check y ray_run.
```

**`ray mcp`.** Es un servidor [MCP](https://modelcontextprotocol.io) que viene en el binario `ray`.
Le da al asistente cinco herramientas:

| Herramienta | Para qué |
|---|---|
| `ray_check` | compila y devuelve los diagnósticos exactos |
| `ray_run` | ejecuta y devuelve la salida y el código de salida |
| `ray_test` | corre las funciones `@test` |
| `ray_fmt` | da el formato canónico |
| `ray_doc` | la firma y la documentación de cualquier función o tipo |

Con Claude Code se conecta con un comando:

```sh
claude mcp add raylang -- ray mcp
```

Desde ese momento el asistente escribe, compila, lee el error y corrige sin que copies nada a
mano. En un proyecto, las herramientas deben recibir **`path`** (el archivo o la carpeta del
proyecto) en lugar del código suelto: así los imports entre archivos y las dependencias del
`ray.toml` resuelven igual que con `ray run`. Las instrucciones del propio servidor se lo dicen al
modelo. El detalle está en [docs/mcp.md](../docs/mcp.md).

Otros clientes MCP, como Claude Desktop o un editor, se configuran con un archivo JSON que lanza
la misma orden:

```json
{ "mcpServers": { "raylang": { "command": "ray", "args": ["mcp"] } } }
```

Con las herramientas conectadas, tres costumbres dan mejores resultados:

- **Pide un proyecto, no un fragmento.** Con un `ray.toml` y un `src/`, el asistente comprueba el
  programa entero, con sus módulos y dependencias, y no un trozo aislado.
- **Que consulte antes de suponer.** `ray_doc` devuelve la firma real de cualquier función de la
  biblioteca estándar o de un paquete del proyecto. Un asistente que la consulta no inventa
  funciones que no existen.
- **Que termine con los tests.** `ray_check` dice que compila; `ray_test` dice que hace lo que
  debe.

## 2. Un agente escrito en raylang

Un agente es un bucle: el modelo pide una herramienta, el programa la ejecuta, le devuelve el
resultado y repite hasta que el modelo contesta. `agent-cli` recibe una pregunta en la terminal;
el modelo escribe raylang, lo compila y lo ejecuta con las herramientas de `ray mcp`, y puede
guardarlo en un archivo cuando tú lo permitas.

```sh
ray new agent-cli && cd agent-cli
ray add agent
ray add llm
ray add mcp
```

| Paquete | Qué pone |
|---|---|
| `llm` | la conversación con el modelo: Claude, OpenAI o cualquier servidor que hable su dialecto |
| `mcp` | las herramientas de un servidor MCP, y el lado servidor de la sección 9 |
| `agent` | el bucle, el presupuesto de pasos, y qué corre solo y qué pregunta |

Con los tres, el programa entero cabe en tres módulos cortos:

| Módulo | Qué hace |
|---|---|
| `model.ray` | con qué modelo se habla, según el entorno |
| `files.ray` | las herramientas propias: leer y escribir archivos |
| `main.ray` | monta el agente, pregunta al usuario y muestra lo que pasa |

## 3. El modelo

El proveedor, el modelo y la clave salen del entorno, así que el mismo binario sirve contra
Claude, contra OpenAI o contra un modelo en tu máquina:

<!-- check: project=examples/apps/agent-cli -->
```rust
/// The configuration from the environment: AGENT_PROVIDER (a known provider; "anthropic" by
/// default), AGENT_MODEL, and AGENT_BASE_URL for an endpoint of your own.
pub fn from_env() -> Result<Config, string> {
    let provider = env("AGENT_PROVIDER").unwrap_or("anthropic");
    let preset = match (config.find_preset(provider)) {
        Option.Some(p) => p,
        Option.None => {
            return Result.Err("unknown AGENT_PROVIDER '${provider}'");
        },
    };
    let model = env("AGENT_MODEL").unwrap_or(default_model(provider));
    if (model == "") {
        return Result.Err("set AGENT_MODEL: '${provider}' has no default model");
    }
    let base_url = env("AGENT_BASE_URL").unwrap_or(preset.base_url);
    var c = config.new_config(preset.dialect, base_url, model, api_key(preset)?);
    c.max_tokens = 8000;
    Result.Ok(c)
}
```

`config.find_preset` conoce a los proveedores por su nombre, y cada uno habla uno de dos
dialectos: el de Anthropic o el de OpenAI.

| `AGENT_PROVIDER` | Dialecto | Clave en |
|---|---|---|
| `anthropic` | Anthropic | `ANTHROPIC_API_KEY` |
| `openai` | OpenAI | `OPENAI_API_KEY` |
| `openrouter`, `groq`, `deepseek`, `mistral`, `xai`, `gemini` | OpenAI | la variable de cada uno |
| `ollama`, `lmstudio` | OpenAI | ninguna: son locales |

```sh
ANTHROPIC_API_KEY=… agent-cli "…"                                     # Claude, por defecto
AGENT_PROVIDER=openai OPENAI_API_KEY=… agent-cli "…"
AGENT_PROVIDER=ollama AGENT_MODEL=llama3.2 agent-cli "…"               # un modelo local, sin clave
AGENT_PROVIDER=ollama AGENT_BASE_URL=http://127.0.0.1:8080/v1 AGENT_MODEL=… agent-cli "…"
```

La última forma vale para cualquier servidor compatible con OpenAI, como llama.cpp o LM Studio
en otro puerto.

El paquete se ocupa de lo que da trabajo en una API de modelos: reintenta un 429 o un 5xx
esperando lo que pida el proveedor, corrige solo los parámetros que este rechaza, y con Claude
reenvía intactos los bloques de razonamiento y usa la caché de prompt. Lo que no modela se añade
a mano: un campo del cuerpo en `c.extra` y una cabecera en `c.headers`.

## 4. Las herramientas

El agente tiene herramientas de dos clases, y el bucle no las distingue.

**Las propias** son funciones del programa. Cada una declara lo que ve el modelo (nombre,
descripción y el JSON Schema de sus argumentos), su riesgo, y la función que la ejecuta:

<!-- check: project=examples/apps/agent-cli -->
```rust
/// Gives the agent `read_file` and `write_file`, both confined to `root`.
pub fn add_tools(a: Agent, root: string) {
    let read = fn(args: Json) -> Result<string, string> { read_file(root, args) };
    let write = fn(args: Json) -> Result<string, string> { write_file(root, args) };
    agent.tool(
        a,
        "read_file",
        "Reads a text file of the working folder",
        READ_SCHEMA,
        agent.READ,
        read
    );
    agent.tool(
        a,
        "write_file",
        "Creates or replaces a text file of the working folder",
        WRITE_SCHEMA,
        agent.WRITE,
        write
    );
}
```

La función recibe los argumentos como `Json` y devuelve `Result<string, string>`:

<!-- check: project=examples/apps/agent-cli -->
```rust
const WRITE_SCHEMA: string = `{
    "type": "object",
    "properties": {"path": {"type": "string"}, "content": {"type": "string"}},
    "required": ["path", "content"]
}`;

// What the model reads back is the tool's answer: an `Err` reaches it as a failed call.
fn write_file(root: string, args: Json) -> Result<string, string> {
    let path = inside(root, json.get_string(args, "path").unwrap_or(""))?;
    let content = json.get_string(args, "content").unwrap_or("");
    fs.write_file(path, content)?;
    Result.Ok("wrote ${content.len()} characters")
}
```

El `Ok` es el texto que lee el modelo; el `Err` le llega como llamada fallida, con su mensaje, y
él decide qué hacer. Una herramienta que aborta tampoco tumba el turno. Las herramientas corren
en la misma fibra que llamó a `agent.run`, así que pueden guardar estado como cualquier función.

Registrar una herramienta no devuelve nada. Lo único que puede salir mal ahí es un error del
programa, un nombre repetido o un esquema que no es JSON, y eso aborta al arrancar con un mensaje
que dice cuál, antes de que el modelo llegue a pedir nada.

Los argumentos los escribe un modelo, que puede haber leído texto de un tercero. Se comprueban
como los de un formulario:

<!-- check: project=examples/apps/agent-cli -->
```rust
/// The path of `name` inside `root`, or why it is refused. The arguments come from a model, so
/// they are checked like any other input: nothing absolute, nothing that climbs out.
pub fn inside(root: string, name: string) -> Result<string, string> {
    if (name.trim() == "") {
        return Result.Err("a path is required");
    }
    if (name.starts_with("/") || name.contains("\\") || name.contains(":")) {
        return Result.Err("only paths relative to the working folder are allowed");
    }
    for part in name.split("/") {
        if (part == "..") {
            return Result.Err("the path may not leave the working folder");
        }
    }
    Result.Ok(root + "/" + name)
}
```

**Las de un servidor MCP** se descubren al conectar. Aquí, las de la propia toolchain:

<!-- check: project=examples/apps/agent-cli -->
```rust
    // The tools of raylang's own toolchain. `RAY_BIN` points at a specific `ray` binary.
    let ray = match (mcp.connect_stdio(env("RAY_BIN").unwrap_or("ray"), ["mcp"])) {
        Result.Ok(s) => s,
        Result.Err(e) => {
            eprint("could not start `ray mcp`: " + e);
            return 1;
        },
    };
    match (agent.connect(a, "ray", ray)) {
        Result.Ok(_) => { },
        Result.Err(e) => eprint("ray mcp: " + e),
    }
```

El modelo las ve como `mcp__ray__ray_check`, `mcp__ray__ray_run` y las demás. Las instrucciones
que declare el servidor se suman al prompt de sistema.

## 5. Qué corre solo y qué pregunta

Cada herramienta tiene un riesgo y el agente un nivel de autonomía. De las dos cosas sale si una
llamada corre sin preguntar:

| Riesgo de la herramienta | `ask` | `edits` | `auto` |
|---|---|---|---|
| `READ`: solo lee | corre | corre | corre |
| `WRITE`: cambia algo que se puede deshacer | pregunta | corre | corre |
| `EXEC`: hace algo que no se puede deshacer | pregunta | pregunta | corre |

`read_file` es `READ` y `write_file` es `WRITE`. De las de `ray mcp`, las que el servidor declara
de solo lectura (`ray_fmt`, `ray_doc`) son `READ`; el resto son `EXEC`, porque un servidor que no
dice nada se trata como si lo cambiara todo.

Lo que haya que preguntar se le pregunta a una función tuya:

<!-- check: project=examples/apps/agent-cli -->
```rust
// Asks the user about one call. Only wired when there is a terminal to ask on.
fn ask_user(call: ToolCall, risk: string) -> Decision {
    eprint("");
    let _ = io.ewrite("? run ${call.name} (${risk})? [y]es / [a]lways / [N]o ");
    match (input()) {
        Option.Some(answer) => {
            let said = answer.trim().to_lower();
            if (said == "y") {
                Decision.Yes
            } else if (said == "a") {
                Decision.Always
            } else {
                Decision.No
            }
        },
        Option.None => Decision.No,
    }
}
```

- `Decision.Yes` la deja correr esta vez.
- `Decision.Always` la deja correr y no vuelve a preguntar por esa herramienta.
- `Decision.No` no la ejecuta, y al modelo se le dice que el usuario no lo permitió.

Un «no» no rompe la conversación: se le contesta al modelo como resultado de la llamada, y él
sigue desde ahí.

La pregunta solo se conecta cuando hay una terminal:

<!-- check: project=examples/apps/agent-cli -->
```rust
    // Without a terminal there is nobody to ask: what needs approval simply does not run.
    if (term.is_tty(0)) {
        agent.on_approve(a, ask_user);
    }
```

Es el defecto seguro. En un script, en CI o en un servidor, `write_file` y `ray_run` no corren
a menos que se arranque con `--autonomy auto`.

## 6. El turno

`agent.run` lleva la pregunta hasta el final y devuelve por qué terminó:

<!-- check: project=examples/apps/agent-cli -->
```rust
    let code = match (agent.run(a, words.join(" "))) {
        Result.Ok(outcome) => {
            print("");
            if (outcome.stop != agent.DONE) {
                eprint("stopped: ${outcome.stop} after ${outcome.steps} steps");
            }
            0
        },
        Result.Err(e) => {
            eprint(e);
            1
        },
    };
```

| `stop` | Qué pasó |
|---|---|
| `DONE` | el modelo contestó sin pedir más herramientas |
| `MAX_STEPS` | se agotó el presupuesto de pasos (20 por defecto); otro `run` puede continuar |
| `TRUNCATED` | la respuesta se cortó en el límite de tokens |
| `REFUSED` | el modelo declinó contestar |
| `CANCELLED` | se abandonó la espera, con `run_cancellable` |

`run` solo devuelve `Err` si falla hablar con el modelo. Una herramienta que falla, o que no se
permitió, es parte de la conversación.

El paquete no sabe de terminales. Lo que ocurre en el turno llega como eventos, y el programa
decide cómo mostrarlo:

<!-- check: project=examples/apps/agent-cli -->
```rust
// The answer goes to standard output as it arrives; what the agent does, to standard error.
fn show(e: Event) {
    match (e) {
        Event.Text(piece) => {
            let _ = io.write(piece);
            let _ = io.flush();
        },
        Event.Calling(call, risk) => eprint("[tool] ${call.name}"),
        Event.Returned(call, output, failed) => {
            if (failed) {
                eprint("[tool] ${call.name} failed");
            }
        },
        Event.Declined(call, why) => eprint("[tool] ${call.name} was not run"),
        _ => { },
    }
}
```

La respuesta va a la salida estándar según llega, porque `a.stream` está activado. Lo que hace el
agente va a la de errores. Así `agent-cli "…" > respuesta.md` guarda solo la respuesta.

La conversación queda en `a.history`: una segunda llamada a `run` sobre el mismo agente continúa
donde quedó la primera, y `a.usage` lleva los tokens de todos los turnos.

## 7. La clave de la API

La clave sale de la variable de entorno del proveedor o, si no está, del llavero del sistema con
`std/keychain`:

<!-- check: project=examples/apps/agent-cli -->
```rust
// The API key of a provider: its environment variable, or the system keychain (service
// "agent-cli", account = the provider), where `keychain.set` stored it once. A local server
// needs none.
fn api_key(p: Preset) -> Result<string, string> {
    if (p.key_env == "") {
        return Result.Ok("");
    }
    match (env(p.key_env)) {
        Option.Some(k) => return Result.Ok(k),
        Option.None => { },
    }
    match (keychain.get("agent-cli", p.id)?) {
        Option.Some(k) => Result.Ok(k),
        Option.None => Result.Err("set ${p.key_env}, or store the key in the keychain"),
    }
}
```

Para guardarla una vez: un programa de una línea con
`keychain.set("agent-cli", "anthropic", clave)`. En macOS queda en Keychain, en Linux en Secret
Service y en Windows en Credential Manager. Nunca en un archivo de texto ni en el repositorio.

## 8. Probar sin red

Los tests no llaman a ninguna API ni necesitan clave. El modelo se sustituye por un servidor
mínimo en el mismo proceso, que habla el dialecto de OpenAI y sigue un guion de dos pasos:

<!-- check: project=examples/apps/agent-cli -->
```rust
// The scripted model: the first request has no tool result yet, so it asks for `write_file`;
// once the result is there, it repeats it as its answer.
fn scripted(req: Request) -> Response {
    let body = webserver.request_text(req).unwrap_or("");
    let messages = json.get_array(json.parse(body).unwrap_or(Json.JNull), "messages").unwrap_or([]);
    let last = messages[messages.len() - 1];
    if (json.get_string(last, "role").unwrap_or("") == "tool") {
        let said = json.stringify(
            Json.JStr("Done: " + json.get_string(last, "content").unwrap_or(""))
        );
        return webserver.json_response(
            `{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":${said}}}]}`
        );
    }
    webserver.json_response(
        `{"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"write_file","arguments":"{\\"path\\": \\"hello.ray\\", \\"content\\": \\"fn main() { }\\"}"}}]}}]}`
    )
}
```

El agente, las herramientas y la aprobación son los de verdad. Este test comprueba que una
escritura espera al usuario y que, con su permiso, el archivo queda escrito:

<!-- check: project=examples/apps/agent-cli -->
```rust
@test
fn a_write_waits_for_the_user_and_runs_when_allowed() {
    let (a, root) = fixture("yes");
    agent.on_approve(a, fn(call: ToolCall, risk: string) -> Decision {
        assert_eq(call.name, "write_file");
        assert_eq(risk, agent.WRITE);
        Decision.Yes
    });
    let outcome = agent.run(a, "save an empty program").unwrap();
    assert_eq(outcome.stop, agent.DONE);
    assert_eq(outcome.text, "Done: wrote 13 characters");
    assert_eq(fs.read_file(root + "/hello.ray").unwrap(), "fn main() { }");
}
```

Otros dos tests cubren los casos contrarios: sin nadie a quien preguntar no se escribe nada, y en
el nivel `edits` la escritura corre sola.

```sh
ray test
```

## 9. Tu propio servidor MCP

El otro lado del protocolo: que tu programa ofrezca sus herramientas a un asistente. `notes-mcp`
expone unas notas a Claude Code, a Claude Desktop o a cualquier agente, incluido el de las
secciones anteriores.

```sh
ray new notes-mcp && cd notes-mcp
ray add mcp
```

Un servidor es un `Provider` con sus herramientas y sus recursos. Las que solo leen se
registran con `read_only_tool`, que es lo que deja a un cliente ejecutarlas sin preguntar; la
que escribe, con `tool`:

<!-- check: project=examples/apps/notes-mcp -->
```rust
/// What the server offers over the notes in `dir`.
pub fn provider_for(dir: string) -> Provider {
    var p = serve.provider("notes", "0.1.0");
    p.instructions = "The user's personal notes. Search before you add, so a note is not duplicated.";
    let search = fn(args: Json) -> Result<string, string> { search_notes(dir, args) };
    let read = fn(args: Json) -> Result<string, string> { read_note(dir, args) };
    let add = fn(args: Json) -> Result<string, string> { add_note(dir, args) };
    let index = fn() -> Result<string, string> { Result.Ok(store.titles(dir).join("\n")) };
    // The two that only read are announced as read-only: a client may run them without asking.
    serve.read_only_tool(
        p,
        "search_notes",
        "Lists the titles of the notes that contain a text; an empty text lists them all",
        SEARCH_SCHEMA,
        search
    );
    serve.read_only_tool(
        p,
        "read_note",
        "Returns the body of the note with this exact title",
        TITLE_SCHEMA,
        read
    );
    serve.tool(
        p,
        "add_note",
        "Creates a note, or replaces the one with the same title",
        NOTE_SCHEMA,
        add
    );
    serve.resource(
        p,
        "notes://index",
        "Index",
        "The title of every note, one per line",
        "text/plain",
        index
    );
    p
}
```

Cada herramienta es una función que recibe los argumentos y devuelve el texto que lee el modelo:

<!-- check: project=examples/apps/notes-mcp -->
```rust
// What the model reads back is the tool's answer: an `Err` reaches it as a failed call.
fn add_note(dir: string, args: Json) -> Result<string, string> {
    let title = text_arg(args, "title")?;
    store.write(dir, title, text_arg(args, "body")?)?;
    Result.Ok("saved '${title}'")
}
```

El título de una nota acaba siendo parte de una ruta, y lo escribe un modelo. Se valida antes de
tocar el disco:

<!-- check: project=examples/apps/notes-mcp -->
```rust
/// Whether `title` can be a file name: letters, digits, spaces, dashes and underscores. A
/// title comes from a model, and it becomes part of a path.
pub fn valid(title: string) -> bool {
    if (title.trim() == "" || title.len() > 80) {
        return false;
    }
    for ch in title.chars() {
        let code = char_code(ch);
        let letter = (code >= 65 && code <= 90) || (code >= 97 && code <= 122);
        let digit = code >= 48 && code <= 57;
        if (!(letter || digit || ch == ' ' || ch == '-' || ch == '_')) {
            return false;
        }
    }
    true
}
```

`main` sirve el proveedor. Sin argumentos habla por la entrada y la salida estándar, que es como
un asistente lanza un servidor local:

<!-- check: project=examples/apps/notes-mcp -->
```rust
fn main() -> int {
    let argv = args();
    // `notes-mcp --http 8765` serves over HTTP on this machine; with no arguments it speaks
    // over standard input and output, which is how an assistant launches a local server.
    if (argv.len() == 2 && argv[0] == "--http") {
        return match (serve.http(build, "127.0.0.1", argv[1].parse_int().unwrap_or(0))) {
            Result.Ok(_) => 0,
            Result.Err(e) => {
                eprint(e);
                1
            },
        };
    }
    serve.stdio(build())
}
```

Para conectarlo a Claude Code, el comando es tu binario:

```sh
ray build --native --release -o notes-mcp
claude mcp add notes -- /ruta/a/notes-mcp
```

| | Por stdio | Por HTTP |
|---|---|---|
| Se arranca con | `serve.stdio(p)` | `serve.http(build, "127.0.0.1", puerto)` |
| Quién lo lanza | el asistente, como proceso hijo | tú; el asistente se conecta a la URL |
| Para | herramientas de esta máquina | un servicio compartido |
| Estado entre peticiones | variables del programa | una base de datos o un canal: cada conexión corre en su fibra |
| Protección | la del proceso | rechaza otro origen; `p.token` exige `Authorization: Bearer` |

Por stdio, la salida estándar es el canal del protocolo: lo que el programa quiera decir de sí
mismo va por `eprint`, nunca por `print`.

El servidor se prueba mensaje a mensaje. `serve.handle` recibe la línea de la petición y devuelve
la de la respuesta, sin proceso ni puerto:

<!-- check: project=examples/apps/notes-mcp -->
```rust
// The `result` of the reply to one request, as JSON.
fn ask(dir: string, request: string) -> Json {
    let reply = serve.handle(main.provider_for(dir), request).unwrap();
    json.member(json.parse(reply).unwrap(), "result").unwrap_or(Json.JNull)
}
```

<!-- check: project=examples/apps/notes-mcp -->
```rust
@test
fn a_bad_call_is_an_error_the_model_can_read() {
    let dir = fs.make_temp_dir("notes-mcp-bad").unwrap();
    assert_eq(
        call(dir, "read_note", `{"title": "holidays"}`),
        ("there is no note titled 'holidays'", true)
    );
    assert_eq(call(dir, "read_note", "{}"), ("'title' is required", true));
    let escape = call(dir, "add_note", `{"title": "../../etc/passwd", "body": "x"}`);
    assert(escape.1);
    assert(escape.0.contains("not a valid title"));
}
```

Los tres casos del test son errores que el modelo lee y de los que puede recuperarse: una nota
que no existe, un argumento que falta y un título que intenta salir de la carpeta.

## 10. Lo que el agente no hace todavía

`agent-cli` es corto a propósito, y los paquetes están en su primera versión. Antes de ponerlo
delante de usuarios conviene saber qué falta:

| Qué | Cómo está hoy |
|---|---|
| Conversación de varios turnos | el paquete la soporta (`a.history`); el ejemplo hace una sola pregunta |
| Cancelar | `agent.run_cancellable` abandona la espera al modelo; el ejemplo no lo conecta a ninguna tecla |
| Herramientas en paralelo | corren una tras otra, en orden |
| Historial largo | no se compacta: una conversación muy larga acaba llenando el contexto del modelo |
| Presupuesto por tokens o por coste | solo hay presupuesto de pasos; `a.usage` da los tokens para que lo midas tú |
| Imágenes y otros contenidos | solo texto |

## Siguiente paso

[**Rendimiento**](performance.md): cómo medir un programa, encontrar dónde se va el tiempo y
hacerlo rápido.
