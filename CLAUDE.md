# CLAUDE.md

Guía para trabajar en **raylang**. Lee también los documentos-contrato antes de
cambiar el lenguaje.

## Qué es

raylang es un lenguaje de programación construido como **proyecto de aprendizaje**
(no para producción): el objetivo es tocar todas las fases y problemáticas de
construir un lenguaje. Host: **Rust**. Estáticamente tipado, orientado a
expresiones, sintaxis de llaves.

## Documentos-contrato (fuente de verdad — leer antes de tocar comportamiento)

- **`DESIGN.md`** — especificación de M1, decisiones fundacionales (§0), gramática
  (§7), reglas semánticas (§8) y hoja de ruta. **Cambiar el lenguaje = actualizar
  este archivo primero.**
- **`IDEAS.md`** — backlog de features futuras con su clasificación de impacto.

## Comandos

> ⚠️ Rust se instaló vía rustup y **no está en el PATH por defecto**. Antes de
> cualquier `cargo`, ejecuta: `source "$HOME/.cargo/env"`

- Tests: `cargo test`
- Ejecutar un programa: `cargo run --quiet -- examples/fib.ray`
- Binario release: `cargo build --release` → `./target/release/raylang prog.ray`
- El código de salida del runner es el `int` que devuelve `main` (0 si es unit).

## Arquitectura (pipeline)

```
fuente → [lexer] → tokens → [parser] → AST → [checker] → [interpreter] → ejecución
```

| Archivo | Fase | Notas |
|---------|------|-------|
| `src/token.rs`, `src/lexer.rs` | léxico | texto → tokens; cada token con (línea, col) |
| `src/ast.rs`, `src/parser.rs` | sintaxis | descenso recursivo; precedencia por jerarquía de reglas |
| `src/checker.rs` | semántica | tipos; dos pasadas (firmas + cuerpos), pila de ámbitos, análisis de divergencia |
| `src/interpreter.rs` | ejecución | tree-walking; valores en runtime; `return` como señal de flujo |
| `src/lib.rs`, `src/main.rs` | librería + CLI | el binario es un cliente delgado |

El **front-end (lexer/parser/checker) se comparte**; M2 reescribirá solo el
*backend* de ejecución como bytecode + VM, reutilizándolo.

## Convenciones

- **Comentarios y documentación en español**, en el propio código.
- Cada fase lleva sus tests (`#[cfg(test)] mod tests` en su archivo).
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

- Proyecto pedagógico: **avanzar UNA fase a la vez**, explicando el diseño y el
  código a fondo mientras se escribe. No adelantarse a fases futuras.
- **Al terminar cada paso, preparar un commit** con los cambios de ese paso; el
  usuario lo autoriza. Conventional Commits en español (`feat(parser): ...`); cada
  commit debe compilar.
- Antes de comprometer una decisión que pueda bloquear features futuras, clasificar
  su impacto en `IDEAS.md`.

## Estado actual

- **M1–M5 COMPLETOS** (138 tests verdes):
  - **M1**: lexer + parser + checker + intérprete.
  - **M2**: bytecode + VM (pila y marcos explícitos). El intérprete es el **oráculo**.
  - **M3**: datos compuestos — arreglos `[T]` y structs (semántica de referencia).
  - **M4**: closures (captura por referencia/upvalues) + **GC mark-and-sweep en la
    VM** (heap propio con handles en `src/gc.rs`; el intérprete sigue con `Rc`).
  - **M5**: tipos suma (`enum`) + pattern matching (`match`), en ambos motores.
    - **M5.1**: enums `Type::Enum`, construcción `Enum.Variante(args)`; `Obj::Enum`
      trazado por el GC. El checker **resuelve** la construcción (reescribe
      `Field`/`Call`→`EnumLit`) y toma `&mut Program`.
    - **M5.2**: `match (e) { patrón => cuerpo, ... }` (patrones planos: variante con
      bindings, `_`, binding suelto). **Exhaustividad** en el checker.
    - **M5.3**: `match` en la VM — bajada a bytecode (`EnumTagEq`/`GetEnumField` +
      saltos; escrutinio en un local temporal). Oráculo VM↔intérprete, incl. estrés.
  - **M6.1**: funciones genéricas (`fn id<T>(x: T) -> T`). `Type::Var`,
    `Function.type_params`. Inferencia **desde argumentos** por unificación (`subst`,
    `unify` en el checker). **Erasure**: el runtime NO cambia.
  - **M6.2**: tipos genéricos del usuario `enum Caja<T>` / `struct Par<A,B>`.
    `Type::Struct`/`Enum` llevan `Vec<Type>` de argumentos; `enum/struct.type_params`.
    Inferencia en construcción + **chequeo bidireccional** (`check_expr_expected`: un
    tipo esperado opcional que fija `Caja.Vacia`, `[]`, etc.). `check_field`/`match`
    **sustituyen** los argumentos de tipo. Runtime sin cambios (erasure).
- **Siguiente: M6.3** (`Option`/`Result` en un prelude + operador `?`).
- Dos motores que deben coincidir; los tests `oracle_*` (en `vm.rs`) lo verifican,
  incluido un modo **estrés** del GC.

## Gotchas

- `source "$HOME/.cargo/env"` antes de `cargo` (PATH).
- `print` es un **builtin**, no palabra clave: un argumento de tipo imprimible.
- `struct`, `fn` (también como expresión: función anónima), `enum` y `match` **ya
  son palabras clave** y se parsean. El escrutinio de `match` va **entre paréntesis**
  (`match (e) { ... }`), como if/while: evita la ambigüedad con el literal de struct.
- **Construcción de enum** `Enum.Variante` es sintácticamente igual a un acceso a
  campo: el parser emite `Field`/`Call` y el **checker los reescribe a `EnumLit`**
  (`resolve_enum_construction`). Por eso `check` toma `&mut Program`.
- Un identificador en posición de **tipo** llega del parser como `Type::Struct`; el
  checker lo **normaliza** (`resolve_type`) a `Type::Enum` si es un enum, o a
  `Type::Var` si es un **parámetro de tipo** en ámbito (M6). `self.type_params` se pone
  en ámbito al registrar/verificar cada función.
- **Genéricos = solo checker** (erasure): el intérprete y la VM no saben de `T`. La
  inferencia es `unify(param_de_la_firma, tipo_del_argumento, σ)` —asimétrica: los
  `Var` de la firma son incógnitas; los del llamador son rígidos— y `subst(retorno, σ)`.
- La VM tiene su **propio valor** (`gc::HeapValue`, con handles), distinto del
  `Value` del intérprete (con `Rc`). Se convierte en el borde (`to_value`).
