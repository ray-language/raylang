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
`map`/`filter`/`fold`, `Set`/`Deque`/`StringBuilder`, `Iterator`, …), la `std/` es **opcional**: solo
se carga lo que se importa.

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

(Más módulos por venir; ver DESIGN §42.9.)
