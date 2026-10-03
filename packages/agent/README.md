# `agent` — el bucle de un agente (adicional, **no** embebido)

Un modelo, tus herramientas y las de servidores MCP, en un bucle: el modelo pide una
herramienta, el agente la ejecuta, le devuelve el resultado y repite hasta que conteste. Con un
presupuesto de pasos, niveles de autonomía y una pregunta al usuario antes de todo lo que cambie
cosas.

Se apoya en dos paquetes: [`llm`](https://github.com/ray-language/llm) habla con el modelo y
[`mcp`](https://github.com/ray-language/mcp) con los servidores de herramientas. Se añaden desde
el índice:

```sh
ray add agent
ray add llm
ray add mcp
```

Con `ray add agent` se descargan también los otros dos, porque el paquete depende de ellos.
Añadirlos a mano deja escrito en tu `ray.toml` qué usa tu programa: `llm` siempre, para decir con
qué modelo se habla, y `mcp` si el agente va a usar herramientas de un servidor MCP.

## Un agente con una herramienta

```rust
import std/json;
import agent/agent;
import llm/llm;
from std/json import Json;

fn main() -> int {
    let a = agent.new(llm.for_anthropic("claude-opus-5-5", env("ANTHROPIC_API_KEY").unwrap_or("")));
    a.config.system = "You help with arithmetic. Use the tools.";
    agent.tool(
        a,
        "add",
        "Adds two integers",
        `{"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}},"required":["a","b"]}`,
        agent.READ,
        fn(args: Json) -> Result<string, string> {
            let sum = json.get_int(args, "a").unwrap_or(0) + json.get_int(args, "b").unwrap_or(0);
            Result.Ok(to_string(sum))
        }
    );
    match (agent.run(a, "What is 19 + 23?")) {
        Result.Ok(outcome) => {
            print(outcome.text);
            0
        },
        Result.Err(e) => {
            eprint(e);
            1
        },
    }
}
```

`agent.run` lleva el turno hasta el final. La conversación queda en `a.history`, así que una
segunda llamada a `run` continúa donde quedó la primera.

## Lo que corre solo y lo que pregunta

Cada herramienta declara su riesgo y el agente tiene un nivel de autonomía. De las dos cosas sale
si una llamada corre sin preguntar:

| Riesgo de la herramienta | `ASK` | `EDITS` | `AUTO` |
|---|---|---|---|
| `READ`: solo lee | corre | corre | corre |
| `WRITE`: cambia algo que se puede deshacer | pregunta | corre | corre |
| `EXEC`: hace algo que no se puede deshacer | pregunta | pregunta | corre |

Un agente nuevo empieza en `ASK`. Lo que haya que preguntar se le pregunta a tu función:

```rust
import std/io;
import agent/agent;
import llm/llm;
from agent/agent import Decision;
from llm/message import ToolCall;

fn main() -> int {
    let a = agent.new(llm.for_endpoint("http://127.0.0.1:11434/v1", "llama3.2", ""));
    agent.on_approve(a, fn(call: ToolCall, risk: string) -> Decision {
        let _ = io.write("Run ${call.name} (${risk}) with ${call.arguments}? [y/a/N] ");
        let _ = io.flush();
        let answer = input().unwrap_or("").trim().to_lower();
        if (answer == "y") {
            Decision.Yes
        } else if (answer == "a") {
            Decision.Always
        } else {
            Decision.No
        }
    });
    print(a.autonomy);
    0
}
```

- `Decision.Yes` la deja correr esta vez.
- `Decision.Always` la deja correr y no vuelve a preguntar por esa herramienta.
- `Decision.No` no la ejecuta, y al modelo se le dice que el usuario no lo permitió.

Sin función de aprobación, lo que necesite permiso no corre y el modelo recibe el motivo. Un
agente que corre en un servidor, sin nadie delante, no ejecuta nada peligroso por defecto.

## Las herramientas de un servidor MCP

```rust
import agent/agent;
import llm/llm;
import mcp/mcp;

fn main() -> int {
    let a = agent.new(llm.for_anthropic("claude-opus-5-5", env("ANTHROPIC_API_KEY").unwrap_or("")));
    let session = match (mcp.connect_stdio("ray", ["mcp"])) {
        Result.Ok(s) => s,
        Result.Err(e) => {
            eprint(e);
            return 1;
        },
    };
    match (agent.connect(a, "ray", session)) {
        Result.Ok(n) => eprint("${n} tools from ray mcp"),
        Result.Err(e) => eprint(e),
    }
    let code = match (agent.run(a, "Write a raylang function that reverses a string, and check it compiles.")) {
        Result.Ok(outcome) => {
            print(outcome.text);
            0
        },
        Result.Err(e) => {
            eprint(e);
            1
        },
    };
    mcp.close(session);
    code
}
```

El modelo las ve como `mcp__ray__ray_check`, `mcp__ray__ray_run`… Una que el servidor declara de
solo lectura es `READ`. Cualquier otra es `EXEC`: un servidor que no dice nada se trata como si
lo cambiara todo. Las instrucciones que declare el servidor se suman al prompt de sistema.

## Ver lo que pasa

El paquete no sabe de terminales ni de ventanas. Lo que ocurre en un turno llega como eventos, y
tú decides cómo mostrarlo:

```rust
import agent/agent;
import llm/llm;
from agent/agent import Event;

fn main() -> int {
    let a = agent.new(llm.for_endpoint("http://127.0.0.1:11434/v1", "llama3.2", ""));
    a.stream = true;
    agent.on_event(a, fn(e: Event) {
        match (e) {
            Event.Thinking(step) => eprint("… step ${step}"),
            Event.Text(piece) => eprint(piece),
            Event.Said(_) => { },
            Event.Calling(call, risk) => eprint("→ ${call.name} (${risk})"),
            Event.Returned(call, output, failed) => eprint("← ${call.name}: ${output.len()} chars, failed=${failed}"),
            Event.Declined(call, why) => eprint("✗ ${call.name}: ${why}"),
        }
    });
    print(a.stream);
    0
}
```

## Superficie

`import agent/agent;`

| Función | Qué hace |
|---|---|
| `new(config) -> Agent` | un agente sobre el modelo de `config`, sin herramientas, en `ASK` |
| `tool(a, name, description, schema, risk, run)` | añade una herramienta tuya; un nombre repetido, un riesgo desconocido o un esquema que no es JSON abortan el programa al arrancar |
| `connect(a, name, session) -> Result<int, string>` | ofrece las herramientas de un servidor MCP; devuelve cuántas |
| `on_approve(a, f)` | a quién se pregunta antes de lo que necesita permiso |
| `on_event(a, f)` | a quién se cuenta lo que pasa |
| `run(a, prompt) -> Result<Outcome, string>` | un turno, hasta el final |
| `run_cancellable(a, prompt, cancel)` | lo mismo, y la espera al modelo se abandona si llega algo por `cancel` |
| `reset(a)` | olvida la conversación; conserva herramientas y ajustes |
| `offered(a)` · `risk_of(a, name)` · `unattended(autonomy, risk)` · `autonomy_note(autonomy)` | para inspeccionar |

Lo que se ajusta en el `Agent`:

| Campo | Para qué | Por defecto |
|---|---|---|
| `config` | el modelo; `config.system` es el prompt del agente | lo que se pasó a `new` |
| `autonomy` | `ASK`, `EDITS` o `AUTO` | `ASK` |
| `max_steps` | cuántas veces se pregunta al modelo en un turno | 20 |
| `stream` | entregar el texto por partes, como `Event.Text` | desactivado |
| `history` | la conversación | vacía |
| `usage` | lo que llevan cobrado todos los turnos | cero |
| `allowed` | herramientas permitidas con `Always` | ninguna |

Lo que devuelve un turno:

| Campo de `Outcome` | Qué es |
|---|---|
| `text` | la última respuesta del modelo |
| `stop` | por qué terminó: `DONE`, `MAX_STEPS`, `TRUNCATED`, `REFUSED` o `CANCELLED` |
| `steps` | cuántas veces se preguntó al modelo |
| `usage` | lo que cobró este turno |

## Cómo se comporta

- **Un «no» no rompe la conversación.** Una llamada que no corre recibe igualmente su resultado,
  con el motivo. Una llamada sin responder invalidaría el historial para el turno siguiente.
- **Una herramienta que falla es un resultado, no un error del turno.** Un `Err`, un `panic`,
  unos argumentos que no son JSON o una herramienta que no existe le llegan al modelo como
  `error: …`, y él decide qué hacer.
- **`run` solo devuelve `Err` si falla hablar con el modelo.** Todo lo demás es parte de la
  conversación.
- **El presupuesto de pasos es un freno, no un final.** Al agotarlo el turno termina con
  `MAX_STEPS` y el historial queda válido: otro `run` puede continuar.
- **Las llamadas de una respuesta corren en orden**, una tras otra, en la fibra que llamó a
  `run`. Tus herramientas pueden guardar estado como cualquier otra función.
- **El nivel de autonomía se le dice al modelo.** En `EDITS` y `AUTO` se añade una nota al prompt:
  un modelo que cree que sus cambios esperan un sí escribe distinto.
- **Cancelar abandona la espera, no la herramienta.** `run_cancellable` deja de esperar al modelo
  cuando llega algo por `cancel`; una herramienta en curso termina. Si se cancela antes de que
  el modelo conteste nada, la conversación queda como estaba.
- **Lo que el proveedor corrige se recuerda.** Si rechaza un parámetro y `llm` lo corrige, el
  agente guarda la corrección para las peticiones siguientes.

No cubre todavía: ejecutar varias herramientas en paralelo, interrumpir una herramienta en
curso, compactar un historial que crece, ni presupuestos por tokens o por coste.

## Seguridad

Un agente ejecuta lo que un modelo decide, con los permisos de tu programa. Tres reglas:

- **Declara el riesgo con honestidad.** `READ` es lo único que corre siempre. Una herramienta que
  escribe marcada como `READ` es una herramienta que escribe sin preguntar.
- **No subas a `AUTO` por comodidad.** Es para tareas acotadas en un entorno que puedes perder.
- **Valida los argumentos.** Llegan de un modelo, que puede haber leído texto de un tercero: una
  ruta, una orden o un identificador se comprueban como los de un formulario.
