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

Declara el paquete en tu `ray.toml` como una **dependencia por ruta** (M40.8a) — o, cuando se publique,
como una dependencia git:

```toml
[dependencies]
net = "path:../ruta/a/packages/net"
```

y luego importa el módulo que necesites (como con `std/`):

```raylang
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
  `jwt_verify(secret: bytes, token) -> Result<string, string>`. Sobre `net/crypto` + `std/base64`.
- **`net/jwt_eddsa`** — JWT firmados con Ed25519 (EdDSA). Sobre `net/crypto` + `std/base64`.
- **`net/sigv4`** — firma AWS Signature V4 para peticiones. Sobre `net/crypto` + `std/url`.
- **`net/scram`** — el handshake SCRAM-SHA-256 (autenticación de PostgreSQL). Sobre `net/crypto` +
  `std/base64`.
- **`net/cookie`** — parseo y serialización de cookies HTTP. Sobre `std/url`.

### HTTP y HTTP/2

- **`net/http`** — cliente/servidor HTTP/1.1 en `bytes` (habla `https://` vía el TLS del runtime). Sobre
  `std/inflate` (gunzip). M90.2: conexiones persistentes (keep-alive) con `connect`/`conn_request`/
  `conn_close` — reusa el socket entre peticiones al mismo servidor (delimitación por
  Content-Length/chunked, reconexión y reintento transparente).
- **`net/http2`** — framing HTTP/2 (preface, SETTINGS, frames). Hoja.
- **`net/hpack`** — compresión de cabeceras HPACK (RFC 7541): `header`, `encode`, `decode` + tabla
  dinámica. Determinista. Hoja.
- **`net/http2_client`** — cliente HTTP/2 sobre `net/http2` + `net/hpack`.
- **`net/grpc_client`** — cliente gRPC sobre `net/http2` + `net/hpack` + `std/protobuf`.

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
- **`net/redis`** — cliente Redis (protocolo RESP). Hoja.
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
  si quieres exclusividad del puerto, escucha tú también en `0.0.0.0`.

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
servidores de juguete, no en el oráculo. Pendiente: `framework` (micro-framework web) — una interacción
de UFCS con imports calificados por resolver.
