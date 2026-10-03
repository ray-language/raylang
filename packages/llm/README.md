# `llm` — hablar con modelos de lenguaje (adicional, **no** embebido)

Una conversación con un modelo, con herramientas y con respuesta por partes, escrita en raylang
puro. Un solo modelo de conversación sobre dos dialectos:

- **Anthropic** (`POST /v1/messages`): Claude.
- **OpenAI** (`POST /chat/completions`): OpenAI, y casi todos los demás, que hablan lo mismo.
  OpenRouter, Groq, Mistral, DeepSeek, xAI, Gemini por su endpoint compatible, y los servidores
  locales: Ollama, LM Studio, llama.cpp.

Paquete adicional, como `net` o `db`. Se añade desde el índice con `ray add llm`:

```toml
[dependencies]
llm = "^0.1.0"
```

Dentro del monorepo de raylang se puede usar la ruta (`llm = "path:../raylang/packages/llm"`).

## Una pregunta

```rust
import llm/llm;

fn main() -> int {
    var c = llm.for_anthropic("claude-opus-5-5", env("ANTHROPIC_API_KEY").unwrap_or(""));
    c.system = "Answer in one sentence.";
    match (llm.send(c, [], [llm.user("What is raylang?")])) {
        Result.Ok(reply) => {
            print(reply.message.text);
            0
        },
        Result.Err(e) => {
            eprint(e);
            1
        },
    }
}
```

Con otro proveedor solo cambia la primera línea:

```rust
import llm/llm;

fn main() -> int {
    // Un servidor local: sin clave.
    let local = llm.for_endpoint("http://127.0.0.1:11434/v1", "llama3.2", "");
    // Un proveedor conocido, por su nombre.
    let groq = llm.for_preset("groq", "llama-3.3-70b-versatile", env("GROQ_API_KEY").unwrap_or(""));
    print(local.base_url);
    print(groq.is_some());
    0
}
```

## Por partes

`send_stream` entrega el texto según llega y devuelve al final lo mismo que `send`:

```rust
import std/io;
import llm/llm;

fn main() -> int {
    let c = llm.for_anthropic("claude-opus-5-5", env("ANTHROPIC_API_KEY").unwrap_or(""));
    let outcome = llm.send_stream(c, [], [llm.user("Tell me a short story.")], fn(piece: string) {
        let _ = io.write(piece);
        let _ = io.flush();
    });
    match (outcome) {
        Result.Ok(reply) => {
            print("");
            eprint("${reply.usage.output_tokens} tokens in ${reply.usage.latency_ms} ms");
            0
        },
        Result.Err(e) => {
            eprint(e);
            1
        },
    }
}
```

## Con herramientas

El modelo no ejecuta nada: pide una herramienta, tu programa la ejecuta y le devuelve el
resultado. El bucle es tuyo:

```rust
import std/json;
import llm/llm;
from std/json import Json;

fn main() -> int {
    let c = llm.for_anthropic("claude-opus-5-5", env("ANTHROPIC_API_KEY").unwrap_or(""));
    let tools = [
        llm.tool(
            "add",
            "Adds two integers",
            `{"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}},"required":["a","b"]}`
        )
    ];
    var history = [llm.user("What is 19 + 23?")];
    var step = 0;
    while (step < 8) {
        step = step + 1;
        let reply = match (llm.send(c, tools, history)) {
            Result.Ok(r) => r,
            Result.Err(e) => {
                eprint(e);
                return 1;
            },
        };
        // El turno del asistente vuelve al historial TAL COMO LLEGÓ.
        history.push(reply.message);
        let calls = llm.tool_calls(reply);
        if (calls.len() == 0) {
            print(reply.message.text);
            return 0;
        }
        for call in calls {
            let args = json.parse(call.arguments).unwrap_or(Json.JNull);
            let sum = json.get_int(args, "a").unwrap_or(0) + json.get_int(args, "b").unwrap_or(0);
            // Un resultado por cada llamada, con su mismo id.
            history.push(llm.tool_result(call.id, to_string(sum)));
        }
    }
    eprint("too many steps");
    1
}
```

Las herramientas de un servidor MCP se ofrecen igual: el paquete
[`mcp`](https://github.com/ray-language/mcp) las descubre, y cada una tiene el `name`, la
`description` y el `schema` que necesita un `Tool` de aquí.

## Superficie

`import llm/llm;`

| Función | Qué hace |
|---|---|
| `for_anthropic(model, api_key) -> Config` | la API de Anthropic |
| `for_openai(model, api_key) -> Config` | la API de OpenAI |
| `for_endpoint(base_url, model, api_key) -> Config` | cualquier endpoint que hable el dialecto de OpenAI |
| `for_preset(id, model, api_key) -> Option<Config>` | un proveedor conocido por su nombre |
| `send(c, tools, history) -> Result<Reply, string>` | una ida y vuelta |
| `send_stream(c, tools, history, show) -> Result<Reply, string>` | lo mismo, entregando el texto a `show` según llega |
| `user(text)` · `tool_result(call_id, text)` | los turnos que añade tu programa |
| `tool(name, description, schema) -> Tool` | una herramienta; `schema` es el JSON Schema como texto, y uno que no sea JSON aborta el programa |
| `tool_calls(reply) -> [ToolCall]` | lo que pide el modelo; vacío si terminó |
| `models(c) -> Result<[string], string>` | los modelos que sirve el endpoint; no gasta tokens |

Lo que devuelve una ida y vuelta:

| Campo de `Reply` | Qué es |
|---|---|
| `message` | el turno del asistente: `text`, `tool_calls`, y `raw` con sus bloques originales |
| `stop_reason` | por qué se detuvo, en palabras del proveedor |
| `model` | el modelo que contestó |
| `usage` | `input_tokens`, `output_tokens`, `cached_tokens`, `latency_ms`, y `measured` (falso si el proveedor no informó) |

`message.was_truncated(stop_reason)` dice si la respuesta se cortó en el límite de tokens, y
`message.was_refused(stop_reason)` si el modelo declinó contestar. Valen para los dos dialectos.

Lo que se ajusta en `Config`:

| Campo | Para qué | Por defecto |
|---|---|---|
| `system` | el prompt de sistema | ninguno |
| `max_tokens` | el tope de la respuesta | 4096 |
| `temperature` | negativa = no se envía | no se envía |
| `effort` | cuánto razona el modelo: `"low"`, `"medium"`, `"high"` | no se envía |
| `timeout_ms` | lo que puede tardar una petición | 10 minutos |
| `headers` | cabeceras extra; mandan sobre las del dialecto | ninguna |
| `extra` | campos extra del cuerpo, para lo que este paquete no modela | ninguno |
| `cache` | Anthropic: marcar el prompt y las herramientas para la caché | activado |
| `max_attempts` | intentos ante un fallo pasajero | 3 |

`config.presets()` lista los proveedores conocidos: `anthropic`, `openai`, `openrouter`, `groq`,
`deepseek`, `mistral`, `xai`, `gemini`, `ollama` y `lmstudio`.

## Cómo se comporta

- **Los fallos pasajeros se reintentan.** Sin conexión, una lectura cortada, un 429, un 408 o un
  5xx se repiten hasta `max_attempts` veces. La espera es la que pida el proveedor en
  `Retry-After` o en su mensaje, con un tope de un minuto; si no pide nada, 1, 2 y 4 segundos.
- **Un flujo solo se reintenta si aún no se entregó texto.** Después, repetir haría ver la misma
  respuesta dos veces.
- **Un parámetro que el proveedor rechaza se corrige solo.** Los modelos nuevos de OpenAI quieren
  `max_completion_tokens` en lugar de `max_tokens`, y los de razonamiento no aceptan temperatura.
  El proveedor lo dice en su error, y la petición se corrige y se reenvía una vez.
- **El turno del asistente se reenvía tal como llegó.** En Anthropic puede traer bloques de
  razonamiento firmados, que la API exige recibir sin cambios: viajan en `message.raw`. Por eso
  el historial guarda `reply.message` entero, no solo su texto.
- **Los resultados de herramientas de un turno van juntos.** En Anthropic se agrupan en un solo
  mensaje, como pide el protocolo.
- **La caché de prompt de Anthropic va activada.** El prompt de sistema y la lista de herramientas
  no cambian dentro de una conversación; marcados, se cobran a una décima parte desde la segunda
  petición. `usage.cached_tokens` dice cuántos se sirvieron de la caché.
- **Un error del proveedor es un `Err` con su estado y su mensaje**, no el JSON en que venía
  envuelto: `HTTP 401 from https://…: invalid x-api-key`.

No cubre todavía la API Responses de OpenAI, las imágenes ni otros contenidos que no sean texto,
el modo por lotes, ni el dialecto nativo de Gemini (se usa por su endpoint compatible).

## La clave

La clave es un parámetro: el paquete no la busca por ti. Tómala del entorno, o del llavero del
sistema con `std/keychain`, y no la escribas en el código ni en el repositorio. Un servidor local
no necesita ninguna.
