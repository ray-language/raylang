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
| `src/prelude.rs` | front-end | stdlib en raylang: Option/Result (M6.3) + map/filter/fold (M7.3) + I/O (parse_int/input/read_int/env, M11.2), inyectados en `check` |
| `src/builtins.rs` | front-end | **registro único** de builtins (L1): nombre + opcode + regla de tipado. Lo consultan checker/compilador/intérprete (la *impl* de ejecución sigue en `eval_builtin`/VM) |
| `src/token.rs`, `src/lexer.rs` | léxico | texto → tokens; cada token con (línea, col) |
| `src/ast.rs`, `src/parser.rs` | sintaxis | descenso recursivo; precedencia por jerarquía de reglas |
| `src/checker.rs` | semántica | tipos; dos pasadas (firmas + cuerpos), pila de ámbitos, análisis de divergencia |
| `src/interpreter.rs` | ejecución | tree-walking; valores en runtime; `return` como señal de flujo |
| `src/diagnostic.rs` | presentación | M8.3: `render` añade la línea de fuente y un `^` bajo la posición. Solo presentación; no toca las fases |
| `src/repl.rs` | cliente externo | REPL (M8.2): acumula y re-ejecuta `fn main` vía la API pública; muestra el valor con `print`. No toca el core |
| `src/test_runner.rs` | cliente externo | runner `@test` (M10.1): sintetiza un `main` que corre las pruebas; código de salida = nº de fallos. No toca el core |
| `src/lsp.rs` | cliente externo | LSP (M10.2): `raylang --lsp`. JSON-RPC a mano (`mod json` + framing) + diagnósticos; `analizar` reusa lex/parse/check. No toca el core |
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
- **M10.1 COMPLETO** (272 tests + integración CLI verdes): **anotaciones**. `@nombre[(args)]`
  sobre fn/struct/enum; `Annotation { name, args }` en cada ítem; conjunto **cerrado**
  conocido por el compilador (`check_annotations`). **`@test`** (función `() -> bool`) +
  runner `--test` (cliente externo `src/test_runner.rs` que sintetiza un `main` y devuelve
  el número de fallos como código de salida). **`@derive(Eq)`** sobre struct/enum no genérico:
  el checker **genera el `impl Eq`** (sintetiza el fuente `impl Eq for T { fn igual(...) }`,
  lo parsea y lo añade a `program.impls`; M9 lo baja) — `generate_eq_derives`. Trait `Eq` en
  el prelude (`igual(self, otro: Self) -> bool`). Igualdad: struct = `&&` de campos; enum =
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
  ni intentar lexar `::`. Bonus: el `mostrar` de Show muestra el nombre **local** (el `::` no entra en
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
  trait derivable, en paralelo a `@derive(Eq)` (M10.1): genera `impl Show { fn mostrar(self) ->
  string }` (trait `Show` en el prelude). `generate_derives`/`validate_derive` se generalizan a
  ambos; `@derive(Eq, Show)` genera los dos. El cuerpo renderiza **por tipo**: primitivos vía
  `to_string`, struct/enum vía `mostrar()` recursivo → **Show sí va con enums recursivos** (a
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
- **M11 + limpieza completos.** Pendientes/diferidos: **cruzar tipos entre módulos**
  (`from M import Punto`, `M.Punto`, `M.Color.Rojo` — namespacar tipos); más string/archivos
  aditivos (`replace`/`contains`, `append`/`exists`…); **M12 concurrencia**; optimización de la VM.
  Capstone: **self-hosting** (ya habilitado: módulos + I/O de archivos). Ver DESIGN §2/§20 / IDEAS.md.
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
