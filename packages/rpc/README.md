# `rpc` — RPC raylang↔raylang (adicional, **no** embebido)

La **comunicación nativa entre servicios** raylang (M88.4), sin el peso de un servidor HTTP/2:
framing con **prefijo de longitud sobre TCP** y **JSON** como payload v1, escrito EN raylang puro
sobre `std/net` + `std/json`. Para interop externo *entrante* ya está el webserver (HTTP/1.1+JSON);
esto es para hablar servicio-a-servicio con id, deadline y trace en el sobre.

Tier 2 del ecosistema (paquete adicional, como `net`/`db`; política de tiers en DESIGN §53). Se
consume por dependencia de ruta/git en `ray.toml`:

```toml
[dependencies]
rpc = "path:../raylang/packages/rpc"
```

## El protocolo

```
frame     = [4 octetos big-endian: longitud N] [N octetos de payload JSON UTF-8]
petición  = {"id": n, "method": "m", "params": <Json>}
            (+ "deadline_ms": n opcional, + "traceparent": "00-…" opcional)
respuesta = {"id": n, "ok": <Json>}  |  {"id": n, "err": "mensaje"}
```

El `traceparent` (W3C, `net/trace` M88.3) viaja como **string opaco**: `rpc` no depende del
paquete `net`. Payload protobuf: diferido (el framing no cambia — solo el cuerpo).

## Servidor

```rust
import rpc/rpc;
from std/json import Json;

fn main() -> int {
    let r = rpc.serve_graceful("0.0.0.0", 7070, 5000, fn(req: rpc.Req) -> Result<Json, string> {
        if (req.method == "ping") { Result.Ok(Json.JStr("pong")) }
        else { Result.Err("método desconocido: " + req.method) }
    });
    0
}
```

- Una **fibra por conexión**; varias peticiones secuenciales por conexión. VM y binario nativo
  (fibras en ambos; el intérprete no tiene concurrencia).
- El handler corre en su **propia tarea** (`try_join`, como el webserver M56.5): un panic
  responde `err` sin tumbar la conexión ni el servidor.
- `Req` trae `method`, `params` (Json), `deadline_ms` (presupuesto declarado por el llamador;
  `0` = ninguno) y `traceparent` (opaco; parsea con `net/trace` si trazas).
- **Apagado ordenado de serie** (patrón M88.1b): `serve_graceful(host, port, drain_ms, handler)`
  cablea `signals()` (SIGTERM/SIGINT → dejar de aceptar, drenar con plazo, devolver 0);
  `serve_shutdown[_limits]` apaga con cualquier canal `stop` (testeable sin señales); `serve` es
  la forma que bloquea para siempre. **Un solo bucle**: `serve` = `serve_shutdown` con un canal
  que nunca llega.
- Límites: `Limits { max_frame_bytes }` (default 10 MiB) — un peer hostil no puede hacer
  reservar memoria sin tope.

## Cliente

```rust
let c = rpc.connect("127.0.0.1", 7070)?;
let pong = rpc.call(c, "ping", Json.JNull)?;                    // Result<Json, string>
let r2 = rpc.call_deadline(c, "consulta", params, 500);         // acota la espera (read timeout)
let r3 = rpc.call_full(c, "m", params, 500, trace.traceparent(trace.child(t)));
rpc.disconnect(c);
```

- Conexión **persistente**, request/response **secuencial**; el `id` correla y se **valida**
  (una respuesta con id inesperado es `Err`).
- `call_deadline` acota la ESPERA de la respuesta (read-timeout del socket) y además declara el
  presupuesto en el sobre (`Req.deadline_ms`), para que el servidor pueda descartar trabajo que
  no llegará a tiempo. **Ojo**: un deadline vencido deja la conexión desincronizada (la
  respuesta tardía quedaría en el buffer) → tras un timeout, `disconnect` y reconectar — o usa
  el pool, que lo hace solo.

### Pool de conexiones (M127)

Handlers concurrentes no pueden compartir UN `Client` (la conexión es secuencial); el pool da
hasta `size` llamadas **en vuelo a la vez** — una conexión por hueco, que es también paralelismo
real del lado servidor (una fibra por conexión):

```raylang
let p = rpc.pool("127.0.0.1", 7070, 8);
// desde CUALQUIER fibra, a la vez:
let r = rpc.pool_call(p, "consulta", params);                 // aparca si el pool está agotado
let r2 = rpc.pool_call_deadline(p, "lenta", params, 500);     // timeout → descarta ESA conexión
rpc.pool_close(p);
```

- Marcado **perezoso** (nada conecta hasta la primera llamada del hueco) y **reconexión
  automática**: un fallo (timeout, id inesperado, cierre del peer) descarta la conexión y deja el
  hueco vacío — la siguiente llamada re-marca. El "disconnect y reconecta" manual desaparece.
- El pool ES un canal acotado: checkout = `recv` (la fibra aparca si los `size` huecos están en
  vuelo → backpressure natural), release = `send`.

## Tests

`tests/rpc_cli.rs`: dos procesos `ray run` (servidor y cliente) → valida el **wire** de verdad;
la batería cubre params de ida y vuelta, `err` del handler, panic sin matar la conexión,
deadline vencido + reconexión, traceparent/deadline en el sobre, apagado ordenado por RPC y dos
clientes concurrentes.
