# Un servidor web concurrente y SSE

M15 dio el cliente HTTP; el servidor de M15.5 sabía aceptar conexiones concurrentes pero hablaba TCP
crudo (un eco). M19 construye encima la **capa de aplicación web**, y lo hace fiel a la regla de M15:
**el transporte son builtins; los protocolos son librerías escritas en raylang**. Así que el servidor
web no es código de Rust: es `examples/webserver.ray`, importable, legible, modificable.

## De TCP a HTTP: parsear, enrutar, responder

Un servidor HTTP es, en esencia, tres cosas sobre el socket concurrente que ya teníamos: **leer** una
petición, **decidir** qué hacer, y **escribir** una respuesta. La librería las modela con dos structs
gemelos del `Response` del cliente (M15.4b):

```rust
pub struct Request  { method: string, path: string, headers: Map<string, string>, body: bytes }
pub struct Response { status: int, headers: Map<string, string>, body: bytes }
```

`read_request(conn)` acumula del socket hasta encontrar el fin de cabeceras (`\r\n\r\n`), parsea la línea
de petición (`GET /ruta HTTP/1.1`) y las cabeceras (con el nombre en minúsculas para *lookup*
case-insensitive), y —si hay `Content-Length`— sigue leyendo hasta completar el cuerpo. `send_response`
hace el camino inverso: serializa la línea de estado, las cabeceras (con `Content-Length` y
`Connection: close`) y el cuerpo. Sobre eso, atajos para los casos comunes: `ok(body)`, `text(status,
body)`, `not_found()`, `json_response(body)`.

## Dos formas de servir

El bucle del servidor es el de M15.5 envuelto: `tcp_listen`, y en bucle `accept → spawn(atender)`, una
fibra por conexión. La librería ofrece dos entradas:

- `serve(host, port, handler)` con `handler: fn(Request) -> Response` — la ergonómica: el handler recibe
  la petición y devuelve la respuesta; `serve` la envía y cierra.
- `serve_raw(host, port, handler)` con `handler: fn(Request, int)` — el handler recibe la petición **y la
  conexión**, para controlarla él mismo. `serve` se define sobre `serve_raw`.

¿Por qué la versión "cruda"? Por SSE.

## SSE: HTTP que no cierra

Los **server-sent events** son un truco precioso: en vez de responder y cerrar, el servidor abre una
respuesta `Content-Type: text/event-stream` y la **deja abierta**, escribiendo eventos `data: …\n\n` a
medida que ocurren cosas. El navegador (o cualquier cliente) los recibe en tiempo real, sin *polling*.

En raylang son dos funciones de una línea sobre `socket_write`:

```rust
pub fn sse_open(conn: int)  -> Result<int, string>   // cabeceras text/event-stream, keep-alive
pub fn sse_event(conn: int, data: string) -> Result<int, string>   // "data: <…>\n\n"
```

El handler (vía `serve_raw`) hace `sse_open` y luego un bucle de `sse_event`. **Cero runtime nuevo**:
es streaming sobre el servidor concurrente. Y como cada conexión es una fibra, un cliente colgado de un
stream SSE no bloquea a los demás: la fibra del stream cede mientras espera, las otras avanzan. El
"servidor web async" de verdad, escrito enteramente en el lenguaje.

```rust
// examples/webserver_demo.ray (resumen)
import webserver;
fn enrutar(req: webserver.Request, conn: int) {
    if (req.path == "/sse") {
        webserver.sse_open(conn);
        // ... bucle de sse_event con sleep entre eventos ...
    } else {
        webserver.send_response(conn, webserver.ok("¡Hola desde raylang!\n"));
    }
}
fn main() -> int {
    webserver.serve_raw("127.0.0.1", 8080, enrutar);
    0
}
```

## Probar un servidor

La concurrencia es **solo VM**, y la red no es determinista para el oráculo, así que se prueba por
**subproceso**: un servidor `.ray` **acotado** (sirve N conexiones con `scope` y termina, imprimiendo su
puerto) que importa la librería, y un cliente de Rust que comprueba la respuesta — el mismo molde que el
test del cliente HTTP. Un caso verifica el enrutado (200 en `/hola`, 404 en otra ruta); otro, que un
stream SSE emite sus eventos `data:`.
