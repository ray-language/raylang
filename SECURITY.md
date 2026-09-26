# Política de seguridad de raylang

raylang es un lenguaje pensado para **producción real**: el runtime es memory-safe por
construcción, el compilador no se cae ante ninguna entrada y el proyecto se endureció de forma
deliberada (M42–M43 y los arcos posteriores). Este documento explica el modelo de seguridad, qué
cuenta como vulnerabilidad y cómo reportarla.

## Versiones soportadas

| Versión | Soporte |
|---------|---------|
| `1.0.x` (línea actual) | ✅ correcciones de seguridad |
| `< 1.0.0` (betas) | ❌ |

Se soporta la última línea estable publicada. La versión del binario (`ray version`) es la del
lenguaje y la de [SPEC.md](SPEC.md): van juntas.

## Cómo reportar una vulnerabilidad

**No abras un issue público** para una vulnerabilidad. Repórtala en privado a:

- **dev@rayala.org** (asunto con el prefijo `[SECURITY]`)
- o, si el repositorio lo tiene habilitado, vía **GitHub Security Advisories** (pestaña *Security* →
  *Report a vulnerability*).

Incluye, en lo posible: una descripción del problema, un caso mínimo que lo reproduzca (un `.ray` y/o los
pasos), la versión/commit afectado, y el impacto que estimas.

**Compromiso de respuesta** (best-effort, al ser un proyecto de un solo mantenedor): acuse de
recibo en **72 h**, una primera evaluación en **7 días**, y divulgación **coordinada** (se acuerda
un embargo razonable hasta que haya corrección; se te dará crédito si lo deseas).

## Modelo de seguridad

Lo que raylang **garantiza por construcción**:

- **Memory safety.** El runtime está escrito en Rust *safe* (con las excepciones auditadas de más
  abajo); el lenguaje **no tiene `null`** (los errores son valores: `Option`/`Result`/`?`) y la
  memoria compuesta la gestiona un **GC mark-and-sweep** (sin use-after-free ni doble free en
  raylang puro). Los índices llevan comprobación de rango y la aritmética de `int` es *checked*:
  un desbordamiento es un **error de ejecución**, nunca un valor corrupto.
- **Sin data races por construcción.** El modelo de concurrencia es de **actores con aislamiento
  de heap** (M38): cada fibra tiene su propio heap y la única comunicación entre ellas son
  **canales** que transfieren la propiedad del valor. Vale igual con el scheduler M:N multicore
  —que es el default— y con el modo determinista de un solo hilo. No hay estado mutable
  compartido → *data-race freedom* sin necesidad de *ownership* en el sistema de tipos.
- **Confinamiento opcional.** Para embeber raylang como lenguaje de *scripts* no confiables, hay **límites de
  recursos** (`ray run --fuel N` acota las instrucciones; `--heap N` acota los objetos vivos): un bucle
  infinito o una entrada maliciosa **no cuelgan ni agotan la memoria** del anfitrión. El servidor
  MCP (`ray mcp`) ejecuta el código de sus tools con esos límites y un plazo, precisamente porque
  su entrada la escribe un modelo. **Lo que el confinamiento NO acota es el I/O**: un programa bajo
  `--fuel`/`--heap` sigue teniendo el disco, la red y los procesos del usuario que lo ejecuta (ver
  «Herramientas de desarrollo» más abajo). Es un límite de recursos, no una sandbox.
- **Compilador sin pánicos.** El front-end (lexer/parser/checker) convierte toda entrada del usuario en un
  **error con posición**, nunca en un *panic* de Rust. Los fallos de invariante interna se centralizan en un
  `ice!()` (Internal Compiler Error) que pide un reporte de bug. Esto se verifica con **fuzzing continuo**
  (`tests/fuzz_frontend.rs`, corre en cada `cargo test` + una campaña nocturna) y una política de ICE
  (`tests/ice_policy.rs`).
- **Cadena de suministro verificada.** El gestor de paquetes usa un **lockfile con hashes
  SHA-256** por dependencia (comprobados en cada resolución), **firmas Ed25519** por versión
  publicada con dueño de nombre fijado por TOFU (`ray registry publish --sign`,
  `ray registry verify` como check de CI del índice) e **índice único por proyecto** para mitigar
  la *dependency confusion*. Detalle en [PUBLISH.md](PUBLISH.md) §6.
  - **Autoactualización** (M137): `ray upgrade` descarga la release desde
    `github.com/ray-language/raylang/releases` — solo ante la orden explícita del usuario
    (mismo canal y confianza que `install.sh`), y el binario descargado se verifica
    ejecutándolo (`ray version` debe reportar el tag pedido) antes de reemplazar nada.
  - **Toolchain privada** (M171): `ray toolchain install` descarga y ejecuta `rustup-init`
    del canal oficial de Rust (`sh.rustup.rs`/`win.rustup.rs`, TLS) y el vendor de `ray-runtime`
    desde la release de la misma versión — solo ante la orden explícita del usuario, nunca desde
    `run`/`build`. Instala bajo `~/.ray/toolchain` sin tocar el Rust ni el PATH del usuario. El
    vendor lleva los `.cargo-checksum.json` de `cargo vendor`, que cargo verifica al compilar.
  - **Endpoint de red por defecto** (M136): el índice oficial
    (`github.com/ray-language/ray-index`) es el default cuando no hay `RAY_INDEX` ni
    `[registry] index`. Solo se contacta al resolver deps **por nombre** (nunca en
    `run`/`build` con deps git/`path:`), y el opt-out es explícito: `index = ""` en
    `ray.toml` o `RAY_INDEX` vacía (builds herméticos/CI sin red).

### La premisa: el programa es de confianza

Todas las garantías de arriba protegen a un programa raylang **de su entrada** (un cuerpo HTTP, un
archivo, una respuesta de red). Ninguna protege al anfitrión **del programa**: raylang no tiene un
modelo de capacidades — un programa que compila puede abrir cualquier archivo, socket o proceso que
pueda el usuario. Los handles del runtime (archivos, sockets, procesos, ventanas) son enteros, pero
desde M296 llevan **dominio**: una tarea lanzada con `spawn_isolated` no puede usar los handles de
otros dominios (se comportan como cerrados) ni exponer los suyos, y sus hijas heredan el
confinamiento. Es la primera de las dos piezas que exige ejecutar código de terceros dentro de un
proceso raylang (plugins, IDEAS §95 P1); la segunda —capacidades verificadas en el despacho de cada
builtin, no solo en compilación— sigue pendiente: hoy un plugin aislado no ve los archivos del
host, pero puede abrir los suyos.

### Herramientas de desarrollo: `ray dev`, dependencias `path:` y `ray mcp`

Tres comportamientos por diseño que conviene saber antes de abrir un proyecto ajeno:

- **`ray dev` ejecuta comandos del proyecto.** El `[frontend] dev` de `ray.toml` corre por el
  shell del sistema (`sh -c` / `cmd /C`) para que el usuario vea Vite tal cual. Clonar un repo y
  correr `ray dev` es ejecutar lo que ese repo diga — la misma clase de confianza que los scripts de
  `npm`, y por eso solo lo hace `ray dev`: el **LSP nunca** ejecuta nada del proyecto al abrirlo
  (diagnostica con el loader, sin red y sin procesos), y `ray run`/`build` no lanzan el frontend.
- **Las dependencias `path:` no están confinadas.** Un `ray.toml` puede apuntar a `../../lo-que-sea`
  y el loader compila esos archivos como módulos. Son parte del proyecto que decides compilar, igual
  que un submódulo git; el aislamiento de origen (índice único, hash, firma) aplica a las
  dependencias **por nombre**, no a las rutas locales.
- **`ray mcp` confina CPU, memoria y tiempo, no el I/O.** `ray_run`/`ray_test`/`ray_check` ejecutan
  el código que escribe el modelo en un subproceso con combustible, tope de heap, plazo con kill y
  un directorio temporal propio — pero con el disco, la red y los procesos **del usuario**. Un
  prompt inyectado puede pedirle al modelo un programa que lea `~/.ssh` o lance procesos, y raylang
  lo ejecutará. Úsalo como usarías cualquier herramienta que ejecute código generado: en una cuenta
  o contenedor con lo que estés dispuesto a exponer. Una lista de capacidades negables (`fs`, `net`,
  `process`) verificada en el despacho de builtins es la mitigación prevista (IDEAS §95 P1, §96 #3).

### Servidores locales en apps de escritorio y móvil (M297)

Una app que sirve su interfaz desde un webserver en `127.0.0.1:<puerto>` tiene **dos atacantes
locales** que no existen en un servidor normal, y el puerto no es un secreto (fijo se conoce;
aleatorio se escanea desde JavaScript en segundos):

- **Cualquier página web abierta en el navegador del usuario** puede enviar peticiones a ese
  puerto. CORS le impide leer la respuesta, pero el efecto secundario ocurre (un POST de formulario
  que borre o ejecute algo); los WebSockets no están sujetos a CORS, así que abre uno y habla con
  el servidor en las dos direcciones; con DNS rebinding además lee las respuestas.
- **Cualquier otro proceso de la máquina**, o cualquier otra app instalada en el móvil (ni Android
  ni iOS aíslan `localhost` entre apps), conecta al puerto como si fuera la ventana legítima.

Lo que raylang ofrece, en orden de preferencia:

1. **Sin puerto: `ray://app/…`** (M226). La ventana carga la interfaz por un esquema servido dentro
   del proceso; no hay socket que atacar. Es el camino por defecto para escritorio y móvil, y el
   que usan las apps de referencia (ray-sublime).
2. **Con backend HTTP local: `web.listen_local(build, listener, token)`** o, en crudo,
   `webserver.local_limits(token)`. Activa las dos defensas: el **token local** de 128 bits del
   CSPRNG (`webserver.local_token()`), que la ventana recibe en la URL (`?ray_token=`) y conserva
   en una cookie `HttpOnly; SameSite=Strict` —otro proceso no lo conoce; otra página web tampoco—,
   y la **guarda de origen**: una petición que llega por loopback, sin cabeceras de proxy, con un
   `Origin` http(s) cuyo host no es el `Host`, es un navegador ajeno → 403, también para el
   handshake WebSocket. Las dos son **opt-in** a propósito: una guarda de origen «siempre activa»
   rompería un servidor real detrás de un proxy que no ponga `X-Forwarded-For`, y toda app con
   CORS configurado.
3. `0.0.0.0` es una decisión de servidor, no de app: expone el puerto a toda la red local.

Un servidor local sin `local_limits` es, para cualquier proceso de la máquina y para cualquier
página web hostil, un API abierto. No es una vulnerabilidad de raylang; es la razón de que exista
`ray://app`.

### Lo que la stdlib y los paquetes hacen por ti (endurecimiento, sep 2026)

Revisión de los bordes donde el código «funciona» y ningún test funcional ve el problema (IDEAS
§96). Lo cerrado, para que nadie lo reabra por accidente:

- **Secretos solo del CSPRNG.** `std/random` (SplitMix64 sembrado del reloj, reproducible con
  `random_seed`) es para simulación y *jitter*, nunca para un valor que un atacante no deba
  adivinar. Los ids de sesión del framework `web` (M289), la máscara de las tramas WebSocket y el
  nonce del handshake (M292) salen de `crypto.random_bytes`. Un módulo de la stdlib o de un paquete
  oficial que derive un secreto de `std/random` **es una vulnerabilidad a reportar**.
- **Sesiones que no se pueden fijar.** `web` acepta una cookie `ray_session` solo con la forma que
  él mismo emite (`is_session_id`); cualquier otro valor se ignora y se estrena una sesión. La
  cookie lleva `HttpOnly; SameSite=Lax` y `Secure` detrás de un proxy que anuncie
  `X-Forwarded-Proto: https`.
- **Contraseñas con derivación lenta.** `crypto.password_hash`/`password_verify` (PBKDF2-HMAC-SHA256,
  sal del CSPRNG, 600 000 iteraciones, formato autodescriptivo, comparación en tiempo constante,
  M290). `sha256(password)` es el error que la stdlib dejaba a mano; ahora hay una forma correcta
  con nombre obvio.
- **Entrada hostil acotada como valor, nunca como caída.** `std/json` rechaza más de
  `max_depth()` (200) niveles con un `Err` (M291: antes, un binario nativo abortaba por
  desbordamiento de pila con un cuerpo de 200 KB); `std/inflate` con `max_out <= 0` es tope cero
  y no «sin tope» (M293: un ZIP con `size = 0` sobre datos deflate descomprimía la bomba entera);
  `db/bson` corta a 200 niveles; el servidor HTTP limita cabeceras, cuerpo, conexiones y tiempo de
  lectura; `static_response` rechaza `..`; los templates escapan HTML por defecto; MySQL y
  PostgreSQL usan sentencias preparadas reales.

### Procesos que lanza `std/ui` (M235)

`ui.open_path`/`ui.reveal` arrancan el lanzador del escritorio (`open`, `xdg-open`, `dbus-send`,
`explorer`, `rundll32`) con la ruta como **argumento**, nunca por una shell: una ruta con espacios,
comillas o `;` no se interpreta. Se exige que la ruta exista antes de lanzar nada; qué aplicación
abre el sistema es decisión del escritorio del usuario, no del programa.
### Pseudo-terminales (M237)

`Cmd.pty(cols, rows)` no amplía lo que `std/process` ya permite (lanzar un programa con argv
tipado, sin shell): solo cambia los pipes por un terminal. Es el programa quien decide lanzar la
shell del usuario, como hace un editor con terminal integrado.

### El llavero del sistema (`std/keychain`, M266)

`keychain.get`/`set`/`delete(service, account)` guardan secretos en el almacén que el sistema ya
protege con la sesión del usuario: Keychain Services (macOS), Secret Service por libsecret (Linux;
gnome-keyring/KWallet) y Credential Manager (Windows). raylang no cifra ni guarda nada por su
cuenta: hereda la política de acceso de cada llavero (en macOS, el ítem lo crea y lo lee el propio
binario; otro programa que lo pida pasa por el diálogo del sistema). `RAY_KEYCHAIN_FILE` sustituye
el llavero por un archivo plano (0600 en unix) SOLO para tests y CI: quien lo ponga en producción
está guardando secretos en claro. Sin llavero disponible el resultado es `Err`, nunca un
almacenamiento alternativo silencioso.

### Política de dependencias

raylang **no** es cero-dependencias: es **dependencias escogidas, acotadas y justificadas**. La
regla es que una dependencia entra solo cuando hacerlo a mano sería **peor ingeniería**
(criptografía, TLS, SQLite, cambio de contexto en ensamblador) o cuando la mejora está **medida**
(allocador, hasher). Todo lo demás —HTTP/1.1 y HTTP/2, HPACK, JSON, TOML, DNS, WebSocket,
protobuf, los clientes de base de datos, el LSP, el poller de E/S— está escrito **en raylang** o
en el propio Rust del proyecto, sin crates.

| Dependencia | Para qué | Alcance |
|---|---|---|
| `ring` | criptografía de producción en tiempo constante (SHA, HMAC, Ed25519, ChaCha20-Poly1305, HKDF, CSPRNG) | feature `net-tls` (vía `ray-runtime/crypto`) |
| `miniz_oxide` | M253: DEFLATE/INFLATE de `std/deflate`/`std/inflate` (y `ray release`). Rust puro sin `unsafe`, el backend de `flate2`; determinista. Los módulos raylang siguen como respaldo sin la feature | feature `deflate` (vía `ray-runtime/deflate`) |
| `num-bigint` (+ `num-traits`, `num-integer`) | M195: enteros grandes de `std/bigint` (`modpow`/`modinv`/aritmética). Ya estaba en el árbol vía `x509-parser`. **No es de tiempo constante**: apto para el DH/RSA de un cliente, no para la clave privada de un servidor expuesto a medidas de tiempo | feature `bigint` (vía `ray-runtime/bigint`) |
| `x25519-dalek` (+ `curve25519-dalek`) | acuerdo de claves X25519 en tiempo constante. `ring` tiene el algoritmo pero **solo** entrega claves efímeras (`EphemeralPrivateKey::generate(alg, rng)`, sin constructor desde octetos, `SecureRandom` sellado): ni clave privada persistible —la identidad de un nodo p2p— ni determinismo para el oráculo VM≡nativo | feature `net-tls` (vía `ray-runtime/crypto`) |
| `subtle` | la comparación en tiempo constante (`constant_time_eq`); su razón de ser es que el compilador no pueda reducirla a un cortocircuito. Ya venía en el árbol como transitiva de `curve25519-dalek` | feature `net-tls` (vía `ray-runtime/crypto`) |
| `rustls` + `webpki-roots` + `rustls-pki-types` | TLS y verificación de certificados | feature `net-tls` |
| `rusqlite` (`bundled`) | SQLite embebido: sin dependencia del sistema, versión determinista | feature `sqlite` |
| `notify` | watch de filesystem por eventos de kernel (FSEvents en macOS, inotify en Linux, kqueue en BSD): la recursividad sobre árboles resuelta — kqueue crudo exige un fd por archivo. Los eventos corren en hilos del crate y se puentean por un self-pipe | feature `watch` |
| `unicode-normalization` (unicode-rs) | M131: NFC/NFD/NFKC/NFKD de `std/text` — las tablas de UnicodeData.txt generadas por el proyecto unicode-rs (lo usan rustc y servo), sin dependencias transitivas. Puro cómputo sobre strings; sin I/O ni unsafe | feature `unicode` (vía `ray-runtime/unicode`, activa por defecto; detectada por uso en el nativo) |
| `x509-parser` (rusticata) | M124: el resumen del certificado del peer (`net.tls_peer_cert` — subject/issuer/validez/SAN desde el DER que rustls ya tiene en mano). Parsear nombres X.500 + GeneralizedTime + GeneralNames es exactamente la clase de código seguridad-adyacente que no se escribe artesanal. Solo LEE certificados ya validados por rustls; no participa en la verificación | feature `net-tls` (vía `ray-runtime/x509`) |
| `libloading` | carga de librerías nativas del FFI — reemplazó a `dlopen`/`dlsym` a mano (arregla Windows y da los errores reales del cargador) | feature `ffi` |
| `webview2-com` (+ `webview2-com-sys`, `windows`, `windows-core`) | M179: el webview del sistema en **Windows** (`std/ui`): los bindings COM de WebView2 y el loader estático de Microsoft, y el Win32 de la ventana/menús/diálogos. Transcribir a mano la treintena de interfaces y handlers de WebView2 sería peor ingeniería (IDEAS §80 lo dejó previsto). Solo entra al árbol en Windows | feature `ui` (vía `ray-runtime/ui`, `[target.'cfg(windows)']`) |
| `corosensei` | cambio de contexto de las fibras del binario nativo: asm auditado de un solo crate en vez de `asm!` propio | binarios nativos con fibras |
| `regex` | motor de regex acelerado del binario nativo (R5) y de la VM (R7); la semántica de referencia sigue siendo `std/regex`, escrito en raylang (los patrones llegan ya validados por su parser) | feature `regex` de la toolchain (vía `ray-runtime/regex`) + binarios nativos que usan regex |
| `fancy-regex` | M232: el dialecto Oniguruma de `regex.onig` (look-around, backreferences, `\G`, atómicos, posesivos) por backtracking sobre el MISMO crate `regex`. Rust puro, sin C ni `unsafe` propio; `backtrack_limit` acota el peor caso (un patrón catastrófico aborta la búsqueda en vez de colgar). Es la fuente de verdad de ese dialecto en los tres motores (no hay espejo raylang) | feature `regex` (vía `ray-runtime/regex`) + binarios nativos que usan `onig` |
| `mimalloc`, `ahash` | allocador y hasher, ambos por mejora **medida** y sin cambio semántico | núcleo (no-wasm) |

Un build **slim** (`--no-default-features --features interp`) deja fuera TLS/cripto, SQLite y la
carga de código nativo: los builtins afectados devuelven un error explícito y el CLI lo dice —
nunca una verificación que "pasa" en silencio. (Excepción deliberada: sin la feature `regex` no
se pierde capacidad — `std/regex` cae a su implementación raylang, la Pike VM interpretada, con
la misma salida.) `cargo audit` corre en CI.

### La frontera insegura: FFI

La **única** vía por la que un programa raylang puede salirse de las garantías anteriores es el **FFI**
(`extern "lib" { fn … }`, M41): permite cargar y llamar a **código C arbitrario**.
**Declarar una función `extern` ES el acto que asume la responsabilidad de seguridad** (no hay un bloque
`unsafe {}` por llamada porque la declaración ya lo es). Todo lo que ocurra al otro lado de esa frontera
(corrupción de memoria, UB, etc.) **es responsabilidad de quien la declara**, exactamente como el `unsafe`
de Rust. El *playground* web y las builds `wasm32` **no** incluyen FFI (ni red/TLS/cripto), y un
binario construido sin la feature `ffi` no puede cargar código nativo en absoluto.

### Auto-actualización (`std/update`, M247)

La confianza es la **clave pública horneada** en la app (`[app] public_key`), no el transporte:
el manifiesto se acepta solo si su firma Ed25519 (`update.json.sig`, sobre los bytes crudos)
verifica con esa clave; sin clave o sin firma, `check` falla. El artefacto se verifica por
`sha256` y tamaño **antes** de escribirse en la instalación, y solo se aceptan `.zip` con un
único directorio raíz y sin rutas `..`. La clave privada nunca pasa por raylang en la app: vive
en la máquina que publica (`ray release`, M248). Lo que NO cubre: un atacante con la clave
privada (rotarla = publicar una versión con la clave nueva mientras la vieja siga vigente). La
firma ante el SO la pone `ray bundle` (M249): `codesign` con hardened runtime + notarización en
macOS, `signtool` en Windows; la identidad y el perfil de notaría se configuran por `ray.toml`,
flags o entorno, y las credenciales viven en el keychain/almacén del sistema, nunca en el repo.

### Ejecutar procesos del sistema (`std/process`)

`std/process` lanza procesos del SO, y está diseñado para que el error clásico no sea el camino
por defecto:

- **No hay shell.** El programa y sus argumentos son un **argv tipado**; una tubería hay que
  escribirla visiblemente (`run("sh", ["-c", …])`). No existe la interpolación de una cadena que
  un shell reinterprete → la clase entera de *command injection* por metacaracteres queda fuera
  del camino habitual. Sigue siendo responsabilidad del programa **no** pasar entrada no confiable
  a un binario que la interprete (incluido `sh -c`).
- **stdin es `/dev/null`** salvo que se pida explícitamente (`.stdin(bytes)`, que escribe y
  cierra): un hijo nunca hereda por accidente la entrada del padre.
- **El hijo va en su propio grupo de procesos**: el plazo (`.timeout_ms`) y `kill` actúan sobre el
  **grupo**, así que los nietos no sobreviven. Un proceso es además **hijo de scope**: una hermana
  que falla lo mata y lo cosecha, y uno que nadie esperó no sobrevive a su scope.
- La captura tiene **tope** (`truncated`) y el modo *streaming* usa canales acotados: un hijo
  parlanchín no agota la memoria del anfitrión, recibe contrapresión.

### Binarios nativos

`ray build --native` produce un ejecutable de código máquina con la **misma semántica** que la VM
(salida byte-idéntica, verificada por un corpus de paridad). Las garantías de arriba viajan con él
—memory safety, aislamiento de heap por fibra, errores como valores, aritmética *checked*— con
dos excepciones explícitas:

- El **confinamiento** `--fuel`/`--heap` es una facilidad de la VM y **no existe** en el binario
  nativo: para ejecutar código no confiable, usa la VM.
- `--fast` cambia a propósito la aritmética *checked* por **envolvente** y quita el **contador de
  profundidad de llamadas** (M295): es un modo de rendimiento para código propio y confiado, no
  para entrada hostil. Sin `--fast`, una recursión demasiado profunda es el mismo error de
  ejecución que en la VM (`stack overflow (recursion too deep: …)`, exit 70) — antes era un aborto
  del proceso en el hilo principal y un SIGBUS mudo dentro de una fibra, por debajo de los 1024
  marcos que la VM permite. La página de guarda de cada pila sigue siendo la última red para
  marcos gigantes.

El binario solo enlaza los subsistemas que el programa usa, y `--without …` permite excluirlos
explícitamente (builds herméticos, contenedores endurecidos).

### Bloques `unsafe` de Rust

El runtime contiene bloques `unsafe` acotados y auditados, cada uno con su invariante `SAFETY`
documentada:

- **`src/ffi.rs`** — la frontera FFI: `transmute` del puntero de función al tipo declarado por la
  firma y `CStr::from_ptr` sobre punteros no-NULL con la `CString` viva. La *carga* ya no lleva
  `unsafe` propio: la hace `libloading`.
- **`src/poll.rs`** — las llamadas al sistema del *poller* de E/S (`kqueue`/`epoll`), declaradas a
  mano para no traer `libc`; y (M119) `poll(2)` con lista **nula** y `nfds = 0` para dormir el hilo
  con precisión (`sleep_ms`): puntero nulo, ningún buffer que la llamada retenga. En Windows (M174):
  `WSAPoll` sobre un arreglo `WSAPOLLFD` propio del tamaño declarado, y el *waitable timer* de alta
  resolución (`CreateWaitableTimerExW`/`SetWaitableTimer`/`WaitForSingleObject`: handle por hilo,
  plazo en un `i64` vivo durante la llamada). En `src/builtins.rs`, `WSAIoctl(SIO_UDP_CONNRESET)`
  sobre un socket propio con un `u32` de entrada vivo durante la llamada.
- **`crates/ray-runtime/src/ui.rs` (mod `android`, M156)** — el puente JNI del shell Android:
  el vtable de `JNIEnv`/`JavaVM` se lee A MANO (puntero sin tipo + `transmute` por sitio a la
  firma exacta, el precedente de `objc_msgSend`), con los índices transcritos del `jni.h` del
  NDK r27 (ABI congelada desde JNI 1.6); `CStr::from_ptr` sobre los C-strings del contrato del
  shell (NUL-terminated, copiados durante la llamada); y el relay stdout/stderr→logcat
  (`pipe`/`dup2` sobre fds propios). Cero dependencias nuevas.
- **`src/builtins.rs`** — el canal de señales del proceso (`signals()`, un *self-pipe* con
  `pipe`/`sigaction`); la adopción del socket de escucha heredado del supervisor de `ray dev`
  (`from_raw_fd` sobre un fd que el padre garantiza, con toma de propiedad única, y `fcntl(F_SETFD,
  FD_CLOEXEC)` inmediato para que los procesos hijos del programa no hereden el listener); la lectura de
  stdin por bytes de `std/io` (M107.2: `poll(2)` + `read(2)` crudos sobre el fd 0, buffers propios
  bien formados que la llamada no retiene); y el terminal de `std/term` (M107.3:
  `isatty`/`tcgetattr`/`tcsetattr`/`cfmakeraw`/`ioctl(TIOCGWINSZ)`/`atexit` — el `termios` se
  maneja como buffer opaco de 128 bytes, mayor que el de cualquier plataforma soportada, y el
  original solo se lee para restaurar tras publicarse completo). En Windows (M173), lo mismo por
  la Console API declarada a mano: `GetConsoleMode`/`SetConsoleMode` (u32 propios),
  `GetConsoleScreenBufferInfo` y `PeekConsoleInputW` sobre estructuras `repr(C)` propias con el
  layout de wincon.h, `ReadConsoleW`/`ReadFile`/`PeekNamedPipe` sobre buffers propios cuyo tamaño
  viaja en la llamada, y `atexit` del CRT para restaurar los modos.
- **`src/dev_host.rs`** — la capa de SO de ese supervisor (M172): en unix `dup2`/`pre_exec` para
  pasar el socket al hijo sin rechazar conexiones entre reinicios, `kill(pid, SIGTERM)` al hijo
  propio y el handler de muerte del padre (`signal` + `kill` + `_exit`, async-signal-safe); en
  Windows las llamadas a kernel32 declaradas a mano — Job Object (`CreateJobObjectW` +
  `SetInformationJobObject` con una estructura `repr(C)` a cero salvo el flag, `AssignProcessToJobObject`
  sobre el handle que posee `Child`), `GenerateConsoleCtrlEvent` al grupo del hijo (sin punteros),
  `SetHandleInformation` sobre el listener propio, `OpenProcess(SYNCHRONIZE)`/`WaitForSingleObject`
  con cierre inmediato del handle, y `ExitProcess` desde el hilo del handler de consola. En
  `src/builtins.rs`, la adopción Windows del listener heredado (`from_raw_socket` sobre el valor
  que el padre garantiza, validado con `local_addr` antes de usarlo; si no es un socket vivo se
  `forget` en vez de cerrar un handle ajeno).
- **`src/lib.rs`** — `setrlimit(RLIMIT_NOFILE)` para subir el límite de descriptores del proceso.
- **`src/vm/mod.rs`** — la aserción `Send`/`Sync` sobre la referencia **inmutable** al programa
  compilado (`ProgRef`), compartida entre los hilos worker sin mutación.
- **`src/wasm.rs`** — la ABI de memoria del *playground* (`alloc`/`run`/`dealloc` sobre la memoria
  lineal del módulo), solo en `wasm32`.
- **`crates/ray-runtime/src/fibers.rs`** — el scheduler de fibras del binario nativo: pilas con
  página de guarda y reactor `kqueue`/`epoll`. El cambio de contexto lo hace `corosensei`. En
  Windows x86_64 (M182) el reactor es `WSAPoll` (ws2_32) sobre un buffer propio de `WSAPOLLFD`, la
  tubería de despertar es un socket UDP conectado a sí mismo (`send`/`recv` de un octeto sobre un
  socket propio no-bloqueante) y el sueño fino usa un *waitable timer* (handles propios, cerrados).
  Los SOCKET viajan como i32 (valores pequeños de Winsock).
- **`crates/ray-runtime/src/process.rs`** (M246) — `spawn_detached`: `pre_exec` con `setsid` (async-signal-safe,
  no toca memoria del padre); el hijo queda FUERA de la cancelación estructural a propósito (es la
  única vía por la que un hijo sobrevive al padre, y se llama así para que se vea).
- **`crates/ray-runtime/src/process.rs`** — `fcntl` variádico, `poll(2)` y `kill` al **grupo** del
  hijo (siempre un grupo creado por nosotros con `process_group(0)`). En Windows (M175): Job Object
  por hijo (`CreateJobObjectW`/`SetInformationJobObject` con estructura `repr(C)` a cero salvo el flag,
  `AssignProcessToJobObject`/`TerminateJobObject` sobre handles propios), `GenerateConsoleCtrlEvent` al
  grupo del hijo, `WaitForSingleObject` sobre el handle que posee `Child`, y `PeekNamedPipe` sobre un
  pipe propio pidiendo solo el contador de octetos.
- **`crates/ray-runtime/src/audio.rs`** — la salida PCM de `std/audio` (M145), **sin crates**: el
  pipe + `read`/`close`/`fcntl` variádico del hilo alimentador e `ioctl(FIONREAD)` del drain; en
  macOS las llamadas a **AudioQueue** (AudioToolbox.framework, enlazado — structs replicadas del
  header, el callback solo toca estado propio sincronizado); en Linux **ALSA por `dlopen`** en
  runtime (`transmute` de cada símbolo a la firma del header de ALSA, API estable; M158 añade
  **AAudio por `dlopen`** en Android con el mismo patrón — `libaaudio.so`, API 26+; sin la
  librería → `Err`, jamás un crash). La decisión sin-crates es deliberada: `cpal` habría exigido
  los headers de ALSA en *build* en todo Linux. En Windows (M178), **WASAPI por COM a mano**: vtables
  `repr(C)` transcritas de mmdeviceapi.h/audioclient.h y llamadas por puntero de método sobre objetos
  que poseemos (`CoCreateInstance`/`Activate`/`GetService` los crean, `Release` en orden inverso),
  `WAVEFORMATEX` empaquetado a 1 byte vivo durante `Initialize`, y `GetBuffer`/`ReleaseBuffer` con
  exactamente los frames prestados; `PeekNamedPipe` sobre el pipe propio para `drain`.
  M298: en unix la cola programa→alimentador es un `socketpair` (`socketpair`/`setsockopt`
  declarados a mano; `SO_SNDBUF`/`SO_RCVBUF` acotados a la latencia, `SO_NOSIGPIPE` en macOS).
- **`src/bundle_windows.rs`** — `ray bundle` en Windows (M180): `BeginUpdateResourceW`/`UpdateResourceW`/
  `EndUpdateResourceW` de kernel32 para inyectar icono y VERSIONINFO en el `.exe` recién copiado
  (ruta NUL-terminada y datos vivos durante la sesión; un fallo descarta la sesión sin tocar el
  archivo). Los enteros de tipo/id son `MAKEINTRESOURCE` (el puntero ES el número, por contrato
  de la API). El resto (subsistema PE, PNG, VERSIONINFO) es Rust seguro sobre bytes.
- **`crates/ray-runtime/src/ui.rs`** — la ventana + webview de `std/ui` (M146), **sin crates**
  (la misma decisión que audio: `wry` exigiría toolchains GTK/WebKit en *build*): el self-pipe de
  la cola de eventos (`pipe`/`read`/`write`/`fcntl` variádico) y, en macOS, **Objective-C a
  mano** — `objc_msgSend` casteado a la firma EXACTA de cada mensaje (arm64: jamás una llamada
  variádica), la clase delegate registrada una vez (`objc_allocateClassPair` tras `OnceLock`) y
  cuyo callback solo toca estado propio sincronizado, y `dispatch_async_f` (libdispatch C plano,
  sin blocks). Los punteros objc viajan como `usize` y SOLO se dereferencian en el hilo
  principal; las ventanas llevan `setReleasedWhenClosed:NO` + retención propia (el use-after-free
  clásico del NSWindow programático) y su liberación es explícita y en main. Frameworks
  AppKit/WebKit/Foundation enlazados con `#[link(kind = "framework")]` — siempre presentes.
  En Linux (M147d), **GTK3 + WebKitGTK por `dlopen`** en runtime (sonames `libgtk-3.so.0` y
  `libwebkit2gtk-4.1.so.0`/`-4.0.so.37`; sin las libs → `Err`, jamás un crash): `transmute` de
  cada símbolo a la firma del header (API C estable, anotado por sitio), `gtk_init_check` (no
  `gtk_init`, que ABORTA sin display), despacho al hilo del loop con `g_idle_add` y un flag
  `alive` por ventana que TODA closure re-chequea antes de tocar punteros (el WM puede destruir
  la ventana por debajo; el contexto del handler `destroy` se libera vía GClosureNotify).
  En Windows (M179/M183) la ventana es Win32 por el crate `windows` (COM de WebView2 por `webview2-com`): el
  estado de cada ventana cuelga de `GWLP_USERDATA` y SOLO se lee en el hilo 1 (también desde el handler
  de `AcceleratorKeyPressed`, que corre ahí); la tabla de aceleradores es propia y se destruye con la
  ventana; `GetKeyState` solo consulta modificadores.
  - M226: el esquema `ray://app/…` (`WKURLSchemeHandler`) sirve la interfaz y los archivos montados **sin socket**: no hay puerto local que otro proceso pueda alcanzar ni permiso de red local en el bundle; los montajes de directorio se canonicalizan y `..` nunca sale de ellos (403 antes de tocar el disco); solo GET/HEAD; sin cabeceras CORS, así que una página de otro origen no puede leer lo servido.
- **`crates/ray-runtime/src/keychain.rs`** — el llavero del sistema (`std/keychain`, M266), **sin
  crates**: en macOS, `SecItemAdd`/`SecItemCopyMatching`/`SecItemUpdate`/`SecItemDelete` con
  diccionarios CoreFoundation propios (RAII sobre `CFRelease`, lectura de los estáticos `kSec*`);
  en Linux, `dlopen("libsecret-1.so.0")` + `transmute` de `secret_password_{store,lookup,clear}_sync`
  a sus firmas variádicas (lista de atributos terminada en NULL) y liberación con
  `secret_password_free`/`g_error_free`; en Windows, `CredWriteW`/`CredReadW`/`CredDeleteW` sobre un
  `CREDENTIALW` propio y `CredFree` del buffer devuelto. Todos los buffers viven durante la llamada.
- **`src/transpile/`** — el mismo tipo de código, pero **emitido** dentro del binario nativo
  generado (FFI, poller, fibras, procesos). Se audita en la plantilla, que es única.

## Qué cuenta como vulnerabilidad

**Sí** son vulnerabilidades a reportar:

- Corrupción de memoria, *use-after-free* o UB alcanzable **sin usar FFI** (desde raylang puro).
- Un *panic*/ICE de Rust (crash del proceso) provocado por **una entrada al compilador o un programa
  válido** (el compilador debe dar un error limpio, no morir).
- Un **escape del confinamiento** de la VM (`--fuel`/`--heap`): un programa que los burla y cuelga
  o agota la memoria del anfitrión.
- Verificación de supply-chain rota: un hash del lockfile que no detecta una manipulación, o una
  firma que no casa con el dueño y aun así resuelve.
- Un fallo en la verificación de certificados TLS del cliente HTTP/red.
- Una **divergencia entre motores** con impacto de seguridad: que el binario nativo o el
  intérprete permitan algo que la VM impide (o al revés) en cualquiera de las garantías de arriba.
- Un módulo de la stdlib o de un paquete oficial (`net`, `web`, `db`, `rpc`) que derive un
  **secreto** (token, id de sesión, nonce, sal, clave) de `std/random` en vez del CSPRNG, o que
  acepte **entrada externa sin tope** (profundidad, tamaño descomprimido, cabeceras) de forma que
  un cuerpo pequeño agote la pila o la memoria.

**No** son vulnerabilidades (comportamiento por diseño, documentado):

- Que un programa que **declara y usa FFI** haga algo inseguro — es la frontera insegura por definición.
- Que un programa pase entrada no confiable a `sh -c` vía `std/process`: el argv es tipado
  precisamente para que eso sea una decisión visible de quien la escribe.
- Que `ray dev` ejecute el comando `[frontend] dev` del `ray.toml` del proyecto, que una
  dependencia `path:` compile archivos fuera del directorio del proyecto, o que un programa bajo
  `ray mcp` tenga acceso al disco y la red del usuario: las tres son decisiones de confianza
  documentadas arriba («Herramientas de desarrollo»).
- Que un **binario nativo** no respete `--fuel`/`--heap`, o que `--fast` no detecte desbordamientos.
- Que la criptografía **pura en raylang** (`examples/`, material de demostración, y los módulos
  LEGADOS `std/crypto/{md5,aes,des}` de M194) no sea de tiempo constante — por eso `std/crypto` se
  apoya en `ring`. Los legados existen para hablar con protocolos que los exigen (VNC, Apple Remote
  Desktop, Kerberos, digest auth); su documentación lo dice en la primera línea y no deben usarse
  para diseñar nada nuevo.
- Que un secreto en memoria **no se pueda borrar de forma garantizada** (zeroización). Los strings
  y bytes de raylang son inmutables y viven en un heap con GC: no hay forma de sobreescribir sus
  octetos ni de controlar cuándo se liberan, y el GC puede haberlos copiado. Es una **decisión de
  diseño consciente** (confirmada en el dogfood de raypass, ago 2026): el modelo de amenaza de
  raylang no incluye a un atacante leyendo la memoria del propio proceso — quien lo necesite
  (HSM-grade) debe mantener el secreto fuera del proceso o tras FFI con memoria propia.
- Que un programa mal escrito produzca un resultado incorrecto sin violar las garantías del runtime.

## Alcance

raylang es obra de un solo mantenedor: las garantías de arriba son reales y se verifican en CI en
cada cambio, pero el proyecto **no ha pasado una auditoría externa**. Evalúalo con ese contexto
antes de ponerlo en un sistema crítico. La corrección de una vulnerabilidad confirmada se prioriza
sobre el trabajo de features.
