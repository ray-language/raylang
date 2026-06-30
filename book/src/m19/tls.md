# TLS — `https://` y `wss://`

Todo lo anterior viaja **en claro**: cualquiera en la red ve las peticiones HTTP y las tramas de
WebSocket. La web real corre sobre TLS — `https://`, `wss://` — y eso es lo que cierra M19. También es
donde el proyecto toma su decisión más incómoda.

## La invariante que se rompe (a propósito)

raylang ha sido **cero dependencias de Cargo** desde el día uno: el lexer, el parser, el GC, los sockets,
el poller `kqueue`/`epoll` (con FFI propio) — todo a mano. Es parte de la pedagogía: para *entender* algo,
constrúyelo. SHA-1 y base64 (el capítulo anterior) se escribieron en raylang justamente por eso.

Pero TLS no es SHA-1. Es criptografía moderna de verdad: AEAD (AES-GCM, ChaCha20-Poly1305), intercambio de
claves ECDHE, validación de certificados X.509, una máquina de estados con docenas de casos. Implementarla
a mano no es pedagógico, es **irresponsable**: un error sutil no es un test rojo, es un agujero de
seguridad. Aquí "hazlo a mano" deja de ser una virtud.

Así que la decisión —tomada explícitamente— es **una excepción consciente y acotada**: añadir `rustls`
(TLS en Rust puro, con el proveedor `ring`) como la primera —y por ahora única— dependencia de Cargo. El
resto del lenguaje sigue cero-deps. La excepción vive en un solo dominio (TLS) y en un solo archivo del
runtime.

## El cliente: `https://` casi gratis

Una conexión TLS es, conceptualmente, "un socket que cifra". Y raylang ya tenía un registro de handles
donde conviven archivos, sockets y sockets de escucha, todos como un `int`. Así que una sesión TLS entra
ahí como un handle más (`OpenHandle::Tls`):

```rust
struct TlsConn {
    conn: rustls::Connection,        // cliente O servidor (el enum unificado)
    sock: std::net::TcpStream,
}
```

Lo elegante: `socket_read_bytes`, `socket_write_bytes` y `close` **ya** operan sobre handles, así que con
desviarlos a TLS cuando el handle es una conexión cifrada, **toda la librería HTTP funciona sin cambios**.
`http.ray` solo necesitó dos retoques: que `parse_url` acepte `https://` (puerto 443) y que elija
`tls_connect` en vez de `tcp_connect`. Un `fetch("https://…")` ya descarga por TLS, verificando el
certificado del servidor contra las raíces de Mozilla (`webpki-roots`), con `SSL_CERT_FILE` para añadir
CAs propias —el mismo mecanismo que honra `curl`—.

## El servidor: conducir rustls sin bloquear

El servidor (`tls_accept` → `wss://`) fue lo difícil. Un servidor concurrente en raylang corre sobre el
**scheduler cooperativo de fibras** (M15.5/M17): cuando un socket bloquearía, la fibra **cede** y el
poller del SO la despierta cuando hay datos. TLS tiene que encajar en eso.

El problema es que `rustls` es una **máquina de estados**, no un stream bloqueante: le das bytes con
`read_tls`, los procesa con `process_new_packets`, y produce bytes para enviar con `write_tls`. La
conveniencia bloqueante de la librería (`rustls::Stream`) no sirve aquí —bloquearía la fibra y con ella
*todas* las demás—. Así que se conduce la máquina a mano:

```
bucle:
  drena las escrituras pendientes (handshake/datos) al socket
  ¿hay texto plano descifrado?  → devuélvelo
  lee más registros del socket (no bloqueante)
     · WouldBlock → devuelve "bloquearía": la VM APARCA la fibra en el fd
     · datos      → process_new_packets, y reitera
```

El punto de cesión es exactamente uno: cuando hace falta **leer** del peer y el socket no tiene datos. Ahí
se reusa el mismísimo mecanismo de los sockets planos (`io_parked` + el poller de M17); el `fd` del TLS es
el del socket subyacente. Las escrituras, casi siempre pequeñas, se drenan en el sitio.

Y un regalo de la unificación: esta **misma bomba sirve a los dos motores**. El intérprete no tiene
scheduler, así que usa sockets *bloqueantes* — y sobre un socket bloqueante, `read_tls` simplemente
bloquea, nunca devuelve `WouldBlock`. La bomba "no bloqueante" se comporta como bloqueante sin una línea
de código especial. Un solo camino para los dos mundos.

Con eso, `examples/web/wss_echo.ray` es el echo server de WebSocket del capítulo anterior con **una línea
nueva**: un `tls_accept(conn, cert, clave)` tras aceptar la conexión. A partir de ahí, el handshake de
upgrade y todas las tramas viajan cifradas, sin que el código de WebSocket se entere. El test lo verifica
de punta a punta: un cliente WebSocket-sobre-TLS de verdad completa el handshake (con el accept canónico
del RFC), intercambia tramas y cierra — todo sobre la fibra que cede y despierta.

raylang habla la web moderna: `http`, `ws`, `https`, `wss`. La criptografía es prestada; el resto, propio.
