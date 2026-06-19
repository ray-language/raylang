# raylang — Backlog de features y su impacto en el diseño

> Registro de ideas que NO entran ahora pero queremos considerar a futuro. Para
> cada una anotamos: **impacto** en el diseño actual, **cuándo** podría llegar, la
> **decisión/recomendación** (si la hay) y la **restricción** que debemos respetar
> hoy para no bloquearla.
>
> Las features ya comprometidas (tipos suma, genéricos, `Result`/`?`, UFCS,
> pipelines, stdlib) viven en [DESIGN.md](DESIGN.md) §2 y §10, no aquí.

## Resumen de impacto

| Idea | ¿Afecta M1? | ¿Dónde pega? | Cuándo | Estado |
|------|-------------|--------------|--------|--------|
| Concurrencia (goroutines / async / suspend) | No | **Arquitectura de la VM (M2)** | pre-M2 | ⚠️ decidir dirección antes de M2 |
| Null safety | No | Sistema de tipos | ya | ✅ decidido (no hay null) |
| Introspección / reflection | No | Modelo de valores de la VM | post-M6 | 💤 solo no cerrar la puerta |
| Structs vs interfaces/traits | No | Sistema de tipos / polimorfismo | M5–M6 | 📌 recomendación fijada |
| Hot code reloading | No | Indirección de llamadas en la VM | tardío | 💤 acomodable |
| Visibilidad (`pub` vs mayúscula) | No | Sistema de módulos | cuando haya módulos | 📌 recomendación fijada |
| **Self-hosting** (raylang en raylang) | No | Capstone: requiere casi todo el lenguaje | post-M7 | 🎯 meta-objetivo |
| **Tooling de editor** (coloreado / LSP) | No | Front-end (reutiliza el checker) | coloreado ✅ ya / LSP M8 | 🔧 parcial |

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

**Pendiente**: elegir la *dirección* antes de arrancar M2. Inclinación inicial:
goroutines + channels (más simple de usar, sin function coloring, enseña a
construir un scheduler) — o async/await si te atrae más la máquina de estados.

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
  - **Cuándo**: hito de tooling, **M8**. Es la forma correcta y barata de soportar
    "más editores": una vez el LSP existe, agregar un editor es casi gratis.
  - Punto intermedio (si se quiere antes): un lint "casero" que al guardar corre el
    binario `raylang` y parsea su salida `error ... en L:C: msg`.

---

## Cómo usar este archivo

- Cuando una idea madure y se comprometa, se **mueve** a [DESIGN.md](DESIGN.md)
  (hoja de ruta §2 o norte de diseño §10) con su hito.
- Cuando aparezca una idea nueva, se **agrega aquí** con su clasificación de
  impacto, no directamente al diseño.
- Antes de cada hito grande (sobre todo **M2**), revisar este archivo: puede que
  alguna decisión "tardía" deba adelantarse por una restricción de arquitectura.
