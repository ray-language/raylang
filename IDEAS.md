# raylang — Backlog de features y su impacto en el diseño

> Registro de ideas que NO entran ahora pero queremos considerar a futuro. Para
> cada una anotamos: **impacto** en el diseño actual, **cuándo** podría llegar, la
> **decisión/recomendación** (si la hay) y la **restricción** que debemos respetar
> hoy para no bloquearla.
>
> Las features ya comprometidas (tipos suma, genéricos, `Result`/`?`, UFCS,
> pipelines, stdlib) viven en [DESIGN.md](DESIGN.md) §2 y §10, no aquí.

## Resumen de impacto

> **Estado tras M14 + M12** (hitos M1–M14 completos; **meta-circularidad lograda** y **concurrencia
> completa**). La columna *Cuándo* refleja la hoja de ruta acordada (ver [DESIGN.md](DESIGN.md) §2). M12
> (concurrencia, CSP sobre la VM) se hizo el último, tras M13 + el self-hosting. Lo único transversal que
> queda abierto es la **optimización de la VM de Rust** (§11, incremental, sin aplicar).

| Idea | ¿Dónde pega? | Cuándo | Estado |
|------|--------------|--------|--------|
| Concurrencia (goroutines / async / suspend) | **Arquitectura de la VM** | **M12** | ✅ **COMPLETO** (DESIGN §21): **CSP sobre la VM** — green threads cooperativos M:1, canales tipados, structured concurrency; data-race freedom **vía CSP** (no ownership); scheduler determinista; intérprete = oráculo secuencial. Surface: `spawn(closure)->Task<T>`, `channel()`/`channel(n)`/`send`/`recv->Option<T>`/`close`, `join`, `scope`, `select` (builtins). Sub-fases: ✅ **M12.1** slice CSP (spawn + canales no acotados + scheduler determinista; solo VM, intérprete da error limpio; `close` ad-hoc polimórfico con el de handles; GC multi-raíz) · ✅ **M12.2** acotados/backpressure (`channel(n)`, `n≥0`; `n=0` rendezvous; `send` se vuelve punto de yield al llenarse la cola; `recv` despierta al emisor bloqueado; `VmChannel.cap`; `Waiting::Recv`/`Send(v)`; el valor del emisor aparcado es raíz del GC) · ✅ **M12.3** structured concurrency (`Task<T>`+`join(t)->T`+`scope(fn()->R)->R`; `spawn` pasa a devolver `Task<T>`; el scope posee las tareas lanzadas dentro y las une al salir; propagación del fallo de una hija vía captura en la `Task` y re-lanzado en `join`/`ScopeEnd`; estado por fibra `task`/`scopes`; GC multi-raíz; diferido: cancelación de hermanas) · ✅ **M12.4** `select(chs: [Channel<T>]) -> int` (bloquea hasta que un canal esté listo para recibir; devuelve el índice del primero listo, determinista; `recv(chs[i])` toma el valor; `Waiting::Select`, `wake_select_waiters`; solo VM) · ✅ **M12.5** cancelación de hermanas (semántica, sin superficie: al fallar una tarea del `scope`, se cancelan las hermanas pendientes —`cancel_task` recursivo: las saca de ready/parked y cancela nietos— y se propaga el fallo original; `ScopeEnd` cancela en vez de esperar; `fail_current_fiber` cancela los hijos de una fibra-hija que falla; cooperativa, no preemptiva). **M12 COMPLETO** (diferido: cancelación preemptiva, `Selected<T>` índice+valor, select de send, `cancel(t)` explícito). Diferido: algebraic effects (intérprete a pila explícita), M:N paralelo (GC thread-safe). Descartado: ownership/regiones |
| **raylang de producción** (cambio de norte) | Todo el runtime | **rama aparte** | 💭 DESIGN §21.1: dejar de ser pedagógico → un solo motor (VM), ownership/actores, GC concurrente + M:N paralelo, algebraic effects, gestor de paquetes, FFI. No es una fase; vive en otra rama |
| Null safety | Sistema de tipos | hecho | ✅ no hay `null` (`Option<T>`, M6) |
| Introspección / reflection | Modelo de valores de la VM | post-M11 | 💤 puerta abierta (los valores cargan tipo en runtime) |
| Structs vs interfaces/**traits** | Sistema de tipos / polimorfismo | **M9** | 📌 recomendación fijada (traits estilo Rust) |
| Hot code reloading | Indirección de llamadas en la VM | tardío | 💤 acomodable |
| Visibilidad (`pub` vs mayúscula) | Sistema de módulos | **M11** | 📌 recomendación fijada (`pub` explícito) |
| **Módulos por directorios** (`import geo/formas/circulo;`) | Loader + parser de `import` | **M11.5** | ✅ separador `/` fijado; **solo leaf-binding** + `as`; prohibido el acceso por ruta en expresiones (ambiguo con `/` y mala práctica); rutas absolutas desde la raíz. Diferido: imports relativos, `pub` granular |
| **Aislamiento de módulos** (`mod.ray` = cápsula) | Loader (resolución + aristas) | **M11.6** | ✅ estrategia "cápsula": `mod.ray` vuelve un directorio direccionable (`import geo;`) y **encapsula** su subárbol; reexport `pub from … import …` (-a) + enforcement del borde (-b); descartados `internal/`-Go y `mod x;`/`pub(crate)`-Rust |
| **Redes + base moderna** (sockets / HTTP / JSON · reloj/RNG/math) | Builtins (transporte/base) + librería raylang (protocolos) | **M15** | 🚧 DESIGN §24. Dirección fijada: **transporte = builtins** (sockets TCP/UDP sobre `std::net`, cero deps, molde de handles de M11.8); **protocolos (HTTP/URL/JSON) = librería en raylang** con `import`; **carga útil = `string`** por ahora (bytes diferido); **bloqueante primero** (async sobre el scheduler de M12 = capstone M15.5). ✅ **M15.1a** matemáticas (`sqrt`/`pow`/`floor`/`ceil`/`round`/`abs`/`min`/`max`/trig/`ln`/`log10`/`exp`/`pi`/`e`; opcode parametrizado `MathF(MathFn)`; determinista → oráculo). ✅ **M15.1b** reloj/RNG (`now`/`monotonic`/`sleep`/`random`/`random_int`; PRNG **SplitMix64** propio sembrado del reloj, cero deps; no determinista → pruebas de propiedades por subproceso). ✅ **M15.2** cliente TCP (`tcp_connect`/`socket_read`/`socket_write` sobre `std::net::TcpStream`; carga útil `string`; lectura por trozos; el handle reusa el registro de archivos de M11.8 → `close` extendido a sockets; helpers clonan el stream para no retener el lock en I/O bloqueante; subproceso vs. servidor de juguete en Rust). ✅ **M15.3** servidor TCP (`tcp_listen`/`tcp_accept`/`local_port` sobre `std::net::TcpListener`; `OpenHandle::Listener` en el mismo registro; `accept` clona el listener para no retener el lock; servidor **secuencial bloqueante** —una conexión a la vez en M:1, el concurrente real es M15.5—; subproceso con el `.ray` como servidor). **Transporte TCP completo.** ✅ **M15.4a** JSON **como librería en raylang** (`examples/json.ray`: `parse`/`stringify` de descenso recursivo; objetos = `Map<string,Json>` → salida canónica con claves ordenadas; errores como `Result`; **cero runtime**, puro front-end + stdlib). Materializa "protocolos/libs en el propio lenguaje". Limitación: escapes `\uXXXX` no soportados (pediría un builtin code-point→char). Probado por subproceso (golden) en ambos motores. ✅ **M15.4b** HTTP **como librería en raylang** (`examples/http.ray`: `fetch`/`request`/`header` + parseo de URL y de respuesta, sobre los builtins TCP de M15.2; solo `http://`, `Connection: close` + leer-hasta-EOF; cabeceras en `Map` con clave en minúsculas; **cero runtime**). Atajo `fetch` (no `get`: chocaría con el `get` de Map, raylang no tiene sobrecarga). **Compone con `json`** (un GET cuyo cuerpo se parsea con la librería JSON) → showcase de librerías de raylang componiéndose. Probado vs. servidor HTTP de juguete en Rust, ambos motores. **M15.4 (protocolos en raylang) COMPLETO.** ✅ **M15.5** (capstone) sockets no bloqueantes integrados con el scheduler de M12: `tcp_accept`/`socket_read` **ceden la fibra** (la VM voltea sus sockets a no bloqueantes; en `WouldBlock` aparca en `io_parked` y el scheduler hace **busy-poll cooperativo** —duerme ~1 ms y re-encola— cuando nadie está listo; cero deps, sin `epoll`). Reusa los opcodes `SocketRead`/`TcpAccept` (solo cambia su ejecución en la VM); GC rootea `io_parked`; `cancel_task` también. El intérprete sigue con sockets bloqueantes (un hilo). Con `spawn` → **servidor concurrente** sobre un hilo (test de ordenación: el 2.º cliente recibe su eco antes de que el 1.º envíe). **Solo VM.** Diferido: cesión en `socket_write` (gira en buffer lleno), `epoll`/`kqueue`, tipo `bytes`, TLS. **M15 COMPLETO.** Sin gestor de paquetes (las "libs externas" son archivos/cápsulas del proyecto) |
| **Tipo `bytes`** (datos binarios) | Nuevo tipo en todo el pipeline (como `char`) + I/O binaria | **M16** | 🚧 DESIGN §25. Secuencia **inmutable** de octetos, hermano de `string` (inline en la VM, `Rc<Vec<u8>>` en el intérprete; no toca el GC). Cierra la deuda de M15 (carga útil binaria correcta) y cimenta TLS (M17) y el backend nativo (M18). ✅ **M16.1a** el tipo: `Type::Bytes` + keyword `bytes`; literal **`b"..."`** con escapes de string + **`\xNN`** (octeto arbitrario); `len(bytes)`, indexar `b[i] -> int`, `==` estructural; oráculo (incl. UTF-8 multibyte y bytes nulos). `print(bytes)` diferido (como `Map`). Pendiente: M16.1b string-interop (`to_bytes`/`from_utf8`/`+`) · M16.1c I/O binaria (`read_file_bytes`/`write_file_bytes`/`socket_read_bytes`/`socket_write_bytes`). Diferido: `bytes` como clave de Map, mutabilidad |
| **Habilitadores de self-hosting** (`Map<K,V>`, `assert`/test, recursión profunda) | Runtime + GC (Map) · runner (test) · hilo/límites (recursión) | **M13** | ✅ **completo** (DESIGN §22): **M13.1** `Map<K,V>` heap obj en ambos motores · **M13.2** `panic`/`assert`/`assert_eq` + runner aislado por prueba (`@test` unit/bool, filtro) · **M13.3** pila grande (hilo worker) + límite de marcos con error limpio + **TCO en ambos motores** (no quedó diferido). Genérica vía `Hash` sigue diferida |
| **Self-hosting** (raylang en raylang) | Capstone: lexer/parser/checker/intérprete/loader en raylang | **M14** | ✅ **LOGRADO — meta-circularidad** (DESIGN §23): el compilador entero escrito en raylang corre **sobre el intérprete auto-alojado** (lex/parse/check + run-on-run idénticos a Rust). Decisiones: intérprete (no VM), checker = validador, resolución en runtime (= *erasure* gratis). Oráculo Rust (texto canónico para front-end, conductual para back-end) |
| **VM auto-alojada** (compilador→bytecode + VM en raylang) | Back-end alternativo en raylang | **M14.5** (opcional) | 💤 diferido: el M2 de este módulo. El intérprete auto-alojado es el oráculo, igual que en Rust M1→M2 |
| **Tooling de editor** (coloreado / LSP) | Front-end (reutiliza el checker) | **M10** | ✅ coloreado (VSCode/Sublime) + **LSP completo**: diagnósticos, hover/def, find-references, rename, completion, signature help (M10.2b–f). Clientes VSCode/Sublime/Neovim/Helix |
| **Anotaciones** (`@test`, `@derive`, …) | Parser + fase que las consume | **M10** | ✅ conjunto cerrado: `@test` + runner, `@derive(Eq, Show)` (genera el `impl`). `@delegate`/macros de usuario → diferidos |
| **API de runtime / I/O** (`args`, `input`, `env`) | Builtins / stdlib | **M11** | ✅ `args`/`input`/`read_int`/`env`/`eprint` + I/O de archivos (`read_file`/`write_file`/`exists`/`append_file`/handles con buffering). `main` sin parámetros |
| **stdlib** (orden superior / string / I/O / arreglos) | prelude + builtins | **M7/M11** | ✅ `map`/`filter`/`fold` (M7.3) + string completa (M11.1/4/7a) + arreglos (`+`/`reverse`/`pop`/`contains`/`position`, M11.7b) + `sort`+`Ord` (M11.7d). Registro único de builtins (L1) |
| **Optimización de la VM** | `bytecode`/`compiler`/`vm` | transversal | 🚀 línea base ~3×; optimizaciones de §11 sin aplicar |
| **Asperezas de M3** | Parser + checker | hecho | ✅ `[]` en campo de struct (M6.2) y coma final en arreglos (limpieza) resueltos |

---

## 1. Concurrencia — goroutines / async-await / suspend

**La de mayor sombra de diseño de todo el lote.** No toca M1 (el intérprete es
mono-hilo), pero **condiciona la arquitectura de la VM en M2.**

Son tres respuestas a la misma pregunta —"¿cómo se suspende y reanuda trabajo?"— y
representan filosofías que compiten:

- **Goroutines + channels (estilo Go)**: green threads con planificador M:N,
  estilo bloqueante, **sin "color" de funciones**. Necesita un *scheduler* y un
  stack por goroutine (coroutines *stackful*). Conceptualmente simple de usar.
- **async/await + futures (estilo Rust/JS)**: coroutines *stackless*
  (transformación a máquina de estados / CPS). Funciones "coloreadas"
  (`async fn`). Más maquinaria de tipos, gran lección de transformación de código.
- **suspend functions (estilo Kotlin)**: coroutines stackless con `suspend`; punto
  medio.

> **Restricción para no bloquear (clave para M2).** Si en M2 construimos la VM
> usando el stack nativo de Rust para los marcos del lenguaje, suspender se vuelve
> casi imposible. Si queremos concurrencia, la VM debe tener su **propio stack
> explícito** (los frames del lenguaje viven en estructuras nuestras, no en el
> stack de Rust). Esa es la decisión de arquitectura que esto impone *cuando
> diseñemos la VM*, no antes.

**Estado tras M2**: ✅ la restricción **se respetó**. La VM (`src/vm.rs`) ejecuta sobre
un stack de operandos y una **pila de marcos explícita** (`frames: Vec<CallFrame>`,
`stack`), en un bucle iterativo — no usa el stack de Rust para los marcos del lenguaje.
Eso es justo lo que permitió, en M12, **guardar una fibra a medias y reanudarla**.

**Estado tras M12**: ✅ **COMPLETO** (DESIGN §21). Se eligió **CSP / green threads
cooperativos M:1** (no async/await, no function coloring) — *stackful* en la práctica: una
fibra es el par `(frames, stack)` de la VM, que se guarda en el scheduler al ceder. Las
cinco sub-fases (slice CSP · backpressure · structured concurrency · `select` · cancelación
de hermanas) están en la fila de la tabla de arriba y en DESIGN §21.2–§21.6. Vive **solo en
la VM** (el intérprete da error limpio y sigue siendo el oráculo secuencial); el scheduler
es **determinista**, así que los programas concurrentes se prueban contra salida exacta.
**Diferidos** (puertas abiertas, no agujeros): M:N paralelo (GC thread-safe), cancelación
preemptiva, algebraic effects (§21.1, rama de producción).

## 2. Null safety

**Ya decidido, implícitamente, al elegir `Option<T>`.** raylang **no tiene
`null`**: la ausencia de valor se modela con `Option<T>` (M6), y el checker obliga
a manejar el caso `None` antes de usar el valor. Es el enfoque de Rust/Swift/Kotlin
moderno.

- Impacto en M1: ninguno (no hay `null` de todos modos).
- Solo lo registramos como **principio**: en raylang nunca habrá referencias nulas.

## 3. Introspección / reflection

Inspeccionar tipos y valores en tiempo de ejecución.

- **Impacto**: medio en el *modelo de valores* de la VM (los valores deben
  conservar metadata de tipo); bajo en el intérprete, donde los valores ya cargan
  su tipo de forma natural.
- **Restricción**: ninguna urgente. Solo evitar un value model en la VM que borre
  toda la información de tipos en runtime.
- **Cuándo**: tardío (post-M6, una vez existan structs/enums que valga la pena
  reflejar).

## 4. Structs vs interfaces / traits

¿Solo structs (datos), o también un mecanismo de abstracción/polimorfismo?

- **Impacto**: en el sistema de tipos (M5–M6). Define la historia de polimorfismo y
  de despacho de métodos. Interactúa con genéricos (los *límites* de un genérico,
  `T: Trait`) y con UFCS.
- **Recomendación**: **structs (datos) + traits estilo Rust** (comportamiento), no
  clases con herencia. Despacho estático por defecto; *trait objects* (despacho
  dinámico) como opción posterior. Encaja limpio con UFCS y genéricos.
- **Cuándo**: decidir al llegar a M5/M6. No afecta M1.
- **Es la primitiva de desacople.** Programar contra un trait (no contra un tipo
  concreto) es lo que permite *delegar/intercambiar implementaciones*. Las
  anotaciones (§9) no desacoplan por sí solas: a lo sumo generan el reenvío
  (`@delegate`/`by` estilo Kotlin) encima de un trait. La DI por reflexión
  (estilo Spring) queda **fuera del camino principal** (runtime + reflexión; el
  trait logra el mismo desacople sin magia y con seguridad de tipos).

## 5. Hot code reloading

Reemplazar código mientras el programa corre.

- **Impacto**: en la arquitectura de runtime — favorece que las llamadas a función
  se resuelvan vía una **tabla mutable** (no punteros fijos), para poder sustituir
  una función en caliente. Aislado y de bajo riesgo.
- **Restricción**: menor; basta tenerlo en mente al diseñar la tabla de funciones
  de la VM (M2).
- **Cuándo**: muy tardío / opcional. Es más tooling que lenguaje.

## 6. Visibilidad (encapsulamiento)

`pub` explícito vs. exportar por mayúscula inicial (estilo Go) vs. otro.

- **Impacto**: pertenece al **sistema de módulos**, que M1 no tiene (un solo
  archivo). Sin módulos, no hay nada que ocultar.
- **Recomendación**: `pub` explícito, en vez de acoplar la visibilidad a la
  capitalización del identificador (la convención de Go mezcla *naming* con
  semántica y es discutible). Pero es cuestión de gusto, a confirmar.
- **Cuándo**: cuando introduzcamos módulos (aún no planificado). No afecta M1.

## 7. Self-hosting (raylang escrito en raylang) — ✅ LOGRADO (M14)

El examen definitivo: que el compilador de raylang esté escrito en **raylang mismo**, y
corra sobre sí mismo (**meta-circularidad**). **Conseguido en M14** (DESIGN §23). Detalle
completo en el libro (parte M14) y en `selfhost/`.

**Qué se logró.** El pipeline entero —lexer, parser, checker, intérprete y loader— vive en
`selfhost/*.ray`, escrito en raylang, y produce **lo mismo que la implementación de Rust**
sobre el código real del proyecto (incluido el suyo propio). Verificado eslabón a eslabón
con Rust como **oráculo**: el lexer se lexea a sí mismo, el parser se parsea, el checker da
el mismo veredicto, y el intérprete ejecuta con el mismo `stdout`+exit. El cierre total es
**run-on-run**: `run.ray` corriendo `run.ray` corriendo un programa → el back-end también.

**Tres decisiones que lo hicieron viable** (no un bootstrap clásico):
1. **Intérprete, no VM** (el back-end auto-alojado es tree-walking; mismo orden M1→M2).
2. **El checker es un validador** (produce el veredicto, NO el lowering de M9).
3. **Resolución en runtime**, no lowering: el intérprete despacha por la etiqueta del valor
   → `dyn`/bounds/genéricos son **no-ops** (el *erasure* ocurre solo). El intérprete
   "cabalga sobre el host": `Value` es un enum de raylang, las celdas de closure son un
   struct, sin GC ni celdas propias.

**Oráculo** (estrategia de todo M14): la misma entrada por ambos pipelines (Rust vs raylang),
y se comparan — **texto canónico** (tokens, AST como S-expr, veredicto del checker) para el
front-end, y **conductual** (stdout + código de salida) para el back-end.

**Habilitadores que lo precedieron**: M13 (`Map`, `panic`/`assert`+test, recursión profunda
+ TCO) lo volvió práctico; M14.6 cerró la stdlib que el compilador usa (`Map`/string/`parse_*`/
`assert`/`sort`/I/O/`pop`/concat de arreglos); M14.7 añadió el loader y la consistencia de
`args()`.

**Diferidos (no necesarios para la meta-circularidad lograda):**
- **VM auto-alojada (M14.5)**: el compilador-a-bytecode + VM en raylang, con el intérprete
  auto-alojado como oráculo. Es el M2 de este módulo; opcional.
- **Loader auto-alojado**: `import M;` calificado, módulos por directorios y cápsulas
  (`mod.ray`), reexports. El loader actual solo cubre `from M import …` (lo que usa el
  compilador). No hace falta *position-shifting* (el checker no baja por posición).
- **Resto de I/O** en el self-hosting: stdin (`input`/`read_line`), `env`, handles de
  archivo —no los usa el compilador, y `args()`/`read_file` ya bastan—.
- **Claves de `Map` genéricas** (vía un trait `Hash` con diccionarios): hoy solo primitivos.

## 8. Tooling de editor (coloreado y validación)

Soporte de los archivos `.ray` en editores. Tiene dos mitades muy distintas:

- **Coloreado (syntax highlighting)** — ✅ **hecho** para VSCode en
  `editors/vscode/` (gramática TextMate). Es **por-editor**: cada editor tiene su
  formato (TextMate en VSCode; tree-sitter en Neovim/Zed/Helix). Es una reescritura
  en regex de las reglas léxicas de `DESIGN.md` §3, independiente del lexer en Rust.

- **Validación / lint en vivo (diagnostics)** — ✅ **hecho** (M10.2). Un **Language
  Server (LSP)** dentro del propio binario (`raylang --lsp`), que **reutiliza el checker**.
  Decisión clave: **cero dependencias de Cargo** → JSON-RPC + *framing* a mano (`mod json`
  en `src/lsp.rs`), no `tower-lsp`. Es un **cliente externo** como el REPL/runner: no toca
  el core. Funciona en todos los editores; VSCode trae un cliente propio (`editors/vscode/`,
  con npm del lado del editor), Sublime/Neovim/Helix lo usan declarándolo.
  - Capacidades (M10.2b–f): diagnósticos, **hover** e **ir-a-definición** (de variables,
    funciones y tipos), **find-references**, **rename**, **completion** (de archivo y por
    ámbito) y **signature help**. El checker pasó de *validador* a *consultable*
    (`semantic_index` recolecta un `SemanticIndex` antes de cualquier lowering).
  - Diferido: hover/def de **métodos** (comparten `(línea,col)` con el receptor, sin spans).

## 9. Anotaciones (`@test`, `@derive`, …)

Metadatos adheridos a declaraciones (`@nombre` o `@nombre(args)` antes de una
función/tipo/campo). El eje que define la complejidad es **quién las consume**.

**Dirección decidida: empezar por anotaciones *integradas* (conjunto cerrado que
el compilador conoce).** Es la primera aproximación: barata, didáctica y de buen
rendimiento. Candidatas:

- `@test` — marca funciones de prueba; base de un framework de tests para `.ray`
  (el win que motiva arrancar por aquí).
- `@deprecated("...")` — el checker advierte al usarla.
- `@inline` — pista para la VM (M2+).
- `@builtin` / `@extern` — la implementación vive en el host (Rust). Permitiría
  **limpiar deuda**: `print` dejaría de ser un caso especial y sería
  `@builtin fn print(...)`.
- `@derive(Eq, Show)` — autogenera igualdad/impresión para `struct`/`enum`. Su caso
  de uso natural aparece **cuando existan structs/enums** (M3/M5), que es lo que las
  motiva de verdad.
- `@delegate` / keyword `by` — autogenera el **reenvío** de los métodos de un trait
  a un campo (`struct App impl Saludo by saludo`). Es *sugar* sobre traits (§4): la
  anotación genera el reenvío, pero el desacople lo da el trait, no la anotación.
  La inyección de dependencias por anotación (`@inject` + contenedor/reflexión)
  queda **fuera del camino principal**.

**Lo que NO hacemos por ahora:**

- **Anotaciones definidas por el usuario que "hacen algo"** = un **sistema de
  macros / metaprogramación** (transformar o generar código). Es de lo más difícil
  del diseño de lenguajes (higiene, fases, manipular el propio AST; conecta con
  reflection §3 y con self-hosting §7). Queda como **capstone de muy largo plazo**,
  opcional, con su propio hito.
- **Retención en runtime + reflexión** (estilo Java `@Retention(RUNTIME)`): atada al
  ítem de introspección §3.

**Estado (M10):** ✅ hecho para el conjunto cerrado. El parser consume `@nombre[(args)]`;
**`@test`** (con runner) y **`@derive(Eq, Show)`** (genera el `impl`, que M9 baja) están
implementados. `@delegate`/`by` y las anotaciones de usuario que "hacen algo" (macros) siguen
diferidos (capstone de muy largo plazo).

## 10. API de runtime / I/O (cómo raylang habla con el exterior)

Hoy raylang tiene un único cable hacia afuera: `print` (stdout) y el código de
salida que devuelve `main`. Para escribir apps de verdad (CLI, interactivas) hace
falta una **API de runtime**: funciones que expongan lo que el host (Rust) ya tiene.

**Decisión de diseño: los argumentos y la I/O NO van en la firma de `main`.**
`main` queda como `main() -> int` (punto de entrada + código de salida). El acceso
al exterior se hace por **funciones builtin/stdlib**, estilo Go (`os.Args`) y
Python (`sys.argv`) — no estilo C (`main(argc, argv)`). Razón: no especializa la
firma de `main`, y la capacidad queda disponible en *cualquier* función, no solo en
la entrada. Encaja con cómo ya funciona `print` (un builtin).

**Estado (M11):** ✅ implementado como builtins (la I/O falible devuelve `Option`/`Result`;
el runtime no sabe de ellos —los arma el prelude en raylang—). Superficie:

- `args() -> [string]` — argumentos de la línea de comandos. ✅
- `input()`/`read_int()` (stdin), `eprint(...)` (stderr), `env(nombre)`. ✅
- I/O de archivos: `read_file`/`write_file`/`append_file`/`exists`/`remove_file`/`list_dir`
  + handles con buffering (`open`/`read_line`/`write`/`close`, M11.8). ✅

**Matiz de orden** (dos capacidades distintas):

- **Interactivo (stdin)**: un builtin de lectura solo necesita strings/enteros, que
  ya existen → **podría llegar relativamente pronto**.
- **Por argumentos (argv)**: `args()` devuelve `[string]`, así que necesita
  **arreglos (M3)** + indexar + `len`. Es de la época de la stdlib (**M7**).

**Impacto en el diseño actual:** ninguno; es puramente aditivo (más builtins). Solo
fija la decisión de mantener `main` sin parámetros.

## 11. Optimización de la VM

**Línea base (M2):** la VM corre ~3x más rápido que el intérprete en `fib(32)`
(`benchmarks/bench.sh`), con mucha menos varianza. El ~3x es el techo de la
arquitectura *actual*, no de la idea: ambos motores comparten el mismo `Value` (que
se clona) y la misma `apply_binary`. Estas optimizaciones, de mayor a menor impacto
esperado, llevarían la VM bastante más allá. Medir cada cambio con el benchmark.

- **Evitar clones de `Value`** (victoria fácil). Hoy `GetLocal` y `Constant` hacen
  `.clone()`. Para `int/float/bool` (que son `Copy`) el clon es barato, pero para
  `string` copia el `String` entero. Pasar a un `Value` con strings compartidos
  (`Rc<str>`) abarata el clon a incrementar un contador.
- **Locales en la pila de operandos** (estilo clox). Hoy las locales viven en un
  `Vec` aparte por marco. Ponerlas en la propia pila de operandos (con un *base
  pointer* por marco) evita una indirección y un arreglo extra por llamada.
- **Despacho más rápido.** El bucle hace `match` sobre `OpCode` clonado por
  instrucción. Opciones: no clonar la instrucción (resolver el préstamo de otra
  forma), *direct threading* / *computed goto* (no disponible en Rust estable de
  forma directa; se aproxima con un `match` bien ordenado o tablas de saltos).
- **Bytecode empaquetado en bytes.** Pasar de `Vec<OpCode>` (un enum por
  instrucción) a bytes mejora la densidad de caché —el sentido original de
  "bytecode". Cuesta legibilidad; es una optimización tardía.
- **Constantes deduplicadas.** `add_constant` hoy siempre agrega; deduplicar
  reduce la tabla y mejora la localidad.
- **Peephole / plegado de constantes** en el compilador (`1 + 2` → `3`), e
  *inline caching* para llamadas. Más avanzado.

**Impacto en el diseño:** ninguno en el lenguaje; es trabajo interno de la VM. No
bloquea nada y se hace de forma incremental, midiendo con `benchmarks/`.

**Progreso (medido en `fib(32)`, release, hyperfine):**
- **Baseline**: VM 735 ms (intérprete 1.76 s; 2.40× más rápida).
- **Opt.1 — no clonar la instrucción por iteración** ✅: el bucle copia el `&CompiledProgram` a un
  local (`let program = self.program`) y toma la instrucción **prestada** (`&program…code[ip]`) en vez
  de clonarla; el préstamo es del programa (inmutable, vive como la VM), no de `self`, así que el cuerpo
  sigue pudiendo mutar `self`. → **685 ms** (~7%; 2.59×).
- **Opt.2 — pool de locales por llamada** ✅: cada llamada necesitaba un `Vec<Local>` nuevo (millones en
  recursión). Ahora un **pool/free-list** (`locals_pool`) recicla los `Vec` de los marcos que retornan
  (Return, fin de chunk, llamada en cola): `new_locals` saca del pool y reconstruye; `recycle_locals`
  devuelve (acotado a 256). **No es raíz del GC** (contenido basura entre reciclar y reusar; se `clear()`+
  reconstruye, nunca se lee lo viejo → seguro). → **552 ms** (~19%; **3.22×**, 25% acumulado vs baseline).
- **Opt.3 — `Rc<str>` para strings** ❌ **evaluado y DESCARTADO** (medido): se cambió `HeapValue::Str`
  de `String` a `Rc<str>` para abaratar el clon (bump de contador). Resultado en `benchmarks/strings.ray`
  (string-heavy): **79 → 77 ms** (~2.6%, dentro del ruido) — porque los builtins que producen strings
  (`to_upper`/`split`/…) devuelven `String` y el `.into()` a `Rc<str>` **copia** los bytes, anulando el
  clon más barato. Peor aún: en `fib` (que NO toca strings) **552 → 609 ms (−10%)**: cambiar el layout de
  `HeapValue` desplazó el codegen de LLVM del gigantesco `run()` y empeoró el bucle aritmético. Net
  negativo → **revertido**. Lección: `Rc<str>` solo gana en código *clone-heavy* (leer el mismo string
  muchas veces sin construir, p. ej. `self.src` del lexer auto-alojado); no compensa el coste de
  construcción ni el riesgo de codegen. Reconsiderar solo con un perfil que muestre el clon de strings
  dominando (string interning sería una alternativa con menos churn de construcción).
- Siguiente (no aplicados): **Opt.4** deduplicar constantes (menor), **Opt.5** peephole/plegado (menor);
  el gran salto restante sería **locales en la pila de operandos** (estilo clox: quita la indirección del
  `Vec` por marco, pero Opt.2 ya capturó la asignación) — refactor grande, ROI decreciente.

## 12. Asperezas de M3

Dos límites pequeños del front-end que afloraron al escribir ejemplos con arreglos
y structs (`examples/pila.ray`, `examples/inventario.ray`). No son bugs —el
lenguaje es consistente— sino refinamientos de ergonomía.

- **Coma final en literales de arreglo.** ✅ **Resuelto** (fase de limpieza post-M8).
  `[1, 2, 3,]` ya se acepta, como la coma final en los campos de un `struct`
  (`array_literal` corta el bucle si tras una coma viene `]`).

- **Inferencia del `[]` vacío en posición de campo.** ✅ **Resuelto en M6.2.** El
  **chequeo bidireccional** (`check_expr_expected`) propaga el tipo esperado del campo
  hacia la expresión, así que `Pila { datos: [], tope: 0 }` ya tipa sin un `let`
  intermedio. Era, como se anticipó aquí, un primer caso del trabajo de inferencia que
  M6.2/M8 generalizaron.

**Impacto**: bajo y aditivo; ningún cambio de semántica del lenguaje, solo acepta
más programas que hoy se rechazan. No bloquea nada.

---

## Cómo usar este archivo

- Cuando una idea madure y se comprometa, se **mueve** a [DESIGN.md](DESIGN.md)
  (hoja de ruta §2 o norte de diseño §10) con su hito.
- Cuando aparezca una idea nueva, se **agrega aquí** con su clasificación de
  impacto, no directamente al diseño.
- Antes de cada hito grande (sobre todo **M2**), revisar este archivo: puede que
  alguna decisión "tardía" deba adelantarse por una restricción de arquitectura.
