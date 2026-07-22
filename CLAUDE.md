# CLAUDE.md

Guía para trabajar en **raylang**. Lee también los documentos-contrato antes de
cambiar el lenguaje.

## Qué es

raylang es un lenguaje de programación. Nació como proyecto de aprendizaje (tocar
todas las fases de construir un lenguaje); desde **julio 2026 el foco es
PRODUCCIÓN REAL** (la etapa pedagógica está archivada): las decisiones se toman
por mejor ingeniería, y las dependencias de Cargo son aceptables cuando lo son
(precedentes: `ring` M43, `rusqlite` M53.3). Host: **Rust**. Estáticamente
tipado, orientado a expresiones, sintaxis de llaves.

## Documentos-contrato (fuente de verdad — leer antes de tocar comportamiento)

- **`DESIGN.md`** — especificación de M1, decisiones fundacionales (§0), gramática
  (§7), reglas semánticas (§8) y hoja de ruta. **Cambiar el lenguaje = actualizar
  este archivo primero.**
- **`IDEAS.md`** — backlog de features futuras con su clasificación de impacto.

## Comandos

> ⚠️ Rust se instaló vía rustup y **no está en el PATH por defecto**. Antes de
> cualquier `cargo`, ejecuta: `source "$HOME/.cargo/env"`

- **`make help`** lista todos los comandos del proyecto unificados (el `Makefile` ya
  exporta el PATH de cargo): build/run/test/clippy/ci, release/slim/pgo, bench,
  book, playground, vscode, install.

- Tests: `cargo test`
- Ejecutar un programa: `cargo run --quiet -- examples/basics/fib.ray` (corre en la **VM**, el
  motor de producto desde M35). `--interp` fuerza el intérprete (oráculo de desarrollo); `--vm`
  se acepta por compatibilidad (ya es el default).
- **CLI de subcomandos (M39a)**: el binario de producto es **`ray`** (`raylang` es un alias del
  mismo binario; la lógica vive en `src/cli.rs`, dos envoltorios finos `src/main.rs`+`src/bin/ray.rs`).
  Subcomandos: `ray new/run/build/test/fmt/doc/lsp/repl/version/help` (`doc` = raydoc, M40.4). La interfaz legada por flags
  (`--vm`/`--interp`/`--test`/`--fmt`/`--lsp`/`--repl`/`<archivo>`) se conserva (los tests la usan).
- REPL interactivo (M8.2): `cargo run --quiet` (sin archivo) o `--repl` (sobre la VM).
- Binario release: `cargo build --release` → `./target/release/raylang prog.ray`
- **Guía de builds** (features slim M89, PGO, flags de adelgazamiento): `docs/build.md`.
  Release PGO: `sh tools/pgo.sh [--slim | --features "a,b,c"]`.
- **Binario nativo del PROGRAMA (arco P2.b)**: `ray build --native prog.ray [-o out] [--release]
  [--without crypto,tls,sqlite,mimalloc,ahash]` transpila el programa a Rust (`src/transpile.rs`) y lo compila
  con `rustc`/`cargo` → binario de código máquina, byte-idéntico a la VM (24–61×). Los subsistemas
  con-crate (TLS/cripto/SQLite) viven en `crates/ray-runtime` (workspace) y se enlazan **bajo
  demanda** (proyecto Cargo generado); **mimalloc y aHash van POR DEFECTO** (N1/N2 jul 2026:
  wordcount/logparse −40%, +ahash −8.5% en Map string-heavy) → el default es la vía Cargo, y
  `--without mimalloc,ahash` recupera el `rustc` pelado. Exclusión estable del
  proyecto: `[native] without = ["tls", …]` en `ray.toml`. Diseño: `docs/transpilador-nativo.md`;
  crónica: `PERFORMANCE.md` (arco P2.b, Fases 34–53). No confundir con construir la toolchain `ray`.
- El código de salida del runner es el `int` que devuelve `main` (0 si es unit).

## Arquitectura (pipeline)

```
fuente → [lexer] → tokens → [parser] → AST → [checker] → [interpreter] → ejecución
```

| Archivo | Fase | Notas |
|---------|------|-------|
| `src/prelude.rs` | front-end | stdlib en raylang: Option/Result (M6.3) + map/filter/fold (M7.3) + I/O (parse_int/input/read_int/env, M11.2), inyectados en `check` |
| `src/builtins.rs` | front-end | **registro único** de builtins (L1): nombre + opcode + regla de tipado. Lo consultan checker/compilador/intérprete (la *impl* de ejecución sigue en `eval_builtin`/VM) |
| `src/token.rs`, `src/lexer.rs` | léxico | texto → tokens; cada token con (línea, col) |
| `src/ast.rs`, `src/parser.rs` | sintaxis | descenso recursivo; precedencia por jerarquía de reglas |
| `src/checker.rs` | semántica | tipos; dos pasadas (firmas + cuerpos), pila de ámbitos, análisis de divergencia |
| `src/runtime.rs` | modelo de valores | M35b: `Value`/`MapKey`/`RuntimeError`/`Closure`/helpers, **compartido** por ambos motores; se compila siempre |
| `src/interpreter.rs` | ejecución | tree-walking; `return` como señal de flujo. **Oráculo de desarrollo** tras la feature `interp` (default); el motor de producto es la VM (M35) |
| `src/diagnostic.rs` | presentación | M8.3: `render` añade la línea de fuente y un `^` bajo la posición. Solo presentación; no toca las fases |
| `src/repl.rs` | cliente externo | REPL (M8.2): acumula y re-ejecuta `fn main` vía la API pública; muestra el valor con `print`. No toca el core |
| `src/test_runner.rs` | cliente externo | runner `@test` (M10.1): sintetiza un `main` que corre las pruebas; código de salida = nº de fallos. No toca el core |
| `src/lsp.rs` | cliente externo | LSP (M10.2): `raylang --lsp`. JSON-RPC a mano (`mod json` + framing) + diagnósticos; `analizar` reusa lex/parse/check. No toca el core |
| `src/lib.rs`, `src/main.rs` | librería + CLI | el binario es un cliente delgado (sin archivo → REPL) |

El **front-end (lexer/parser/checker) se comparte**; M2 reescribirá solo el
*backend* de ejecución como bytecode + VM, reutilizándolo.

## Convenciones

- **Idioma: los IDENTIFICADORES en inglés; los comentarios `//` en español; la
  documentación `///` (visible en LSP/raydoc) en inglés.** Nombres de funciones/
  métodos, variables, parámetros, tipos y campos → **inglés** (`load`, `analyze`,
  `receiver`, `source`, `other`; los métodos de trait son `eq`/`show`/`less`).
  (La **limpieza de identificadores a inglés** L1+L2+L3 está **COMPLETA** — todo el
  core Rust, el core raylang y los métodos Eq/Show/Ord; ver
  `docs/limpieza-nombres-en-ingles.md`.)
  - **Alcance del inglés (decisión jul 2026; `tests/` y `tools/` incluidos 21 jul
    2026)**: aplica a `src/`, `selfhost/`, `packages/`, `benchmarks/`, `tools/` y
    `tests/` (integración) — incluidos los nombres de funciones de test dentro de
    `#[cfg(test)] mod tests` y los snippets raylang embebidos ahí. `examples/` y
    `book/` siguen siendo flexibles (código de usuario / didáctico). Lo vigila el
    check CI `tests/naming_policy.rs` (wordlist `tests/naming_policy_es.txt`;
    excepción puntual con `// es-ok`).
- **Comentarios y documentación en español**, en el propio código.
- **TODO lo que el lenguaje entrega al usuario: en INGLÉS** (regla generalizada 21 jul 2026;
  antes solo diagnósticos, decisión 13 jul). Cubre: mensajes de diagnóstico (cabeceras
  `type error at L:C:`, `syntax error at`, `lex error at`), y **toda la UI del LSP** —
  labels/`detail`/`documentation` de completion, snippets y sus **placeholders**
  (`${1:condition}`, no `${1:condicion}`), hover, signature help. Evitar siempre el
  spanglish de cara afuera. Los mensajes `expect()`/descripciones de asserts en tests
  son internos → siguen en español.
  La migración se hizo por lotes (con `tools/spanglish.py`, ya retirado — vive en el
  historial de git) y está **COMPLETA** (lote 1 compilador + lote 2 runtime + lote 3
  tooling + lote 4 stdlib/paquetes; se conservan en español las descripciones de
  asserts de test, los fixtures raylang embebidos, los comentarios, y el TEXTO del
  sitio de `tools/registry_site.ray` —fuera de la política de mensajes—). ⚠️ El espejo
  selfhost (`selfhost/checker.ray` etc.) debe emitir
  mensajes byte-idénticos a Rust: todo cambio de mensaje va en tándem con su
  espejo y con los tests que lo aseveran.
- Cada fase lleva sus tests (`#[cfg(test)] mod tests` en su archivo; en los módulos-directorio
  divididos —`vm/`, `transpile/`, `checker/`, `lsp/`— viven en el `tests.rs` del módulo, ver
  `docs/organizacion-codigo.md` §7).
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

- **Avanzar UNA fase a la vez**, sin adelantarse a fases futuras. (Con el foco en
  producción, la explicación didáctica extendida ya no es necesaria: nivel colega
  senior.)
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
  mensaje propuesto y **espera el visto bueno** del usuario. (El método de arriba
  sigue valiendo: un commit por paso, en Conventional Commits en español.)
- **Excepción** (siguen requiriendo aviso): acciones **hacia afuera del repo** o
  difícilmente reversibles — `git push`, publicar paquetes, borrar archivos que no
  creó esta sesión, `git reset --hard`/`rebase`, o cualquier cosa fuera del directorio
  del proyecto.
- Nota: esta preferencia la **cumple Claude por instrucción**; la supresión real de
  los diálogos de permiso depende del modo de permisos / `.claude/settings.json` de
  Claude Code, que controla el usuario.

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
- **M9.2 COMPLETO** (246 tests + integración CLI verdes): **bounds** de genéricos
  (`fn f<T: A + B>(...)`) vía **paso de diccionarios**. `Function.bounds: Vec<(String,String)>`;
  el parser los parsea en `type_params_with_bounds`. Un bound se baja a **parámetros ocultos
  de tipo función** (uno por método del trait, nombre `T#Trait#metodo`, añadidos en
  `append_dict_params`); `x.metodo()` con `x: T` acotado baja a una llamada al diccionario
  (reusa `ufcs_sites`/`lower_ufcs`); cada sitio de llamada añade los diccionarios como
  argumentos (`record_dict_args`/`dict_for`/`lower_dict_calls`), eligiendo el método manglado
  del impl concreto o **reenviando** el diccionario propio cuando `T` resuelve a un parámetro
  acotado del llamador. La inferencia (`σ` de M6) decide qué diccionario va. **Runtime
  intacto**: los diccionarios son valores función (M4); cero opcodes, oráculo sin tocar.
- **M9.2b COMPLETO** (287 tests + integración CLI verdes): **impls genéricos** (`impl<T: B>
  Trait for Caja<T>`). Idea central: un método de impl genérico **es una función genérica
  acotada** —en el paso 0c el método manglado **hereda `type_params`/`bounds` del impl** (antes
  ambos vacíos)—, así `append_dict_params`/`resolve_bound_method` lo manejan **sin código nuevo**.
  Resolución por **constructor** (`type_key_of(Caja<T>) = "Caja"`); solo impls **plenamente
  genéricos** (`Caja<T>`, no `Caja<int>`), uno por `(constructor, trait)`. Caso nuevo:
  **diccionarios anidados** — pasar `Caja<int>` a otro genérico acotado necesita un **closure**
  que capture el diccionario interno (recursivo para `Caja<Caja<int>>`). Por eso `dict_for`
  devuelve `Expr` (antes `String`) y `synth_dict_closure` arma el closure. `ImplBlock` gana
  `type_params`/`bounds`; `ensure_impl_target` valida el objetivo genérico; `generic_impls` mapea
  `(clave, trait)`→datos del impl. **`renumber_fn_exprs`** reasigna ids densos a los closures
  inyectados (intérprete/VM los exigen). Runtime intacto (closures de M4). Diferido: instancias
  solapadas/especializadas, `dyn` sobre impls genéricos.
- **M9.3a COMPLETO** (252 tests + integración CLI verdes): **métodos por defecto**. Una
  firma de trait puede traer cuerpo (`MethodSig.default_body: Option<Block>`; el parser acepta
  `;` o un bloque). Front-end puro (erasure): un método del trait con defecto no redefinido por
  el impl se **sintetiza** como un método más (función manglada `Tipo#metodo` con el cuerpo del
  defecto, `Self`→destino) en el paso 0c de `check`; `register_traits_impls` relaja la cobertura
  (falta solo si no hay defecto) y registra los defects en la tabla de métodos. Compone con
  bounds (el defecto está en la lista del trait). Runtime intacto.
- **M9.3b COMPLETO** (260 tests + integración CLI verdes): **trait objects** (`dyn Trait`,
  despacho dinámico). `Type::Dyn(String)` (keyword `dyn`). **Realización: struct sintetizado**
  `__dyn_Trait { data, métodos... }` (el fat value/vtable) → reusa structs + funciones de
  primera clase, **runtime intacto** (cero opcodes, cero GC nuevo). La **coerción** concreto→
  objeto (en `check_expr_expected`, registrada en `dyn_coercions`) baja a construir el struct;
  el **despacho** `obj.m(args)` (en `check_call`, `dispatch_dyn_method`, registrado en
  `dyn_dispatch`) baja a `{ let r = obj; (r.m)(r.data, args) }` (evita doble evaluación).
  *Object safety*: un método que usa `Self` fuera del receptor no es invocable sobre el objeto.
  Lowering en `lower_dyn`. **Gotcha resuelto**: los cuerpos de métodos por defecto (M9.3a) se
  **clonan** por impl; sus posiciones se **renumeran** (`freshen_positions`) para que las
  bajadas por posición no colisionen entre clones.
- **M9 COMPLETO.**
- **M9.4 COMPLETO** (318 tests lib): **bounds en parámetros de tipo de struct/enum** (`struct
  Caja<T: Show>`, `enum Lista<T: Eq>`). Cierra el diferido de M9.2 (DESIGN §18.6c). `StructDef`/
  `EnumDef` ganan `bounds`; el parser reusa `type_params_with_bounds` (se elimina el ya-redundante
  `type_params`). **Semántica: comprobación en construcción, cero runtime** (un struct es datos, no
  llama métodos → sin diccionarios). Tras inferir los args, `check_construction_bounds` exige que
  cada param acotado satisfaga su bound vía `satisfies_bound` (extraído de la lógica de `dict_for`:
  impl concreto, o `Var` del llamador con el mismo bound). Esto da **propagación gratis**: construir
  `Caja<U>` dentro de `fn g<U>` exige `U: Show`. `check_type_def_bounds` valida los bounds (param
  real + trait existente). El `TypeRewriter` del loader namespaca el trait del bound. Erasure total.
  **(b) `dyn Trait` sobre impls genéricos** (319 tests lib): coercionar `Caja<int>` (impl genérico
  acotado) a `dyn Trait`. La *vtable* del trait object (M9.3b) necesitaba el método manglado plano,
  que no vale para un impl genérico acotado (lleva params-diccionario). Fix: la vtable se calcula en
  el checker con **`dict_for`** (plano-vs-closure-anidado, como los diccionarios) y se guarda en
  `dyn_coercions` (ahora `(trait, Vec<Expr>)` en vez de `(trait, clave)`); `lower_dyn` solo la coloca.
  Funciona anidado (`Caja<Caja<N>>`). Closures sintéticos renumerados por `renumber_fn_exprs`.
- **M9.5a COMPLETO** (321 tests lib): **trait objects multi-trait `dyn A + B`**. `Type::Dyn(String)` →
  `Type::Dyn(Vec<String>)` (conjunto **canónico**: ordenado, sin duplicados → `dyn A+B` == `dyn B+A`).
  Parser lee `dyn A + B + …`. El struct sintetizado lleva `data` + un campo por método de la **unión**
  (orden canónico: traits ordenados × métodos en orden de decl). Coerción: el concreto implementa
  **todos** los traits; vtable vía `dict_for` por método (compone con M9.4). Despacho busca el método
  entre todos los traits del conjunto. **Reglas**: método repetido entre traits del conjunto = error
  (ambiguo, en `ensure_type`); `lower_dyn` genera un struct por **conjunto** distinto (no por trait),
  `dyn_struct_name(set)`/`dyn_method_names(set)`. `dyn_coercions` lleva `(Vec<String>, Vec<Expr>)`.
  Tocó ~8 sitios de `Dyn` (todos front-end; runtime nunca ve `dyn`).
- **M9.5b COMPLETO** (322 tests lib): **upcasting** `dyn S1` → `dyn S2` con **S2 ⊆ S1** (olvidar
  traits; no se pueden añadir). `coerce_to_dyn` detecta `actual = Dyn(source)`: idéntico → no-op;
  subconjunto → registra en `dyn_upcasts`; si no es subconjunto → error. `lower_dyn` baja el upcast a
  reconstruir el struct menor proyectando los campos del mayor (`{ let r = obj; __dyn_S2 { data:
  r.data, m: r.m, … } }`; temp para no doble-evaluar). Los structs destino de upcasts se generan junto
  a los de coerciones. **M9.5 COMPLETO. Cluster 3 (traits/genéricos) COMPLETO** (M9.4 + M9.5; se
  descartó instancias solapadas/especializadas por research-grade). Runtime intacto.
- **M10.1 COMPLETO** (272 tests + integración CLI verdes): **anotaciones**. `@nombre[(args)]`
  sobre fn/struct/enum; `Annotation { name, args }` en cada ítem; conjunto **cerrado**
  conocido por el compilador (`check_annotations`). **`@test`** (función `() -> bool`) +
  runner `--test` (cliente externo `src/test_runner.rs` que sintetiza un `main` y devuelve
  el número de fallos como código de salida). **`@derive(Eq)`** sobre struct/enum no genérico:
  el checker **genera el `impl Eq`** (sintetiza el fuente `impl Eq for T { fn eq(...) }`,
  lo parsea y lo añade a `program.impls`; M9 lo baja) — `generate_eq_derives`. Trait `Eq` en
  el prelude (`eq(self, otro: Self) -> bool`). Igualdad: struct = `&&` de campos; enum =
  `match` anidado, payload con `==`. Compone con bounds (`T: Eq`). Runtime intacto (erasure).
- **M10.2 COMPLETO** (281 tests + integración CLI verdes, incl. `tests/lsp_cli.rs`): **LSP**
  (Language Server, diagnósticos en vivo). `raylang --lsp` habla **LSP por stdin/stdout**.
  **Decisiones**: transporte **JSON-RPC a mano** (mantiene la invariante **cero dependencias de
  Cargo**: `mod json` propio —parser+serializador en std— y framing `Content-Length` en
  `src/lsp.rs`) + alcance **solo diagnósticos** (sin hover/definición → M10.2b). **Cliente
  externo** como el REPL/`--test`: `analizar(src)` corre lexer→parser→checker (**sin ejecutar**)
  y devuelve el **primer** error (*fail-fast* → un diagnóstico por documento); **cero cambios en
  el núcleo**. Traduce `(línea, col)` 1-basado → 0-basado de LSP; el mensaje es el `Display` del
  error (el subrayado lo pinta el editor, no M8.3). `serve` es genérico sobre los flujos → se
  prueba en memoria con un `Cursor` + subproceso real (`tests/lsp_cli.rs`). Neovim/Helix lo usan
  directo (un par de líneas de config, sin npm).
- **M10.2c COMPLETO**: **cliente LSP de VSCode**. La extensión `editors/vscode/` pasó de
  *solo-declarativa* (gramática) a *con código*: `src/extension.ts` (sobre `vscode-languageclient`)
  lanza `raylang --lsp` por stdio y conecta sus diagnósticos a la UI (build TS→JS con `tsc`;
  ajustes `raylang.serverPath`/`raylang.enableLsp`; v0.10.0). Trae deps de **npm** pero **del lado
  del editor** —raylang (Rust) sigue cero-dependencias—. `node_modules`/`out` en `.gitignore`.
- **M10.2d COMPLETO**: **paquete de Sublime Text 4** (`editors/sublime/`). Coloreado
  `raylang.sublime-syntax` (port del grammar de VSCode, **mismos scopes**) + `Comments.tmPreferences`
  (toggle `//`). Diagnósticos: se conecta `raylang --lsp` declarándolo en el paquete **LSP**
  (sublimelsp) — **solo config, sin compilar** (su soporte LSP es un paquete externo, como
  Neovim/Helix). Solo VSCode necesita compilar un cliente propio. README con instalación + config.
- **M10.2b COMPLETO** (288 tests + integración CLI verdes): **hover e ir-a-definición** en el LSP.
  El checker pasa de *validador* a **consultable**: `semantic_index(program)` (en `checker.rs`)
  corre un front-end factorizado (`prepare_program` + `check_program`) con un flag `gather` que
  recolecta un `SemanticIndex` —`hovers` (posición→tipo) y `defs` (uso→posición de declaración)—
  **antes de cualquier lowering** (posiciones de la fuente original). `check` no cambia su firma
  (coste cero salvo `gather`). Para los defs, `VarInfo` lleva su posición y hay un mapa
  `fn_defs`. Granularidad: **identificadores** (variables, params, funciones); métodos/tipos
  diferidos. El **LSP guarda documentos** (hover/def traen solo uri+posición), anuncia
  `hoverProvider`/`definitionProvider`, y `hover_at`/`definition_at` consultan el índice. Sin
  spans: el rango es `[col, col+largo_nombre)`; la def de un `let` apunta al `let` (degradación
  honesta). Introspección pura: runtime y semántica intactos.
- **M10.2d-LSP COMPLETO** (326 tests lib + 7 en `tests/lsp_cli.rs`): **find-references + rename** en
  el LSP (cluster 4 de la limpieza de diferidos). Reusa el índice `defs`: una declaración se
  identifica por su **clave** `(def_line, def_col)`; todos los usos con esa clave son el mismo símbolo
  (ámbitos ya resueltos). `symbol_occurrences` (en `src/lsp.rs`) halla la clave (uso bajo el cursor, o
  nombre de la declaración) y reúne declaración + usos; `references_result` → lista de `Location`
  (honra `includeDeclaration`), `rename_result` → `WorkspaceEdit`. Para el **nombre** de la
  declaración (sin spans, la def apunta al `let`/`fn`) se escanea la línea por el primer identificador
  igual al nombre — heurística en el **cliente LSP, cero cambios en el núcleo**. Anuncia
  `referencesProvider`/`renameProvider`.
- **M10.2e-LSP COMPLETO** (327 tests lib + 8 en `tests/lsp_cli.rs`): **completion** (cluster 4). Al
  pedir sugerencias, `completion_result` corre el front-end (inyecta prelude + métodos manglados) y
  ofrece funciones y tipos definidos (incl. prelude), **builtins** (`builtins::names()`, nuevo) y
  **palabras clave**; oculta nombres sintéticos (`#`, `::`, `__`). Completion "de archivo" (no por
  ámbito ni prefijo; el editor filtra). `completionProvider` (objeto vacío). Diferido: completion por
  ámbito, signature help, hover/def de métodos/tipos. **Cluster 4 (LSP avanzado): references + rename
  + completion COMPLETO.**
- **M11.1 COMPLETO** (293 tests + integración CLI verdes): **stdlib de string** (DESIGN §20.1).
  Los strings dejan de ser opacos. **Primer cambio de runtime desde M6.3** → oráculo
  VM↔intérprete (incl. estrés del GC para `split`). Operaciones como **builtins** (estilo
  `print`/`len`/`push`) → **UFCS gratis** (`s.len()`, `s.trim().split(",")`). **-a**: `+`
  concatena (reusa el opcode `Add`; checker permite `string+string`), `len` acepta string (nº de
  caracteres, extiende `Len`), `to_string(int/float/bool/string)` (opcode nuevo `ToString`, misma
  repr que `print` → cuadra el oráculo). **-b**: `trim` (opcode `Trim`), `split(s,sep) -> [string]`
  (opcode `Split`; único que asigna en el heap). Diferido: tipo `char`/indexar string,
  `parse_int` (→ M11.2), `replace`/`contains`/etc.
- **M11.3a COMPLETO** (integración `tests/modules_cli.rs`): **módulos multi-archivo** (DESIGN §20.3).
  Módulo = archivo (su *stem*). **Arquitectura: aplanar en el front-end** — un *loader*
  (`src/loader.rs`, cliente host-side como REPL/runner/LSP) carga la entrada y sus `import`
  (transitivos, ciclos seguros: BFS con `visitados`), **namespaca** las funciones de módulos
  no-entrada a `modulo::fn` (`::` ilegal en identificadores, como el `#` de M9) y **fusiona** todo
  en un `Program` plano → checker/intérprete/VM **intactos**. `import M;` + uso **calificado**
  `M.f(...)` (reusa el `.`; el resolutor *scope-aware* desambigua: local tapa módulo → campo/UFCS;
  módulo importado → ruta `M::f`). **`pub`** explícito; referenciar un ítem no-`pub` de otro módulo
  es error. **Tipos globales-únicos** (no se namespacan; un choque es error). Tokens `Pub`/`Import`;
  `Program.imports`; `Function.is_pub`.
- **M11.3b COMPLETO** (294 tests + 6 en `tests/modules_cli.rs`): **`from M import a [as b]{, …};`**.
  Trae **funciones `pub`** al ámbito del módulo (sin calificar), con **alias** para evitar
  colisiones. Tokens `From`/`As`; `Program.from_imports: Vec<FromImport>` (`ImportName { name,
  alias }`, `local()`). El loader sigue también estos imports como dependencias; el `Resolver`
  **inyecta** cada nombre local (alias u original) en su mapa `own` apuntando al global `M::a` (con
  chequeo `pub`). Colisión sin `as` → error que pide renombrar. **Importar un tipo** con `from`
  queda **diferido** (los tipos no se namespacan; el loader da un error claro). Estilo Python: `from
  M import x` **no** trae `M` (solo `x`). Runtime intacto.
- **M11.3c-1 COMPLETO** (308 tests + 11 en `tests/modules_cli.rs`): **tipos por módulo** (DESIGN §20.3).
  Los tipos (`struct`/`enum`/`trait`) dejan de ser globales-únicos: se **namespacan** a `modulo::Tipo`
  (el de entrada no → un solo archivo idéntico). `pub` en tipos (relaja `no_pub`, `is_pub` en
  Struct/Enum/TraitDef). Dos módulos pueden **reusar un nombre** (`Node`); un tipo es **privado** salvo
  importado (-2). Pieza central: el **`TypeRewriter`** (`src/loader.rs`) reescribe **todas** las
  referencias de tipo —posiciones de tipo (anotaciones, campos, payloads, `impl` target/trait, bounds,
  `dyn`) y expresiones que nombran tipos (struct lit, `Color.Rojo` que es `Field`, patrones)— *scope-
  aware* sobre los **params de tipo** (no reescribe `T`; el parser emite `Type::Struct` para todo,
  incl. `T`). **Gotcha `@derive`**: los nombres namespacados llevan `::`, **no re-lexables** por
  `generate_derives` (que genera fuente y la parsea). Solución: el loader **expande `@derive` por
  módulo con nombres locales** (re-lexables) antes de namespacar, y `generate_derives` se hizo **pub +
  idempotente** (salta `(trait,tipo)` ya implementados) para que el checker la reejecute sin duplicar
  ni intentar lexar `::`. Bonus: el `show` de Show muestra el nombre **local** (el `::` no entra en
  los string literals). Runtime intacto.
- **M11.3c-2 COMPLETO** (305 tests + 14 en `tests/modules_cli.rs`): **`from M import Tipo [as T]`** —
  cruzar **tipos `pub`** entre módulos. Cierra M11.3c. Idea: el `from`-import deja de ser solo de
  funciones; cada nombre se **clasifica** (`clasificar_from_imports`) en **valor** (función `pub` → al
  `own` del `Resolver`) o **tipo** (tipo `pub` → al mapa del `TypeRewriter`, junto a los tipos propios).
  Así una referencia al tipo importado (`Punto`/alias `P`) se reescribe a `M::Tipo` **igual que un tipo
  propio** —cero código nuevo de reescritura—. `recolectar_pub_tipos` (análogo a `recolectar_pub_fns`);
  importar un tipo **privado** → error claro (`recolectar_tipos` distingue privado de inexistente).
  `Resolver::new` ya no clasifica (recibe el mapa de valores ya hecho → deja de devolver `Result`).
- **M11.3c-3 COMPLETO** (308 tests + 17 en `tests/modules_cli.rs`): **referencias calificadas por
  módulo** — `M.Punto` en posición de tipo (anotación, campo, payload, `dyn`, bounds), `M.Punto { … }`
  (literal de struct calificado), `M.Color.Rojo[(…)]` (construcción de enum calificada) y `M.Color.Rojo`
  en patrones. Idea (front-end puro): el **parser** guarda el `.` *dentro del nombre* —`Type::Struct(
  "M.Punto")`, `enum_name = "M.Color"`, nombre `"M.Punto"` del literal; la construcción de enum llega
  como `Field`/`Call` anidados— y el **loader** resuelve `M.X → M::X` validando `import M;` + `pub`.
  Reparto valores-vs-tipos: las posiciones de **valor** (`M.Color.Rojo`) las colapsa el `Resolver`
  (consciente de ámbitos; `qualified_field` ahora resuelve también tipos `pub`); las de **tipo**
  (anotación, nombre del literal, `enum_name` del patrón) el `TypeRewriter` (`rewrite_name` parte un
  nombre con `.` y valida con `imports`+`pub_types`). Una referencia que **no resuelve se deja con el
  `.`** → el checker la rechaza (ningún tipo definido lleva `.`) → **encapsulación**. Gotcha de
  gramática: `M.Tipo { … }` se ancla a que el receptor del `.` sea un `Ident` (mismo compromiso
  struct-literal-vs-bloque que `Tipo { … }`). **M11.3c COMPLETO.** Diferido: submódulos/directorios,
  `pub` granular por campo, re-exports. Runtime intacto (oráculo VM↔intérprete sin tocar).
- **M11.5 COMPLETO** (330 tests lib + 22 en `tests/modules_cli.rs`): **módulos por directorios**
  (`import geo/formas/circulo;`, DESIGN §20.3). La identidad de un módulo deja de ser su *stem* y pasa
  a ser su **ruta**, desacoplando los cinco roles que el stem cumplía: ruta de archivo, prefijo de
  namespacing, clave de `pub` y nombre local. **Decisiones** (fijadas con el usuario): separador
  **`/`** (en la línea de `import` no hay división → sin ambigüedad; el `parser` lee `IDENT ('/'
  IDENT)*` en `module_path`), **solo leaf-binding** (`import a/b/c;` liga el último segmento `c`; `as`
  para colisiones; `ImportDecl` gana `alias` + `leaf()`), rutas **absolutas desde la raíz**, y
  **prohibido el acceso por ruta en expresiones** (ambiguo con `/` + mala práctica → el `/` solo vive
  en el `import`). **Front-end puro**: el loader namespaca con el mismo `::` traduciendo `/`
  (`ns_prefix`: `geo/formas/circulo` → `geo::formas::circulo::area`); los mapas de acceso calificado
  (`Resolver.imports`, `TypeRewriter.imports`) pasan de `HashSet<módulo>` a **`ImportMap` = leaf →
  ruta** (`build_import_map`, detecta colisión de leaf); `qualified_field`/`rewrite_name` buscan por
  leaf, validan `pub` contra la ruta y bajan a `ns_prefix(ruta)::nombre`. Compatible hacia atrás: sin
  `/`, `ns_prefix` es la identidad y el leaf es el propio nombre. Runtime intacto. Diferido: imports
  relativos, `mod.ray`/directorio-como-módulo, `pub` granular por campo, re-exports.
- **M11.6a COMPLETO** (331 tests lib + 26 en `tests/modules_cli.rs`): **aislamiento de módulos — la
  cápsula `mod.ray`** (DESIGN §20.3; estrategia elegida frente a `internal/`-Go y `mod x;`/`pub(crate)`-
  Rust). **Regla**: la presencia de `geo/mod.ray` vuelve `geo/` una **cápsula** *direccionable*
  (`import geo;` carga `geo/mod.ray`, identidad `geo`). Sintaxis nueva: **`pub from … import …`**
  (reexport; `FromImport.is_pub`, parser con lookahead `Pub`+`From` vía `check_next`) — un from-import
  que además añade los nombres a la **cara pública** de la cápsula. **Pieza central**: la superficie
  pública pasa de `módulo → {nombres}` (recolectar_pub_fns/tipos, **eliminados**) a **`Surface {
  values, types }` = nombre → global de destino** (`build_surfaces`): un `pub` definido → `ns_prefix(m)
  ::nombre`; un reexport → el global **resuelto en el origen** (a punto fijo, para cadenas). Esto era
  necesario porque un reexport apunta a un ítem definido en *otro* módulo (no vale recomputar
  `ns_prefix(este)::n`). `qualified_field`/`clasificar_from_name`/`rewrite_name` consultan `Surface`.
  Resolución de módulo: `resolve_module_path` prueba `P.ray` y si no `P/mod.ray` (error si ambos →
  forma canónica única). Compat hacia atrás exacta (sin `mod.ray`, idéntico a M11.5; para ítems
  definidos `Surface` da el mismo global). **Front-end puro, runtime intacto.** Falta M11.6b: el
  *enforcement* (importar un submódulo interno desde fuera de la cápsula = error). Diferido: imports
  relativos, `pub` granular por campo.
- **M11.6b COMPLETO** (331 tests lib + 30 en `tests/modules_cli.rs`): **enforcement de la cápsula**.
  Cierra M11.6: ahora la fachada es una **barrera**, no solo ergonomía. `capsula_violada(root, I, T)`
  (en `loader.rs`) busca el **ancestro-directorio estricto más cercano** de `T` con un `mod.ray`
  (filesystem: prefijos de `T` de profundo a superficial); si lo hay, exige que el importador `I` sea
  esa cápsula (`I == C`) o viva bajo `C/`, si no → error ("'`T`' es interno a la cápsula '`C`';
  impórtalo con 'import `C`;'"). Se llama en cada arista del BFS de `load` (imports + from-imports),
  **aunque `T` ya esté visitado** (cada sitio cuenta). Las cápsulas anidadas componen (basta la más
  cercana). Coste cero sin `mod.ray`. **M11.6 COMPLETO.** Runtime intacto.
- **M11.7 — cierre de la stdlib aditiva** (DESIGN §20.5). Tocan runtime → oráculo (estrés del GC para
  los que asignan heap). Naming: sin sobrecarga, `index_of` (string) vs `position` (arreglos).
  - **M11.7a COMPLETO** (332 tests lib): **más string** — `starts_with`/`ends_with` (`StartsWith`/
    `EndsWith`), `to_upper`/`to_lower` (`ToUpper`/`ToLower`, heap), `substring(s,i,j)` (`Substring`,
    por carácter con *clamp* → sin error de runtime), `repeat(s,n)` (`Repeat`, `n<=0`→""),
    `index_of(s,sub) -> Option<int>` (primitivo `__index_of`/`IndexOf` + envoltorio en el prelude) y
    `join(arr,sep)` (`Join`). Helpers puros compartidos en `builtins.rs` (`char_index_of`,
    `substring_chars`, `repeat_str`). Todo por carácter (consistente con `len`/`chars`/`s[i]`).
  - **M11.7b COMPLETO** (333 tests lib): **más arreglos** — concatenación `a + b` (se extiende la
    regla de `Add` en el checker + la ejecución en ambos motores; dos `Obj` con `Add` son arreglos,
    los strings son inline), `reverse` (`Reverse`, heap), `__pop`/`ArrayPop` (muta + `[T]` → `Option`
    en el prelude; ojo: el opcode de pila ya se llamaba `Pop`), `contains` **extendido** a arreglos
    (ad-hoc: string-subcadena o arreglo-pertenencia por `values_equal`), y `__position`/`Position`
    (`[int]` → `Option<int>`). Naming sin sobrecarga: `index_of` (string) vs `position` (arreglos).
  - **M11.7c COMPLETO** (333 tests lib + 8 en `tests/io_cli.rs`): **I/O de archivos** —
    `remove_file(ruta) -> Result<int,string>` (`RemoveFile`) y `list_dir(ruta) ->
    Result<[string],string>` (`ListDir`; nombres **ordenados** → determinista; helper `list_dir` en
    `builtins.rs`). Patrón de arreglo etiquetado: `__remove_file -> ["ok"]/["err",msg]`,
    `__list_dir -> ["ok", n0, …]/["err",msg]`; los envoltorios del prelude lo traducen a `Result`
    (list_dir reconstruye el `[string]` con un `while`+`push`, no hay slice de arreglos). I/O real
    no determinista → integración por subproceso (no oráculo).
  - **M11.7d COMPLETO** (334 tests lib): **sort + trait `Ord`**. Trait `Ord { fn less(self, otro:
    Self) -> bool }` en el prelude, con `impl Ord for int/float/string/char` (vía `<`) y
    `sort<T: Ord>(a: [T]) -> [T]` (insertion sort) **escrito en raylang** → reusa bounds/diccionarios
    de M9.2: **front-end puro, cero opcodes**. Habilitadores: (1) se **extienden** los comparadores
    `< <= > >=` a **string** (lexicográfico) y **char** (code point) en checker + ambos motores; (2)
    el prelude ahora inyecta **impls** (nueva `prelude::impls()` + paso 0b4 idempotente en `check`);
    (3) `ensure_impl_target` admite `Type::Char` (faltaba). Un tipo de usuario que implemente `Ord` es
    ordenable por `sort` (probado en el oráculo). **M11.7 COMPLETO.**
- **M11.8 COMPLETO** (334 tests lib + 10 en `tests/io_cli.rs`): **I/O con buffering — handles de
  archivo** (DESIGN §20.6). `open(ruta, modo) -> Result<int,string>` (modos `"r"`/`"w"`/`"a"`),
  `read_line(h) -> Option<string>` (bufferizada), `write(h, s) -> Result<int,string>`, `close(h) ->
  int`. **Sin nuevo tipo de valor ni tocar el GC**: el handle es un `int` y los archivos abiertos
  viven en un **almacén de proceso** del host (`Mutex<HashMap<i64, OpenHandle>>` + helpers en
  `builtins.rs`, como el de `args`); `OpenHandle = Reader(BufReader) | Writer(File)`. Opcodes `Open`/
  `ReadLineHandle`/`WriteHandle`/`Close` + primitivos `__open`/`__read_line_handle`/`__write_handle`
  (arreglo etiquetado/`[T]` + envoltorio en el prelude; `open` decodifica el handle con `parse_int`).
  I/O real no determinista → integración por subproceso (no oráculo).
- **M11.4 — cierre de diferidos aditivos de la stdlib** (DESIGN §20.4). Tocan runtime → oráculo
  (con estrés del GC para los que asignan heap). Tras L1, cada builtin = fila en `BUILTINS` + opcode
  + impl por motor; checker/compilador sin cambios.
  - **M11.4a COMPLETO** (309 tests lib): **más string** — `contains(s, sub) -> bool` (opcode
    `Contains`) y `replace(s, de, a) -> string` (opcode `Replace`, asigna heap). UFCS gratis
    (`s.contains(...)`, `s.replace(...)`). Oráculo `string_contains_replace_oraculo` (estrés GC).
  - **M11.4b COMPLETO** (309 tests lib + 7 en `tests/io_cli.rs`): **I/O de archivos aditiva** —
    `exists(ruta) -> bool` (opcode `Exists`, total) y `append_file(ruta, cont) -> Result<int,string>`
    (primitivo `__append_file` opcode `AppendFile` + arreglo etiquetado `["ok"]`/`["err",msg]` +
    envoltorio en el prelude, como M11.2c). Helper `builtins::append_to_file` compartido por ambos
    motores (`OpenOptions::append`). I/O no determinista → integración por subproceso, no oráculo.
  - **M11.4c-1 COMPLETO** (312 tests lib): **tipo `char`** — primer tipo nuevo desde M5. `Type::Char`
    + keyword `char`; literal `'a'` con escapes (`\n \t \\ \'`) en lexer (`TokenKind::Char`) y parser
    (`ExprKind::Char`); runtime `Value::Char`/`HeapValue::Char` en ambos motores (constante del chunk,
    como Int/Str); `print`/`to_string`/`==` (Display = el carácter, sin comillas). Compone con
    `@derive(Eq, Show)` (campo `char`) gratis. **El oráculo cazó un bug real**: el `PartialEq` manual
    de `Value` (interpreter) no tenía la rama `Char` → `'a' == 'a'` daba `false` solo en el intérprete.
    Toca ~10 archivos pero todo mecánico (literal + tipo + valor por motor).
  - **M11.4c-2 COMPLETO** (313 tests lib): **indexar `s[i] -> char`** (extiende `Index`/opcode `Index`
    para strings; out-of-bounds = error de runtime como en arreglos; `s[i] = c` **prohibido** en el
    checker, strings inmutables) y **`chars(s) -> [char]`** (builtin, opcode `Chars`, asigna heap →
    estrés del GC). Oráculo `char_indexar_y_chars_oraculo`. **M11.4c COMPLETO.** **M11.4 COMPLETO.**
- **M10.2f COMPLETO** (335 tests lib + 11 en `tests/lsp_cli.rs`): **LSP avanzado** — hover/def de
  **tipos**, **signature help** y **completion por ámbito**. (a) *Tipos*: el índice semántico
  (`semantic_index`/`checker.rs`) gana `type_defs` (posición de cada struct/enum/trait) y registra
  hover+def en los literales de struct (`Nombre { … }`) y la construcción de enum (`Color.Rojo`) vía
  `record_named` —esas posiciones SÍ son el nombre del tipo, a diferencia del nombre de método, que
  comparte `(línea,col)` con el receptor (sin spans) y queda **diferido**—. (b) *Signature help*
  (`lsp.rs`): `signatureHelpProvider` (trigger `(`/`,`); `enclosing_call` halla la función en curso y
  el param activo escaneando paréntesis hacia atrás; la firma se extrae **textualmente** de la fuente
  (`find_fn_signature`/`split_top_commas`) → robusto con el documento a medio escribir (el parse del
  archivo falla mientras tecleas args). (c) *Completion por ámbito*: `scope_locals` añade params y
  `let`/`var` de la función envolvente (kind Variable); sin spans, el alcance es la función, no el
  bloque (degradación honesta). **Cliente LSP, cero cambios de runtime/semántica.**
- **M11.2 COMPLETO** (297 tests lib + `tests/io_cli.rs`): **I/O y API de runtime** (DESIGN §20.2).
  `main` sigue sin parámetros (§0); el exterior se toca por **builtins**. **La I/O falible devuelve
  `Option`** (norte "errores como valores"). **Patrón** (como M7.3): primitivos builtin que devuelven
  **`[T]`** (vacío/único) + **envoltorios en el prelude (raylang)** que arman el `Option` con
  `Some/None` corrientes → el runtime **no sabe de `Option`** (cero maquinaria de enums nueva). Toca
  los dos motores → oráculo (determinista) + integración por subproceso (stdin/stderr/argv/env).
  - **M11.2a**: `eprint(x)` (opcode `EPrint`, stderr); `parse_int(s) -> Option<int>` (primitivo
    `__parse_int -> [int]`, opcode `ParseInt`); `input() -> Option<string>` (primitivo
    `__read_line -> [string]`, opcode `ReadLine`); `read_int() -> Option<int>` (composición pura en
    el prelude, usa `?`).
  - **M11.2b**: `env(s) -> Option<string>` (primitivo `__env -> [string]`, opcode `Env`);
    `args() -> [string]` (opcode `Args` + almacén de proceso `OnceLock` en `interpreter.rs`,
    `set_program_args`/`program_args`, leído por ambos motores). `main.rs` acepta `raylang
    [--vm|--test] <archivo> [args...]` y deja los args tras la ruta en el almacén.
  - **M11.2c** (I/O de archivos, devuelve **`Result`**): `read_file(ruta) -> Result<string,string>`,
    `write_file(ruta,cont) -> Result<int,string>`. El truco del `[T]` se generaliza a un **arreglo
    etiquetado**: el primitivo (`__read_file -> [string]` opcode `ReadFile`; `__write_file` opcode
    `WriteFile`) devuelve `["ok", payload]` / `["err", msg]`, y el prelude lo traduce a `Result` →
    runtime sigue sin saber de `Result`. **Tras L1, añadir el builtin fue solo una fila en la tabla
    `BUILTINS` + opcode + impl por motor + envoltorio** (cero cambios en checker/compilador).
- **Limpieza post-M11 L1 COMPLETO** (300 tests + integración verdes): **registro único de builtins**
  (`src/builtins.rs`). Antes cada builtin (`print`/`len`/`split`/`args`/…, ya ~13) se repetía en
  ~4 sitios (checker ×2, intérprete, compilador). Ahora una tabla `BUILTINS` con `name` + `opcode`
  + `check` (regla de tipado: `fn(&[Type]) -> Result<Type, (Option<usize> arg_culpable, String)>`).
  La consultan `check_named_call`/`name_is_callable` (checker), `emit_call` (compilador) y el
  despacho del intérprete; las *impls* de ejecución siguen en `eval_builtin` (intérprete) y el
  `match` por opcode (VM) —son código, no metadatos—. Mensajes/posiciones idénticos. Se eligió
  tabla en Rust (opción B) frente a `@builtin fn` porque 4 builtins son **ad-hoc polimórficos**
  (`print`/`eprint`/`len`/`to_string`) y no tendrían firma raylang ordinaria.
- **Limpieza post-M11 L2 COMPLETO** (303 tests + integración verdes): **`@derive(Show)`**. Segundo
  trait derivable, en paralelo a `@derive(Eq)` (M10.1): genera `impl Show { fn show(self) ->
  string }` (trait `Show` en el prelude). `generate_derives`/`validate_derive` se generalizan a
  ambos; `@derive(Eq, Show)` genera los dos. El cuerpo renderiza **por tipo**: primitivos vía
  `to_string`, struct/enum vía `show()` recursivo → **Show sí va con enums recursivos** (a
  diferencia de Eq). Formato `Nombre { c: v, … }` / `Nombre.Variante(v0, …)`. Difiere arrays/
  funciones (error claro) y genéricos. Inyección de traits del prelude pasó a **per-trait** (Show
  se inyecta aunque el usuario redefina Eq). Front-end puro (M9 baja el impl); runtime intacto.
- **Limpieza post-M11 L3 COMPLETO** (305 tests + integración verdes): **desambiguación de posiciones
  entre módulos + errores multi-archivo** (`src/loader.rs`). Descubierto un **bug real**: el lowering
  por posición de M9 (UFCS/dicts/`dyn`) indexa por `(línea,col)`; sobre el programa fusionado, dos
  sitios de módulos distintos en la misma `(línea,col)` **colisionaban → crash en ambos motores**.
  Fix: el loader da a cada módulo una **banda de líneas disjunta** (desplaza todas sus posiciones por
  un `delta`; el de entrada en `delta` 0 → un solo archivo idéntico a antes). Posiciones globalmente
  únicas. `shift_program`/`shift_*` recorren todo el AST del módulo. `Loaded` pasa a `{ program,
  modules: Vec<LoadedModule{name,source,start_line}> }`; `main.rs` localiza un error global por su
  banda y lo renderiza contra **su archivo** con la **línea local**, prefijado `[módulo]` (solo si
  hay >1 módulo; archivo único sin cambios). Runtime intacto (las posiciones se borran al ejecutar).
- **M11 + limpieza completos.** (Lo que aquí figuraba como pendiente ya está hecho: cruzar tipos
  entre módulos → M11.3c; string/archivos aditivos → M11.4/M11.7; **self-hosting** → M14, **logrado**;
  **concurrencia** → M12, **completa**.) Único transversal abierto: **optimización de la VM de Rust**
  (incremental, midiendo). Ver DESIGN §2/§21/§23 / IDEAS.md.
- **M13 — habilitadores de self-hosting** (DESIGN §22; va **antes que M12**). Tres hilos: `Map<K,V>`,
  `assert`/tooling de test, robustez de recursión profunda.
  - **M13.3a COMPLETO** (336 tests lib): **recursión profunda sin segfaults**. (1) `lib::with_big_stack`
    corre todo el trabajo del binario en un **hilo con pila de 256 MiB** (vs ~8 MiB del principal) →
    el parser de descenso recursivo (Rust) y el intérprete tree-walking (recurre sobre la pila de Rust)
    alcanzan profundidades altas sin reventar. (2) **Límite compartido** `interpreter::MAX_CALL_DEPTH =
    1024`, que la VM reusa como `MAX_FRAMES`: el intérprete cuenta llamadas a `call_body` (campo `depth`,
    chequeado **antes** de incrementar, como la VM con `frames.len()` antes de empujar) → **ambos motores
    coinciden en la frontera** y dan el mismo error ("desbordamiento de pila …"). La VM ya tenía el
    límite; M13.3a añade el del intérprete (que antes desbordaba la pila de Rust) y la pila grande.
    Oráculo `overflow_recursion_oraculo`. Diferido: **M13.3b** TCO en la VM (reusar marco en posición
    de cola).
  - **M13.2a COMPLETO** (338 tests lib): **`panic` + `assert` + `assert_eq`**. Único toque de runtime:
    builtin **`panic(msg)`** (opcode `Panic`) que aborta con `msg` en la posición de la llamada — el
    intérprete lo intercepta en `eval_call` (devuelve `Flow::Error`), la VM lo baja al opcode `Panic`;
    ambos motores dan el mismo mensaje (oráculo `panic_y_assert_falla_oraculo`). Sobre él, en el
    **prelude (raylang)**: `assert(cond)` y `assert_eq<T: Eq + Show>(a, b)` (vía `.eq()`+`.show()`,
    bounds→dicts M9.2). Sin sobrecarga → no hay `assert(cond, msg)`; para mensaje a medida, `panic("…")`.
    Habilitadores: `impl Eq`/`impl Show` para **primitivos** en el prelude (los pedía `assert_eq`) y
    **`panic` diverge** (`expr_diverges` lo reconoce → una rama que termina en panic cede el tipo a la
    otra).
  - **M13.2b COMPLETO** (338 tests lib + 6 en `tests/test_cli.rs`): **runner de `@test` mejorado**
    (cliente externo `test_runner.rs`, no toca el core). (1) `@test` admite `() -> unit` además de
    `() -> bool` (el checker relaja la firma; el runner lee el tipo del AST: bool pasa si `true`, unit
    pasa si no dispara `assert`/`panic`). (2) **Aislamiento por prueba**: cada test corre en su propia
    ejecución del intérprete (clona el programa base + `main` sintético que llama solo a esa prueba),
    así un `panic`/aserción que falla aborta *esa* ejecución y no la batería, y se captura su mensaje
    (antes: un único `main` con todas → un panic abortaba todo). (3) Reporte por test + resumen; código
    de salida = nº de fallos (compat). (4) Filtro por subcadena del nombre (`--test archivo.ray patron`).
    **M13.2 COMPLETO.**
  - **M13.1a COMPLETO** (341 tests lib): **`Map<K,V>` (núcleo)** — primer tipo compuesto nuevo desde
    M5, **objeto del heap en ambos motores** (no almacén del host: las claves/valores son `Value`, que
    difiere por motor). `Type::Map(Box<Type>, Box<Type>)` (el parser lo trae como `Struct("Map",[K,V])`;
    `resolve_type` lo reclasifica como a `Enum`/`Var`; reflejado en subst/unify/ensure_type/etc.).
    Runtime: `Value::Map(Rc<RefCell<HashMap<MapKey,Value>>>)` (intérprete) y `Obj::Map(HashMap<MapKey,
    HeapValue>)` (VM, **trazado por el GC**: solo valores; las claves son primitivos inline). `MapKey` =
    enum hashable Int/Str/Char/Bool (**no** float). Builtins (opcodes `MapNew`/`MapInsert`/
    `MapContainsKey`/`MapGet`): `map_new`, `insert`, `contains_key`, `__map_get -> [V]` (+ envoltorio
    `get -> Option<V>` en el prelude); `len` extendido. UFCS gratis. `map_new()` es **indeterminado**
    (como `[]`/`None`): su tipo lo fija el esperado (`check_expr_expected`); clave hashable validada en
    `ensure_type` (`Map<float,_>` se rechaza). Oráculo + estrés de GC. `print` de Map **diferido** (no
    printable). Ejemplo `examples/data/mapa.ray`.
  - **M13.1b COMPLETO** (343 tests lib): **recorrido de Map** — `__map_remove -> [V]` (+ `remove ->
    Option<V>` en el prelude), `keys -> [K]`, `values -> [V]` (opcodes `MapRemove`/`MapKeys`/
    `MapValues`; asignan heap → estrés de GC). `MapKey` gana `Ord`: `keys` ordenada y `values` en ese
    mismo orden de clave (casan posición a posición) → **determinista** pese al `HashMap` (oráculo).
    Libro `m13/mapas.md`. **M13.1 COMPLETO.**
  - **M13.3b COMPLETO** (346 tests lib): **TCO (recursión de cola en O(1) de pila) en AMBOS motores**
    —no solo la VM— para no romper el oráculo (con TCO solo en la VM, una recursión de cola profunda
    correría en la VM y el intérprete cortaría en `MAX_CALL_DEPTH` → divergencia). Detección de
    posición de cola con **reglas estructurales idénticas** en ambos (cuerpo de fn, ramas if/match,
    tail de bloque, valor de `return`). **VM**: peephole `optimize_tail_calls` (un `Call`/`CallValue`
    cuya continuación es `Return` —directo o vía saltos, `returns_immediately`— → `TailCall`/
    `TailCallValue`, que **reutilizan el marco**; no toca la emisión). **Intérprete**: trampolín —
    `Flow::TailCall`, `call_body` es un bucle que reemplaza la función actual y reitera (no recurre,
    no crece `depth`); `eval_tail`/`eval_tail_block` evalúan en cola; `return e` evalúa `e` en cola;
    builtins (incl. `panic`) NO son tail. **Gotcha**: el viejo `overflow_recursion_oraculo` usaba
    `bucle(n+1)` (cola) esperando desbordar; con TCO eso es bucle infinito legítimo → se cambió a
    no-cola (`1 + bucle(...)`). Verificado: 1M llamadas en cola + mutua profunda corren en O(1) y
    coinciden. **M13 COMPLETO** (13.1 + 13.2 + 13.3a + 13.3b). Habilitadores de self-hosting listos;
    siguiente gran hito: **self-hosting** (capstone) o **M12**.
- **Self-hosting (branch `feature/self-hosting`)** — capstone: escribir el compilador de raylang en
  raylang, empezando por el **lexer**, con el lexer de Rust como **oráculo** (DESIGN §23).
  - **`parse_float(s) -> Option<float>`** (347 tests lib): builtin aditivo (patrón M11.4: primitivo
    `__parse_float -> [float]` opcode `ParseFloat` + envoltorio en el prelude). Prerrequisito del
    lexer (raylang tenía `parse_int` pero no flotantes). Ambos motores usan el `f64` de Rust → el
    oráculo casa. Oráculo `parse_float_oraculo`.
  - **M14.1 — el lexer auto-alojado** (347 lib + 8 en `tests/selfhost_lexer.rs`): `selfhost/lexer.ray`
    (port casi 1:1 de `src/lexer.rs`: `TokKind`/`Token` + `lex(src) -> [Token]`) + driver
    `selfhost/lex_dump.ray` (imprime tokens en formato canónico `<KIND>@<l>:<c>`). **Oráculo** Rust↔
    raylang en `tests/selfhost_lexer.rs`: compara el formato canónico de ambos lexers sobre snippets
    y **archivos reales** (ejemplos + el propio `lexer.ray`/`lex_dump.ray` → el lexer se lexea a sí
    mismo igual que el de Rust). Viabilidad: structs con mutación de campos por referencia (estado del
    cursor), `chars`/`s[i]`/comparación de char, `parse_int`/`parse_float`. Port: sin `match` sobre
    literales de char → `if/else`; EOF con guardas `at_end`+indexación (no centinela `'\0'`). Camino
    feliz (errores → `panic`; `Result` en M14.1b). **Prerrequisitos aditivos destapados**: `parse_float`
    y **escape `\r`** (lexer de Rust + auto-alojado). **Huecos de divergencia cerrados**: un brazo de
    `match` que termina en `panic`/`return` cede el tipo a los demás (extiende M13.2a, que solo cubría
    `if`) — el lexer lo usa por doquier. DESIGN §23.
  - **M14.1b COMPLETO** (347 lib + 9 en `tests/selfhost_lexer.rs`): **errores del lexer como valores**.
    `lex` pasa de `[Token]` (camino feliz con `panic`) a **`Result<[Token], LexError>`** + `struct
    LexError { msg, line, col }` (espejo del de Rust). `number`/`string_lit`/`char_lit`/`next_token`
    devuelven `Result<TokKind, LexError>`; `lex` propaga con **`?`**; `lex_error(lx, msg)` fija la
    posición al **inicio del token**. Los mensajes se construyen **idénticos** a los de Rust (incluido
    el fragmento ofensor: `carácter inesperado '#'`, `secuencia de escape inválida '\q'`…). El driver
    imprime el error con el formato del `Display` de `LexError` (`error léxico en <l>:<c>: <msg>`) y el
    oráculo (`canonical`) hace `format!("{e}")` al fallar → cubre **también entradas inválidas**.
    Gotcha: `parse_int`/`parse_float` devuelven `Option` → `?` no cruza Option→Result; se desenvuelven
    con `match`. El único `panic` que queda marca código inalcanzable. **M14.1 COMPLETO.** DESIGN §23.2.
  - **M14.2a COMPLETO** (347 lib + 9 en `tests/selfhost_parser.rs`): **el parser auto-alojado (núcleo)**.
    `selfhost/parser.ray` (tokens → AST) se alimenta del lexer auto-alojado (`from lexer import Token,
    TokKind;`). Cubre expresiones (toda la precedencia `logic_or`→…→`primary`), sentencias (let/var/
    assign/return/expr), tipos básicos (primitivos, `[T]`, `fn(..)->R`, nombre), bloques y funciones de
    nivel superior → fib/fizzbuzz. **Oráculo = volcado canónico del AST** (S-expression con `@línea:col`
    en cada nodo de Expr/Stmt; decisión: posiciones SÍ, máximo rigor): driver `selfhost/parse_dump.ray`
    lo imprime, `tests/selfhost_parser.rs` lo reconstruye desde el AST de Rust con `dump_program`. El
    dump se hace sobre el **AST crudo** (sin checker) → nombres de tipo son `Struct(n,[])` (no Enum/Var),
    sin `EnumLit`; el parser raylang produce `TNamed` y cuadra. **Viabilidad** (spikes): AST mutuamente
    recursivo (`struct Expr`↔`enum EKind`), `[Expr]` y `Option<Expr>` (heap → sin tamaño infinito); el
    `struct Parser` se muta por referencia (como el `Lexer`); `tok_name(k)->string` da la grafía canónica
    del token para `check`/`eat`/`expect` (sin números mágicos). Camino feliz con `panic`. Diferido:
    M14.2b (structs/enums/match/fn-anónimas), M14.2c (traits/impls/genéricos/dyn/Map/`?`/pipelines/
    anotaciones/imports). DESIGN §23.3.
  - **M14.2b COMPLETO** (347 lib + 13 en `tests/selfhost_parser.rs`): **parser auto-alojado — datos y
    control**. Añade al parser EN raylang: definiciones `struct`/`enum` (sin genéricos), literal de
    struct `Nombre { campo: valor }`, funciones anónimas `fn(..){..}` (`id` denso en pre-orden, igual
    que Rust → casa en el dump) y `match`/patrones (`_`, binding, `Enum.Variante(sub-bindings)`). El AST
    crece con `StructDef`/`EnumDef`/`VariantDef`/`FnExpr`/`MatchArm`/`Pattern` y tres variantes de
    `EKind` (`EStructLit`/`EFunc`/`EMatch`); `Program` pasa a `funcs`+`structs`+`enums` (orden de volcado
    fijo: funciones, structs, enums; el oráculo usa el mismo). Corpus: snippets + `examples/data/enums.ray` y
    `match_figuras.ray` reales. **Gotcha**: `push(binds, Option.None)` no infiere `T` → se materializa
    en `var bv: Option<string> = Option.None;` (el tipo declarado fija el `None`). Diferido: M14.2c
    (traits/impls/genéricos/dyn/Map/`?`/pipelines/anotaciones/imports/`pub`). DESIGN §23.3.
  - **M14.2c-1 COMPLETO** (347 lib + 16 en `tests/selfhost_parser.rs`): **parser auto-alojado — sistema
    de tipos**. Añade: genéricos `<T: A + B>` en fn/struct/enum/impl, args de tipo (`Caja<int>`; `Map<K,
    V>` es genérico ordinario —sin nodo especial, el checker reclasifica, como en Rust—), `dyn A + B`
    (conjunto canónico vía `sort` del prelude + dedup lineal), `trait` (firmas + cuerpos por defecto),
    `impl [<…>] Trait for Tipo`, receptor `self`. AST: `Bound`/`TraitDef`/`MethodSig`/`ImplBlock`,
    genéricos en declaraciones, `TNamed(string, [Type])` (antes sin args) + `TDyn([string])`; `Program`
    gana `traits`+`impls`. **Fidelidad**: el `self` se representa `TNamed("Self",[])` y se vuelca
    `"Self"` (igual que el `SelfType` de Rust) → el dump cuadra sin nodo `Self` propio. Oráculo:
    snippets + 14 ejemplos reales (genericos/bounds/traits/tipos_genericos/impls_genericos/
    trait_objects/metodos_por_defecto/ufcs/inferencia/funciones). Diferido a c-2: `?`, `|>`,
    anotaciones, `pub`, imports, tipos calificados `M.Tipo`/`M.Enum.V`. DESIGN §23.3.
  - **M14.2c-2 COMPLETO** (347 lib + 20 en `tests/selfhost_parser.rs`): **parser auto-alojado — azúcar
    y módulos, CIERRA el parser**. Añade: `?` (`ETry`), pipelines `|>` (desugar puro a `Call`, receptor
    como primer arg, `make_pipeline`), anotaciones `@nombre[(args)]`, `pub`, `import M [as x]`/`import
    a/b/c` (`module_path`), `[pub] from M import a [as b]{,…}`, y refs calificadas `M.Tipo` (tipo),
    `M.Tipo { … }` (literal, en `call()` si el receptor del `.` es Ident) y `M.Enum.Variante` (patrón).
    AST: `Annotation`/`ImportDecl`/`ImportName`/`FromImport`, `annotations`+`is_pub` en fn/struct/enum
    (+`is_pub` en trait), `Program` con `imports`+`from_imports`. **Hito de fidelidad**: el test fuerte
    parsea los **35 ejemplos** + los **4 fuentes del self-hosting** → **el parser se parsea a sí mismo**
    idéntico al de Rust, nodo a nodo con posiciones. DESIGN §23.3.
  - **M14.2d COMPLETO** (347 lib + 21 en `tests/selfhost_parser.rs`): **errores del parser como
    valores**, **cierra el parser**. `parse` pasa de camino feliz (`panic`) a **`Result<Program,
    ParseError>`** + `struct ParseError { msg, line, col }`; cada función de parseo propaga con `?`
    (como el lexer en M14.1b). `expect`/`expect_ident` → `Result`; `perr_here` fija la posición en el
    token actual (como `error_here` de Rust), `perr_at` en una explícita. Mensajes **idénticos** a
    Rust, incluido "se esperaba una expresión, se encontró `<Debug>`": se reproduce la repr **Debug**
    de `TokenKind` con `tok_debug(k)` (nombres de variante: `Semicolon`, `LParen`…). Se añade el
    enforcement de `parse_program` (anotaciones/`pub` sobre trait/impl). Oráculo cubre **entradas
    inválidas** (11 casos). **M14.2 COMPLETO** (parser auto-alojado, con errores como valores).
    DESIGN §23.3.
  - **M14.3 — el checker (DISEÑO fijado, DESIGN §23.4)**: el checker auto-alojado será un **validador**
    (produce solo el veredicto `ok`/`error de tipos en L:C: msg`, byte-idéntico a Rust; **sin** el
    lowering de M9, que queda para el back-end). Oráculo de **veredicto** (misma fuente por ambos
    pipelines; corpus válido + inválido). Reusa el AST del parser; `Map` para ámbitos; prelude diferido.
    Sub-fases: a (núcleo monomórfico) → b (datos: arrays/structs/enums/match) → c (genéricos) → d
    (traits/impls/dyn). Errores como valores inherentes.
  - **M14.3a COMPLETO** (347 lib + 3 en `tests/selfhost_checker.rs`): **checker auto-alojado — núcleo
    monomórfico**. `selfhost/checker.ray` valida el AST del parser auto-alojado → `Result<int,
    TypeError>` (`ok` / `error de tipos en L:C: msg`, byte-idéntico a Rust). Dos pasadas (firmas en
    `Map<string,FnSig>` → exigir `main` → cuerpos); pila de ámbitos `[Map<string,VarInfo>]`. Cubre
    literales, operadores (mismas reglas/mensajes: `bin_op_str`/`is_comparable`/`order_ok`), variables
    (let/var/mutabilidad/ámbito), llamadas (aridad+tipos, builtin `print`), if/while/block/return,
    anotaciones (`ensure_type`/`resolve_type` monomórficos), divergencia. El `Type` del parser dobla
    como tipo inferido; `type_eq`/`type_str`(=Display) propios. Driver `selfhost/check_dump.ray`.
    Oráculo: 8 válidos + 20 errores + 4 ejemplos reales (fib/fizzbuzz/gcd/primes). Diferido: M14.3b
    (datos), c (genéricos), d (traits). DESIGN §23.4.
  - **M14.3b COMPLETO** (347 lib + 5 en `tests/selfhost_checker.rs`): **checker auto-alojado — datos**
    (monomórficos). Arreglos (literal `[T]`, índice `a[i]` con string→char, `len`/`push`), structs
    (definición + tablas `structs`/`enums` con campos/variantes resueltos en `register_types`, literal
    `Nombre { c: v }`, acceso a campo, asignación a campo/índice sin exigir `var`) y enums (construcción
    `Enum.Variante(args)` reconocida **en el sitio** —sin reescribir el AST a `EnumLit` como Rust—,
    `match` con patrones `_`/binding/variante, **exhaustividad**, brazos convergentes, tipos recursivos).
    El `Type` del parser **dobla** para struct y enum (`TNamed`); se distinguen por la tabla. Chequeo
    bidireccional **mínimo** (`check_expr_expected`): el esperado fija el `[]` vacío y se propaga al
    cuerpo de función (`check_block_expected`); el bidireccional completo (`None`) → M14.3c. `ensure_type`
    pasa a recibir `c`. Oráculo: 8 datos válidos + 31 errores + 9 ejemplos reales (añade structs/
    match_figuras/enums/arrays/matriz). Diferido: UFCS/métodos, `Map`, genéricos (M14.3c), traits/dyn
    (M14.3d). DESIGN §23.4.
  - **M14.3c-1 COMPLETO** (347 lib + 7 en `tests/selfhost_checker.rs`): **checker auto-alojado —
    funciones genéricas**. `FnSig.type_params`; `Checker.tparams` (params de tipo rígidos en ámbito: un
    `TNamed(T,[])` con `T` ahí es tipo VÁLIDO, lo acepta `ensure_type`); `unify`/`subst`/`unify_list`
    (incógnitas = variables de la firma llamada, pasadas como `holes`); `check_generic_call` infiere
    params↔args y devuelve el retorno sustituido (mensajes byte-idénticos: inferencia fallida,
    `'T' no puede ser X y Y a la vez`, aridad). `check_unique_tparams`; `type_arity` valida aridad de
    args de tipo. El `Type` del parser **dobla** como variable de tipo: dos `T` iguales por nombre
    (`type_eq`), así los cuerpos genéricos cuadran sin código nuevo. Gotcha: `c.tparams = []` y
    `let m = map_new()` son indeterminados → helper `no_tparams()` y anotación. Oráculo: 4 válidos +
    5 errores + `genericos.ray`. Diferido a c-2 (tipos genéricos + bidireccional), c-3 (Option/Result +
    `?`). DESIGN §23.4.
  - **M14.3c-2 COMPLETO** (347 lib + 9 en `tests/selfhost_checker.rs`): **checker auto-alojado — tipos
    genéricos + bidireccional completo**. `check_struct_lit`/`check_enum_lit` infieren args de tipo
    (`seed_sigma_from_expected` siembra σ del esperado, `unify` con campo/payload, `finalize_type_args`
    exige cada parámetro determinado; mensajes idénticos: `'A' no puede ser X y Y a la vez`, `no se pudo
    inferir el parámetro de tipo 'T' … anota el tipo`). `check_field` **sustituye** el tipo del campo con
    los args del objeto; `check_match` arma `enum_sigma` y `check_pattern` sustituye el payload de los
    bindings. **Bidireccional completo**: `check_expr_expected` propaga a struct lit, construcción de enum
    (`Caja.Vacia`), `if`, `match` (`check_expr_opt`/`check_block_opt`); `check_call`/`check_call_field`/
    `check_field_or_enum` llevan `expected: Option<Type>`; `type_has_var`/`check_value_against` deciden si
    el esperado es concreto. El monomórfico (M14.3b) es el caso σ-vacía → mensajes idénticos. Oráculo:
    4 válidos + 5 errores + `tipos_genericos.ray`/`opcional.ray`. Diferido a c-3 (Option/Result + `?`),
    d (traits/dyn/UFCS). DESIGN §23.4.
  - **M14.3c-3 COMPLETO → M14.3c COMPLETO** (347 lib + 11 en `tests/selfhost_checker.rs`): **checker
    auto-alojado — prelude (Option/Result) + `?`**. `inject_prelude` registra `Option<T>`/`Result<T,E>`
    como enums genéricos conocidos: en Rust el prelude se PARSEA de un fuente raylang, pero el checker
    auto-alojado es un VALIDADOR que solo recibe el AST → registra sus defs **directamente** (mismo
    veredicto); el usuario puede override declarando ese nombre. Reusa la maquinaria de c-2 (`Result.Ok(x)`
    siembra T,E del esperado; `match` sustituye el payload). `?` (`check_try`, nodo `ETry`): el operando
    debe ser `Result<T,E>`/`Option<T>` y la función envolvente declarar retorno compatible (mensajes
    byte-idénticos). Oráculo: 4 válidos + 5 errores + `errores.ray`. **Queda M14.3d**: traits/impls/bounds/
    `@derive`/`dyn`/UFCS-métodos + resto del prelude (Eq/Show/Ord, map/filter/fold). DESIGN §23.4.
  - **M14.3d-1 COMPLETO** (347 lib + 13 en `tests/selfhost_checker.rs`): **checker auto-alojado — UFCS +
    funciones anónimas**. `check_call_field` resuelve `recv.f(args)` por orden: (1) construcción de enum,
    (2) **campo** del struct receptor de tipo función (gana sobre UFCS), (3) **UFCS** (`check_ufcs`):
    `f(recv, args)` con el receptor como primer arg, reusando `check_named_call` (builtins/libre/genérica
    → el receptor cuenta para la inferencia). Si `name` no es llamable → `no existe campo ni función '…'
    aplicable a …`. Helpers `struct_field_type`/`name_is_callable`/`is_known_builtin`. Funciones anónimas
    (`check_func_expr`, nodo `EFunc`): cierre con captura (cuerpo ve ámbitos envolventes; `current_return`
    guardado/restaurado) → `fn(params) -> R`. Oráculo: 6 válidos + 3 errores + `ufcs.ray`/`closures.ray`.
    Diferido a d-2 (traits/impls), d-3 (bounds), d-4 (dyn/@derive/resto prelude). DESIGN §23.4.
  - **M14.3d-2 COMPLETO** (347 lib + 15 en `tests/selfhost_checker.rs`): **checker auto-alojado — traits
    + impls** (despacho estático, `Self`, métodos por defecto). `Checker` gana `traits` (nombre→firmas) y
    `methods` (`Tipo#metodo`→FnSig). `register_traits_impls` valida traits + cada impl CONCRETO (cobertura,
    sin extras/repetidos, firmas que casan con `check_method_sig` Self→target) y puebla la tabla de métodos
    (`method_fnsig`: params con Self→target, self incluido; los defectos no redefinidos también).
    `check_impl_bodies` verifica cada cuerpo de método como función con `self` concreto (defectos por
    impl). **Resolución** en `check_call_field`: campo → método de trait (`type_key_of`/`mangle`) → UFCS;
    un método aparece en errores con su nombre manglado (`'Tipo#m'`), como Rust. `ensure_impl_target`
    (concreto: struct/enum no genérico o primitivo; genéricos/bounds → d-3). Helpers `subst_self`/
    `type_key_of`/`mangle`/`has_default`. Gotcha: `Option.None => []` en match necesita anotar el `let`.
    Oráculo: 6 válidos + 7 errores + `traits.ray`. Diferido a d-3 (bounds + impls genéricos), d-4
    (dyn/@derive/resto prelude). DESIGN §23.4.
  - **M14.3d-3a COMPLETO** (347 lib + 17 en `tests/selfhost_checker.rs`): **checker auto-alojado — bounds
    en funciones**. `FnSig.bounds`, `Checker.bounds` (bounds en ámbito) y `Checker.impl_traits`
    (`Tipo#Trait`→sí, lo puebla `register_impl`). `check_bounds` valida los bounds de una función.
    **Resolución por bound** (`resolve_bound_method`, paso 3b de `check_call_field`): `x.m()` con `x: T`,
    `T: Trait` → busca el trait acotado con el método (ambigüedad=error), valida args contra la firma
    (Self→T), devuelve el retorno. **Satisfacción en el sitio** (`check_call_bounds`/
    `check_bound_satisfied`, tras inferir σ): el tipo de cada param acotado debe tener impl del trait
    (`impl_traits`) o ser un param rígido del llamador con el mismo bound (reenvío del diccionario); si no
    → `{T} no implementa '{Trait}' (requerido por la llamada)`. El paso de diccionarios (lowering) se
    OMITE; solo la satisfacción (veredicto). Oráculo: 4 válidos + 4 errores + `bounds.ray`/
    `metodos_por_defecto.ray`. Limitación: un cuerpo de defecto INVÁLIDO reporta su posición original
    (Rust renumera el clon; solo errores contrived). Diferido a d-3b (impls genéricos), d-4. DESIGN §23.4.
  - **M14.3d-3b COMPLETO → M14.3d-3 COMPLETO** (347 lib + 19 en `tests/selfhost_checker.rs`): **checker
    auto-alojado — impls genéricos** (`impl<T: B> Trait for Caja<T>`). `ensure_generic_impl_target` valida
    el objetivo (aridad + aplicado exactamente a los propios params del impl, Var distintos). Idea central
    (como Rust): el método de un impl genérico **es una función genérica acotada** — `method_fnsig` hereda
    `type_params`/`bounds` del impl → su FnSig (`Caja#medir<T: Medir>(self: Caja<T>)`) se resuelve con la
    misma `check_generic_call` (inferencia + bounds), cero código nuevo. `call_method` ramifica (concreto→
    `check_args`, genérico→`check_generic_call`). `check_impl_bounds` valida los bounds del impl;
    `check_impl_bodies` pone `type_params`/`bounds` del impl en ámbito. La satisfacción anidada (`Caja<
    Caja<int>>`, pasar `Caja<int>` a otro genérico) la cubre `impl_traits` por constructor (shallow; el
    diccionario anidado es lowering, omitido; el corpus válido no da falsos positivos). Oráculo: 4 válidos
    + 3 errores + `impls_genericos.ray`. Diferido a d-4: dyn + @derive + resto del prelude (Eq/Show/Ord,
    map/filter/fold). DESIGN §23.4.
  - **M14.3d-4a COMPLETO** (347 lib + 21 en `tests/selfhost_checker.rs`): **checker auto-alojado — trait
    objects** (`dyn Trait`). `ensure_type(TDyn)` valida el conjunto (cada trait existe, ningún método
    repetido). **Coerción** concreto→objeto (`coerce_to_dyn`, en `check_expr_expected` si se espera `dyn`
    y la expr no propaga): el concreto implementa **todos** los traits; `dyn`→`dyn` idéntico o subconjunto
    (**upcasting**); si no es subconjunto, error. **Despacho** `obj.m(args)` con `obj: dyn`
    (`dispatch_dyn_method`, paso 1.5 de `check_call_field`): busca el método entre los traits, exige
    *object-safety* (`Self` solo en el receptor; `method_uses_self`/`type_uses_self`), valida args y
    devuelve el retorno. Helpers `propaga_esperado`/`subset_strs`/`find_dyn_method`. La síntesis del struct
    vtable (lowering) se OMITE. Oráculo: 4 válidos + 5 errores + `trait_objects.ray`. Diferido a d-4b:
    `@derive` + resto del prelude (Eq/Show/Ord, map/filter/fold). DESIGN §23.4.
  - **M14.3d-4b COMPLETO** (347 lib + 22 en `tests/selfhost_checker.rs`): **checker auto-alojado — prelude
    de orden superior** (map/filter/fold). `inject_prelude_fns` registra las FIRMAS de `map<T,U>`/
    `filter<T>`/`fold<T,A>` en `c.funcs` (en Rust se parsean del prelude; el validador solo necesita la
    firma). `inject_fn`/`user_declares_fn` saltan las que el usuario redefina (override). Compone con UFCS
    (`xs.map(f)`), pipelines (`xs |> map(f)`; el receptor cuenta para la inferencia) y closures inline.
    Oráculo: 4 válidos + 1 error + `stdlib.ray`. Diferido a d-4c: Eq/Show/Ord + impls de primitivos +
    `@derive` + anotaciones (cierra M14.3). DESIGN §23.4.
  - **M14.3d-4c COMPLETO → M14.3d COMPLETO → M14.3 COMPLETO** (347 lib + 24 en `tests/selfhost_checker.rs`):
    **checker auto-alojado — @derive + Eq/Show/Ord + anotaciones**. `inject_prelude_traits` registra
    `Eq`/`Show`/`Ord` (firmas) + impls de primitivos (`int#igual`/`int#mostrar`/… en `methods`+
    `impl_traits`). **`@derive(Eq, Show)`** (`generate_derives`/`validate_derive`) sobre struct/enum no
    genérico: registra los métodos derivados (`eq`/`show`) + `impl_traits`, idempotente; NO chequea
    el cuerpo (codegen conocido; un campo no derivable no se detecta: limitación). **`check_annotations`**:
    `@test` solo en funciones `()->bool`/`()->unit`, `@derive` solo en tipos, otras desconocidas (mensajes
    byte-idénticos). Un tipo derivado satisface `T: Eq` y responde a `.eq()`/`.show()`. Oráculo:
    6 válidos + 8 errores + `anotaciones.ray`. **El checker auto-alojado valida el LENGUAJE COMPLETO**
    (núcleo+datos+genéricos+traits/impls/bounds/dyn+prelude+derive), byte-idéntico a Rust sobre 22
    ejemplos reales + ~80 casos. Diferidos (fuera del corpus): `Map` en el checker, bounds anidados
    profundos, posición de cuerpos de defecto inválidos, prelude más allá de map/filter/fold. **Siguiente
    gran hito: el back-end (ejecución/lowering) para cerrar el self-hosting.** DESIGN §23.4.
  - **M14.4 — el back-end (intérprete auto-alojado), DISEÑO fijado** (DESIGN §23.5). Cierra el
    self-hosting: ejecutar el AST validado. **Decisiones** (con el usuario): (1) **motor = intérprete
    tree-walking** (port de `src/interpreter.rs`, el oráculo simple); la VM (compilador+pila+GC) queda
    como M14.5 opcional —mismo orden M1→M2—. (2) **Resolución en runtime, NO lowering**: como el
    checker auto-alojado es solo validador (omitió el lowering de M9), el intérprete resuelve
    construcción de enum / UFCS / métodos / `dyn` **en tiempo de evaluación** mirando la **etiqueta del
    valor**; consecuencia elegante: **`dyn`/bounds/genéricos son no-ops** (el intérprete nunca consulta
    tipos → el **borrado ocurre solo**, sin pasada de lowering). Diverge a propósito del intérprete de
    Rust (que es tonto porque el lowering ya pasó), pero el oráculo es **conductual** → invisible. (3)
    **Oráculo conductual = stdout + código de salida** (no texto canónico): la misma `.ray` por ambos
    pipelines (Rust `cargo run` vs `raylang selfhost/run.ray`), comparar comportamiento; corpus = los
    ejemplos deterministas (I/O no determinista excluida, como `tests/io_cli.rs`). **Factible porque
    cabalga sobre el host**: el `Value` es un enum de raylang en el heap de la VM anfitriona (su GC lo
    recolecta) y la semántica de referencia + las **celdas** de closure (M4.2) las da gratis un
    arreglo/struct de raylang (un `[Value]` de longitud 1 = celda mutable compartida) → **ni GC ni celdas
    propias**. Sub-fases: **a** núcleo (primitivos, control, llamadas, recursión) → **b** datos
    (arreglos/structs/enums/match) → **c** primera clase (closures, orden superior) → **d** despacho
    dinámico (tabla de métodos, UFCS/métodos/`dyn`/`@derive`/bounds). Driver `selfhost/run.ray`.
  - **M14.4a COMPLETO** (347 lib + 5 en `tests/selfhost_interpreter.rs`): **intérprete auto-alojado —
    núcleo**. `selfhost/interpreter.ray` ejecuta el AST validado → `Result<Value, RuntimeError>`. `Value`
    = enum de raylang (primitivos `VInt`/`VFloat`/`VBool`/`VStr`/`VChar`/`VUnit`); flujo `enum Flow {
    FReturn, FError }` propagado por `?` (como el `Flow` de Rust, sin `TailCall`). `struct Interp { funcs:
    Map<string,Func>, scopes: [Map<string,Value>] }` mutado por referencia. Cubre literales, aritmética/
    comparación/lógica (cortocircuito de `&&`/`||`, div/módulo por cero → error de ejecución), variables
    (`define`/`lookup_opt`/`assign`; mutación; shadowing), if/while/block/return, llamadas nombradas +
    recursión (`call_named` guarda/restaura `scopes` → scoping léxico; desenvuelve `FReturn`), builtins
    print/eprint/to_string/panic. Las formas no-núcleo terminan en `panic` con su sub-fase. Driver
    `selfhost/run.ray` (lex→parse→check→ejecuta; exit = `int` de `main`). **Oráculo CONDUCTUAL**
    (`tests/selfhost_interpreter.rs`): mismo stdout + código de salida que el runner de Rust sobre fib/
    fizzbuzz/gcd/primes + snippets. Corpus solo usa lo que aceptan **ambos** checkers (print/eprint;
    `to_string` lo rechaza el checker auto-alojado de M14.3 → fuera del corpus). Diferido: b (datos), c
    (primera clase), d (despacho dinámico); TCO/`MAX_CALL_DEPTH` (el host ya da TCO + pila grande).
  - **M14.4b COMPLETO** (347 lib + 9 en `tests/selfhost_interpreter.rs`): **intérprete auto-alojado —
    datos**. `Value` gana `VArray`/`VStruct(string,[SField])`/`VEnum(string,string,[Value])`; `Interp`
    gana tablas `structs`/`enums`. Aquí luce **cabalgar sobre el host**: la semántica de **referencia**
    de arreglos/structs del invitado es la del `[Value]`/`[SField]` del host (alias comparten el objeto;
    `push`/`a[i]=v`/`obj.f=v` mutan en el sitio → `r.origen.x=99 ⇒ p.x=99`). Y la **resolución en
    runtime**: la construcción de enum (`Enum.Variante[(args)]`), que el checker-validador no reescribió,
    se reconoce en eval mirando `c.enums` (`enum_has_variant`); el resto de `obj.f` es campo o método
    (M14.4d). Cubre arreglos (literal/índice/`len`/`push`/asignación/anidados; string[i]→char), structs
    (literal en orden de declaración, acceso/asignación de campo, aliasing), enums (construcción, `match`
    con patrones `_`/binding/variante+payload, recursivos). `value_str`/`values_equal` structural recursivo.
    Oráculo: 5 ejemplos de datos + snippets, mismo stdout+exit que Rust. Diferido: c (closures/orden
    superior/Option/Result/`?`), d (UFCS/métodos/dyn/@derive).
  - **M14.4c COMPLETO** (347 lib + 12 en `tests/selfhost_interpreter.rs`): **intérprete auto-alojado —
    primera clase**. `Value` gana `VFunc(Func)`/`VClosure(FnExpr,[Capture])`. **Ventaja del host**: el
    valor guarda el `Func`/`FnExpr` directamente (referencia + GC) → sin el esquema de índices+tabla de
    anónimas de Rust. **Las CELDAS**: ámbitos `Map<string,Cell>` (`struct Cell { v }`); `define` crea
    celda nueva (shadowing), `assign` MUTA la celda (closures ven el cambio), `lookup` lee `cell.v`. Una
    `Cell` del host = el `Rc<RefCell<Value>>` de Rust gratis (semántica de referencia). `capture_env`
    snapshotea las celdas visibles; `call_body` liga capturadas (base) + params (encima). Llamada
    indirecta (`call_value`), `?` (`eval_try`), enums del prelude Option/Result inyectados en `c.enums`
    (solo nombres de variantes; el intérprete no consulta tipos). Oráculo: closures/errores/opcional +
    snippets (estado por celda independiente, `?` encadenado) = mismo stdout+exit que Rust. Diferido: d
    (UFCS/métodos/dyn/@derive + map/filter/fold del prelude).
  - **M14.4d-1 COMPLETO** (347 lib + 16 en `tests/selfhost_interpreter.rs`): **intérprete auto-alojado —
    despacho dinámico**. Aquí la **resolución en runtime** luce al máximo. `Interp` gana `methods:
    Map<string, Method>` (`Tipo#metodo`), poblado por `register_methods` desde los `impl` + métodos por
    DEFECTO del trait. `dispatch_method(recv, fname, args)` resuelve: (a) campo-función del struct →
    (b) método (clave `type_key_of_value(recv)#fname`) → (c) `@derive` (igual ≡ values_equal, mostrar ≡
    value_str; el checker garantiza Eq/Show → solo queda el caso derivado, sin leer la anotación) →
    (d) UFCS a función libre `fname(recv,args)` → (e) builtin como método (`xs.len()`). **Elegante**:
    **bounds/genéricos = no-ops** (despacha por el tipo concreto, sin diccionarios) y **`dyn` trivial**
    (el "objeto" ES el valor concreto, sin vtable; `[dyn T]` = arreglo de concretos). Impls genéricos
    por **constructor** (`Caja<T>`→"Caja"); anidamiento por despacho recursivo. Oráculo: ufcs/traits/
    bounds/metodos_por_defecto/impls_genericos/trait_objects/anotaciones + snippets = mismo stdout+exit
    que Rust. Diferido: d-2 (map/filter/fold del prelude → stdlib.ray, cierra el self-hosting).
  - **M14.4d-2 COMPLETO → M14.4d → M14.4 → SELF-HOSTING CERRADO** (347 lib + 18 en
    `tests/selfhost_interpreter.rs`): **map/filter/fold del prelude**. El checker auto-alojado es
    validador (no inyecta el prelude en el programa) y el intérprete necesita los CUERPOS → se replica el
    `check()` de Rust: `selfhost/prelude.ray` (map/filter/fold escritos EN raylang) que `selfhost/run.ray`
    parsea y **fusiona** en el programa del usuario (`add_prelude`: solo las que no redefina → override;
    sin desplazar posiciones —el validador no baja por posición, el intérprete despacha por etiqueta).
    Fusionadas, map/filter/fold son funciones ordinarias: UFCS cae en la rama UFCS de `dispatch_method`,
    pipelines los desazucara el parser. **Verificado: los 22 ejemplos corren idénticos por ambos
    pipelines** (Rust `cargo run` vs `raylang selfhost/run.ray`). **raylang lexea/parsea/chequea/EJECUTA
    raylang de punta a punta — self-hosting CERRADO.** Diferido (fuera del corpus): builtins string/IO/Map
    en el intérprete, TCO/`MAX_CALL_DEPTH`, resto del prelude (assert/sort). Siguiente posible: **M14.5**
    (VM auto-alojada, opcional).
  - **M14.6 — diferidos hacia la META-CIRCULARIDAD** (que el intérprete auto-alojado ejecute el PROPIO
    compilador auto-alojado). Cada grupo = fila en el checker + impl en el intérprete (como M11.4).
    - **M14.6a COMPLETO** (347 lib + 19 en `tests/selfhost_interpreter.rs`; checker oráculo 24 intacto):
      **builtins de string** (checker + intérprete). El **checker auto-alojado** acepta `to_string`/`trim`/
      `split`/`chars`/`contains`/`replace`/`starts_with`/`ends_with`/`to_upper`/`to_lower`/`substring`/
      `repeat`/`join` (reglas/mensajes byte-idénticos a `src/builtins.rs`; helpers `b_arity`/`want_string`/
      `want_int` + `check_*` por builtin). El **intérprete** los implementa delegando en los del host.
      **Gotcha (cazado por el host checker)**: un `match` con TODAS las ramas divergentes (`return`+`panic`,
      al reescribir `push`) no tipa ("hay al menos un brazo") → `return match {...}` con el brazo normal
      cediendo el valor. Pendiente: `Map`, `panic` en el checker, `parse_int`/`parse_float` → luego correr
      `selfhost/lex_dump.ray` sobre el intérprete.
    - **M14.6b COMPLETO** (347 lib + 20 en `tests/selfhost_interpreter.rs`; checker oráculo 24 intacto):
      **`Map<K,V>`** (checker + intérprete; el diferido más invasivo). **Checker**: `Map<K,V>` llega como
      `TNamed("Map",[K,V])` (sin variante `TMap`, se trata por nombre); `ensure_type` valida aridad 2 +
      clave hashable (`is_hashable_key`), `len` lo acepta, builtins `map_new`/`insert`/`get`/`remove`/
      `contains_key`/`keys`/`values` (mensajes byte-idénticos; `get`/`remove` → `Option<V>`). **`map_new()`
      indeterminado** (como `[]`/`None`): lo fija el esperado vía bidireccional (`check_map_new(expected)`,
      interceptado en `check_call`). **Intérprete**: `VMap(MapData{keys,vals})` —arrays PARALELOS + búsqueda
      lineal por `values_equal`, NO un `Map` del host (claves serían `Value`/enum, no hasheable); `MapData`
      es struct → mutación compartida como `VArray`—. `keys()`/`values()` ORDENADAS por clave (`key_lt` +
      insertion sort) → deterministas como Rust. Oráculo: claves string/int + remove. (`examples/data/mapa.ray`
      espera M14.6c por `assert_eq`/`assert`.) Pendiente: `panic` en el checker, `parse_int`/`parse_float`,
      `assert`/`sort` → luego correr el compilador auto-alojado.
    - **M14.6c-1 COMPLETO** (347 lib + 21 en `tests/selfhost_interpreter.rs` + 25 en `tests/selfhost_checker.rs`):
      **`panic` + `parse_int`/`parse_float`** (lo que el lexer usa al tokenizar números y abortar).
      **Checker**: builtin `panic(string) -> unit` (`check_panic`; `expr_diverges` ya lo reconocía por
      nombre, ahora también lo TIPA) + primitivos `__parse_int`/`__parse_float` (`check_parse_prim`,
      `(string) -> [int]`/`[float]`) en `is_known_builtin`/`check_named_call`; firmas de los envoltorios
      `parse_int`/`parse_float` (`-> Option<…>`) en `inject_prelude_fns`. **Intérprete**: `__parse_int`/
      `__parse_float` delegan en el host (`Option`→`[T]` de 0/1); `panic` ya estaba (M14.4a). **Cuerpos**
      `parse_int`/`parse_float` en `selfhost/prelude.ray` (fusionados por el driver). **El lexer entero NO
      lo bloquea la stdlib**, sino dos diferidos mayores: **carga de módulos** (el pipeline auto-alojado
      procesa un solo archivo) y los **builtins de I/O** (`args`/`read_file`) de `lex_dump.ray`. Pendiente
      de M14.6c: `assert`/`assert_eq`/`sort`.
    - **M14.6c-2 COMPLETO → M14.6c COMPLETO** (347 lib + 22 en `tests/selfhost_interpreter.rs` + 25 en
      `tests/selfhost_checker.rs`): **`assert`/`assert_eq`/`sort`** (prelude de aserciones + orden, sobre
      `panic` + Eq/Show/Ord). **Checker**: firmas en `inject_prelude_fns` (`assert(bool)`, `assert_eq<T:
      Eq+Show>(T,T)`, `sort<T: Ord>([T])->[T]`; los bounds resuelven contra los traits+impls de primitivos
      de M14.3d-4c). **Cuerpos** en `selfhost/prelude.ray` (fusionados por el driver). **Intérprete**: `sort`
      usa `.less()` (Ord) pero el validador omitió el lowering de diccionarios → se resuelve **`.less()`
      sobre primitivos por fallback** en `dispatch_method` (junto a `eq`/`show`; helper `value_lt` para
      int/float/string/char); un tipo de usuario con `impl Ord` se resuelve antes por la tabla `Tipo#menor`,
      así que el fallback solo ve los 4 primitivos (lo que garantiza `T: Ord`). Oráculo: intérprete (sort
      int/string/float, tipo de usuario con `impl Ord`, assert/assert_eq ok y assert_eq que falla → exit 70,
      + **`examples/data/mapa.ray`** antes diferido); checker (válidos + error de bound `sort` sin Ord). El
      compilador entero sobre el intérprete sigue bloqueado por **carga de módulos** + **builtins de I/O**.
    - **M14.6d COMPLETO** (347 lib + 23 en `tests/selfhost_interpreter.rs` + 25 en `tests/selfhost_checker.rs`):
      **I/O de archivos** (`read_file`/`write_file`/`exists`). **Checker**: primitivos `__read_file`/
      `__write_file` (`-> [string]` etiquetado) + builtin `exists -> bool` en `is_known_builtin`/
      `check_named_call` (mensajes byte-idénticos); firmas de los envoltorios `read_file -> Result<string,
      string>` / `write_file -> Result<int,string>` en `inject_prelude_fns`. **Intérprete**: `__read_file`/
      `__write_file`/`exists` delegan directo en los primitivos del host; **cuerpos** `read_file`/`write_file`
      (traducen el arreglo etiquetado a Result) en `selfhost/prelude.ray`. **Oráculo determinista**: ambos
      pipelines escriben el MISMO contenido a un temporal y lo releen → mismo stdout. **Diferidos a
      propósito**: `args()` (diverge: el self-hosted ve el path de `run.ray` como `argv[0]`), stdin/env (no
      deterministas), handles/remove_file/list_dir (no los usa el compilador). Bloqueo restante para la
      meta-circularidad: **solo carga de módulos**.
  - **M14.7 — el loader auto-alojado** (carga de módulos; último bloqueo de la meta-circularidad).
    `selfhost/loader.ray` (cliente host-side como `run.ray`) **aplana** la entrada + sus `from`-imports
    transitivos en un `Program` plano. Port recortado de `src/loader.rs`. **Simplificaciones**: solo `from M
    import …` (sin `import M;` calificado/directorios/cápsulas/reexports), y **sin position-shifting** (el
    checker auto-alojado no baja por posición, el intérprete despacha por etiqueta → posiciones irrelevantes
    al comportamiento de programas válidos).
    - **M14.7a COMPLETO** (347 lib + 24 en `tests/selfhost_interpreter.rs`): máquina de carga + cruce de
      **funciones**. `load(entry) -> Result<Program, LoadError>`: **BFS** de `from`-imports (`read_file` +
      lex + parse, ciclos seguros con `visited`; ruta = `dir(entry)/dep.ray`); por módulo: `build_surfaces`
      (función `pub` → `modulo::fn`), `clasificar_from_values` (valida `pub`), el **`Resolver`** (reescribe
      `EIdent` → global, consciente de ámbitos: local/param/binding tapa a la función top-level), y
      **renombrar defs + fusionar**. `run.ray` usa `load(argv[0])` (y para el prelude); archivo único = sin
      from-imports → loader **identidad** (cero regresión). Tipos fusionados SIN namespacar aún (→ M14.7b).
      Mutación in-place del AST apoyada en la **semántica de referencia del host**. Oráculo: 2 archivos,
      alias, shadowing, cadena A→B→C, función-como-valor. Pendiente: M14.7b (tipos: TypeRewriter +
      namespacing + `from M import Tipo` → desbloquea módulos reales), M14.7c (correr el compilador +
      `args()` consistente).
    - **M14.7b COMPLETO** (347 lib + 25 en `tests/selfhost_interpreter.rs`): cruce de **tipos** (desbloquea
      los módulos reales, que cruzan `Type`/`Expr`/…). (1) `Surface` gana `types` (`build_surfaces` la puebla
      con struct/enum/trait `pub`); (2) `clasificar_from` reparte cada `from M import X` en valor (función)
      o tipo, validando `pub`; (3) el **`TypeRewriter`**: `rename_type_defs` renombra defs propias a
      `modulo::Tipo`, y `tw_program` reescribe TODAS las referencias —posiciones de tipo (anotaciones/campos/
      payloads/target+trait de impl/bounds/dyn/args genéricos) y expresiones que nombran tipos (struct lit,
      construcción de enum `Tipo.Variante` como `Field`/`Call`, patrones)— consciente de los `tparams` en
      ámbito (`T` no se reescribe). El parser emite `TNamed` para todo (incl. `Map`/`T`); `tw_name` deja los
      no encontrados igual → cubre ambos sin caso especial. Sin `import M;` calificado → el caso `M.Tipo` no
      hace falta. Oráculo: cruce de struct+enum (construcción+match), trait+impl+genérico+dyn, alias de tipo.
      Pendiente: M14.7c (correr el compilador de punta a punta + `args()` consistente).
    - **M14.7c COMPLETO → M14.7 COMPLETO → META-CIRCULARIDAD LOGRADA** (347 lib + 25 intérprete + 25 checker
      + `tests/selfhost_metacircular.rs`). El compilador auto-alojado entero corre **sobre el intérprete
      auto-alojado**. (1) **`args()` consistente**: `run.ray` consume `argv[0]` (el path del driver) y
      enhebra `argv[1..]` al intérprete (`run(prog, args)`; `Interp.args`; builtin `args()`), así un driver
      ve sus propios args como bajo Rust; archivo único → `args()==[]` en ambos. (2) `args()` en el checker
      (nulario→`[string]`). (3) `pop` (último builtin que faltaba, lo usa `checker.ray`): primitivo `__pop`
      en checker+intérprete + envoltorio `pop<T>` en el prelude. (4) **concatenación de arreglos** `a + b`
      en `eval_add` (la usa `run.ray`; el checker ya la aceptaba). **Verificado** (oráculo conductual,
      driver por Rust vs sobre el intérprete auto-alojado): `lex_dump`, `parse_dump`, `check_dump` y
      **run-on-run** (`run.ray` corriendo `run.ray` → back-end incluido) dan stdout+exit idénticos. raylang
      lexea/parsea/chequea/EJECUTA raylang con raylang corriendo sobre raylang. (run-on-run `#[ignore]`,
      ~1 min; `cargo test --test selfhost_metacircular -- --ignored`.) Diferidos: VM auto-alojada (M14.5),
      `import M;`/directorios/cápsulas en el loader, resto de I/O (stdin/env/handles).
  - **M14.5 — la VM auto-alojada** (DESIGN §23.6; el M2 de este módulo, en paralelo al intérprete de
    M14.4). **Decisión central**: la VM **reusa el `Value` y el runtime del intérprete** (`value_str`/
    `values_equal`/`eval_add`/`eval_arith`/`eval_cmp`/`dispatch_builtin`, hechos `pub`) — UN solo tipo de
    valor y sin GC propio (a diferencia de Rust, que tiene dos), ambos cabalgan sobre el GC del host. Y el
    bytecode es **compacto**: opcode genérico `OBuiltin(nombre, argc)` que delega en `dispatch_builtin`
    (refactorizado para tomar `prog_args` en vez de `Interp`), en vez de un opcode por builtin.
    `selfhost/compiler.ray` (AST → `CProgram` de `CompiledFn`+`Chunk`; resolución de slots/ámbitos como
    `src/compiler.rs`) + `selfhost/vm.ray` (pila de operandos + pila de marcos explícitas; cada marco con
    su propia pila → sin *base pointer*; `Flow` como canal de error) + driver `selfhost/run_vm.ray`.
    - **M14.5a COMPLETO** (347 lib + 5 en `tests/selfhost_vm.rs`): el **núcleo** — escalares, aritmética/
      comparación/lógica (con cortocircuito), variables locales (slots + ámbitos), if/while, llamadas
      nombradas + builtins escalares, recursión. Oráculo conductual (VM auto-alojada vs Rust): corpus
      fib/fizzbuzz/gcd/primes + snippets. El **prelude NO se fusiona aún** (map/filter/fold/sort usan
      llamadas indirectas/métodos → M14.5c). Pendiente: M14.5b (datos), M14.5c (closures/primera clase +
      prelude), M14.5d (despacho dinámico), TCO.
    - **M14.5b COMPLETO** (347 lib + 9 en `tests/selfhost_vm.rs`): **datos** — arreglos (literal/índice/
      asignación), structs (literal/campo/asignación de campo) y enums (construcción `Enum.Variante[(args)]`
      + `match`). Filosofía clave (como el intérprete auto-alojado, M14.4b, y a diferencia de la VM de Rust
      que usa *tags*/*ids* en tablas): **resolución por NOMBRE en runtime** — los opcodes llevan los nombres
      (`OMakeStruct(nombre, campos)`, `OMakeEnum(enum, variante, aridad)`, `OEnumTagEq(variante)`,
      `OGetEnumField(i)`, `OIndex`/`OSetIndex`/`OGetField`/`OSetField`/`OMakeArray`/`OMatchFail`) y la VM
      construye/compara los `Value` por nombre. El compilador gana tablas `structs`/`enums` (de `prog` +
      Option/Result del prelude, vía `inject_prelude_enums`) para reconocer la construcción de enum en
      compilación (lo que el intérprete decide en runtime); `emit_match` es port de Rust pero comparando la
      variante por nombre (sin tags); el escrutinio va a un local temporal `$match`. La VM **reusa el
      runtime del intérprete** otra vez (`as_int`/`struct_get`/`struct_set`/`SField`, ahora `pub`): la
      **semántica de referencia** de arreglos/structs es la del `[Value]`/`[SField]` del host (mutación
      compartida gratis). Oráculo conductual = mismo corpus de datos del intérprete (structs/enums/
      match_figuras/arrays/matriz + snippets de aliasing/payload/recursivos/OOB) → stdout+exit idénticos.
      Pendiente: M14.5c (closures/primera clase/`?` + fusión del prelude), M14.5d (despacho dinámico), TCO.
    - **M14.5c COMPLETO** (347 lib + 12 en `tests/selfhost_vm.rs`): **primera clase** — funciones como valor,
      funciones anónimas + **closures** (captura por upvalues), llamada indirecta (`OCallValue`) y el operador
      `?` (`OTry`). Esquema de upvalues **estilo clox** (resolución transitiva en el compilador:
      `resolve_upvalue`/`add_upvalue`, `UpSrc.ULocal`/`UUp`), PERO sin el análisis de *boxing* de Rust
      (`captured_slots`): **toda local es una `Cell`** en la VM (como el intérprete auto-alojado), así la celda
      siempre existe y el upvalue solo la referencia. El compilador pasó de `Cc` (una función) a `Comp` con una
      **pila de `Fscope`** (las envolventes quedan debajo para resolver upvalues); las anónimas se compilan en
      línea y se **anexan** a `comp.out` (funciones nombradas con índices reservados 0..n; las anónimas después).
      Única extensión del `Value` compartido: **`VVmClosure(int, [Cell])`** (índice de la fn compilada + celdas
      capturadas) — la representación de un cierre difiere genuinamente entre AST (intérprete: `FnExpr`+capturas
      por nombre) y bytecode (VM); en Rust también difieren los dos motores. La VM: `OGetLocal`/`OSetLocal`
      leen/mutan `Cell.v`, `OInitLocal` estrena celda (shadowing/bucle); `OClosure` captura las celdas **por
      referencia** (compartidas → la mutación se ve); `OTry` desempaqueta Ok/Some o retorna el valor entero como
      `OReturn`. El **prelude no se fusiona aún** (map/filter/fold no usan métodos, pero sort/assert_eq sí → el
      prelude completo compila en M14.5d). Oráculo = corpus de primera clase del intérprete (closures/errores/
      opcional + snippets de HOF/captura/estado/transitiva/`?` con Result y Option) → stdout+exit idénticos.
      Pendiente: M14.5d (despacho dinámico: métodos/UFCS/dyn/@derive + fusión del prelude), TCO.
    - **M14.5d COMPLETO → M14.5 COMPLETO** (347 lib + 15 en `tests/selfhost_vm.rs`): **despacho dinámico**
      (métodos de trait, UFCS, `dyn`, `@derive`, bounds) + **fusión del prelude completo**. Como el intérprete
      auto-alojado (M14.4d): **resolución por la ETIQUETA del valor en runtime** → `dyn`/bounds/genéricos son
      **no-ops** (se despacha por el tipo concreto, sin diccionarios ni vtable; el "objeto" ES el valor). El
      compilador baja `recv.f(args)` (que no sea construcción de enum) a un único opcode `ODispatch(fname,
      argc)`; `compile_methods` compila los métodos de los `impl` (+ defectos del trait) como funciones con
      `self` y puebla `CProgram.methods` (`Tipo#metodo → índice`, por constructor: `Caja<T>`→"Caja"); `CProgram`
      también lleva `indices` (función libre → índice, para UFCS). La VM resuelve en `resolve_dispatch` (espejo
      de `dispatch_method`): (a) campo-función del struct → (b) método (tabla) → (c) `@derive`
      igual/mostrar/`less` de Ord sobre primitivos (vía `values_equal`/`value_str`/`value_lt`, reusados del
      intérprete) → (d) UFCS a función libre → (e) builtin como método; devuelve un `Dispatch` (apilar marco o
      empujar valor) para no usar `return` dentro del bucle. **Prelude completo fusionado** en `run_vm.ray` (como
      `run.ray`): map/filter/fold (indirectas) + sort/assert_eq (métodos por `ODispatch`) ya compilan. Gotcha
      (M14.6a): un `match` con TODAS las ramas divergentes no tipa → el match interno cede valor en el brazo
      normal. Oráculo conductual = corpus de despacho del intérprete (ufcs/traits/bounds/metodos_por_defecto/
      impls_genericos/trait_objects/anotaciones) + prelude (stdlib/mapa) + snippets (método/UFCS/@derive/sort);
      **los 33 ejemplos deterministas corren idénticos por la VM auto-alojada y por Rust**. **La VM auto-alojada
      ejecuta el LENGUAJE COMPLETO** (núcleo+datos+primera clase+despacho dinámico+prelude). Pendiente: TCO
      (opcional), VM meta-circular (correr el compilador sobre la VM).
    - **M14.5e COMPLETO** (347 lib + 15 en `tests/selfhost_vm.rs` + 1 `#[ignore]`): **TCO** (recursión de cola
      en O(1) marcos) en la VM auto-alojada. Port de M13.3b: un **peephole** `optimize_tail_calls` (en
      `compile_body`) reescribe toda llamada (`OCall`/`OCallValue`/`ODispatch`) cuya continuación sea un
      `OReturn` —directo o vía saltos incondicionales (`returns_immediately`)— a su variante `OTailCall`/
      `OTailCallValue`/`OTailDispatch`, que **reutilizan el marco** (`frames[top] = new_frame(...)`) en vez de
      apilar uno → la recursión de cola corre en O(1) marcos. Cubre también `ODispatch` (que Rust no tiene; sus
      métodos son `Call`): `OTailDispatch` reutiliza el marco si resuelve a una función, o empuja el valor si es
      directo (@derive/builtin). Gotcha: `Option.None` del peephole necesitó anotar `let nuevo: Option<Op>`
      (la inferencia no cruza al `else`). Verificado: 1M de recursión de cola directa y mutua corre por la VM
      auto-alojada idéntico a Rust (oráculo `recursion_de_cola`, `#[ignore]` por lento ~7 min doble-interpretado:
      `cargo test --test selfhost_vm -- --ignored`); los 33 ejemplos deterministas siguen idénticos con el
      peephole activo. **M14.5 (la VM auto-alojada) COMPLETA con TCO.** Pendiente: VM meta-circular (verificar
      `run_vm.ray` sobre `run_vm.ray`).
    - **M14.5f COMPLETO → SELF-HOSTING POR LA VM CERRADO** (3 en `tests/selfhost_metacircular_vm.rs` + 1
      `#[ignore]`): **VM meta-circular**. Gemelo de `selfhost_metacircular.rs` (intérprete, M14.7c) para el
      SEGUNDO back-end: los drivers del self-hosting (`lex_dump`/`parse_dump`/`check_dump`) **compilados y
      corridos sobre la VM auto-alojada** (`raylang run_vm.ray <driver> <input>`) dan stdout+exit idénticos a
      Rust → raylang lexea/parsea/chequea raylang con el compilador+VM de raylang corriendo sobre la VM de
      raylang. **run-on-run de la VM** (`run_vm.ray` compilando y corriendo `run_vm.ray`, TRES niveles: Rust →
      VM → VM → ejecuta) verificado idéntico a Rust (`#[ignore]` por lento; `cargo test --test
      selfhost_metacircular_vm -- --ignored`): **la VM auto-alojada se ejecuta a sí misma**. Cero código nuevo
      (solo el test) — la VM ya soportaba el lenguaje completo (M14.5a–e) + builtins (Map/I/O/args vía
      `dispatch_builtin`). **Self-hosting CERRADO por AMBOS back-ends** (intérprete M14.7 + VM M14.5f).
- **M12 — concurrencia COMPLETA** (DESIGN §21; CSP sobre la VM, fijado con el usuario). La última gran
  problemática de diseño del proyecto, hecha al final (tras M13 + self-hosting). Modelo: green threads
  cooperativos **M:1** + canales tipados, data-race freedom **vía CSP** (no ownership), scheduler
  **determinista**, intérprete = oráculo secuencial. Cinco sub-fases (12.1 slice · 12.2 backpressure ·
  12.3 structured · 12.4 select · 12.5 cancelación), todas COMPLETAS; libro en `book/src/m12/`.
  - **M12.1 COMPLETO** (347 lib + 8 en `tests/concurrency_cli.rs`): **el slice CSP** — `spawn(closure)` +
    canales (`channel`/`send`/`recv`/`close`) + scheduler cooperativo determinista, **solo en la VM**. Surface
    (decidida con el usuario): `spawn(f: fn()->T)` (resultado descartado), `Channel<T>` (tipo nuevo,
    reclasificado como `Map`; `channel()` indeterminado → lo fija el esperado), `send(ch,v)` (no acotado →
    nunca bloquea), `recv(ch) -> Option<T>` (bloquea si vacío+abierto; `None` si cerrado+vacío; primitivo
    `__recv -> [T]` + envoltorio en el prelude, patrón M11.2), `close(ch)` (**`close` ad-hoc polimórfico**:
    handle de archivo→int, canal→unit, reusa `OpCode::Close`). UFCS gratis. **VM**: una **fibra** = `(frames,
    stack)`; `ready: VecDeque<Fiber>` FIFO + `parked` (fibra+canal que espera); único punto de yield = `recv`
    bloqueante; `spawn` solo encola; `send` entrega directo a un receptor bloqueado o encola. Fin cuando
    **main** retorna (semántica Go); **deadlock** si todas las fibras quedan bloqueadas. **GC multi-raíz**:
    `collect` rootea TODAS las fibras (ejecución + ready + parked) + el canal que cada parked espera; canal =
    `Obj::Channel(VmChannel { queue, closed })` trazado por el GC. **Intérprete**: error limpio ("requiere la
    VM") en spawn/channel/send/recv → sigue siendo oráculo secuencial; los programas concurrentes corren con
    `--vm` y, por el scheduler determinista, se testean contra **salida esperada exacta** (no hay oráculo
    cruzado). Ejemplo `examples/concurrency/concurrencia.ray` (pipeline de fibras). Diferido: M12.2 canales acotados
    (backpressure), M12.3 structured concurrency (scope+join), M12.4 `select`.
  - **M12.2 COMPLETO** (347 lib + 12 en `tests/concurrency_cli.rs`): **canales acotados / backpressure**
    (DESIGN §21.3). `channel(n)` crea un canal acotado a la capacidad `n` (`int` ≥ 0; `n=0` = **rendezvous**
    síncrono); `channel()` sigue no acotado. El tipo de elemento sigue **indeterminado** (la capacidad es un
    valor de runtime, no entra en `Channel<T>`); el checker acepta 0/1 args (la capacidad ha de ser `int`).
    **`send` se vuelve el segundo punto de yield**: (1) si hay receptor bloqueado → entrega directa; (2) si
    hay hueco (no acotado o `len<cap`) → encola; (3) cola llena → **bloquea al emisor** (`Waiting::Send(v)`,
    aparcado con su valor pendiente → backpressure). `recv`, al liberar un hueco, **despierta a un emisor
    bloqueado** (su valor entra a la cola); con `cap=0` toma su valor directo. `Parked` pasa a llevar
    `Waiting::Recv`/`Send(v)`; el valor de un emisor aparcado es **raíz del GC** nueva. Dos opcodes
    (`ChannelNew` no acotado / `ChannelNewBounded` saca la capacidad); el compilador elige por aridad
    (special-case de `channel`, como en el checker). **`close` con un emisor bloqueado = error de ejecución**
    en el sitio del `close` (determinista, a diferencia de panic-en-otra-fibra). Determinismo FIFO intacto;
    deadlock cubre también emisores bloqueados. Diferido: M12.3 structured concurrency, M12.4 `select`.
  - **M12.3 COMPLETO** (347 lib + 17 en `tests/concurrency_cli.rs`): **structured concurrency** (DESIGN
    §21.4). El modelo estructurado (Trio/Kotlin) sobre el slice CSP: tareas con **valor de retorno**, un
    `scope` que **posee** las que se lanzan dentro y **las une** al salir, y **propagación** del fallo de
    una hija. **Surface** (builtins + closures, cero gramática): `Task<T>` (tipo nuevo, como `Channel<T>`);
    `spawn(f: fn()->T)` **cambia su firma** → devuelve `Task<T>` (retrocompat: como sentencia se descarta);
    `join(t: Task<T>) -> T` bloquea hasta que la tarea termina (re-lanza si falló); `scope(body: fn()->R)
    -> R` corre el cuerpo y al volver une todas sus tareas y propaga el primer fallo. **`join` ad-hoc
    polimórfico** (como `close`): colisionaba con el `join(arr,sep)` de strings (M11.7a) y raylang no tiene
    sobrecarga → un builtin que ramifica por tipo + el compilador elige el opcode por **aridad** (1=`TaskJoin`,
    2=`Join` string). **Runtime (solo VM)**: `Obj::Task(VmTask{state: Pending|Done(v)|Failed(msg)})`; cada
    `Fiber` gana `task`/`scopes` (la VM espeja `current_task`/`scopes`, salva/restaura al conmutar); opcodes
    `Spawn` (crea Task + adscribe al scope), `TaskJoin`, `ScopeBegin`/`ScopeEnd` (el compilador baja
    `scope(body)` a `ScopeBegin; body(); ScopeEnd`, special-case como `channel`; la llamada al cuerpo NO es
    cola → el TCO no la toca). `join`/`ScopeEnd` que bloquean **rebobinan el ip** y re-ejecutan al despertar
    (TaskJoin re-empuja el handle que sacó). **Propagación**: el bucle de la VM corre cada instrucción en un
    **cierre** y **captura** el error de una fibra hija (frames activos, no `main`) en su `Task` como
    `Failed`, planificando la siguiente en vez de abortar; los de `main` y del scheduler (frames vacíos =
    deadlock) abortan; un `Failed` se re-lanza en el `join`/`ScopeEnd` que lo observe → encadena hacia
    arriba. **GC multi-raíz**: `Done(v)`, el handle de tarea de cada joiner aparcado, y los hijos de cada
    `ScopeFrame` (en curso/listas/aparcadas). Ejemplo `examples/concurrency/structured.ray`. Diferido: **cancelación**
    de hermanas cuando una falla (sin primitivo de cancelación; el cuerpo del scope que hace panic deja
    huérfanas), M12.4 `select`.
  - **M12.4 COMPLETO** (347 lib + 21 en `tests/concurrency_cli.rs`): **`select` sobre varios canales**
    (DESIGN §21.5). **`select(chs: [Channel<T>]) -> int`** bloquea hasta que algún canal de la lista esté
    **listo para recibir** (cola no vacía ∨ cerrado ∨ con emisor bloqueado) y devuelve el **índice** del
    primero listo (determinista: menor índice); luego `recv(chs[i])` toma el valor (o `None` si se cerró).
    Mínimo, sin tipos ni tuplas nuevas; seguro porque entre `select` y `recv` no hay yield (M:1). UFCS
    gratis. **Runtime (solo VM, cero objetos nuevos)**: opcode `Select` (es un builtin ordinario, no
    necesita special-case del compilador); `Waiting` gana `Select` (el `Parked.on` del selector es el
    **handle del arreglo** de canales → el GC lo rootea y con él los canales). Al bloquear, rebobina el `ip`
    y re-ejecuta al despertar (re-empuja el arreglo, como `TaskJoin`). Se le despierta cuando un canal suyo
    pasa a estar listo: `wake_select_waiters(chan)` (recorre los `Select` cuyo arreglo contiene `chan`) se
    llama al **encolar** (`send`), al **bloquearse un emisor** y al **cerrar**. Despertar espurio →
    re-ejecuta y se re-bloquea. **Prioridad**: un `send` entrega antes a un `recv` plano que a un `select`
    (que solo ve el valor vía la cola). Gotcha (documentado): un canal **cerrado** queda listo para siempre
    → si haces `select` sobre una lista que lo incluye, lo elegiría siempre; hay que quitarlo de la lista (el
    "poner a nil" de Go). Ejemplo `examples/concurrency/select.ray`. Diferido: `Selected<T>` (índice+valor), `select`
    de operaciones de send.
  - **M12.5 COMPLETO** (347 lib + 23 en `tests/concurrency_cli.rs`): **cancelación de hermanas** (DESIGN
    §21.6). Cierra el diferido de M12.3: cuando una tarea de un `scope` falla, se **cancelan** las hermanas
    pendientes y se propaga el fallo **original**, en vez de esperarlas (o dejarlas huérfanas). **Sin
    superficie nueva** (semántica automática, como Trio); solo runtime de la VM. Cancelar en M:1 es trivial:
    una fibra solo corre en los yields → `cancel_task(t)` la marca `Failed`, la **saca** de `ready`/`parked`
    (el GC reclama sus marcos) y cancela **recursivamente** los hijos de sus scopes (transitiva: sin nietos
    huérfanos). Se dispara en (1) `ScopeEnd`: escanea los hijos **antes** de bloquearse; si alguno falló,
    cancela las hermanas pendientes y propaga el fallo original de inmediato (antes esperaba a todas); (2)
    `fail_current_fiber`: una fibra hija que hace panic con tareas en vuelo cancela los hijos de sus scopes
    (cierra "cuerpo del scope falla → huérfanas" para fibras no-main). Reusa `TaskState::Failed` (cero
    opcodes/tipos nuevos). Cooperativa, **no preemptiva** (no interrumpe código que corre sin ceder ni el
    cuerpo a mitad). Tests: cancelación de hermana bloqueada + cuerpo de fibra hija que cancela sus
    subtareas. **M12 COMPLETO** (12.1–12.5). Diferido: cancelación preemptiva, `Selected<T>`, select de send,
    `cancel(t)` explícito.
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
  - **Multi-módulo (L3)**: estas tablas por posición operan sobre el programa **fusionado**; dos
    módulos en la misma `(línea, col)` colisionarían. El **loader** lo evita dando a cada módulo una
    **banda de líneas disjunta** (`shift_program`), así las posiciones son globalmente únicas.
- **Métodos de trait (M9.1)** reusan ese mismo mecanismo: `recv.m(args)` resuelve en
  `check_call` con prioridad **campo → método de trait → función libre**, y comparte el
  lowering. Por eso `ufcs_sites` es un **mapa** sitio→**nombre destino**: para UFCS de
  función libre el destino es el mismo nombre; para un método de trait, el **manglado**
  `Tipo#metodo` (que el usuario no puede escribir). Los métodos se inyectan como funciones
  ordinarias en `program.functions`, así que el intérprete/VM no saben de traits (erasure).
- **Bounds (M9.2)**: el checker trabaja con **firmas limpias** (solo params de usuario) —
  `x.metodo()` con `x: T` acotado se verifica contra la firma del trait, no contra ningún
  param—. Toda la plomería de diccionarios es **lowering post-check**: `append_dict_params`
  añade los params ocultos `T#Trait#metodo` a las funciones con bounds, y `lower_dict_calls`
  añade los argumentos en los sitios. Por eso los params-diccionario NO están en `FnSig`
  (si lo estuvieran, una llamada `f(x)` del usuario fallaría por aridad). El runtime los ve
  como funciones más (valores de primera clase, M4).
- Un identificador en posición de **tipo** llega del parser como `Type::Struct`; el
  checker lo **normaliza** (`resolve_type`) a `Type::Enum` si es un enum, o a
  `Type::Var` si es un **parámetro de tipo** en ámbito (M6). `self.type_params` se pone
  en ámbito al registrar/verificar cada función.
- **Genéricos = solo checker** (erasure): el intérprete y la VM no saben de `T`. La
  inferencia es `unify(param_de_la_firma, tipo_del_argumento, σ)` —asimétrica: los
  `Var` de la firma son incógnitas; los del llamador son rígidos— y `subst(retorno, σ)`.
- La VM tiene su **propio valor** (`gc::HeapValue`, con handles), distinto del
  `Value` del intérprete (con `Rc`). Se convierte en el borde (`to_value`).
