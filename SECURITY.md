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
  su entrada la escribe un modelo.
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
| `x25519-dalek` (+ `curve25519-dalek`) | acuerdo de claves X25519 en tiempo constante. `ring` tiene el algoritmo pero **solo** entrega claves efímeras (`EphemeralPrivateKey::generate(alg, rng)`, sin constructor desde octetos, `SecureRandom` sellado): ni clave privada persistible —la identidad de un nodo p2p— ni determinismo para el oráculo VM≡nativo | feature `net-tls` (vía `ray-runtime/crypto`) |
| `subtle` | la comparación en tiempo constante (`constant_time_eq`); su razón de ser es que el compilador no pueda reducirla a un cortocircuito. Ya venía en el árbol como transitiva de `curve25519-dalek` | feature `net-tls` (vía `ray-runtime/crypto`) |
| `rustls` + `webpki-roots` + `rustls-pki-types` | TLS y verificación de certificados | feature `net-tls` |
| `rusqlite` (`bundled`) | SQLite embebido: sin dependencia del sistema, versión determinista | feature `sqlite` |
| `notify` | watch de filesystem por eventos de kernel (FSEvents en macOS, inotify en Linux, kqueue en BSD): la recursividad sobre árboles resuelta — kqueue crudo exige un fd por archivo. Los eventos corren en hilos del crate y se puentean por un self-pipe | feature `watch` |
| `unicode-normalization` (unicode-rs) | M131: NFC/NFD/NFKC/NFKD de `std/text` — las tablas de UnicodeData.txt generadas por el proyecto unicode-rs (lo usan rustc y servo), sin dependencias transitivas. Puro cómputo sobre strings; sin I/O ni unsafe | feature `unicode` (vía `ray-runtime/unicode`, activa por defecto; detectada por uso en el nativo) |
| `x509-parser` (rusticata) | M124: el resumen del certificado del peer (`net.tls_peer_cert` — subject/issuer/validez/SAN desde el DER que rustls ya tiene en mano). Parsear nombres X.500 + GeneralizedTime + GeneralNames es exactamente la clase de código seguridad-adyacente que no se escribe artesanal. Solo LEE certificados ya validados por rustls; no participa en la verificación | feature `net-tls` (vía `ray-runtime/x509`) |
| `libloading` | carga de librerías nativas del FFI — reemplazó a `dlopen`/`dlsym` a mano (arregla Windows y da los errores reales del cargador) | feature `ffi` |
| `corosensei` | cambio de contexto de las fibras del binario nativo: asm auditado de un solo crate en vez de `asm!` propio | binarios nativos con fibras |
| `regex` | motor de regex acelerado del binario nativo (R5) y de la VM (R7); la semántica de referencia sigue siendo `std/regex`, escrito en raylang (los patrones llegan ya validados por su parser) | feature `regex` de la toolchain (vía `ray-runtime/regex`) + binarios nativos que usan regex |
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
- `--fast` cambia a propósito la aritmética *checked* por **envolvente**: es un modo de
  rendimiento para código propio y confiado, no para entrada hostil.

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
  con precisión (`sleep_ms`): puntero nulo, ningún buffer que la llamada retenga.
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
  original solo se lee para restaurar tras publicarse completo).
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
  página de guarda y reactor `kqueue`/`epoll`. El cambio de contexto lo hace `corosensei`.
- **`crates/ray-runtime/src/process.rs`** — `fcntl` variádico, `poll(2)` y `kill` al **grupo** del
  hijo (siempre un grupo creado por nosotros con `process_group(0)`).
- **`crates/ray-runtime/src/audio.rs`** — la salida PCM de `std/audio` (M145), **sin crates**: el
  pipe + `read`/`close`/`fcntl` variádico del hilo alimentador e `ioctl(FIONREAD)` del drain; en
  macOS las llamadas a **AudioQueue** (AudioToolbox.framework, enlazado — structs replicadas del
  header, el callback solo toca estado propio sincronizado); en Linux **ALSA por `dlopen`** en
  runtime (`transmute` de cada símbolo a la firma del header de ALSA, API estable; M158 añade
  **AAudio por `dlopen`** en Android con el mismo patrón — `libaaudio.so`, API 26+; sin la
  librería → `Err`, jamás un crash). La decisión sin-crates es deliberada: `cpal` habría exigido
  los headers de ALSA en *build* en todo Linux.
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

**No** son vulnerabilidades (comportamiento por diseño, documentado):

- Que un programa que **declara y usa FFI** haga algo inseguro — es la frontera insegura por definición.
- Que un programa pase entrada no confiable a `sh -c` vía `std/process`: el argv es tipado
  precisamente para que eso sea una decisión visible de quien la escribe.
- Que un **binario nativo** no respete `--fuel`/`--heap`, o que `--fast` no detecte desbordamientos.
- Que la criptografía **pura en raylang** (`examples/`, material de demostración) no sea de tiempo
  constante — por eso `std/crypto` se apoya en `ring`.
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
