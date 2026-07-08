# La biblioteca estándar de raylang (`std/`)

Módulos de biblioteca **escritos en raylang**, importables con la sintaxis de módulos por ruta:

```raylang
import std/math;

fn main() -> int {
    print(math.gcd(48, 36)); // 12
    0
}
```

A diferencia del **prelude** (que se inyecta automáticamente en cada programa: `Option`/`Result`,
`map`/`filter`/`fold`, `Iterator`, …), la `std/` es **opcional**: solo se carga lo que se importa. (Las
colecciones `Set`/`Deque`/`StringBuilder` vivían en el prelude; M50.2 las movió a `std/collections/`.)

`std/` es el **tier 1** del ecosistema (embebido, universal, estable). Qué va aquí vs. en un paquete
adicional (`packages/*`) vs. queda como demo en `examples/` lo fija la **política de tiers** en
[DESIGN.md](../DESIGN.md) §53 (criterios: universalidad · peso e independencia · estabilidad de API ·
seguridad). La instalación de paquetes adicionales (por nombre, cuando exista el registro) → §54.

## Empaquetado en el binario

Desde M40.5 la `std/` va **embebida en el ejecutable** (`include_str!`, como el prelude): estos `.ray`
del repo se compilan dentro del binario, así `ray run prog.ray` con `import std/math;` funciona **sin que
`std/` exista en disco** — el binario es auto-contenido. El loader resuelve `std/…` contra la fuente
embebida (`src/stdlib.rs`) antes de tocar el filesystem, de modo que el prefijo `std/` queda **reservado**.

Estos archivos siguen siendo la **única fuente de verdad**: se editan aquí y `include_str!` los recompila.
Los módulos son **públicos** (`pub`) y están documentados con comentarios `///`; genera su documentación
con `ray doc std/math.ray` (que lee el archivo directamente).

**Añadir un módulo**: crea `std/<nombre>.ray` y añade su fila a `MODULOS` en `src/stdlib.rs`.

## Módulos

- **`std/math`** — utilidades enteras que complementan los builtins matemáticos: `iabs`, `sign`,
  `clamp`, `gcd`, `lcm`, `ipow`, `factorial`, `is_prime`.
- **`std/text`** — utilidades de string más allá de los builtins: `is_empty`, `pad_left`, `pad_right`,
  `capitalize`, `reverse`, `count`, `words`.
- **`std/sort`** — orden y búsqueda sobre arreglos genéricos (`T: Ord`), alrededor del `sort` del prelude:
  `is_sorted`, `sort_desc`, `min`, `max`, `binary_search`, `dedup`, `merge`.
- **`std/net`** (M50.3) — transporte de red: `net.tcp_connect`/`tcp_listen`/`tcp_accept`,
  `net.tls_connect`/`tls_connect_h2`/`tls_accept`, `net.socket_read`/`socket_write`/`socket_read_bytes`/
  `socket_write_bytes`, `net.local_port`. Los sockets se cierran con `close` (global). UDP vive aparte en
  el módulo `net/udp` del paquete `net`.
- **`std/collections/{set,deque,stringbuilder}`** (M50.2) — estructuras de datos puras en raylang, en
  submódulos (leaf-binding): `import std/collections/set;` → `set.new`/`add`/`has`/`remove`/`size`/`items`
  (hash set sobre `Hash`+`Eq`); `import std/collections/deque;` → `deque.new`/`push_back`/`push_front`/
  `pop_back`/`pop_front`/`peek_front`/`len`/`is_empty`; `import std/collections/stringbuilder;` (o `as sb`)
  → `sb.new`/`push`/`build`/`count`. Los tipos se namespacan al submódulo (`set.Set`/`deque.Deque`/
  `stringbuilder.StringBuilder`).

### Encoding (promovidas de `examples/`, M40.7a)

Estas librerías **ya existían** como ejemplos y se promueven a `std/` embebiéndolas: la fuente es el
`examples/web/*.ray` original (no se duplica), el ejemplo sigue siendo el artefacto pedagógico y a la vez
la fuente del módulo `std/`.

- **`std/hex`** — `hex_encode(data: [int]) -> string`, `hex_decode(s) -> Result<[int], string>`.
- **`std/base64`** — `base64`/`base64url` (encode) y `base64_decode`/`base64url_decode` (`Result`).
- **`std/url`** — `url_encode`/`url_decode`, `parse_query`/`build_query` (sobre `Map<string, string>`).
- **`std/json`** — `enum Json`, `parse(s) -> Result<Json, string>`, `stringify(j) -> string`.

### Hashing (M40.7b)

Operan sobre `bytes` (convierte un `string` con el builtin `to_bytes`).

- **`std/sha256`** — `sha256_octets`/`sha256`/`sha256_hex`.
- **`std/sha512`** — `sha512_octets`/`sha512`/`sha512_hex`.
- **`std/sha1`** — `sha1`/`sha1_hex`.
- **`std/hmac`** — `hmac_sha256`/`hmac_sha256_hex` (sobre `std/sha256` + `std/hex`).

### Compresión (M40.7c)

- **`std/inflate`** — `inflate_raw`/`zlib_inflate`/`gunzip` (DEFLATE/zlib/gzip), `crc32`.
- **`std/deflate`** — `deflate_raw`/`zlib_compress`/`gzip_compress` (sobre `std/inflate` para el CRC).
- **`std/huffman`** — `huffman_encode`/`huffman_decode`.

### Procesamiento de texto y datos (M40.7d)

- **`std/regex`** — motor NFA lineal (Thompson): `full_match`, `search`, `find`, `find_str`, `find_all`,
  `replace_all`.
- **`std/csv`** — `parse_csv(src) -> Result<[[string]], string>`, `write_csv(rows) -> string`.
- **`std/toml`** — `enum TomlValue`, `parse_toml`, `toml_get`, `toml_show` (subconjunto de TOML).
- **`std/template`** — motor de plantillas HTML con autoescape: `enum TVal`, `ctx_*`, `render`.

### Criptografía y serialización (M40.7e)

- **`std/chacha20`** — `chacha20_block`, `chacha20_encrypt` (cifrado de flujo RFC 8439).
- **`std/poly1305`** — `poly1305_mac` (MAC).
- **`std/chacha20poly1305`** — AEAD `aead_seal`/`aead_open` (`struct Sealed`; sobre chacha20 + poly1305).
- **`std/ed25519`** — `ed25519_public_key`/`ed25519_sign`/`ed25519_verify` (firmas EdDSA; sobre `std/sha512`).
- **`std/protobuf`** — writer/reader de Protocol Buffers: `writer`/`write_varint`/`write_string`/`finish`/
  `parse`/`get_int`/`get_string` + enmarcado gRPC.
- **`std/uuid`** — `uuid_v4()` (RFC 4122 v4, sobre el PRNG del runtime) y `is_uuid_v4(s)` (validación).

## Lo que NO está en `std/` (tier de red)

Las librerías que dependen de sockets/TLS y no son deterministas —`udp`, `dns`, `http`, `http2`,
`websocket`, `grpc`, `postgres`, `redis`, `oauth2`, `jwt`, `scram`, `sigv4`, `framework`, `webserver`…—
**siguen en `examples/`**: son un *framework de aplicación*, no una biblioteca estándar. Se importan
directamente desde ahí, o esperan a un paquete `net`/aplicación propio (ver DESIGN §42.9).
