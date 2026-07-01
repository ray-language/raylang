# Cerrar gRPC: HPACK-Huffman, HTTP/2 vivo y un cliente gRPC

M26 nos dejó gRPC **a medias**: teníamos el framing de HTTP/2, un códec HPACK que emitía cabeceras en
crudo y el marco gRPC sobre protobuf, pero nada de eso se hablaba por un socket de verdad. Faltaban dos
piezas grandes, y hasta tenerlas las dos, "gRPC en raylang" era una maqueta. M31 las cierra: la
compresión Huffman de HPACK, el transporte HTTP/2 **vivo** sobre TLS, y con ambas encima, una llamada
gRPC unaria de extremo a extremo. Casi todo, como siempre, librería raylang; el único toque de runtime
es exponer una opción de TLS que ya teníamos casi a mano.

## M31.1 — HPACK-Huffman y un acertijo con la tabla

HPACK comprime las cabeceras HTTP/2 de dos maneras: con un índice a una tabla de cabeceras conocidas, y
codificando las cadenas literales con un **código Huffman canónico** —una tabla fija de 257 símbolos (los
256 octetos más el fin-de-cadena, EOS) publicada en el Apéndice B del RFC 7541—. Nuestro `hpack.ray`
solo hacía lo primero; emitía las cadenas tal cual. Cerrar HPACK es escribir esa tabla y sus dos
funciones: `huffman_encode` concatena los códigos MSB-first y **rellena el último octeto con unos** (que
resulta ser un prefijo de EOS, por diseño del RFC), y `huffman_decode` recorre un **trie binario** bit a
bit, rechazando tanto el relleno inválido como el propio EOS (RFC §5.2).

Lo interesante no fue el algoritmo, sino **conseguir la tabla exacta**. Un `WebFetch` de una versión
*resumida* de la tabla —de esas que agrupan rangos "251–255: 28 bits"— entró con errores sutiles: el
símbolo 255 acababa colisionando con el código de EOS, y la **suma de Kraft** salía rota. La suma de
Kraft es la prueba de oro de un código prefijo: para un código completo sobre este alfabeto debe dar
exactamente `2^30`. Si no cuadra, o dos símbolos comparten prefijo (ambigüedad al decodificar) o sobra
espacio de códigos.

> La lección: no te fíes de una **transcripción resumida** de un dato exacto. La fuente fiable fue doble
> —una tabla legible por máquina (la del paquete `x/net/http2` de Go, ya en producción) **más**
> validación algorítmica propia—: comprobar que Kraft da `2^30`, que no hay dos códigos con el mismo
> prefijo, y que se reproduce byte a byte el vector oficial C.4.1 (`www.example.com`). Un dato "correcto
> de aspecto" no basta cuando un solo bit desplazado corrompe todo el flujo.

En raylang la tabla vive en dos arreglos paralelos, `huff_codes()` (el código de cada símbolo alineado a
LSB) y `huff_lens()` (su número de bits), y el trie se construye recorriéndolos. Verificado contra
C.4.1/C.4.2/C.4.3 y C.6.1, más ida y vuelta, en los dos motores. (Queda fuera del corpus del parser
auto-alojado, que no lleva operadores de bits.)

## M31.2a — el único cambio de runtime: ALPN `h2`

Para hablar HTTP/2 con un servidor real no basta con abrir un TLS y empezar a mandar frames: el protocolo
se **negocia en el propio handshake** mediante ALPN (*Application-Layer Protocol Negotiation*), donde el
cliente ofrece `h2` y el servidor lo acepta. Nuestro `tls_connect` reusaba una `ClientConfig` cacheada
que no anunciaba ningún protocolo. De ahí el único añadido de runtime de todo M31: el builtin
`tls_connect_h2(host, port)`, que arma su propia `ClientConfig` con `alpn_protocols = [b"h2"]`,
**completa el handshake de forma bloqueante** (necesario para poder preguntar qué protocolo se negoció) y
**exige** que el resultado sea `h2` —si el servidor no lo ofrece, error—. Después, la VM vuelve a poner
el socket en modo no bloqueante para que el framing pueda ceder entre fibras.

Fíjate en el equilibrio: todo el trabajo de HTTP/2 (preface, SETTINGS, HEADERS, HPACK, Huffman) es
raylang puro; lo único que no podía serlo es la negociación criptográfica dentro del handshake TLS, que
vive necesariamente en `rustls`. Un solo builtin, y el resto de la torre se apoya en él.

## M31.2b — el cliente HTTP/2 vivo

Con ALPN en su sitio, `http2_get(host, port, path)` cablea las primitivas de M26 sobre el socket. El
guión es el que dicta el protocolo:

```raylang
let salida = preface() + settings_empty() + headers_frame(1, block, true);
```

Primero la **connection preface** (la cadena mágica que abre todo HTTP/2), luego un SETTINGS vacío del
cliente y el HEADERS de la petición (stream 1, con END_STREAM porque un GET no lleva cuerpo). Después, un
bucle que acumula bytes del socket, extrae frames completos con `frame_size`/`parse_frame`, responde con
ACK al SETTINGS del servidor, decodifica el HEADERS de respuesta (`:status`) y concatena los DATA, hasta
ver END_STREAM en el stream 1. Ni multiplexado ni ventanas de flujo sofisticadas: lo justo para una
petición-respuesta correcta.

Aquí saltó un **bug de verdad, en el checker del propio lenguaje**. El bucle usaba un `match` en el que
*todos* los brazos terminaban en `return`. El checker, al calcular el tipo del `match`, hacía `panic` con
"hay al menos un brazo" —asumía que siempre habría un brazo del que sacar un valor—. Pero un `match` cuyos
brazos **divergen** todos no produce valor: diverge él mismo. El arreglo fue enseñarle esa forma: un
`match` con todos los brazos divergentes type-checkea con tipo unit, igual que ya hacíamos con `if`. Un
recordatorio de que escribir librerías ambiciosas es la mejor prueba de estrés del compilador.

El test levanta un **servidor h2 de juguete escrito a mano** —solo `std` + `rustls`, sin arrastrar `h2`,
`hyper` ni nada— que responde `:status: 200` (índice HPACK `0x88`) y un DATA. El cliente raylang saca su
200 y su cuerpo, en ambos motores. Servidores de juguete propios: la forma de verificar de verdad sin
romper el cero-dependencias.

## M31.3 — un cliente gRPC unario, apilándolo todo

gRPC es, en esencia, HTTP/2 con un contrato de cabeceras y un marco de mensaje propios. Con las piezas
anteriores, `grpc_call(host, port, path, mensaje)` no inventa casi nada: **apila** toda la torre. TLS con
ALPN `h2` (M31.2a), framing HTTP/2 + HPACK con Huffman (M26/M31.1), y protobuf enmarcado con `grpc_frame`
(M25). La llamada es un HEADERS `POST` con las cabeceras que gRPC exige —`content-type:
application/grpc`, `te: trailers`, `:path` = el método— **sin** END_STREAM, seguido de un DATA con el
mensaje enmarcado y **con** END_STREAM.

La respuesta tiene la peculiaridad de gRPC: el código de estado no viaja en las cabeceras iniciales, sino
en un segundo bloque de **trailers** al final. Así que el cliente lee el HEADERS de respuesta (`:status`
HTTP), el DATA (el mensaje gRPC-framed) y luego un **segundo HEADERS** con `grpc-status`, hasta END_STREAM;
después desenmarca con `grpc_unframe`. El servidor de juguete —otra vez a mano, `std` + `rustls`, sin
`tonic`— responde `:status: 200`, un mensaje protobuf enmarcado y el trailer `grpc-status: 0`; el cliente
raylang obtiene `grpc-status 0` y parsea el string de la respuesta (`"hola, raylang"`), en los dos
motores.

---

Y con eso, gRPC está cerrado. Merece pararse en lo que significa: raylang tiene un **cliente gRPC real**
—HPACK con compresión Huffman canónica, transporte HTTP/2 vivo negociado por ALPN, protobuf y trailers—,
y todo salvo el handshake TLS es librería escrita en el propio lenguaje. El patrón que recorre M20–M31 se
confirma una vez más: un runtime mínimo (aquí, un builtin de ALPN), y encima una torre de protocolos de
verdad, verificada contra vectores del RFC y servidores de juguete propios, sin traer una sola
dependencia pesada.
