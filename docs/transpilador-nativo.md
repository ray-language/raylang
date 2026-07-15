# El transpilador nativo (`ray build --native`) — estado actual y rumbo de las dependencias externas

> Documento de referencia del **arco P2.b** (jul 2026). Complementa a `PERFORMANCE.md` (crónica
> fase a fase, con mediciones) fijando: (1) una foto de la implementación actual, (2) el análisis
> de la pregunta de diseño sobre dependencias de Cargo en el binario transpilado, y (3) la
> decisión recomendada y su plan.

## 1. Qué es

El transpilador (`src/transpile.rs`) es el **tercer backend** de raylang: traduce el programa
**ya chequeado** a código Rust y lo compila a un **binario nativo**. El modelo es
**dev = VM / deploy = nativo** (como el ciclo dev/release de Rust): la VM da el arranque
instantáneo y el REPL; el binario nativo da la velocidad (24–61× sobre la VM; fib gana a node
5.4×). El contrato de corrección es el mismo del proyecto: **salida byte-idéntica a la VM**,
verificada con oráculos, nunca asumida.

```
fuente → [loader] → [checker] → AST bajado → [transpile.rs] → .rs → rustc → binario
```

## 2. Implementación actual

### 2.1 El CLI (`src/cli.rs::build_native`, ~línea 416)

- `ray build --native [-o salida] [--release]`. Re-carga y re-chequea la entrada, llama a
  `transpile::transpile(&program)`, escribe el Rust a un temporal
  (`$TMP/ray_native_<stem>_<pid>.rs`, PID para builds concurrentes) y lo compila con **`rustc`
  directamente** (sin Cargo).
- **Tiers medidos** (PERFORMANCE.md fase 33): por defecto `-O` (opt-level=2, ~0.2 s, portable);
  `--release` = `opt-level=3 + lto=fat + codegen-units=1 + target-cpu=native` (~10 % extra solo
  en cargas de asignación/Map, 9× de tiempo de compilación, binario no portable). PGO descartado
  por medición.

### 2.2 El transpilador (`src/transpile.rs`, ~4.300 líneas)

**Modelo de valores** (espejo del intérprete): escalares *unboxed* (`i64`/`f64`/`bool`/`char`),
`string → Rc<str>`, `bytes → Rc<[u8]>`, arreglos → `Rc<RefCell<Vec<T>>>`, `Map` →
`Rc<RefCell<HashMap>>`, structs → `Rc<RefCell<S>>`, enums → `Rc<E>`, `Option`/`Result` nativos,
closures → `Rc<dyn Fn>`, `ptr` → `i64`. La semántica de valor de raylang sobre la de movimiento
de Rust se resuelve **clonando al leer** (bump de refcount, O(1)). Traits por **erasure** (los
métodos ya llegan bajados como funciones `Tipo#metodo` → `mangle` los hace identificadores Rust);
bounds `T: Eq`/`T: Show` por **paso de diccionarios**; `print`/`to_string` vía un trait propio
`RayShow` (con impls para `Rc<RefCell<…>>`, Map, tuplas).

**El mecanismo clave para lo que sigue — emisión bajo demanda del runtime**: el struct
`Transpiler` lleva flags `needs_handles` / `needs_concurrency` / `needs_signals` /
`needs_time_rng` / `needs_net` que se **activan al toparse el uso** (un `open`, un `spawn`, un
`tcp_connect`…) y gatean la emisión de cada bloque de runtime auxiliar (registro de handles,
canales con hilos de SO, handler de señales, `Instant` global, helpers de sockets). Un programa
que no usa un subsistema **no arrastra su código**.

**Honestidad sobre el alcance**: todo nodo fuera del subconjunto devuelve `Err` claro. Desde la
fase 45, una función *no-main* que no transpila se emite como **stub que panica** con su firma
declarada (`panic!("'f' no está soportada en el binario nativo…")`): el binario compila, y si el
flujo real nunca alcanza el código no soportado, corre idéntico a la VM (fue el mayor desbloqueo:
web-demos 24 → 38 byte-idénticos).

**Otras piezas estructurales**:

- **Prelude en banda de líneas disjunta** (`prelude::LINE_BASE = 1e9`): las lowerings del checker
  indexan por `(línea, col)` sobre el programa fusionado; el prelude desplazado no colisiona con
  módulos de usuario (fix de la corrupción de `string#hash`/poly1305, fase 42).
- **Closures con estado** (B1): análisis de "celdas" — una variable mutable capturada por un
  closure baja a `Rc<RefCell<T>>` con lectura `.borrow().clone()` y escritura por temporal.
- **Iteradores** (B2): `for x in it` baja a un `loop`/`match` sobre `next(it)`; el protocolo
  `Iter` del prelude (`map`/`filter`/`enumerate`/`zip`…) se emite como código normal (la
  inferencia genérica `unify`/`subst_type` recorre Struct/Enum/Tuple con argumentos).
- **FFI** (fase 49): `extern "lib" { … }` emite el bloque `#[link(name = "lib")] extern "C"` +
  un wrapper que marshala (string→`CString`, retorno `int` = **`c_int` de 32 bits con extensión
  de signo** — el gotcha de ABI que convertía el EOF de `fgetc` en bucle infinito).

### 2.3 Cobertura y verificación

- **Lenguaje completo del corpus**: 37/37 ejemplos deterministas byte-idénticos (escalares,
  strings, arreglos, Map, structs/enums/match, Option/Result/`?` + From, closures, genéricos,
  traits/`dyn`, tuplas, operator overloading, `@derive`, u8/u32/u64, multi-módulo/cápsulas → un
  binario).
- **I/O completa de `std/fs`** (texto/binaria/handles/dirs/stdin/env/args), `std/time`,
  `std/random`, `std/math`, sockets **TCP y UDP** de `std/net`.
- **Concurrencia CSP con hilos de SO reales**: spawn/canales (acotados)/`Task`/`join`/`scope`/
  `select`/`signals` (canales de tipos inmutables: int/float/bool/char/string/bytes).
- **FFI a C** (libm/libc verificados byte-idénticos).
- **38 de ~46 web-demos deterministas byte-idénticos**; 605 tests lib + 19 de integración
  `build_native` en `tests/cli_cli.rs`.

### 2.4 Fuera del alcance hoy, y POR QUÉ

Dos categorías muy distintas:

1. **Diferidos de ingeniería** (proyectos con tradeoffs, no huecos): canales de tipos mutables
   (struct/arreglo/Map cruzando hilos), `spawn` de función nombrada, cancelación M12.5,
   `try_join`, `print` de `ptr`, structs/arreglos por FFI.
2. **El techo de los crates** — la razón de este documento: **TLS (`rustls`), criptografía de
   producción (`ring`) y bases de datos (`rusqlite`, y los clientes que cuelgan de TLS)**. La VM
   los tiene (M43, M53.3) porque el binario `ray` es un proyecto Cargo. El transpilado **no**,
   y la causa raíz no está en el transpilador: es que `build_native` compila con **`rustc`
   pelado**, y un `rustc` suelto no puede resolver dependencias de crates.io (eso lo hace Cargo).
   Los `__tls_*`, `ring::*`, `sqlite_*` hoy quedan como stubs que panican si se alcanzan.

## 3. La pregunta de diseño: dependencias externas en el binario transpilado

**Planteamiento** (Roberto, 14 jul 2026): no hay problema en que el binario transpilado lleve
dependencias de Cargo y así quitar las limitaciones. Dos opciones sobre la mesa:

- **Opción 1 — flags sustractivos**: `build --native` enlaza todo por defecto, con flags para
  desactivar dependencias selectivamente.
- **Opción 2 — transpilador inteligente**: detectar y añadir una dependencia externa **solo
  cuando el programa la necesita**.

### 3.1 El prerrequisito común

Ambas opciones exigen lo mismo primero: cuando haya al menos un crate, `build_native` debe dejar
de invocar `rustc` pelado y **generar un mini-proyecto Cargo** (un `Cargo.toml` + `src/main.rs`
con el Rust emitido) y compilar con `cargo build`. Es un cambio de *fontanería del CLI*, no del
transpilador. Consecuencias a asumir: primera compilación más lenta (descarga/compila los crates;
mitigable con `CARGO_TARGET_DIR` compartido como caché), y necesidad de red la primera vez (o
vendoring).

### 3.2 Análisis comparativo

| | Opción 1 (flags sustractivos) | Opción 2 (bajo demanda) |
|---|---|---|
| Default | Todo enlazado siempre | Solo lo que el programa usa |
| «Hola mundo» | Arrastra rustls+ring+rusqlite: build lento, binario gordo | **Cero crates → sigue el camino `rustc` pelado actual** (~0.2 s) |
| Carga cognitiva | El usuario debe saber qué apagar | Ninguna |
| Encaje con el código | Ninguno | **Es el diseño que ya existe**: los flags `needs_*` |
| Riesgo | Regresión de DX para el 90 % de programas | La detección debe ser precisa (pero el transpilador ya visita cada llamada) |

El argumento decisivo es el último de la columna derecha: el transpilador **ya funciona bajo
demanda** para su propio runtime (§2.2). `needs_net` no enlaza un crate pero es exactamente la
misma idea: *detectar el uso → emitir/enlazar solo entonces*. Extenderlo a crates
(`needs_tls → rustls`, `needs_ring → ring`, `needs_sqlite → rusqlite`) es la continuación natural
del diseño, no un mecanismo nuevo. La opción 1 en solitario, además, contradice la prioridad DX
del proyecto: penaliza por defecto a los programas que no tocan esos subsistemas.

### 3.3 Decisión recomendada: **bajo demanda (núcleo) + camino rápido preservado + flags como override**

1. **Detección bajo demanda** (el núcleo, opción 2): nuevos flags `needs_<crate>` que se activan
   al interceptar la feature correspondiente (los primitivos `__tls_*`, los builtins de crypto
   de producción, los `sqlite_*`). El `Cargo.toml` generado lista **exactamente** esos crates,
   con versiones fijadas por el transpilador (las mismas que usa la VM → misma semántica).
2. **Camino rápido de cero-deps intacto**: si ningún `needs_<crate>` se activa —el caso de TODO
   lo cubierto hasta hoy—, se sigue compilando con `rustc` pelado. Cero regresión de velocidad
   de build ni de requisitos (sin red, sin Cargo) para el caso común.
3. **Flags encima, como escape hatch** (lo útil de la opción 1): p. ej. `--without tls` fuerza
   el comportamiento actual (stub que panica) aunque se detecte el uso — para builds herméticos,
   cross-compile complicado o depuración. Se implementan apagando el mismo flag `needs_*`, así
   que cuestan casi nada una vez existe el punto 1.

**Nota sobre los stubs**: la detección debe distinguir *uso alcanzable* de *referencia muerta*.
Hoy un `import http` arrastra `tls_connect` como stub aunque el programa hable HTTP plano
(fase 45). La regla propuesta: un uso que hoy acaba en **stub** activa el `needs_*` (el crate
hace que el stub se vuelva implementación real); `--without <dep>` lo devuelve a stub. Así el
mismo programa compila siempre, con o sin la dep — solo cambia si el camino TLS funciona o panica.

### 3.4 Plan incremental (un crate a la vez, midiendo, como todo el arco)

- **Paso 0 — la fontanería**: `build_native` genera el proyecto Cargo solo si hay algún
  `needs_<crate>`; si no, `rustc` pelado como hoy. Caché de target compartido. Sin crates reales
  aún: el paso entrega el mecanismo + un test de que el hola-mundo sigue por la vía rápida y de
  que un programa marcado compila vía cargo.
- **Paso 1 — primer crate real: `rusqlite`** (DB). Elegido para validar el diseño end-to-end
  porque ya es precedente aprobado del proyecto (M53.3), su superficie es pequeña y sus
  resultados son deterministas → oráculo contra la VM directo (`examples/db/sqlite_demo.ray`).
- **Paso 2 — `rustls`** (TLS): desbloquea `https`/`webserver` y los clientes de DB remotos que
  cuelgan de TLS. Verificación: los web-demos hoy limitados por el techo TLS.
- **Paso 3 — `ring`** (crypto de producción, precedente M43), donde los ejemplos lo pidan.
- En cada paso: oráculo byte-idéntico contra la VM + test de integración `build_native_*` +
  entrada en PERFORMANCE.md (incl. medir el coste de la primera build con cargo y el binario
  resultante).

**Estado**: pendiente del visto bueno para arrancar el Paso 0.
