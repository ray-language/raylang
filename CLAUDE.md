# CLAUDE.md

Guía para trabajar en **raylang**. Lee también los documentos-contrato antes de
cambiar el lenguaje.

## Qué es

raylang es un lenguaje de programación cuyo **foco es producción real**: las
decisiones se toman por mejor ingeniería, y las dependencias de Cargo son
aceptables cuando lo son. Host: **Rust**. Estáticamente tipado, orientado a
expresiones, sintaxis de llaves.

## Documentos-contrato (fuente de verdad — leer antes de tocar comportamiento)

- **`DESIGN.md`** — especificación, decisiones fundacionales (§0), gramática (§7),
  reglas semánticas (§8) y hoja de ruta. **Cambiar el lenguaje = actualizar este
  archivo primero.**
- **`IDEAS.md`** — backlog de features futuras con su clasificación de impacto.
- **`PERFORMANCE.md`** — crónica de rendimiento (arcos, mediciones, decisiones).

## Comandos

> ⚠️ Rust se instaló vía rustup y **no está en el PATH por defecto**. Antes de
> cualquier `cargo`, ejecuta: `source "$HOME/.cargo/env"`

- **`make help`** lista todos los comandos del proyecto unificados (el `Makefile` ya
  exporta el PATH de cargo): build/run/test/clippy/ci, release/slim/pgo, bench,
  book, playground, vscode, install.
- Tests: `cargo test`
- Ejecutar un programa: `cargo run --quiet -- examples/basics/fib.ray` (corre en la
  **VM**, el motor de producto). `--interp` fuerza el intérprete (oráculo de
  desarrollo); `--vm` se acepta por compatibilidad (ya es el default).
- **CLI de subcomandos**: el binario de producto es **`ray`** (`raylang` es un alias
  del mismo binario; la lógica vive en `src/cli.rs`, dos envoltorios finos
  `src/main.rs`+`src/bin/ray.rs`). Subcomandos: `ray
  new/run/build/test/fmt/doc/lsp/repl/version/help` (`doc` = raydoc). La interfaz
  legada por flags (`--vm`/`--interp`/`--test`/`--fmt`/`--lsp`/`--repl`/`<archivo>`)
  se conserva (los tests la usan).
- REPL interactivo: `cargo run --quiet` (sin archivo) o `--repl` (sobre la VM).
- Binario release: `cargo build --release` → `./target/release/raylang prog.ray`
- **Guía de builds** (features slim, PGO, flags de adelgazamiento): `docs/build.md`.
  Release PGO: `sh tools/pgo.sh [--slim | --features "a,b,c"]`.
- **Binario nativo del PROGRAMA**: `ray build --native prog.ray [-o out] [--release]
  [--without crypto,tls,sqlite,mimalloc,ahash,regex]` transpila el programa a Rust
  (`src/transpile.rs`) y lo compila con `rustc`/`cargo` → binario de código máquina,
  byte-idéntico a la VM. Los subsistemas con-crate (TLS/cripto/SQLite) viven en
  `crates/ray-runtime` (workspace) y se enlazan **bajo demanda** (proyecto Cargo
  generado); **mimalloc y aHash van POR DEFECTO** → el default es la vía Cargo, y
  `--without mimalloc,ahash` recupera el `rustc` pelado. Exclusión estable del
  proyecto: `[native] without = ["tls", …]` en `ray.toml`. Diseño:
  `docs/transpilador-nativo.md`. No confundir con construir la toolchain `ray`.
- El código de salida del runner es el `int` que devuelve `main` (0 si es unit).

## Arquitectura (pipeline)

```
fuente → [lexer] → tokens → [parser] → AST → [checker] → [interpreter] → ejecución
```

| Archivo | Fase | Notas |
|---------|------|-------|
| `src/prelude.rs` | front-end | stdlib en raylang: Option/Result + map/filter/fold + I/O, inyectados en `check` |
| `src/builtins.rs` | front-end | **registro único** de builtins: nombre + opcode + regla de tipado. Lo consultan checker/compilador/intérprete (la *impl* de ejecución sigue en `eval_builtin`/VM) |
| `src/token.rs`, `src/lexer.rs` | léxico | texto → tokens; cada token con (línea, col) |
| `src/ast.rs`, `src/parser.rs` | sintaxis | descenso recursivo; precedencia por jerarquía de reglas |
| `src/checker.rs` | semántica | tipos; dos pasadas (firmas + cuerpos), pila de ámbitos, análisis de divergencia |
| `src/runtime.rs` | modelo de valores | `Value`/`MapKey`/`RuntimeError`/`Closure`/helpers, **compartido** por ambos motores; se compila siempre |
| `src/interpreter.rs` | ejecución | tree-walking; `return` como señal de flujo. **Oráculo de desarrollo** (feature `interp`, default); el motor de producto es la VM |
| `src/diagnostic.rs` | presentación | `render` añade la línea de fuente y un `^` bajo la posición. Solo presentación; no toca las fases |
| `src/repl.rs` | cliente externo | REPL: acumula y re-ejecuta `fn main` vía la API pública; muestra el valor con `print`. No toca el core |
| `src/test_runner.rs` | cliente externo | runner `@test`: sintetiza un `main` que corre las pruebas; código de salida = nº de fallos. No toca el core |
| `src/lsp.rs` | cliente externo | LSP: `raylang --lsp`. JSON-RPC a mano (`mod json` + framing) + diagnósticos; `analizar` reusa lex/parse/check. No toca el core |
| `src/lib.rs`, `src/main.rs` | librería + CLI | el binario es un cliente delgado (sin archivo → REPL) |

El **front-end (lexer/parser/checker) se comparte** entre ambos motores; el backend
de ejecución es bytecode + VM.

## Convenciones

- **Idioma: los IDENTIFICADORES en inglés; los comentarios `//` en español; la
  documentación `///` (visible en LSP/raydoc) en inglés.** Nombres de funciones/
  métodos, variables, parámetros, tipos y campos → **inglés** (`load`, `analyze`,
  `receiver`, `source`, `other`; los métodos de trait son `eq`/`show`/`less`). Ver
  `docs/limpieza-nombres-en-ingles.md`.
  - **Alcance del inglés**: aplica a `src/`, `selfhost/`, `packages/`,
    `benchmarks/`, `tools/` y `tests/` (integración) — incluidos los nombres de
    funciones de test dentro de `#[cfg(test)] mod tests` y los snippets raylang
    embebidos ahí. `examples/` y `book/` siguen siendo flexibles (código de usuario
    / didáctico). Lo vigila el check CI `tests/naming_policy.rs` (wordlist
    `tests/naming_policy_es.txt`; excepción puntual con `// es-ok`).
- **Comentarios y documentación en español**, en el propio código.
- **TODO lo que el lenguaje entrega al usuario: en INGLÉS.** Cubre: mensajes de
  diagnóstico (cabeceras `type error at L:C:`, `syntax error at`, `lex error at`), y
  **toda la UI del LSP** — labels/`detail`/`documentation` de completion, snippets y
  sus **placeholders** (`${1:condition}`, no `${1:condicion}`), hover, signature
  help. Evitar siempre el spanglish de cara afuera. Los mensajes
  `expect()`/descripciones de asserts en tests son internos → siguen en español.
  ⚠️ El espejo selfhost (`selfhost/checker.ray` etc.) debe emitir mensajes
  byte-idénticos a Rust: todo cambio de mensaje va en tándem con su espejo y con los
  tests que lo aseveran.
- Cada fase lleva sus tests (`#[cfg(test)] mod tests` en su archivo; en los
  módulos-directorio divididos —`vm/`, `transpile/`, `checker/`, `lsp/`— viven en el
  `tests.rs` del módulo, ver `docs/organizacion-codigo.md` §7).
- **Todo token/nodo lleva `(línea, columna)`**; los errores siempre reportan
  ubicación. Es un principio, no un extra.
- El tipo `Type` (en `ast.rs`) se diseña **extensible** (futuros genéricos/enums);
  no tratarlo como un enum cerrado de primitivos.

## Decisiones del lenguaje (resumen; detalle en DESIGN.md §0)

- Estático con anotaciones explícitas. **Orientado a expresiones** (`if`/bloques
  producen valor; retorno implícito).
- `let` inmutable / `var` mutable. Parámetros inmutables. `main` devuelve int o unit.
- **Sin `null`**. Norte de diseño: errores como valores (`Result`/`Option`/`?`),
  UFCS + pipelines, `enum` + pattern matching, genéricos.

## Método de trabajo (IMPORTANTE)

- **Avanzar UNA fase a la vez**, sin adelantarse a fases futuras. Nivel colega senior.
- **Al terminar cada paso, preparar un commit** con los cambios de ese paso; el
  usuario lo autoriza. Conventional Commits en español (`feat(parser): ...`); cada
  commit debe compilar.
- Antes de comprometer una decisión que pueda bloquear features futuras, clasificar
  su impacto en `IDEAS.md`.

### Autonomía dentro del proyecto (preferencia del usuario)

- Ejecuta **sin pedir confirmación** todas las herramientas de consola y demás
  acciones **dentro de este proyecto**: `cargo` (build/test/clippy/run), `mdbook`,
  leer/crear/editar archivos del repo, `git status`/`diff`/`add`, `npm`/`tsc` en
  `editors/vscode`, scripts de prueba, etc. No hace falta avisar para cada comando.
- El **único punto de confirmación es el commit**: antes de `git commit`, muestra el
  mensaje propuesto y **espera el visto bueno** del usuario. (Un commit por paso, en
  Conventional Commits en español.)
- **Excepción** (siguen requiriendo aviso): acciones **hacia afuera del repo** o
  difícilmente reversibles — `git push`, publicar paquetes, borrar archivos que no
  creó esta sesión, `git reset --hard`/`rebase`, o cualquier cosa fuera del
  directorio del proyecto.
- Nota: esta preferencia la **cumple Claude por instrucción**; la supresión real de
  los diálogos de permiso depende del modo de permisos / `.claude/settings.json` de
  Claude Code, que controla el usuario.

## Estado y hoja de ruta

El estado actual, las fases completadas y la hoja de ruta se registran fuera de este
archivo: **`DESIGN.md`** (especificación + roadmap), **`PERFORMANCE.md`** (crónica de
rendimiento), **`IDEAS.md`** (backlog), la **memoria** de Claude y el **historial de
git**. Consúltalos para retomar contexto.

## Gotchas

- `source "$HOME/.cargo/env"` antes de `cargo` (PATH).
- `print` es un **builtin**, no palabra clave: un argumento de tipo imprimible.
- `struct`, `fn` (también como expresión: función anónima), `enum` y `match` **ya
  son palabras clave** y se parsean. El escrutinio de `match` va **entre paréntesis**
  (`match (e) { ... }`), como if/while: evita la ambigüedad con el literal de struct.
- **Construcción de enum** `Enum.Variante` es sintácticamente igual a un acceso a
  campo: el parser emite `Field`/`Call` y el **checker los reescribe a `EnumLit`**
  (`resolve_enum_construction`). Por eso `check` toma `&mut Program`.
- **UFCS** `recv.f(args)` también llega como `Call(Field)`. A diferencia de la
  construcción de enums (pre-pasada sin tipos), UFCS **necesita el tipo del receptor**
  (campo-vs-función), así que se resuelve **durante** el checado: se registra el sitio
  `(línea, col, nombre)` y se baja después con `lower_ufcs`. La clave lleva el
  **nombre** porque el `Call` y su receptor comparten `(línea, col)` (el parser
  arranca el `Call` en el callee), y la posición sola los confunde en cadenas
  `a.f().g()`. Sobre el programa **fusionado** multi-módulo, el loader da a cada
  módulo una **banda de líneas disjunta** (`shift_program`) para que estas tablas por
  posición no colisionen.
- **Métodos de trait** reusan ese mecanismo: `recv.m(args)` resuelve en `check_call`
  con prioridad **campo → método de trait → función libre**, y comparte el lowering.
  `ufcs_sites` es un **mapa** sitio→**nombre destino**: función libre → el mismo
  nombre; método de trait → el **manglado** `Tipo#metodo`. Los métodos se inyectan
  como funciones ordinarias en `program.functions` (el intérprete/VM no saben de
  traits: **erasure**).
- **Bounds**: el checker trabaja con **firmas limpias** (solo params de usuario). Toda
  la plomería de diccionarios es **lowering post-check** (`append_dict_params` añade
  los params ocultos `T#Trait#metodo`, `lower_dict_calls` los argumentos), así que NO
  están en `FnSig` (si lo estuvieran, `f(x)` fallaría por aridad). El runtime los ve
  como funciones más (valores de primera clase).
- Un identificador en posición de **tipo** llega del parser como `Type::Struct`; el
  checker lo **normaliza** (`resolve_type`) a `Type::Enum` si es un enum, o a
  `Type::Var` si es un **parámetro de tipo** en ámbito. `self.type_params` se pone en
  ámbito al registrar/verificar cada función.
- **Genéricos = solo checker** (erasure): el intérprete y la VM no saben de `T`. La
  inferencia es `unify(param_de_la_firma, tipo_del_argumento, σ)` —asimétrica: los
  `Var` de la firma son incógnitas; los del llamador son rígidos— y `subst(retorno, σ)`.
- La VM tiene su **propio valor** (`gc::HeapValue`, con handles), distinto del
  `Value` del intérprete (con `Rc`). Se convierte en el borde (`to_value`).
