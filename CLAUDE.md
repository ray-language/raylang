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
- REPL interactivo (M8.2): `cargo run --quiet` (sin archivo) o `--repl`
- Binario release: `cargo build --release` → `./target/release/raylang prog.ray`
- El código de salida del runner es el `int` que devuelve `main` (0 si es unit).

## Arquitectura (pipeline)

```
fuente → [lexer] → tokens → [parser] → AST → [checker] → [interpreter] → ejecución
```

| Archivo | Fase | Notas |
|---------|------|-------|
| `src/prelude.rs` | front-end | stdlib en raylang: Option/Result (M6.3) + map/filter/fold (M7.3), inyectados en `check` |
| `src/token.rs`, `src/lexer.rs` | léxico | texto → tokens; cada token con (línea, col) |
| `src/ast.rs`, `src/parser.rs` | sintaxis | descenso recursivo; precedencia por jerarquía de reglas |
| `src/checker.rs` | semántica | tipos; dos pasadas (firmas + cuerpos), pila de ámbitos, análisis de divergencia |
| `src/interpreter.rs` | ejecución | tree-walking; valores en runtime; `return` como señal de flujo |
| `src/diagnostic.rs` | presentación | M8.3: `render` añade la línea de fuente y un `^` bajo la posición. Solo presentación; no toca las fases |
| `src/repl.rs` | cliente externo | REPL (M8.2): acumula y re-ejecuta `fn main` vía la API pública; muestra el valor con `print`. No toca el core |
| `src/lib.rs`, `src/main.rs` | librería + CLI | el binario es un cliente delgado (sin archivo → REPL) |

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

- **M1–M8 COMPLETOS + limpieza** (222 tests + integración CLI verdes):
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
  - **M6.3**: `Option<T>`/`Result<T,E>` en un **prelude** (enums genéricos en raylang,
    inyectados en `check`; `src/prelude.rs`) + operador **`?`** (`ExprKind::Try`).
    Único toque de runtime: el intérprete reusa `Flow::Return`; la VM baja `?` a un
    temp local + `EnumTagEq(0)`/`GetEnumField(0)` + `Return` (sin opcode nuevo).
  - **M7.1**: **UFCS** (`recv.f(args)` ≡ `f(recv, args)`). Azúcar de **front-end**:
    el checker resuelve `Call(Field)` (necesita el tipo del receptor) —campo del struct
    gana sobre función libre— y registra los sitios (`(línea, col, nombre)`); una pasada
    `&mut` (`lower_ufcs`) los **reescribe a llamadas ordinarias** tras verificar. El
    receptor cuenta para la inferencia de genéricos. Runtime intacto.
  - **M7.2**: **pipelines** `|>` (`x |> f(a)` ≡ `f(x, a)`, receptor como primer
    argumento). Token `PipeArrow`; puro **desugaring del parser** (`make_pipeline`):
    precedencia mínima, asociativo a la izquierda, operando derecho a nivel `call`. No
    toca checker ni runtime. Compone con UFCS.
  - **M7.3**: **stdlib** de orden superior (`map`/`filter`/`fold`) **escrita en raylang**
    en el prelude (`src/prelude.rs`) e inyectada en `check` (se saltan las que el usuario
    redefina → override). Reusa `len`/`push` + genéricos + closures: **front-end puro**,
    cero opcodes nuevos. Builtins de string (`trim`/`split`/`to_string`) **diferidos**
    (necesitan un opcode por builtin en la VM; DESIGN §16.4/§16.8).
  - **M8.1**: **inferencia local** (`let x = 3` sin anotación). `StmtKind::Let.ty` pasa a
    `Option<Type>`; la anotación es opcional en el parser; el checker, si falta, **infiere
    del inicializador** (`check_expr` sin tipo esperado). Lo indeterminado (`[]`, `None`,
    `Caja.Vacia`) sigue pidiendo anotación. Solo locales: firmas explícitas (§0). Runtime
    intacto (los tipos se borran).
  - **M8.2**: **REPL** interactivo (`src/repl.rs`; `raylang` sin archivo, o `--repl`).
    **Cliente 100% externo**: usa solo la API pública (`lex`/`parse`/`check`/`run` + el
    builtin `print`); **cero cambios** en checker/interpreter. Estrategia **re-ejecutar el
    preámbulo**: acumula definiciones y sentencias y, por entrada, reconstruye
    `fn main() { <historial> print(<entrada>); }` y lo verifica/ejecuta. Muestra el
    **valor** (vía `print`; el tipo exigiría una API del checker que no se quiso añadir).
    Entradas de tipo unit → *fallback* a ejecutar sin `print`. Una entrada con error se
    descarta (no contamina el estado). Tests: unitarios (estado/rollback) + integración
    por subproceso (`tests/repl_cli.rs`).
  - **M8.3**: **mejores errores** — diagnósticos con **contexto de fuente** (línea + `^`).
    Módulo `src/diagnostic.rs` (`render`), **solo presentación**: antepone el `Display` del
    error y dibuja la línea + cursor. Reusa `(línea, col)` —**sin spans**— y es texto
    plano. Lo usan `main.rs` (las 4 fases) y el REPL (contra su fuente sintetizada).
  - **Limpieza** (post-M8): `@` reservado en el lexer (`TokenKind::At`, para anotaciones de
    M10; el parser aún no lo usa) y **coma final** permitida en literales de arreglo
    (`[1, 2, 3,]`). La aspereza del `[]` en campo de struct ya estaba resuelta (M6.2).
- **M9.1 COMPLETO** (237 tests + integración CLI verdes): **traits** (`trait`/`impl Trait
  for Tipo`) con **despacho estático** sobre tipos concretos. `Type::SelfType` (`Self`);
  `self` receptor implícito. **Front-end puro / erasure**: cada método de impl se baja a una
  función ordinaria con nombre **manglado** (`Tipo#metodo`, vía `mangle`/`type_key_of`/
  `subst_self`) e inyectada en `program.functions` (`check`, paso 0c); la validación
  (cobertura + firmas, `register_traits_impls`) y la tabla de resolución `(tipo,método)→
  manglado` van en `check_program`. La resolución por punto en `check_call` es **campo →
  método de trait → función libre (UFCS)**; `ufcs_sites` pasó a **mapa** `(línea,col,nombre)
  → nombre_destino` y un único `lower_ufcs` baja ambos. **Runtime intacto** (cero opcodes;
  oráculo VM↔intérprete sin tocar). Impls genéricos / bounds / trait objects → diferidos.
- **Siguiente: M9.2** (bounds `T: Trait` en genéricos; decisión de despacho —diccionarios
  vs. monomorfización vs. tipo en runtime—). Ver hoja de ruta (DESIGN §2, §18) / IDEAS.md.
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
- **UFCS (M7.1)** `recv.f(args)` también llega como `Call(Field)`. A diferencia de la
  construcción de enums (pre-pasada sin tipos), UFCS **necesita el tipo del receptor**
  (campo-vs-función), así que se resuelve **durante** el checado: se registra el sitio
  `(línea, col, nombre)` y se baja después con `lower_ufcs`. La clave lleva el **nombre**
  porque el `Call` y su receptor comparten `(línea, col)` (el parser arranca el `Call`
  en el callee), y la posición sola los confunde en cadenas `a.f().g()`.
- **Métodos de trait (M9.1)** reusan ese mismo mecanismo: `recv.m(args)` resuelve en
  `check_call` con prioridad **campo → método de trait → función libre**, y comparte el
  lowering. Por eso `ufcs_sites` es un **mapa** sitio→**nombre destino**: para UFCS de
  función libre el destino es el mismo nombre; para un método de trait, el **manglado**
  `Tipo#metodo` (que el usuario no puede escribir). Los métodos se inyectan como funciones
  ordinarias en `program.functions`, así que el intérprete/VM no saben de traits (erasure).
- Un identificador en posición de **tipo** llega del parser como `Type::Struct`; el
  checker lo **normaliza** (`resolve_type`) a `Type::Enum` si es un enum, o a
  `Type::Var` si es un **parámetro de tipo** en ámbito (M6). `self.type_params` se pone
  en ámbito al registrar/verificar cada función.
- **Genéricos = solo checker** (erasure): el intérprete y la VM no saben de `T`. La
  inferencia es `unify(param_de_la_firma, tipo_del_argumento, σ)` —asimétrica: los
  `Var` de la firma son incógnitas; los del llamador son rígidos— y `subst(retorno, σ)`.
- La VM tiene su **propio valor** (`gc::HeapValue`, con handles), distinto del
  `Value` del intérprete (con `Rc`). Se convierte en el borde (`to_value`).
