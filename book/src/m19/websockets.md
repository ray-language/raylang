# WebSockets

HTTP es petición-respuesta: el cliente pregunta, el servidor contesta, se acabó. Para un chat, una
notificación en vivo o un juego, esa forma no encaja — quieres un canal **bidireccional** que quede
abierto y por el que ambos lados manden mensajes cuando quieran. Eso es **WebSocket** (RFC 6455): arranca
como una petición HTTP normal y, si el servidor acepta, la conexión TCP **se reaprovecha** para un
protocolo de mensajes propio. M19.3 lo implementa sobre `ws://`, entero como librería de raylang.

Lo interesante es lo que el protocolo nos **obliga** a tener primero.

## M19.3a — el lenguaje no sabía contar en binario

El handshake de WebSocket exige calcular `SHA-1(clave + GUID)`, y SHA-1 es aritmética de 32 bits a base
de `AND`, `OR`, `XOR` y desplazamientos. raylang tenía `&&`/`||` (lógicos) pero **ninguna operación bit a
bit**. Sin ellas, ni SHA-1 ni el framing son escribibles.

La decisión fue añadirlas como **operadores**, no como funciones (`band`/`shl`/…). El motivo es
legibilidad: una ronda de SHA-1 escrita con operadores —`(b & c) | (~b & d)`— se lee de un vistazo,
mientras que `bor(band(b,c), band(bnot(b),d))` es ruido. Y la precedencia bit a bit es un clásico de los
lenguajes con sintaxis de C, justo el terreno que este proyecto quiere recorrer. Se añadieron seis
operadores:

```raylang
a & b    a | b    a ^ b    ~a    a << n    a >> n
```

con precedencia estilo C (`|` por debajo de `^`, por debajo de `&`, por debajo de la igualdad; los
desplazamientos entre la comparación y la suma). Operan sobre `int` (64 bits) y los desplazamientos usan
una semántica *envolvente* (`wrapping`), idéntica en el intérprete y la VM — lo verifica un oráculo.

### El acertijo de `>>` y los genéricos

Hay una trampa famosa al meter `>>` en un lenguaje con genéricos: en `Caja<Caja<int>>`, esos dos `>`
**cierran dos niveles**, no son un desplazamiento. El lexer, que lee de izquierda a derecha, junta el
`>>` en un solo token. La solución (la misma de Rust y Java) es no tocar el lexer y arreglarlo en el
parser: cuando está cerrando argumentos de tipo y se encuentra un `>>`, lo **parte** en dos `>` —consume
uno y deja el otro para el nivel de fuera—. Un detalle pequeño con una lección grande: a veces el token
correcto depende del **contexto gramatical**, no solo de los caracteres.

## M19.3b — SHA-1 y base64, en raylang y sin tocar el runtime

Con los bits en su sitio, las dos piezas criptográficas se escriben **enteras en raylang**
(`examples/web/sha1.ray`, `examples/web/base64.ray`). Y aquí pasó algo bonito: **no hizo falta ni un builtin
nuevo**. Para leer el mensaje octeto a octeto bastaba con indexar `bytes` —`b[i]` ya daba un `int` desde
M16— y el digest de 20 octetos se modela como un `[int]` corriente.

SHA-1 trabaja con palabras de 32 bits, pero el `int` de raylang tiene 64. La técnica es enmascarar tras
cada operación que pueda desbordar:

```raylang
fn mask32() -> int { 4294967295 } // 0xFFFFFFFF

fn rotl(x: int, n: int) -> int {
    ((x << n) | (x >> (32 - n))) & mask32()
}
```

`base64` (RFC 4648) es aún más directo: agrupa de tres octetos en tres y emite cuatro caracteres,
rellenando con `=`. Ambas se validan contra **vectores estándar** —`SHA-1("abc")`, `base64("Man")`— y,
la prueba de fuego, contra el ejemplo canónico del propio RFC 6455:

```
base64(SHA-1("dGhlIHNhbXBsZSBub25jZQ==" + GUID)) == "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
```

idéntico bit a bit en los dos motores.

## M19.3c — el handshake, las tramas y un eco

El handshake es HTTP: el cliente manda un `GET` con una cabecera `Sec-WebSocket-Key`; el servidor
responde `101 Switching Protocols` con `Sec-WebSocket-Accept` (el base64 del SHA-1 de arriba). A partir
de ahí la conexión deja de hablar HTTP y pasa a **tramas**.

Una trama lleva un bit `FIN`, un *opcode* (texto, binario, close, ping/pong), un flag de máscara y una
longitud (de 7, 16 o 64 bits), seguidos de la carga. Las tramas del **cliente van siempre enmascaradas**
(un XOR con una clave de 4 octetos que viaja en la propia trama); las del servidor, no. Decodificar es
parsear esa cabecera y des-enmascarar; codificar es construirla.

Y *construir* es lo único que pidió tocar el runtime. Leer un octeto era `b[i]`, pero no había forma de
**armar** un `bytes` a partir de enteros calculados (el byte de cabecera `0x81`, las longitudes…). Un
byte como `0x81` ni siquiera es UTF-8 válido, así que no vale pasar por un `string`. El builtin nuevo es
el **dual** del indexado:

```rust
bytes_of(xs: [int]) -> bytes   // cada elemento truncado a octeto
```

Con él, una trama de texto del servidor es una línea:

```raylang
fn encode_text(texto: string) -> bytes {
    encode_frame(op_text(), to_bytes(texto))   // bytes_of([cabecera...]) + carga
}
```

El resultado es `examples/web/websocket_echo.ray`: escucha, completa el handshake y reenvía cada trama de
texto hasta que el cliente cierra. La prueba de extremo a extremo levanta el servidor en la VM y lo ataca
con un cliente WebSocket de verdad (en el test): handshake con el accept canónico, ida y vuelta de tramas
enmascaradas, y el `close` de cortesía. Un protocolo bidireccional real, construido sobre los sockets de
M15, el `bytes` de M16 y los bits de M19.3a — sin una sola dependencia externa.

Lo que queda fuera es `wss://`: WebSocket sobre TLS. Eso ya no es cómputo de bits, sino criptografía de
verdad, y choca con la invariante de cero dependencias. Es el tema —aún abierto— de M19.4.
