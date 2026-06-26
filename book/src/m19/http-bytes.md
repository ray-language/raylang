# HTTP en bytes

El cliente y el servidor HTTP nacieron hablando `string`. Para texto y JSON va perfecto, pero arrastra la
misma deuda que M15 dejó en los sockets: un cuerpo **binario** (una imagen, un `.zip`, un protocolo
binario) se corrompe al pasar por un `string` UTF-8 *lossy*. M16 dio el tipo `bytes` y lo cableó en los
sockets; M19.2 lo lleva a la **capa de protocolo**: el cuerpo de las peticiones y respuestas pasa a ser
`bytes`, mientras las cabeceras siguen siendo texto.

## Una estimación equivocada (y por qué importa)

La intuición decía: "los sockets ya tienen variantes `_bytes`, así que portar HTTP es front-end puro,
solo cambiar qué builtin se llama". **Era falsa**, y vale la pena ver por qué.

HTTP es un protocolo **mixto**: cabeceras de texto ASCII seguidas, tras un `\r\n\r\n`, de un cuerpo que
puede ser binario. Cuando lees del socket, un mismo trozo puede traer el final de las cabeceras **y** el
principio del cuerpo. Para separarlos hay que **cortar el buffer de bytes** en dos: la parte de cabeceras
(que decodificas a texto con `from_utf8` para parsearla) y el cuerpo (que dejas en bytes crudos).

Y resulta que raylang **no tenía cómo cortar bytes**. `substring` es de `string` (por carácter); indexar
`b[i]` da un `int`, pero no hay forma de reconstruir un sub-`bytes` a partir de índices. Así que M19.2
**sí** tocó runtime, con un builtin nuevo:

```rust
sub_bytes(b, i, j) -> bytes   // sub-secuencia [i, j) por octeto, con clamp
```

Es el análogo binario de `substring`, y se añadió con el patrón de siempre: una fila en la tabla de
builtins, un opcode `SubBytes`, la implementación en ambos motores compartiendo un helper, y un
**oráculo**. El separador `\r\n\r\n` sí se localiza en raylang puro, escaneando octetos (`b[i] == 13 &&
…`), sin builtin extra. La lección: una estimación de "front-end puro" es una hipótesis hasta que la
revisa el código — igual que en rendimiento, conviene medir antes de afirmar.

## La API: bytes por debajo, texto cómodo por encima

`Response.body` y `Request.body` pasan a `bytes`. Para no perder ergonomía en el caso común (texto), los
atajos siguen aceptando strings y codifican por dentro:

- `ok(s)`, `text(status, s)`, `json_response(s)` — reciben `string`, guardan `to_bytes(s)`.
- `bytes_response(status, b)` — para cuerpos binarios crudos.
- `body_text(r)` / `request_text(req)` — `from_utf8` del cuerpo, para leerlo como texto.

Un efecto secundario agradable: `Content-Length` ahora se calcula en **octetos** (`len` del cuerpo en
bytes), que es lo correcto. Antes, en strings, era el número de *caracteres* → ligeramente erróneo con
contenido no-ASCII. Portar a bytes no solo permitió lo binario: corrigió un bug latente del texto.

## La prueba que lo justifica

El valor de M19.2 se demuestra con un **round-trip binario**: un servidor `.ray` que eco-devuelve el
cuerpo de un POST, y un cliente de Rust que envía 7 octetos con `\x00` y `\xff` dentro y verifica que
vuelven **intactos**. Eso ejercita las dos mitades: `read_request` leyendo un cuerpo binario por
`Content-Length`, y `send_response` enviándolo. La composición HTTP+JSON del cliente sigue funcionando,
ahora decodificando el cuerpo con `body_text` antes de parsearlo.

Como los dos archivos pasan a usar `bytes`, salen del corpus del parser auto-alojado (que no soporta el
tipo `bytes`, igual que `binario.ray`) — un diferido honesto del toolchain self-hosted. Pero en el
lenguaje real, raylang ya sirve y descarga binario correctamente. Lo que queda de la capa web —WebSockets
`ws://` y TLS— se construye sobre esta base de bytes.
