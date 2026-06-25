# Sockets TCP: el transporte

Un socket es un canal de bytes entre dos máquinas. Abrirlo, escribirle y leerle es lo único que el
sistema operativo nos da; todo lo demás (HTTP, JSON) se construye encima. M15.2 y M15.3 añaden ese
transporte como **builtins** sobre `std::net`, y la elección de diseño clave es que **no inventan
nada nuevo**: reusan, casi al pie de la letra, la maquinaria de los handles de archivo de M11.8.

## Un handle es un `int`

Cuando M11.8 añadió archivos con *buffering*, tomó una decisión que ahora paga dividendos: un
archivo abierto **no es un tipo de valor nuevo** ni toca el GC. Es un `int` —un *handle*— y los
recursos abiertos viven en un **almacén de proceso** del host: un `Mutex<HashMap<i64, OpenHandle>>`
detrás de un `OnceLock`. El builtin `close(h)` simplemente quita la entrada del mapa.

Los sockets encajan ahí sin esfuerzo. El `enum OpenHandle` —que tenía variantes para archivos de
lectura y escritura— gana dos más:

```rust
enum OpenHandle {
    Reader(BufReader<File>),
    Writer(File),
    Tcp(TcpStream),        // M15.2: una conexión
    Listener(TcpListener), // M15.3: un socket de escucha
}
```

Y como conviven en el **mismo** registro (el mismo contador de handles), un solo `close(h)` cierra
archivos, conexiones y sockets de escucha sin necesidad de saber de cuál se trata: quita la entrada,
y el `Drop` de Rust libera el descriptor. `close` ya era *ad-hoc polimórfico* (cerraba archivos y
canales de M12); ahora cubre también los sockets, sin una línea de código nuevo en su lógica.

## El cliente (M15.2) y el servidor (M15.3)

La API es deliberadamente pequeña:

```raylang
tcp_connect(host, port) -> Result<int, string>   // resuelve el nombre y conecta; da un handle
socket_read(h)          -> Result<string, string> // una lectura (hasta 64 KiB); "" = el otro cerró
socket_write(h, s)      -> Result<int, string>     // escribe s completo
tcp_listen(host, port)  -> Result<int, string>     // bind + listen; port 0 → puerto efímero
tcp_accept(listener)    -> Result<int, string>     // bloquea hasta una conexión; da un handle
local_port(h)           -> int                     // el puerto local (para descubrir el efímero)
```

Dos decisiones merecen comentario:

- **La carga útil es `string`, por ahora.** Un socket transporta bytes, y los bytes no siempre son
  texto UTF-8 válido. Pero introducir un tipo `bytes` es un tipo nuevo en todo el *pipeline* (como
  fue `char` en M11.4). Para arrancar, los sockets usan `string` (UTF-8 *lossy*), exactamente como el
  I/O de archivos de M11.2. El tipo `bytes` queda como un *milestone* futuro bien acotado.

- **Lectura por trozos, no hasta EOF.** `socket_read` hace **una** llamada `read()` y devuelve lo que
  venga. No intenta leer "todo": deja que el código raylang **itere** (acumular hasta `""`, o hasta
  tener el cuerpo esperado). Es justo lo que necesita el cliente HTTP de M15.4, y es la primitiva
  honesta —un socket no sabe cuándo "termina" un mensaje; eso lo define el protocolo de encima—.

Igual que el I/O falible de M11, cada builtin sigue el patrón ya conocido: un **primitivo** que
devuelve un **arreglo etiquetado** (`["ok", payload]` / `["err", msg]`) y un **envoltorio en el
prelude** (en raylang) que lo traduce a `Result`. Así el runtime nunca aprende qué es un `Result`: lo
construye el prelude con `Result.Ok`/`Result.Err` corrientes.

## Bloqueante, primero

En M15.2 y M15.3 los sockets son **bloqueantes**: `tcp_accept` y `socket_read` detienen el hilo del
sistema hasta que haya una conexión o lleguen datos. En el modelo M:1 de M12 —donde todas las fibras
comparten un único hilo— eso significa que un servidor bloqueante atiende **una conexión a la vez**.
Es la base honesta y simple; basta para un cliente, para un servidor de eco secuencial y para servir
peticiones cortas. El servidor **concurrente** —donde `accept` y `read` ceden al scheduler en vez de
bloquear— es el capstone M15.5, y se construye precisamente sobre estos builtins.

## Probar la red sin red de verdad

La red no es determinista, así que no entra al oráculo VM↔intérprete. Se prueba por **subproceso**
contra un **servidor (o cliente) de juguete escrito en el propio Rust del test**:

- Para el **cliente** (M15.2): un hilo de Rust levanta un servidor de eco en un puerto efímero, y el
  `.ray` se conecta, escribe y comprueba la respuesta.
- Para el **servidor** (M15.3): al revés. El `.ray` es el servidor —escucha en el puerto 0, **imprime
  el puerto** y acepta una conexión— y el test, en Rust, lee ese puerto de su `stdout` **en vivo** (no
  con `.output()`, que esperaría a que el proceso terminara: el servidor está bloqueado en `accept`)
  aprovechando que `println!` vacía su buffer en cada salto de línea, y se conecta como cliente.

Ambos casos corren en los dos motores: el intérprete y la VM ejecutan el mismo `.ray` y deben
comportarse igual. Es el oráculo, adaptado a un mundo no determinista: en vez de comparar dos motores
entre sí, comparamos cada motor contra un comportamiento de red esperado.
