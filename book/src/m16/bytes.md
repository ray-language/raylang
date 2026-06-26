# Datos binarios: el tipo `bytes`

M15 conectó raylang al mundo, pero con una deuda escrita en el propio código: la carga útil de los
sockets y los archivos era `string`. Y un `string` en raylang es **UTF-8 válido**. En el momento en
que llega un octeto que no forma parte de una secuencia UTF-8 legal —el byte `0xFF`, un `0x00` en
medio, los bytes de una imagen PNG o un `.zip`— la conversión a `string` lo **corrompe** (lossy). Para
texto funciona; para datos binarios, no.

M16 salda esa deuda con un tipo nuevo: **`bytes`**, una secuencia **inmutable** de octetos (0–255), sin
la restricción de ser UTF-8. Es el primer tipo nuevo desde `char` (M11.4c), y el cimiento de todo lo
binario que viene después (HTTP binario en M19, el framing de WebSockets, y en su día TLS).

## Hermano de `string`, no pariente lejano

La decisión de diseño es deliberada: `bytes` es el **hermano** de `string`. Ambos son inmutables, ambos
se construyen por literal o concatenación, y su representación en runtime se espeja:

- En el **intérprete**, `Value::Bytes(Rc<Vec<u8>>)` (como `Str` con su `Rc`).
- En la **VM**, *inline* en el valor (`HeapValue::Bytes(Vec<u8>)`), como `Str` — **no es un objeto del
  heap ni lo traza el GC**, porque no contiene handles.

El literal es `b"..."`, con los mismos escapes que un string (`\n`, `\t`, `\\`, `\"`, `\r`) más uno
propio: **`\xNN`**, un octeto arbitrario en hexadecimal. Así se escribe lo que un string no puede:

```rust
let cabecera: bytes = b"RAY\x00\x01";   // incluye un byte nulo
let crudo: bytes = b"\xff\xfe";          // dos octetos que NO son UTF-8 válido
```

Las operaciones son las que esperarías de una secuencia: `len(b)` (número de octetos), indexar
`b[i] -> int` (el valor del octeto, 0–255), igualdad `==` estructural, y concatenación `b1 + b2`.

## Interoperar con texto

Un `bytes` y un `string` son mundos distintos, pero hay puentes explícitos:

- `to_bytes(s) -> bytes` — codifica un string a sus octetos UTF-8.
- `from_utf8(b) -> Result<string, string>` — decodifica octetos a string, **fallando** (con `Result`)
  si no son UTF-8 válido. La falibilidad es honesta: no todo `bytes` es texto.

`from_utf8` sigue el patrón de M11.2: un primitivo `__from_utf8` devuelve un arreglo etiquetado y un
envoltorio en el prelude lo traduce a `Result` → el runtime no sabe de `Result`.

## La I/O binaria y un *gotcha* del arreglo homogéneo

Con el tipo en su sitio, M16.1c añade la I/O que de verdad lo aprovecha: `read_file_bytes`/
`write_file_bytes` (disco) y `socket_read_bytes`/`socket_write_bytes` (red). Ahora un round-trip
binario conserva cada octeto, incluidos `\x00` y `\xff`.

Aquí asoma un detalle bonito del diseño de raylang. La I/O falible de M11.2 usa el truco del **arreglo
etiquetado**: el primitivo devuelve `["ok", payload]` o `["err", msg]`, y el prelude lo convierte a
`Result`. Pero un arreglo en raylang es **homogéneo**: no puede mezclar un tag `string` con un payload
`bytes`. La solución es elegante: las **lecturas** devuelven `[bytes]` con el tag *también* en bytes
(`[b"ok", datos]` / `[b"err", msg_utf8]`), y el prelude desempaqueta el mensaje de error con `from_utf8`;
las **escrituras** siguen con `[string]`, porque su payload de éxito es solo un contador.

`socket_read_bytes` además se integra con el scheduler de M15.5: cede la fibra en `WouldBlock`, igual
que `socket_read`. Como toca runtime, se valida con el **oráculo** (intérprete ↔ VM), incluido el estrés
del GC.

## Lo que `bytes` enseña

El grueso de añadir `bytes` fue **mecánico** —literal en el lexer, tipo en el checker, valor por motor—,
porque el lenguaje ya tenía el molde de `char`/`string`. El runtime solo creció donde era inevitable
(un valor nuevo, un puñado de builtins), y el checker apenas cambió: un builtin es una fila en la tabla.
Esa es la recompensa de haber construido el lenguaje por capas: añadir un tipo de primera clase, en M16,
ya no es un terremoto, es un patrón.
