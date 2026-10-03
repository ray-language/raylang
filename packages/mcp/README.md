# `mcp` — Model Context Protocol (adicional, **no** embebido)

El [Model Context Protocol](https://modelcontextprotocol.io) escrito en raylang puro, por los dos
lados:

- **Cliente** (`mcp/mcp`): conecta con un servidor MCP, descubre sus herramientas y recursos, y
  los usa. Es la pieza que necesita un agente para ofrecerle a un modelo herramientas que no ha
  escrito él.
- **Servidor** (`mcp/serve`): tu app ofrece sus propias herramientas y recursos a un asistente
  como Claude Code o Claude Desktop, o a cualquier agente.

Paquete adicional, como `net` o `db`. Se añade desde el índice con `ray add mcp`:

```toml
[dependencies]
mcp = "^0.1.0"
```

Dentro del monorepo de raylang se puede usar la ruta (`mcp = "path:../raylang/packages/mcp"`).

## El cliente

```rust
import mcp/mcp;

fn main() -> int {
    // Un servidor por stdio: un programa que se lanza y con el que se habla por su entrada y
    // su salida estándar. Aquí, el de la propia toolchain.
    let c = match (mcp.connect_stdio("ray", ["mcp"])) {
        Result.Ok(c) => c,
        Result.Err(e) => {
            eprint(e);
            return 1;
        },
    };
    for t in mcp.tools(c).unwrap_or([]) {
        print("${t.name}: ${t.description}");
    }
    match (mcp.call_json(c, "ray_run", `{"code": "fn main() { print(6 * 7); }"}`)) {
        Result.Ok(output) => print(output),
        Result.Err(e) => eprint(e),
    }
    mcp.close(c);
    0
}
```

Un servidor por HTTP se conecta con `mcp.connect_http(url)`. Para darle cabeceras, un directorio
de trabajo, variables de entorno o plazos propios, se construye el `Server` y se ajusta:

```rust
import mcp/mcp;

fn main() -> int {
    var server = mcp.http_server("issues", "https://mcp.example.com/mcp");
    server.headers.insert("Authorization", "Bearer " + env("ISSUES_TOKEN").unwrap_or(""));
    server.call_timeout_ms = 120000;
    match (mcp.connect(server)) {
        Result.Ok(c) => {
            print(mcp.instructions(c));
            mcp.close(c);
            0
        },
        Result.Err(e) => {
            eprint(e);
            1
        },
    }
}
```

### Superficie del cliente

`import mcp/mcp;`

| Función | Qué hace |
|---|---|
| `stdio_server(name, command, args) -> Server` | un servidor que se lanza como proceso |
| `http_server(name, url) -> Server` | un servidor por Streamable HTTP |
| `connect(server) -> Result<Session, string>` | arranca el servidor y hace el apretón de manos |
| `connect_stdio(command, args)` · `connect_http(url)` | los dos atajos de `connect` |
| `tools(c) -> Result<[Tool], string>` | las herramientas que ofrece |
| `call(c, tool, arguments: Json) -> Result<string, string>` | llama a una; `Err` si la herramienta falla |
| `call_json(c, tool, arguments: string)` | lo mismo con los argumentos como texto JSON, la forma en que llegan de un modelo |
| `resources(c) -> Result<[Resource], string>` | los recursos que publica; `[]` si no declara ninguno |
| `read_resource(c, uri) -> Result<string, string>` | lee uno, como texto |
| `instructions(c) -> string` | el texto que el servidor pide incluir en el prompt de sistema |
| `offers(c, capability) -> bool` | si declaró `"tools"`, `"resources"` o `"prompts"` |
| `request(c, method, params: Json) -> Result<Json, string>` | una petición cruda, para lo que no cubre lo anterior |
| `close(c)` | termina la sesión y espera a que el servidor se vaya |

Los campos de `Server` que se pueden ajustar: `dir`, `env`, `headers`, `client_name`,
`client_version`, `start_timeout_ms` (15 s por defecto) y `call_timeout_ms` (60 s).

`import mcp/protocol;` tiene las piezas puras, para quien hable el protocolo a mano o lo pruebe:
`request`, `notification`, `initialize`, `reply_with_id`, `result_of`, `parse_tools`,
`parse_resources`, `call_text`, `resource_text`, `offers`, `instructions_of`, y la convención de
nombres `tool_name(server, tool)` = `mcp__<server>__<tool>` con su inversa `split_name`.

### Cómo se comporta el cliente

- **Una sesión por servidor, y la posee una fibra.** Una `Session` se puede copiar a cualquier
  fibra: todas las copias hablan con el mismo servidor, y las peticiones se atienden de una en una.
- **Un servidor que muere se relanza.** Si el proceso termina o agota el plazo, se mata, la
  petición devuelve `Err` con el motivo, y la siguiente petición lo vuelve a lanzar.
- **Una sesión HTTP caducada se reabre.** Si el servidor responde 404 a una sesión que ya no
  conoce, el cliente inicializa otra y reintenta esa petición una vez.
- **La salida de error del servidor se descarta**, para que un servidor que escribe mucho en su
  log no se quede bloqueado.
- **Una herramienta que falla es un `Err`**, con el texto que devolvió. Un bloque que no es texto
  se resume: `[image 12 KiB]`.
- **`read_only` es lo que el servidor declara** en `annotations.readOnlyHint`: una pista, no una
  garantía. Sin declaración, vale `false`.

Habla la versión `2024-11-05` del protocolo. No implementa todavía *prompts* como funciones
propias (se llega a ellos con `request`), ni las peticiones del servidor al cliente (*sampling*,
*roots*), ni la reanudación de un flujo SSE cortado.

## El servidor

```rust
import std/json;
import mcp/serve;
from std/json import Json;
from mcp/serve import Provider;

// Lo que ofrece la app. Una función de nivel superior: el transporte HTTP la llama en la fibra
// de cada conexión.
fn build() -> Provider {
    var p = serve.provider("notes", "0.1.0");
    p.instructions = "Notes of the current user. Search before you add.";
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
    serve.resource(
        p,
        "notes://greeting",
        "Greeting",
        "A friendly line",
        "text/plain",
        fn() -> Result<string, string> { Result.Ok("hello") }
    );
    p
}

fn main() -> int {
    // Por la entrada y la salida estándar: así lanza un asistente un servidor local.
    serve.stdio(build())
}
```

Para conectarlo a Claude Code, el comando que lanza el servidor es tu programa:

```sh
ray build --native --release -o notes-mcp
claude mcp add notes -- /ruta/a/notes-mcp
```

Por HTTP, el mismo `build` se sirve con `serve.http(build, "127.0.0.1", 8765)`.

### Superficie del servidor

`import mcp/serve;`

| Función | Qué hace |
|---|---|
| `provider(name, version) -> Provider` | un proveedor vacío; `instructions` y `token` son campos |
| `tool(p, name, description, schema, run)` | añade una herramienta; `schema` es el JSON Schema de sus argumentos como texto, `""` si no tiene |
| `read_only_tool(…)` | lo mismo, declarada como de solo lectura |
| `resource(p, uri, name, description, mime, read)` | añade un recurso |
| `stdio(p) -> int` | sirve por la entrada y la salida estándar hasta que el cliente cierra |
| `http(build, host, port) -> Result<int, string>` | sirve por Streamable HTTP |
| `http_on(build, listener)` | lo mismo sobre un listener ya abierto, para elegir un puerto libre |
| `answer(p, req) -> Response` | contesta una petición HTTP: para montar el protocolo en una ruta de un servidor mayor |
| `handle(p, line) -> Option<string>` | contesta un mensaje del protocolo; puro, para tests |

Una herramienta recibe sus argumentos como `Json` y devuelve `Result<string, string>`: el `Ok` es
el texto que lee el modelo, y el `Err` le llega como llamada fallida, con su mensaje.

### Cómo se comporta el servidor

- **Una herramienta que aborta no tumba el servidor.** Un `panic` o un índice fuera de rango se
  convierten en una llamada fallida que el modelo lee.
- **Registrar dos veces el mismo nombre, o un esquema que no es JSON, aborta el programa** al
  arrancar, con un mensaje que dice qué herramienta. Son errores de programación, no situaciones
  de ejecución, y es mejor verlos antes de servir nada que en la primera llamada del modelo.
- **Por stdio, la salida estándar es el canal del protocolo.** Lo que el programa quiera decir de
  sí mismo va por `eprint`, nunca por `print`.
- **Por HTTP no hay sesiones:** cada POST lleva un mensaje y recibe su respuesta. Cada conexión
  corre en su fibra y construye su copia del proveedor, así que el estado compartido entre
  peticiones vive en una base de datos o tras un canal, nunca en una variable capturada.
- **Por HTTP se rechaza una petición de otro origen** (cabecera `Origin` distinta del `Host`),
  para que una página web no pueda hablar con un servidor local. Con `token` puesto, cada
  petición debe llevar `Authorization: Bearer <token>`, comparado en tiempo constante.
- **No anuncia lo que no tiene:** sin recursos registrados, no declara la capacidad `resources`.

No implementa *prompts*, notificaciones del servidor al cliente (progreso, cambios en la lista de
herramientas) ni el flujo `GET` de Streamable HTTP.

## Seguridad

Un servidor MCP por stdio es un programa que corre con los permisos de tu aplicación. Conectar
con uno es ejecutarlo: hazlo solo con los que el usuario haya elegido. Lo que devuelve una
herramienta es texto de un tercero; trátalo como dato, no como instrucciones.

Del lado servidor, una herramienta es código tuyo que un modelo decide cuándo llamar y con qué
argumentos: valida lo que recibe como validarías la entrada de un formulario. Marca
`read_only_tool` solo lo que de verdad no cambia nada, porque es la pista con la que un cliente
decide ejecutarla sin preguntar. Un servidor HTTP debe escuchar en `127.0.0.1`, y llevar `token`
si escucha en cualquier otra dirección.
