# `net` — el paquete de red de raylang (adicional, **no** embebido)

A diferencia de la biblioteca estándar (`std/`, embebida en el binario base), el tier de **red y
protocolos** vive aquí, en un **paquete adicional**: son librerías que dependen de sockets/TLS o que solo
interesan a quien construye servicios, y serían peso muerto en el binario de todo el mundo. Se apoyan en
las `std/` embebidas para lo fundacional (`from std/base64 import …`).

Este es el **tier 2** del ecosistema (paquete adicional, no embebido). La regla de qué va aquí vs. en `std/`
vs. como demo en `examples/` está en la **política de tiers** ([DESIGN.md](../../DESIGN.md) §53); la
instalación por nombre desde un registro central se diseña en §54 (M51). Hoy se consume por dependencia de
ruta/git en `ray.toml`.

**Criptografía de producción (M43)**: el módulo `net/crypto` expone SHA/HMAC/Ed25519 respaldados por
`ring` (tiempo constante, auditado) en la forma (`[int]`/hex) que estos módulos consumen. Las
implementaciones en raylang puro (`examples/web/sha256.ray`, etc.) se conservan como **demostración del
lenguaje**, no como el backend de producción: correctas, pero sobre la VM interpretada no garantizan
resistencia a canales laterales de temporización (requisito para tocar secretos reales).

## Cómo usarlo

Declara el paquete en tu `ray.toml`. El camino recomendado es el **índice** (`ray add net`
lo escribe por ti; con el índice oficial por defecto no hay nada que configurar):

```toml
[dependencies]
net = "^0.3"
```

Si desarrollas en el monorepo, por ruta (`net = "path:../ruta/a/packages/net"`); la dependencia
git directa (`git+https://github.com/ray-language/net@v0.3.7`) queda para un pin sin índice.

y luego importa el módulo que necesites (como con `std/`):

```rust
import net/jwt;

fn main() -> int {
    let tok = jwt.jwt_sign(to_bytes("secreto"), "{\"sub\":\"ada\"}");
    print(tok);
    match (jwt.jwt_verify(to_bytes("secreto"), tok)) {
        Result.Ok(payload) => { print(payload); },
        Result.Err(e) => { print("firma inválida: " + e); },
    }
    0
}
```

## Módulos

### Autenticación y firma (deterministas)

- **`net/jwt`** — JSON Web Tokens HS256: `jwt_sign(secret: bytes, payload_json) -> string`,
  `jwt_verify(secret: bytes, token) -> Result<string, string>`. M347: `jwt_sign_kid(secret, kid,
  payload)` pone el `kid` en la cabecera (rotación de claves vía JWKS), `jwt_header(token)` /
  `jwt_kid(token)` la leen SIN verificar (para elegir la clave) y `jwt_check_claims(payload, iss,
  aud)` comprueba emisor y audiencia (string o lista) del payload ya verificado — sirve también para
  EdDSA. Sobre `net/crypto` + `std/base64`.
- **`net/jwt_eddsa`** — JWT firmados con Ed25519 (EdDSA); `jwt_eddsa_sign_kid` para el `kid`. Sobre
  `net/crypto` + `std/base64`.
- **`net/sigv4`** — firma AWS Signature V4 para peticiones. Sobre `net/crypto` + `std/url`.
- **`net/scram`** — el handshake SCRAM-SHA-256 (autenticación de PostgreSQL). Sobre `net/crypto` +
  `std/base64`.
- **`net/session_store`** — el almacén de sesiones de `web` como actor con protocolo (`Msg`:
  get/set/delete/drop/sweep), `memory(path, persist, ttl_s)` (RKV1 opcional) y `from_channel` para
  backends propios; el SQLite vive en `db/sessions` (M350). Sobre `std/kv`.
- **`net/cookie`** — parseo y serialización de cookies HTTP (`with_path`/`with_domain`/`with_max_age`/
  `with_http_only`/`with_secure`/`with_same_site`). Sobre `std/url`.

### HTTP y HTTP/2

- **`net/http`** — cliente/servidor HTTP/1.1 en `bytes` (habla `https://` vía el TLS del runtime). Sobre
  `std/inflate` (gunzip). M90.2: conexiones persistentes (keep-alive) con `connect`/`conn_request`/
  `conn_close` — reusa el socket entre peticiones al mismo servidor (delimitación por
  Content-Length/chunked, reconexión y reintento transparente). M318: `pool(size)` +
  `pool_fetch`/`pool_request`/`pool_request_bytes` + `pool_close` — un pool de `Conn`s keep-alive
  compartido por todas las fibras (un proxy que abre una conexión por petición agota los puertos). M108: **streaming** —
  `stream`/`stream_with` devuelven status y cabeceras en cuanto llegan y `stream_read` entrega el
  cuerpo a trozos según llegan (des-chunkeado incremental; plazo de OCIO por lectura, no total;
  `Ok(None)` = fin limpio, truncado = `Err`). Para respuestas que se generan en vivo (tokens de un
  LLM, logs) pintando cada trozo al llegar. M347: `Response.raw_headers` guarda TODAS las líneas de
  cabecera en orden (también las repetidas, que el `Map` `headers` no puede) — `header_all(r, name)`
  y `set_cookies(r)` las leen.
- **`net/sse`** — cliente **Server-Sent Events** (`text/event-stream`) sobre `net/http.stream`:
  `open(url, headers)` suscribe (sin plazo de ocio: un stream sano puede callar minutos) y
  `next(es)` entrega cada `Event { data, event, id }` (data multilínea unida con `\n`; los
  comentarios keep-alive y los eventos sin data no se despachan, según la spec). El decodificador
  `decode(bytes) -> Option<(Event, int)>` es **puro** (mismo patrón que `term.decode`): el
  acumulador es bytes y solo se decodifica a texto por línea completa, así un trozo puede partir
  un evento — incluso un carácter UTF-8 — por cualquier octeto. `retry:`/reconexión: fuera de v1
  (el llamador conserva `id`).
- **`net/http2`** — framing HTTP/2 (preface, SETTINGS, frames). Hoja.
- **`net/hpack`** — compresión de cabeceras HPACK (RFC 7541): `header`, `encode`, `decode` + tabla
  dinámica. Determinista. Hoja.
- **`net/http2_client`** — cliente HTTP/2 sobre `net/http2` + `net/hpack`.
- **`net/grpc_client`** — cliente gRPC de un solo disparo sobre TLS (`grpc_call`), sobre `net/http2` +
  `net/hpack` + `std/protobuf`.
- **`net/grpc_server`** — **servidor gRPC unario** sobre HTTP/2 en claro con conocimiento previo (h2c:
  lo que hablan `grpcurl -plaintext` y los clientes Go/Java sin TLS), M321 (findings #63, subido desde
  `libs/grpc` de raymart): `bind(host, port)` (puerto 0 = efímero, `l.port`), `serve(l, handler)` /
  `serve_until(l, stop, opts, handler)` / `serve_router[_until](l, …, build)` con un `Router`
  (`route(path, f)`, `fallback(f)`, `dispatch`) construido DENTRO de cada fibra de conexión; el handler
  recibe un `Call { path, message, metadata, deadline }` y devuelve `Result<bytes, Status>`. Control de
  flujo, `grpc-timeout` → `deadline` (y DEADLINE_EXCEEDED al vencer), metadata, `grpc-message`
  percent-encoded, tope de 4 MiB (`Options.max_message`, RESOURCE_EXHAUSTED), UNIMPLEMENTED para rutas
  sin handler, `serve_conn` para bucles de accept propios. Sobre `net/grpc_h2`.
- **`net/grpc_conn`** — cliente gRPC unario sobre una **conexión persistente**: `connect(host, port)`
  (h2c) o `connect_tls(host, port)` (ALPN `h2`), `call(c, path, message, metadata, deadline_ms) ->
  Result<Reply, string>` (`Err` = transporte; un estado no-OK es `Ok(Reply)` con `status` y
  `status_message`; `into_result`/`outcome` lo vuelven `Result<bytes, Status>`), `call_once`,
  `usable`, `disconnect`. Un plazo vencido cancela el stream (RST_STREAM) y la conexión sigue usable.
- **`net/grpc_status`** — los códigos gRPC (`OK`…`UNAUTHENTICATED`), `Status { code, message }`,
  `new`/`fail`, `code_name`, `from_http` y el percent-encoding de `grpc-message`. Hoja.
- **`net/grpc_h2`** — el núcleo de conexión HTTP/2 que comparten servidor y cliente (`Link`: frames
  con CONTINUATION fusionado y padding quitado, SETTINGS/PING/WINDOW_UPDATE/GOAWAY, HPACK por
  dirección, ventanas de envío). Sobre `net/http2` + `net/hpack`.

### Transporte y servicios (dependen de sockets)

- **`net/udp`** — sockets UDP: `bind`/`send_to`/`recv_from`. Hoja.
- **`net/dns`** — resolución DNS (7 tipos de registro). Sobre `net/udp`.
- **`net/ntp`** — cliente SNTP v4 (RFC 4330): `query(host, port)` → hora del servidor + offset/delay
  del reloj local (ms Unix) + stratum. Sobre `net/udp` (M90.7).
- **`net/dns_cache`** — caché DNS con TTL. Sobre `net/dns`.
- **`net/websocket`** — handshake + framing WebSocket (`ws://`/`wss://`). Sobre `net/crypto` + `std/base64`.
  Lectura robusta (M58.1): `WsConn` + `read_frame`/`read_message` (tramas partidas/pegadas, ping→pong
  automático, fragmentación reensamblada, límite de payload validado antes de leer).
- **`net/websocket_client`** — cliente WebSocket. Sobre `net/websocket` + `std/base64`. `connect`/
  `connect_tls` devuelven un `WsConn` con estado (M58.1).
- **`net/redis`** — cliente Redis (protocolo RESP). Hoja. M318: `pool(host, port, size)` + `pool_command`
  (reintento único sobre una conexión fresca si la reutilizada falla por el cable; `pool_command_with(p,
  args, false)` para comandos que no deben repetirse) + `pool_close`.
- **`net/pool`** — el pool de conexiones **genérico** (M318, findings #68/#69): un canal de huecos
  `Slot<T>` que las fibras comparten aunque no compartan heap. `new<T>(size)`, `acquire` (Ready-first,
  aparca si está agotado), `release`/`release_empty`, `run(p, dial, drop, op, retry)` (descarta la
  conexión ante un fallo de cable y repite `op` una vez sobre una fresca si venía del pool),
  `run_tx(p, dial, drop, begin, op)` (M320: repite solo el preámbulo `begin` — un `BEGIN` — si
  falló por el cable sobre una conexión reutilizada), `shutdown`. Es la base de `http.pool`, `redis.pool` y de los pools de `db/*`. Hoja.
- **`net/postgres`** — cliente PostgreSQL (protocolo de frontend/backend). Sobre `net/scram`.
- **`net/oauth2`** — flujo OAuth2 (client credentials, authorization code). Sobre `net/http` + `std/json`
  + `std/url`.
- **`net/webserver`** — servidor HTTP async + SSE (sobre el scheduler de fibras). Sobre `std/url`.
  Con límites de seguridad por defecto (M56.1/M56.4: cabeceras 64 KiB, cuerpo 10 MiB, 1024 conexiones
  simultáneas, 10 s para leer una petición — anti-slowloris; configurables con
  `serve_limits`/`serve_raw_limits`/`read_request_limits` + `Limits`).
  El `path` de la petición llega percent-decodificado y sin query string (M56.2); la query va aparte
  (`req.query` cruda, `query_params(req)` parseada). HTTPS con `serve_tls`/`serve_raw_tls[_limits]`
  (M56.3: cert/clave en PEM; upgrade TLS por conexión, en su fibra). Un handler que panica responde
  500 y cierra su conexión sin tumbar el servidor ni fugar recursos (M56.5, vía `try_join`).
  `serve`/`serve_tls` mantienen la conexión viva entre peticiones (M56.6: keep-alive HTTP/1.1;
  honran `Connection: close` y el ocio lo corta el read timeout); `serve_raw` sigue siendo
  una-petición-y-cerrar (el handler crudo posee la conexión — SSE). Varias cookies por respuesta
  con `Response.set_cookie`/`with_cookie` (M56.7: una línea `Set-Cookie` por cookie). Cuerpos
  `Transfer-Encoding: chunked` entrantes decodificados, archivos estáticos con
  `static_response(dir, req.path)` (saneo de `..`, mime por extensión, `index.html`), y HEAD
  responde cabeceras sin cuerpo (M56.8). **Estáticos de producción** (M56.9):
  `static_mount(prefix, dir, req)` monta un directorio bajo un prefijo de URL
  (`static_mount("/static/", "assets", req)` sirve `/static/app.css` desde `assets/app.css`) y
  añade **caching HTTP** — cada 200 lleva un `ETag` fuerte (tamaño+mtime del archivo, sin hashear
  por petición) y un `If-None-Match` que casa responde `304 Not Modified` sin releer ni reenviar
  el cuerpo — más 405 (`Allow: GET, HEAD`) para otros métodos y 404 fuera del prefijo. El
  `Cache-Control` lo pone el llamador si lo quiere (`r.headers.insert("Cache-Control", …)`);
  `mime_of(file)` es `pub` para handlers propios. **Apagado ordenado** (M88.1b):
  `serve_graceful(host, port, drain_ms, handler)` — con SIGTERM/SIGINT (vía `signals()`, M88.1)
  deja de aceptar, drena las conexiones en vuelo con plazo y devuelve 0 (cero peticiones perdidas
  al desplegar); la forma general `serve_shutdown[_limits]` apaga con cualquier canal `stop`
  (testeable sin señales).
  **Gotcha del puerto "ocupado" (macOS/BSD)**: un puerto tomado en la MISMA dirección sí falla
  claro (`serve` devuelve `Err("Address already in use")` — decide el llamador). Pero atarse a
  `127.0.0.1:P` con otra app escuchando en `0.0.0.0:P` **no es error para el SO**: ambos listeners
  coexisten (semántica BSD + `SO_REUSEADDR`, que Rust pone en todo listener) y la dirección más
  específica gana el tráfico de loopback — tu servidor SÍ atiende `localhost:P` mientras la otra
  app sigue en el resto de interfaces. No hay error que reportar ni forma portable de detectarlo;
  si quieres exclusividad del puerto, escucha tú también en `0.0.0.0`. M110:
  **streaming** (`stream_response(status, ch)`: el cuerpo son los trozos de un `Channel<bytes>`,
  en chunked según llegan — el handler `spawn`ea al productor y devuelve ya; canal acotado =
  backpressure) y **HTTP Range** en `static_mount` (`Accept-Ranges`/206/`Content-Range`/416,
  `If-Range` contra el ETag; multi-rango cae a 200 completo). **M271 (raystream)**: un estático de
  disco de 1 MB o más se sirve **por trozos desde el archivo** (`fs.open` + `seek` + `read_bytes`
  de 256 KB en un productor), nunca leído entero: un `Range` de 11 bytes sobre un vídeo de 1 GB
  cuesta 256 KB de memoria, no 1 GB. **M279**: con la cola por defecto (1) NO hay productor ni
  canal: la respuesta lleva un `FileBody` (ruta, rango, trozo) y la fibra de la conexión lee y
  escribe el fichero trozo a trozo — un trozo vivo por conexión (medido: 2,1 MB → 0,66 MB por
  conexión a 32 clientes, el mismo coste que un bucle a mano, con más caudal). `serve_file_with(file,
  req, chunk_bytes, queue)` y `static_mount_with(prefix, dir, req, chunk_bytes, queue)` ajustan el
  trozo (≥ 4 KB); `queue >= 2` recupera el productor en su fibra con lectura adelantada (M271/M276),
  que solo compensa cuando el disco es más lento que el cliente. **`stream_response_len(status, ch, length)`**: un stream de
  tamaño conocido va con `Content-Length` (sin chunked) y **keep-alive** — descargas y medios con
  barra de progreso; el productor debe enviar exactamente `length` octetos (si cierra antes, la
  conexión se cierra). **`send_response_for(req, conn, r)`** (M283, raystream [23]): como `send_response` pero sabiendo la
  petición — un HEAD lleva las cabeceras completas y ningún cuerpo (también con streams y
  cuerpos-fichero); es lo que un handler crudo debe usar cuando la petición pueda ser un HEAD.
  **`serve_raw_with(host, port, make_handler)`**: estado para un handler
  crudo por fábrica llamada en la fibra de cada conexión (como `serve_with`); en un binario nativo
  un closure guardado en una variable no puede cruzar a las fibras de conexión (el compilador lo
  dice con el nombre): escríbelo inline en la llamada, nombra una función o usa la fábrica.

### Observabilidad

- **`net/time`** — COMPAT (M57.1): las fechas civiles UTC viven ahora en **`std/time`**; este módulo
  reexporta (`now_utc`, `from_epoch_millis`, `to_iso8601`, `date_stamp`, …). Para código nuevo,
  importa `std/time`. (`now_utc` no
  es determinista; el formateo sí.)
- **`net/log`** — logging estructurado (niveles, campos, JSON). Sobre `net/time`. M88.3:
  `with_trace(lg, trace_id)` estampa un campo `trace_id` en cada línea (correlación distribuida).
- **`net/trace`** (M88.3) — tracing distribuido W3C Trace Context (`traceparent`): `Trace`
  (`trace_id`/`span_id`/`flags`), `new_trace`, `child`, `traceparent`/`parse_traceparent`,
  `from_headers`. El webserver adopta el trace entrante con `trace_of(req)`; el cliente http lo
  propaga con `request_traced`/`fetch_traced` (un span hijo por salto). Hoja (solo `std/random`).
- **`net/metrics`** — métricas estilo Prometheus (counter/gauge/histogram + labels), `render` en formato
  de exposición. Hoja.

Los que dependen de **sockets vivos** (http/http2/websocket/dns/udp/redis/postgres/oauth2) se prueban con
servidores de juguete, no en el oráculo. El micro-framework web vive en **`packages/web`** (M93,
promovido de examples): enrutado, estáticos con ETag/304, logging JSON, TLS y graceful sobre
`net/webserver` — ver `docs/web-framework.md`.

## Licencia

[Apache License 2.0](LICENSE) (M281). Copyright 2026 Roberto Ayala.
