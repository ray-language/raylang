# raylang — Backlog de features y su impacto en el diseño

> Registro de ideas que NO entran ahora pero queremos considerar a futuro. Para
> cada una anotamos: **impacto** en el diseño actual, **cuándo** podría llegar, la
> **decisión/recomendación** (si la hay) y la **restricción** que debemos respetar
> hoy para no bloquearla.
>
> Las features ya comprometidas (tipos suma, genéricos, `Result`/`?`, UFCS,
> pipelines, stdlib) viven en [DESIGN.md](DESIGN.md) §2 y §10, no aquí.

## Resumen de impacto

> **Estado tras M8** (hitos M1–M8 completos). La columna *Cuándo* refleja la hoja de
> ruta M9+ acordada (ver [DESIGN.md](DESIGN.md) §2).

| Idea | ¿Dónde pega? | Cuándo | Estado |
|------|--------------|--------|--------|
| Concurrencia (goroutines / async / suspend) | **Arquitectura de la VM** | **M12** | ✅ **desbloqueada**: la VM tiene stack/marcos explícitos (`frames`/`stack` en `vm.rs`); falta elegir dirección |
| Null safety | Sistema de tipos | hecho | ✅ no hay `null` (`Option<T>`, M6) |
| Introspección / reflection | Modelo de valores de la VM | post-M11 | 💤 puerta abierta (los valores cargan tipo en runtime) |
| Structs vs interfaces/**traits** | Sistema de tipos / polimorfismo | **M9** | 📌 recomendación fijada (traits estilo Rust) |
| Hot code reloading | Indirección de llamadas en la VM | tardío | 💤 acomodable |
| Visibilidad (`pub` vs mayúscula) | Sistema de módulos | **M11** | 📌 recomendación fijada (`pub` explícito) |
| **Módulos por directorios** (`import geo/formas/circulo;`) | Loader + parser de `import` | **M11.5** | ✅ separador `/` fijado; **solo leaf-binding** + `as`; prohibido el acceso por ruta en expresiones (ambiguo con `/` y mala práctica); rutas absolutas desde la raíz. Diferido: imports relativos, `pub` granular |
| **Aislamiento de módulos** (`mod.ray` = cápsula) | Loader (resolución + aristas) | **M11.6** | 📌 dirección fijada (estrategia "cápsula"): `mod.ray` vuelve un directorio direccionable (`import geo;`) y **encapsula** su subárbol; reexport `pub from … import …`; descartados `internal/`-Go y `mod x;`/`pub(crate)`-Rust. Diseñado en DESIGN §20.3 (M11.6a fachada + M11.6b enforcement) |
| **Self-hosting** (raylang en raylang) | Capstone: requiere módulos + I/O | transversal (post-M11) | 🎯 meta-objetivo, ya habilitado por el lenguaje |
| **Tooling de editor** (coloreado / LSP) | Front-end (reutiliza el checker) | coloreado ✅ / **LSP M10** | 🔧 parcial (LSP pendiente) |
| **Anotaciones** (`@test`, `@derive`, …) | Parser + fase que las consume | **M10** | 📌 dirección fijada; `@` ✅ reservado en el lexer (`TokenKind::At`); falta el parser |
| **API de runtime / I/O** (`args`, `input`, `env`) | Builtins / stdlib | **M11** | 📌 dirección fijada |
| **stdlib** (orden superior / string / I/O) | prelude + builtins | parcial | 🟡 `map`/`filter`/`fold`+`len`/`push` ✅ (M7.3); string/I/O → M11 |
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
Así que la concurrencia **no está bloqueada arquitectónicamente**; se puede abordar en
**M12**.

**Pendiente para M12**: elegir la *dirección*. Inclinación inicial: goroutines + channels
(más simple de usar, sin function coloring, enseña a construir un scheduler) — o
async/await si atrae más la máquina de estados. (Un modelo *stackful* podría querer un
stack por goroutine; conviene revisar el bucle de la VM al diseñarlo.)

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

## 7. Self-hosting (raylang escrito en raylang) — meta-capstone

El examen definitivo: que el compilador/intérprete de raylang esté escrito en
**raylang mismo**. Es *self-hosting*; el proceso para lograrlo es *bootstrapping*.

- **Impacto en el diseño actual**: ninguno. No condiciona nada; es un objetivo que
  se *habilita* al completar el resto, no una restricción a respetar hoy.
- **Madurez requerida**: prácticamente todo el lenguaje. Un compilador es un
  programa que manipula texto y árboles, así que raylang necesitaría:
  - structs + arreglos (M3) — nodos de AST, listas de tokens,
  - **tipos suma + pattern matching (M5)** — casi imprescindibles para un AST,
  - genéricos (M6) — `Vec<Token>`, `Option<T>`, …,
  - stdlib con I/O de archivos y manejo de strings (M7+).
- **Proceso (bootstrap)**:
  1. compilador v0 en Rust (lo que estamos haciendo);
  2. reescribir el compilador en raylang;
  3. compilarlo con el de Rust → ya corre solo;
  4. que se recompile a sí mismo (si el binario es idéntico, el bootstrap es
     estable). A partir de ahí Rust deja de ser necesario.
- **Matiz para nuestro stack** (intérprete/VM): el self-hosting consistiría en
  escribir *en raylang* el front-end + el compilador-a-bytecode, y ejecutarlo
  sobre la VM (que podría seguir en Rust o reescribirse también).
- **Cuándo**: hito natural **después de M7**. Es la señal de que raylang dejó de
  ser un juguete.

## 8. Tooling de editor (coloreado y validación)

Soporte de los archivos `.ray` en editores. Tiene dos mitades muy distintas:

- **Coloreado (syntax highlighting)** — ✅ **hecho** para VSCode en
  `editors/vscode/` (gramática TextMate). Es **por-editor**: cada editor tiene su
  formato (TextMate en VSCode; tree-sitter en Neovim/Zed/Helix). Es una reescritura
  en regex de las reglas léxicas de `DESIGN.md` §3, independiente del lexer en Rust.

- **Validación / lint en vivo (diagnostics)** — ⏳ pendiente. Se hace con un
  **Language Server (LSP)**. La clave estratégica: el LSP se escribe **una sola vez
  en Rust reutilizando nuestro checker** (que ya produce errores con línea/columna),
  y **funciona en todos los editores** (VSCode, Neovim, Emacs, Helix, Zed…). Cada
  editor solo necesita un pequeño "pegamento" para lanzar el servidor.
  - Implementación sugerida: crate `tower-lsp` o `lsp-server`, un binario
    `raylang-lsp` que ante cada cambio de documento corre lexer→parser→checker y
    devuelve los errores como `Diagnostic`.
  - **Cuándo**: hito de tooling, **M10** (junto a las anotaciones). M8.3 ya dejó el
    renderizador de diagnósticos (`src/diagnostic.rs`), que el LSP puede aprovechar. Es
    la forma correcta y barata de soportar "más editores": una vez el LSP existe,
    agregar un editor es casi gratis.
  - Punto intermedio (si se quiere antes): un lint "casero" que al guardar corre el
    binario `raylang` y parsea su salida `error ... en L:C: msg`.

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

**Impacto en el diseño actual:** casi nulo. La sintaxis `@nombre[(args)]` es
**aditiva** (un pequeño cambio en el parser). ✅ El lexer ya **reserva `@`**
(`TokenKind::At`, fase de limpieza post-M8); el parser todavía no lo consume, así que un
`@` da error de sintaxis hasta M10. `@derive`/`@delegate` además dependen de **traits (M9)**.

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

Superficie prevista (se declararían como `@builtin`, ver §9):

- `args() -> [string]` — argumentos de la línea de comandos.
- `input()` / `read_line() -> string`, `read_int() -> int` — entrada estándar.
- `eprint(...)` — escribir a stderr (hoy `print` solo va a stdout).
- `env(nombre) -> string` — variables de entorno.
- I/O de archivos (leer/escribir) — más adelante.

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
