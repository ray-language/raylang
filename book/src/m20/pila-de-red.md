# La pila de red y protocolos (M20–M26)

Entre la capa web de M19 y las ergonomías del lenguaje de M27 hay un arco largo y denso: siete módulos
(M20–M26) en los que raylang deja de ser un lenguaje que *habla* HTTP y pasa a tener una **pila de red
seria** —criptografía, identidad, compresión, DNS, OAuth2, WebSocket, protobuf, HTTP/2—. Cubrir cada
fase con el detalle que merece llenaría un libro entero; este capítulo la recorre en **panorámica**,
porque lo interesante no es cada protocolo por separado, sino que **todos se construyeron con el mismo
patrón**, y ese patrón es la lección.

## El patrón consolidado

Cada capa nueva es una **librería de raylang puro apilada sobre el runtime que ya teníamos**: los
sockets de M15, el tipo `bytes` de M16, los operadores bit a bit de M19.3, la concurrencia de M17. La
regla —heredada de M15/M19— es tozuda: *todo lo que pueda escribirse en raylang se escribe en raylang;
el runtime solo se toca cuando algo es físicamente imposible en el lenguaje*. En todo el arco, esa
excepción se disparó **una sola vez** (UDP: tres builtins). Lo demás —SHA-256, HMAC, JWT, DEFLATE,
DNS, HPACK— es código raylang que no añade ni un opcode.

El segundo pilar es cómo se **verifica**. La criptografía y la compresión son cómputo puro y
determinista, así que se prestan a un doble control: el **oráculo VM↔intérprete** (los dos motores deben
coincidir bit a bit) y la comparación contra un **vector estándar externo** —los test vectors de NIST,
los ejemplos canónicos de un RFC, la salida de `openssl`, o una implementación de referencia en Python—.
Cuando el protocolo depende de la red (Redis, DNS, OAuth2, WebSocket) y no puede ser determinista, el
vector externo se sustituye por un **servidor de juguete en Rust** contra el que se prueba de extremo a
extremo, por ambos motores. Nunca "confía en que funciona": siempre hay una segunda fuente de verdad.

## M20 — cripto, identidad y clientes cloud

El módulo más grande. Arranca con el **cimiento criptográfico**, cada pieza montada sobre la anterior:
**SHA-256** (FIPS 180-4, aritmética de 32 bits enmascarada, gemelo del SHA-1 de M19.3), **HMAC-SHA256**
(RFC 2104, ese doble hash con `ipad`/`opad`), los codificadores **base64url** y **hex**, y encima el
*capstone* de identidad: **JWT HS256** (`jwt_sign`/`jwt_verify`, con comparación en tiempo casi constante
para no filtrar el secreto) y **UUID v4**. Cada eslabón se verifica contra su RFC o contra Python antes
de que el siguiente se apoye en él.

Luego vienen los **clientes cloud**: URL/query/cookies (percent-encoding, `struct Cookie` encadenable por
UFCS cross-module), **tiempo UTC** (cero runtime: `now()` da los milisegundos del epoch y el resto es la
aritmética "civil from days" de Hinnant), un **cliente Redis** RESP2 sobre TCP, y un **cliente HTTP
robusto** que sigue redirecciones, decodifica `Transfer-Encoding: chunked` y descomprime gzip
automáticamente. La única grieta en el runtime es **UDP** (sockets sin conexión: tres builtins, un
handle nuevo), que luego M20.11 hace ceder cooperativamente al scheduler como TCP.

Dos capstones cierran el módulo. **AWS Signature V4** apila casi todo lo anterior —HMAC encadenado,
SHA-256 hex, URL encoding, formato de fecha— en los cuatro pasos de la firma, y se valida contra el
vector oficial *get-vanilla* de AWS. Y el trabajo más duro de toda la stdlib: **gzip/DEFLATE**, con
`inflate.ray` (el descompresor RFC 1951, port del `puff.c` de zlib: bit-stream LSB-first, Huffman
canónico, referencias LZ77) y `deflate.ray` (el compresor, con matching LZ77 por cadenas de hash y
Huffman fijo). La prueba de fuego del encoder: el gzip que produce raylang lo descomprime **Python**.

## M21 — observabilidad

Lo que un servicio necesita para ser *operable*, dos librerías puras. **Logging estructurado** (`log.ray`):
cada entrada es una línea JSON con niveles, filtrado y campos tipados, API encadenable por UFCS, validada
haciendo que Python `json.loads` la relea. Y **métricas Prometheus** (`metrics.ray`): counters, gauges e
histogramas con **labels**, cada conjunto de labels con su propia familia de series (buckets cumulativos,
`_sum`/`_count`, `+Inf`), renderizados al formato de exposición de texto. El módulo termina con un
**endpoint `/metrics` real** montado sobre el servidor web de M19: un `Registry` compartido que se captura
por closure en el handler y que un Prometheus de verdad podría escrapear.

## M22 — DNS sobre UDP

El estreno de los sockets UDP con un protocolo real (RFC 1035). `dns.ray` resuelve **siete tipos de
registro** —A, AAAA, MX, CNAME, TXT, NS, SRV— a un `enum Record` tipado. Las dos piezas difíciles: la
**compresión de nombres** (un nombre puede terminar en un puntero `0xC0xx` a un offset anterior;
`read_name` sigue el salto pero devuelve la posición siguiente en el flujo original) y la **IPv6
canónica** (los 16 octetos de un AAAA colapsados con `::` en la racha de ceros más larga, RFC 5952). Se
verifica contra un servidor DNS de juguete —y, a mano, contra 8.8.8.8 real—. M22.1 añade una **caché por
TTL**: el servidor de juguete cuenta las consultas, y tres resoluciones de dos claves distintas producen
solo dos consultas en el hilo.

## M23–M26 — OAuth2, WebSocket, protobuf, HTTP/2

El tramo final apila protocolos de aplicación sobre todo lo anterior. **OAuth2** (`oauth2.ray`): el grant
*client_credentials*, apilado sobre el cliente HTTP, el form-encoding de `url.ray` y el parser de
`json.ray`, verificado contra un token endpoint de juguete. El **cliente WebSocket** (`websocket_client.ray`,
el espejo del servidor de M19.3): enmascara las tramas que envía (RFC 6455 §5.3), reusa la cripto del
handshake, y se prueba haciendo que un cliente raylang hable con un servidor raylang, con eco de UTF-8
multibyte —y con `wss://` sobre TLS—.

**Protobuf** (`protobuf.ray`) es el corazón autocontenido de gRPC: el códec del formato wire proto3
(varints LEB128, tags `número<<3 | wire_type`, campos length-delimited y fixed) más el **framing de gRPC**
(el prefijo de 5 octetos), validado por los vectores canónicos de la doc (`08 96 01`) y por un
decodificador Python sin dependencias. Y **HTTP/2** (`http2.ray` + `hpack.ray`) entrega las dos piezas
verificables: el **framing** (cabecera de 9 octetos, tipos DATA/HEADERS/SETTINGS/…, la connection preface)
y la parte difícil, **HPACK** (RFC 7541): tabla estática de 61 entradas + tabla dinámica con evicción,
enteros con prefijo de N bits, y las cuatro representaciones de campo. Se valida **byte a byte contra los
vectores oficiales del RFC 7541 §C.3** —las tres peticiones, incluidas las referencias a la tabla
dinámica—. Quedan dos diferidos grandes, que se cierran en M31: el **Huffman de HPACK** (los 257 códigos
del Apéndice B) y el **transporte HTTP/2 vivo** (streams multiplexados sobre TLS con ALPN `h2`, y con
ello un cliente gRPC completo).

## La lección

Siete módulos, una pila de red que rivaliza con la de un lenguaje maduro, y el runtime apenas engordó:
tres builtins de UDP y un puñado de opcodes de fecha. Todo lo demás vive en librerías de raylang que se
apilan unas sobre otras —bits sobre `bytes`, HMAC sobre SHA-256, JWT sobre HMAC, SigV4 sobre todo—. El
patrón lo hizo posible: **librería pura + verificación contra un vector externo + oráculo de dos
motores**. La librería mantiene el núcleo delgado; el vector externo garantiza que es *correcta* de
verdad, no solo autoconsistente; y el oráculo asegura que los dos motores no divergen jamás. Cuando algo
no cabía —HPACK-Huffman, el transporte HTTP/2 vivo—, se difirió con honestidad y se dejó anotado para
M31. Es el mismo principio que atraviesa el proyecto entero desde M7: el lenguaje puede crecer mucho sin
que el núcleo crezca con él.
