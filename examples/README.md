# Ejemplos de raylang

Programas de ejemplo, organizados por categoría. Ejecútalos con:

```sh
cargo run -- examples/<categoría>/<archivo>.ray          # intérprete
cargo run -- --vm examples/<categoría>/<archivo>.ray     # VM (necesaria para concurrencia/red)
```

## `basics/` — fundamentos
Control de flujo, funciones, recursión y aritmética.
`fib` · `fizzbuzz` · `funciones` · `gcd` · `primes` · `palindromo` · `matematicas` · `inferencia`

## `data/` — datos y colecciones
Arreglos, structs, enums, `Map`, matrices y estructuras enlazadas.
`arrays` · `structs` · `enums` · `mapa` · `matriz` · `lista_enlazada` · `lista_recorrido` · `pila` · `inventario` · `match_figuras`

## `types/` — sistema de tipos
Genéricos, traits, bounds, trait objects, `Option`/`Result` y anotaciones.
`genericos` · `tipos_genericos` · `bounds` · `traits` · `trait_objects` · `impls_genericos` · `metodos_por_defecto` · `opcional` · `errores` · `anotaciones`

## `stdlib/` — funcional y biblioteca estándar
Closures, UFCS, pipelines, strings y orden superior (`map`/`filter`/`fold`/`sort`).
`closures` · `ufcs` · `pipelines` · `strings` · `stdlib` · `ordenar`

## `concurrency/` — concurrencia (solo VM)
Green threads (CSP): `spawn`/canales, structured concurrency y `select`.
`concurrencia` · `structured` · `select`

## `io/` — entrada/salida y sistema
Archivos, datos binarios (`bytes`), reloj y aleatoriedad.
`io` · `archivos` · `binario` · `reloj_aleatorio`

## `net/` — redes (TCP crudo)
Cliente y servidor TCP; servidor concurrente (solo VM).
`tcp_cliente` · `tcp_servidor` · `servidor_concurrente`

## `web/` — la capa web
Librerías en raylang (importables) + sus demos. **El servidor es solo VM.**
- **Librerías**: `http` (cliente HTTP/HTTPS), `json` (parse/stringify), `webserver` (servidor + SSE),
  `websocket` (handshake + framing), `sha1` / `sha256` / `hmac` / `base64` / `hex` (cripto),
  `jwt` (JSON Web Tokens HS256), `uuid` (v4), `url` (percent-encoding + query string), `cookie`,
  `time` (fechas/horas UTC: ISO 8601, RFC 1123, duraciones), `redis` (cliente RESP2 sobre TCP),
  `udp` (datagramas sin conexión), `sigv4` (firma de requests AWS Signature V4),
  `inflate` (descompresión DEFLATE/gzip/zlib + CRC-32), `deflate` (compresión DEFLATE/gzip/zlib, LZ77 + Huffman fijo),
  `log` (logging estructurado en JSON), `metrics` (métricas Prometheus: counters/gauges/histogramas).
- **`metrics_server_demo`**: un endpoint `/metrics` real (monta `metrics` sobre `webserver`,
  escrapeable por Prometheus).
- **`dns`** / **`dns_cache`**: un cliente DNS (RFC 1035) sobre UDP que resuelve registros A/AAAA/MX/
  CNAME/TXT (con compresión de nombres e IPv6 canónica) + una caché que respeta el TTL.
- **`oauth2`**: un cliente OAuth 2.0 (grant client_credentials, URL de autorización, cabecera Bearer)
  sobre `http`/`json`/`url`.
- **`websocket_client`**: un cliente WebSocket (`ws://`) con tramas enmascaradas (`websocket_client_demo`
  habla con el servidor `websocket_echo`).
- **`protobuf`**: el códec del formato wire de Protocol Buffers (proto3) + framing de gRPC.
- **Demos**: `http_demo`, `json_demo`, `https_demo`, `webserver_demo`, `websocket_demo`,
  `websocket_echo` (echo `ws://`), `wss_echo` (echo `wss://`), `crypto_demo` (vectores SHA-1/base64).
- **`framework`**: un micro-framework web tipo Express (enrutado, parámetros de ruta, middleware,
  respuestas) sobre `webserver`, como **librería reutilizable**. `framework_demo` lo importa y usa la
  API por punto (`app.GET(...)`, `r.text(...)`) — UFCS resuelve las funciones importadas.

## Módulos (multi-archivo)
Ejemplos del sistema de módulos, con su propia estructura de directorios:
- `modulos/` — `import` + `from … import` entre archivos.
- `capsula/` — encapsulación con `mod.ray` (cápsulas) y reexports.
- `proyecto/` — un proyecto con submódulos por directorio (`geo/formas/circulo`).
