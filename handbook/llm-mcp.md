# LLM y MCP

Español · [English](llm-mcp.en.md)

Este capítulo tiene dos mitades. La primera es **usar un asistente LLM para escribir raylang**: qué
darle para que el código que escribe compile. La segunda es **escribir un agente en raylang**: un
programa de terminal que habla con Claude por la API de mensajes y usa herramientas a través de
MCP.

El proyecto de la segunda mitad está en
[`examples/apps/agent-cli`](../examples/apps/agent-cli/), con sus tests. Los bloques de raylang
están copiados de él y el CI comprueba que sigan siéndolo.

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

## 2. Un agente escrito en raylang

`agent-cli` recibe una pregunta en la terminal. Claude la responde y, por el camino, compila y
ejecuta el raylang que escribe con las herramientas de `ray mcp`. Son tres módulos:

| Módulo | Qué hace |
|---|---|
| `claude.ray` | el cliente de la API de mensajes, por HTTP |
| `mcp.ray` | el cliente MCP por stdio |
| `agent.ray` | el bucle: pedir, ejecutar herramientas, devolver resultados |

No hay SDK oficial de Anthropic para raylang, pero la API es JSON sobre HTTPS y `net/http` basta.

## 3. Hablar con la API de mensajes

Una petición es un `POST /v1/messages` con tres cabeceras: la clave, la versión de la API y el
tipo de contenido.

<!-- check: project=examples/apps/agent-cli -->
```rust
/// Sends one request. A long answer can take minutes, hence the 10-minute timeout.
pub fn send(c: Config, body: string) -> Result<string, string> {
    var headers: Map<string, string> = Map.new();
    headers.insert("content-type", "application/json");
    headers.insert("x-api-key", c.api_key);
    headers.insert("anthropic-version", "2023-06-01");
    // `fallbacks: "default"`: if a safety classifier declines, the API retries on a fallback model.
    headers.insert("anthropic-beta", "server-side-fallback-2026-07-01");
    let r = http.request_bytes(
        "POST",
        c.base_url + "/v1/messages",
        body.to_bytes(),
        headers,
        600000
    )?;
    let text = http.body_text(r)?;
    if (r.status >= 400) {
        return Result.Err("HTTP " + to_string(r.status) + ": " + text);
    }
    Result.Ok(text)
}
```

El plazo es de 10 minutos porque una respuesta larga puede tardar varios. El cuerpo lleva el
modelo, las herramientas y la conversación:

<!-- check: project=examples/apps/agent-cli -->
```rust
/// The request body. `tools` and `messages` are JSON arrays already rendered as text.
pub fn request_body(model: string, system: string, tools: string, messages: [string]) -> string {
    `{
  "model": ${quote(model)},
  "max_tokens": 16000,
  "output_config": {"effort": "medium"},
  "fallbacks": "default",
  "system": ${quote(system)},
  "tools": ${tools},
  "messages": [${messages.join(",")}]
}`
}
```

- **`claude-opus-5-5`** es el modelo por defecto. `AGENT_MODEL` lo cambia.
- **`effort: "medium"`** controla cuánto razona el modelo antes de responder. Es el valor por
  defecto de este modelo; se escribe explícito para que se vea dónde ajustarlo.
- **`fallbacks: "default"`**, junto con la cabecera `anthropic-beta` de arriba, hace que si un
  clasificador de seguridad rechaza la petición, la API la reintente con otro modelo en la misma
  llamada. Sin él, la respuesta simplemente se detiene con `stop_reason: "refusal"`.

Los mensajes viajan como texto JSON. El turno del asistente se devuelve a la API **tal como
llegó**: puede traer bloques de razonamiento, y la API exige recibirlos sin cambios.

## 4. El bucle del agente

El modelo responde con un motivo de parada. Si es `tool_use`, quiere herramientas; el agente las
ejecuta y le devuelve los resultados. Si es otro, la respuesta está lista.

<!-- check: project=examples/apps/agent-cli -->
```rust
    while (step < max_steps) {
        step = step + 1;
        let reply = claude.parse_reply(ask(claude.request_body(model, SYSTEM, tools, messages))?)?;
        messages.push(claude.assistant_turn(reply));
        if (reply.stop_reason == "refusal") {
            return Result.Err("the model declined this request");
        }
        if (reply.stop_reason != "tool_use") {
            // end_turn (done), or max_tokens (the answer was cut: return what there is)
            return Result.Ok(reply.text);
        }
        var results: [string] = [];
        for call in reply.calls {
            eprint("[tool] " + call.name);
            results.push(match (mcp.call_tool(server, call.name, call.input)) {
                Result.Ok(out) => claude.tool_result(call.id, out, false),
                Result.Err(e) => claude.tool_result(call.id, e, true),
            });
        }
        messages.push(claude.results_turn(results));
    }
```

Tres detalles importan:

- **Todos los resultados de un turno van en un solo mensaje.** Repartirlos en varios enseña al
  modelo a dejar de pedir herramientas en paralelo.
- **Una herramienta que falla no se calla:** su salida vuelve con `is_error: true`, y el modelo
  decide qué hacer.
- **`refusal` y `max_tokens` se comprueban** antes de leer el texto. Un límite de pasos evita que
  un bucle se alargue sin fin.

## 5. Un cliente MCP por stdio

Un servidor MCP es un proceso que lee peticiones JSON-RPC por su entrada estándar, una por línea,
y responde por su salida estándar. El cliente lo lanza una vez y conversa con él con
`std/process`:

<!-- check: project=examples/apps/agent-cli -->
```rust
/// Starts `program args…` as an MCP server and performs the handshake.
pub fn connect(program: string, args: [string]) -> Result<Client, string> {
    let proc = process.cmd(program, args).stdin_pipe().stream()?;
    let c = Client { proc: proc, pending: "", next_id: 0 };
    let _ = call(
        c,
        "initialize",
        `{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"agent-cli","version":"0.1.0"}}`
    )?;
    let _ = c.proc.write(
        (`{"jsonrpc": "2.0", "method": "notifications/initialized"}` + "\n").to_bytes()
    )?;
    Result.Ok(c)
}
```

`stdin_pipe()` deja abierta la entrada del proceso para escribir una petición tras otra, y
`stream()` entrega su salida como un canal. Ese canal trae trozos de bytes, no líneas, así que el
cliente guarda lo que sobra tras el último salto de línea:

<!-- check: project=examples/apps/agent-cli -->
```rust
// The next complete line of output. The process delivers chunks of bytes, not lines, so the
// client keeps what is left after the last newline for the next call.
fn read_line(c: Client) -> Result<string, string> {
    while (true) {
        match (c.pending.index_of("\n")) {
            Option.Some(i) => {
                let line = c.pending.substring(0, i);
                c.pending = c.pending.substring(i + 1, c.pending.len());
                return Result.Ok(line);
            },
            Option.None => { },
        }
        match (recv(c.proc.out)) {
            Option.Some(chunk) => {
                c.pending = c.pending + from_utf8(chunk).unwrap_or("");
            },
            Option.None => return Result.Err("the MCP server closed its output"),
        }
    }
    Result.Err("unreachable")
}
```

Con eso, `tools(c)` pide la lista de herramientas (`tools/list`) y `call_tool(c, nombre,
argumentos)` llama a una (`tools/call`). Las herramientas de MCP se pasan a la API con el mismo
nombre y su esquema JSON como `input_schema`.

## 6. La clave de la API

La clave sale de `ANTHROPIC_API_KEY` o, si no está, del llavero del sistema con `std/keychain`:

<!-- check: project=examples/apps/agent-cli -->
```rust
// The API key: ANTHROPIC_API_KEY, or the system keychain (service "agent-cli", account
// "anthropic"), where `keychain.set("agent-cli", "anthropic", key)` stored it once.
fn api_key() -> Result<string, string> {
    match (env("ANTHROPIC_API_KEY")) {
        Option.Some(k) => return Result.Ok(k),
        Option.None => { },
    }
    match (keychain.get("agent-cli", "anthropic")?) {
        Option.Some(k) => Result.Ok(k),
        Option.None => Result.Err("set ANTHROPIC_API_KEY or store the key in the keychain"),
    }
}
```

Para guardarla una vez: un programa de una línea con
`keychain.set("agent-cli", "anthropic", clave)`. En macOS queda en Keychain, en Linux en Secret
Service y en Windows en Credential Manager. Nunca en un archivo de texto.

## 7. Probar sin red

Los tests no llaman a la API ni necesitan clave. El bucle recibe la función que envía las
peticiones como parámetro, así que un test la sustituye por un guion: la primera vez pide
`ray_run`, y cuando le llega el resultado, responde. El servidor MCP sí es el `ray mcp` real.

<!-- check: project=examples/apps/agent-cli -->
```rust
@test
fn the_loop_runs_a_tool_and_returns_the_answer() {
    let server = mcp.connect(ray_bin(), ["mcp"]).unwrap();
    // The scripted API: the first request has no tool result yet, so it asks for `ray_run`;
    // once the result (with 42 in it) comes back, it answers.
    let seen: Channel<string> = Channel.bounded(8);
    let ask = fn(body: string) -> Result<string, string> {
        send(seen, body);
        if (body.contains("tool_result")) { Result.Ok(END_TURN) } else { Result.Ok(TOOL_USE) }
    };
    let answer = agent.run(ask, server, "claude-opus-5-5", "what does 6 * 7 print?", 5).unwrap();
    assert_eq(answer, "It prints 42.");
    let _first = recv(seen).unwrap();
    let second = recv(seen).unwrap();
    assert(second.contains("tool_use_id"));
    assert(second.contains("42"));
    mcp.close(server);
}
```

```sh
ray test                                                    # sin red, sin clave
ANTHROPIC_API_KEY=… ray run -- "invierte las palabras de un string en raylang"
```

Este agente implementa los dos protocolos a mano, en unas 400 líneas con el bucle incluido. Se está
evaluando convertirlo en paquetes estándar, `llm` y `mcp`, para que cualquier app los use sin
copiarlos. Mientras tanto, este ejemplo es la referencia.

## Siguiente paso

[**Rendimiento**](performance.md): cómo medir un programa, encontrar dónde se va el tiempo y
hacerlo rápido.
