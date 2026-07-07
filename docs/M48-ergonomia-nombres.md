# M48 — Ergonomía de nombres y stdlib

Rama: `feature/stdlib-namespaces`.

## Motivación

La capa de **valores** (funciones libres + locales + builtins) está saturada: los
builtins son globales privilegiados que se resuelven **antes** que cualquier función
del usuario ("un builtin no se tapa"), así que ~60 nombres (`len`, `map`, `get`,
`keys`, `split`, `open`…) no se pueden redefinir sin un *shadowing silencioso al
revés*. Además hay constructores poco idiomáticos (`map_new()`, `channel(n)`).

Diagnóstico clave: **raylang ya tiene varios espacios de nombres** (tipos, rutas de
módulo `::`, métodos de trait `Tipo#metodo`). Lo que falta es (a) sacar los builtins
del espacio de valores y (b) un par de azúcares. Es **evolución, no revolución**:
todo cae sobre la maquinaria de traits/genéricos/UFCS ya construida.

Objetivo global de cara al usuario:
- `Map.new()` / `[:]` en vez de `map_new()`.
- Redefinir un nombre de builtin → **error claro**, no silencio.
- `len`/`push`/`get`/… como **métodos de trait**: misma sintaxis con punto, pero
  extensibles a tipos propios y usables en bounds (`fn f<T: Len>(x: T)`).

Método (recordatorio, CLAUDE.md): **una sub-fase a la vez**, con su commit; tocar
runtime ⇒ oráculo VM↔intérprete (con estrés de GC si asigna heap); clasificar en
IDEAS.md antes de comprometer decisiones que bloqueen features; actualizar DESIGN.md
al cambiar el lenguaje.

---

## Fase 1 — Funciones asociadas + literal de Map (azúcar, alto ROI)

### M48.1 — Funciones asociadas a tipos (`Tipo.fn()`) — ✅ COMPLETO

Implementado en 5 pasos (a: motores Rust; b: compilador auto-alojado; c: migración de todo el corpus;
d: retiro de `map_new`/`channel`; e: LSP). `Map.new()`/`Channel.new()`/`Channel.bounded(n)` sustituyen a
`map_new()`/`channel()`, retirados. Ver DESIGN.md §50.1. Detalle de diseño abajo (conservado).


Un namespace **indexado por el tipo**, estilo `Vec::new()`. Reusa el parseo de
`Tipo.algo` que ya existe para la construcción de enums (`Color.Rojo`).

- **Sintaxis**: `Map.new()`, `Channel.new()`, `Channel.bounded(n)`. Llegan del parser
  como `Call(Field(Ident("Map"), "new"))`, igual que la construcción de enum. El
  checker las distingue: si el receptor es un **tipo** y el nombre es una función
  asociada registrada para ese tipo → llamada asociada; si es un enum y el nombre es
  una variante → construcción (ya existe); si no, error.
- **Alcance M48.1**: no hace falta un mecanismo general de `impl` con funciones sin
  `self` todavía. Basta un **registro interno** de asociadas para los tipos
  incorporados: `Map.new` → el indeterminado que hoy es `map_new()`; `Channel.new` /
  `Channel.bounded` → lo que hoy es `channel()` / `channel(n)`. Se reusa el mecanismo
  "indeterminado + tipo fijado por el esperado" (`check_expr_expected`,
  `check_map_new`) — solo cambia la **superficie** (de `map_new()` a `Map.new()`).
- **Compat**: **decidido — migración de golpe.** `map_new()`/`channel()` se
  **retiran** y todos los ejemplos/tests/stdlib se migran a `Map.new()`/`Channel.new()`
  /`Channel.bounded(n)` en el mismo cambio (un commit de churn, más limpio). No hay
  alias deprecado.
- **Runtime**: intacto (mismo opcode `MapNew`/`ChannelNew`; solo cambia cómo el
  front-end reconoce la llamada). Sin oráculo nuevo (el comportamiento no cambia).
- **Implementaciones asociadas** (Rust): el **registro de asociadas** (nombre de tipo
  → {fn asociada → firma/opcode}) lo consultan checker (resolución + tipado), compilador
  (bajada al opcode) y —para el LSP— completado/hover/firma. Retirar `map_new`/`channel`
  del registro de builtins (`BUILTINS`) y su `doc`/`signature`; trasladar esa doc a la
  asociada.
- **LSP** (necesario, no opcional):
  - *Completado* `Map.` / `Channel.` → ofrecer `new`/`bounded` (kind Function) con su
    firma. Hoy `member_completion` sobre un **nombre de tipo** cae en la rama de enum
    (`enum_variantes`); añadir una rama "asociadas del tipo" antes/junto a esa.
  - *Hover* sobre `Map.new` → su firma (`Map.new() -> Map<K,V>`), registrado en la
    posición del nombre asociado (como el hover de variante de M48-previo).
  - *Signature help* dentro de `Map.new(` / `Channel.bounded(` → la firma de la
    asociada (extender `signature_help_result`, como se hizo con las variantes de enum).
  - *Builtins doc/hover incorporado*: si `map_new`/`channel` desaparecen como builtins,
    quitar su entrada de `builtins::doc`/`signature`; la doc vive ahora en la asociada.
- **Diferido**: funciones asociadas **definidas por el usuario** (`impl Tipo { fn
  new() {…} }` sin `self`) → M48.x posterior o parte de C. En M48.1 solo las
  incorporadas.

### M48.2 — Literal de Map — ✅ COMPLETO (ver DESIGN.md §50.2)

Mata `map_new()` de raíz para el caso común.

- **Sintaxis**: **decidido — `[:]` (vacío) y `[k: v, …]` (poblado)**, estilo Swift,
  para no chocar con `{}` (bloque/struct-literal). Extiende el literal de array.
- **Semántica**: `[:]` es indeterminado (como `map_new()`), su tipo lo fija el
  esperado; `[1: "a", 2: "b"]` infiere `Map<int,string>` de los pares (unificando
  claves y valores, como el literal de array infiere `[T]`).
- **Parser**: extender el literal de array; distinguir `[a, b]` (array) de `[a: b]`
  (map) por el `:` tras el primer elemento; `[:]` es el mapa vacío.
- **Runtime**: baja a `MapNew` + `MapInsert` por par (o un opcode nuevo si conviene;
  medir). Asigna heap ⇒ **oráculo con estrés de GC**.
- **LSP**: el analizador debe tragar `[:]`/`[k: v]` sin falsos diagnósticos (reusa el
  pipeline `analizar`). Sin completado especial. Verificar que hover/def dentro de las
  claves/valores del literal siguen funcionando (son expresiones normales).
- **Editores (resaltado)**: la gramática TextMate de VSCode
  (`editors/vscode/syntaxes/raylang.tmLanguage.json`) y la de Sublime
  (`editors/sublime/raylang.sublime-syntax`) ya colorean `:` como operador y `[…]`; con
  `[:]`/`[k: v]` probablemente no haga falta nada, pero **revisar** que `[:]` no se
  pinte raro (bump de versión de la extensión si se toca).
- **DESIGN.md**: nueva sección de literal de Map. **Libro** (`book/`): añadir/actualizar
  el ejemplo de Map.

---

## Fase 2 — Footgun: diagnóstico en vez de silencio (barato, alto valor)

### M48.3 — Colisión de nombre con un builtin → error claro — ✅ COMPLETO (ver DESIGN.md §50.3)

Antes del refactor grande, eliminar la sorpresa.

- **Regla**: **decidido — error duro.** Si el usuario declara una función de nivel
  superior cuyo nombre es un builtin (`fn len(…)`), el checker emite un **error claro**
  en la declaración: *"'len' es un builtin y no puede redefinirse"*. Conservador y
  honesto; el override real llega gratis con la Fase 3 (cuando `len` deje de ser un
  builtin y pase a ser un método de trait, redefinir `fn len` como función libre será
  legal porque ya no colisiona).
- **Alcance**: solo funciones top-level. No afecta locales (una local que tape a una
  función libre ya es válido y deseado; y un builtin no se tapa por una local — eso
  se mantiene).
- **Runtime**: intacto (puro checker). Sin oráculo.
- **LSP**: el nuevo error se propaga **gratis** por el pipeline de diagnósticos
  (`analizar` devuelve el primer error). Verificar que renderiza bien (posición en la
  declaración). Nada específico que implementar.
- **Tests**: unitarios del checker (declarar `fn len` → error con la redacción
  exacta) + un caso en `tests/lsp_cli.rs` que confirme el diagnóstico.

---

## Fase 3 — Builtins de contenedor → traits (el arreglo de fondo)

### M48.4 — La familia de traits de contenedor

> **Estado**: M48.4a–d ✅ COMPLETOS (traits en coexistencia con los builtins; ver DESIGN.md §50.4).
> **M48.4e (retiro de los builtins) DIFERIDO** a una rama de seguimiento: exige reescribir ~1.346 sitios
> prefijos del corpus a `.metodo()` (transformación por AST, no regex) + reformatear ~121 archivos. El
> valor (extensibilidad + bounds) ya está entregado con la coexistencia. Rama: `feature/stdlib-builtin-removal`.


De cara al usuario: **la forma con punto no cambia** (`xs.len()` idéntico); se pierde
la forma prefija `len(xs)`; a cambio, extensibilidad a tipos propios + bounds. El
runtime **no se toca**: patrón "primitivo `__x` + envoltorio en el prelude", ahora
envolviendo en `impl Trait` en vez de en función libre (mismo movimiento de M11.4).

**Mecánica** (por builtin de contenedor):
1. Renombrar el builtin público a un **primitivo `__x`** (oculto, `__`-prefijado; el
   opcode y la impl de ejecución no cambian).
2. Definir el trait en el prelude y sus `impl` para los tipos incorporados, cuyo
   cuerpo llama al primitivo:
   ```
   trait Len { fn len(self) -> int }
   impl Len for [T]     { fn len(self) -> int { __len(self) } }
   impl Len for string  { fn len(self) -> int { __len(self) } }
   impl Len for Map<K,V> { fn len(self) -> int { __len(self) } }
   ```
3. `xs.len()` resuelve como método de trait (UFCS ya lo prioriza sobre función libre);
   baja a `[T]#len(xs)` → `__len(xs)`. **Runtime intacto** (cero opcodes nuevos).

**Puntos de mutación**: `push`/`insert` mutan; NO necesitan `&mut self` — la semántica
de referencia de arrays/maps hace que `self` (una referencia) se mute en el sitio,
como hoy. (Lección: por qué la referencia ahorra el sistema de préstamos.)

**LSP** (en cada sub-fase que migre un grupo de builtins):
- *Completado* `xs.` — hoy `enumerate_members` ofrece los builtins-como-método por
  categoría (`methods_for`) **y** los métodos de trait (paso 2). Al migrar `len` a
  trait, hay que **quitarlo de `methods_for`/`method_takes_args`** y dejar que salga por
  la rama de métodos de trait; si no, aparecería **duplicado**. Verificar que la lista
  no duplica ni pierde entradas tras cada migración.
- *Hover* `xs.len()` — pasa de mostrar la doc del builtin (`builtins::doc`) a la firma
  del método de trait + su `///` (ya soportado por `record_field_hover` de métodos). La
  doc del trait method se escribe en el prelude con `///`.
- *Signature help* `xs.len(` — hoy sale de `builtins::signature`; tras migrar, de la
  firma manglada del método (que `SigCtx.firma` ya busca en las fuentes del prelude).
  Verificar que sigue resolviendo.
- *Doc incorporada*: retirar de `builtins::doc`/`signature`/`methods_for`/
  `method_takes_args` cada nombre migrado.
- **Editores (resaltado)**: la lista `builtins` (regex) de la gramática TextMate incluye
  `len|push|get|keys|…`. Al migrar, esos nombres siguen coloreándose bien por la regla
  `function-call` (`name(`), pero conviene **podar** la lista de builtins para reflejar
  la realidad (los que ya no son builtins). Aplicar a VSCode + Sublime (paridad) y bump
  de versión.

### Prerrequisito de viabilidad (verificado en el código)

Hoy `impl Trait for X` (`ensure_impl_target`) acepta solo **primitivos** (`int/float/
bool/string/char`) y **struct/enum**; **NO** `[T]`, `Map<K,V>` ni `bytes`. Y `type_key_of`
(la clave de despacho de métodos) devuelve `None` para arreglos/Map/bytes/tuplas. Por
tanto `impl Len for string` YA funciona (string es primitivo; cf. `impl Ord for string`),
pero `impl Len for [T]`/`Map<K,V>`/`bytes` **no compilan aún**. La **primera pieza de
M48.4a** es extender esa maquinaria (se paga una vez, luego todo es incremental):
- `ensure_impl_target` + `ensure_generic_impl_target`: aceptar `Type::Array`,
  `Type::Map`, `Type::Bytes` como objetivos (Array/Map genéricos; bytes concreto).
- `type_key_of`: dar clave estable — p. ej. `"[]"` (array), `"Map"`, `"bytes"`.
- Resolución de impl genérico por constructor (M9.2b) para esas claves.

### Catálogo de traits — alcance COMPLETO (decidido con el usuario)

Granularidad: **traits finos y cross-type** donde la operación se comparte (mejor
extensibilidad); **traits agrupados por tipo** donde las operaciones son específicas de
un tipo (un solo impl). Solo se traitifican los **builtins** (los que contaminan el
namespace); los envoltorios del prelude (`pop`/`get`/`remove`/`position`/`index_of`) ya
son funciones y no se tocan.

- **`Len { fn len(self) -> int }`** → `string`, `[T]`, `Map<K,V>`, `bytes`. (estrella:
  cross-type, bounds `T: Len`, extensible.)
- **`Contains<T> { fn contains(self, x: T) -> bool }`** → `string`(sub-cadena),
  `[T]`(pertenencia), `bytes`(sub-cadena). **D2 resuelto: un trait genérico** (unifica el
  nombre; se acepta la conflación subcadena/pertenencia, documentada).
- **`Push<T> { fn push(self, x: T) }`** → `[T]`. **D3 resuelto: trait genérico**
  (`impl<T> Push<T> for [T]`; muta por referencia, sin `&mut`).
- **`Reverse { fn reverse(self) -> Self }`** → `[T]` (el builtin `reverse` es solo de
  arreglos; string-reverse vive en `std/text`, no es builtin).
- **`MapOps<K,V> { insert; contains_key; keys; values }`** → `Map<K,V>` (agrupado, un
  impl). `get`/`remove` son envoltorios del prelude (Option) → **se quedan** como
  funciones del prelude, no entran al trait (no son builtins).
- **`StrOps { trim; split; replace; chars; starts_with; ends_with; to_upper; to_lower;
  substring; repeat; join; char_code }`** → `string` (agrupado, un impl). `index_of` es
  prelude → fuera.
- **`BytesOps { sub_bytes }`** → `bytes`. (`to_bytes` es string→bytes: entra a `StrOps`;
  `bytes_of` es `[int]`→bytes, constructor: **se queda builtin**, no tiene receptor claro.)

**Fuera de M48.4** (se quedan como builtins — "no todo es un método"):
- `print`/`eprint` (universal/variádico), `panic`.
- **`to_string`** (D4 diferido): universal/ad-hoc sobre primitivos; solaparía `Show`/
  `mostrar`. Se queda builtin por ahora; enrutar por `Show` es una mejora aparte.
- `a[i]` **Index** (D-Index resuelto: **no**): es sintaxis/operador, no un builtin-
  función; traitificarlo sería *operator overloading de `[]`*, otra fase. Se queda con
  reglas ad-hoc (array/string). Map no se indexa (usa `get`/`insert`).
- `bytes_of`, constructores sin receptor; matemáticas (`sqrt`/`pow`/…, sobre `float`,
  ya monomórficas); concurrencia (`spawn`/`send`/`select`/`scope`), I/O (`open`/`write`/…).
- `map`/`filter`/`fold`/`sort` ya son **funciones del prelude** (no builtins).

### Sub-fases de M48.4 (una a la vez, oráculo por cada una que toque heap)

- **M48.4a** — **maquinaria** (impl-para-`[T]`/`Map`/`bytes`) + **`Len`**. Valida el
  patrón end-to-end sobre los 4 tipos; entrega bounds `T: Len`.
- **M48.4b** — `Push<T>` + `Reverse` (builtins de arreglo) + `Contains<T>` (cross-type).
- **M48.4c** — `MapOps<K,V>` (builtins de Map: insert/contains_key/keys/values).
- **M48.4d** — `StrOps` + `BytesOps` (builtins de string/bytes).
- **M48.4e** — limpieza y cierre: retirar los builtins migrados (la forma prefija
  `len(xs)` deja de existir), migrar **todo** el corpus (ejemplos/stdlib/`packages`) de
  la forma prefija a `.metodo()`, **reconciliar el self-hosted** (D5), podar la gramática
  de resaltado, actualizar DESIGN.md + libro + raydoc.

**Transición sin romper**: cada sub-fase usa **coexistencia** (builtin + trait a la vez;
el método de trait gana por prioridad de resolución en `check_call_field`) hasta el
retiro en M48.4e — así los 156 ejemplos siguen verdes durante toda la fase.

**Self-hosting (D5)**: el checker/intérprete/VM auto-alojados replican el manejo de
builtins. Como `.len()` etc. se resuelven por UFCS→builtin en el self-hosted (que no
tiene el lowering de M9), lo más simple es que el self-hosted **siga tratándolos como
builtins internamente** (sus fuentes usan `.len()` que cae en la rama UFCS→builtin ya
existente) — así la meta-circularidad se mantiene **sin** reimplementar los traits en
raylang. Es decir: en Rust `len` pasa a trait; en el self-hosted `len` sigue siendo el
builtin que el intérprete/VM despachan. Divergencia interna, comportamiento idéntico
(el oráculo es conductual). Se confirma al llegar a M48.4e.

---

---

## Migración transversal (ejemplos, stdlib, editores, docs)

Cada fase toca superficie del lenguaje ⇒ hay que barrer **todo** el corpus, no solo
`examples/`. Inventario de sitios a migrar y en qué fase:

- **`examples/`** (156 archivos `.ray`): `map_new()`→`Map.new()`,
  `channel()`→`Channel.new()`, `channel(n)`→`Channel.bounded(n)` (M48.1); usar
  `[:]`/`[k: v]` donde aplique (M48.2); `len(x)`/`push(x,…)` prefijos → `x.len()`/
  `x.push(…)` (M48.4e). Verificar que **corren** (`ray run`) tras cada migración, no solo
  que compilan.
- **`std/`** (3 archivos: `math.ray`, `sort.ray`, `text.ray`): mismos reemplazos.
- **`packages/net/`** (23 archivos): idem; usan Map/canales intensivamente.
- (Referencia: ~25 archivos del corpus usan hoy `map_new`/`channel(`; los usos de `len`/
  `push` prefijos se cuentan al llegar a M48.4e.)
- **`selfhost/*.ray`** (lexer/parser/checker/interpreter/vm/prelude/loader): **crítico**.
  El compilador auto-alojado usa estos builtins. Si M48.4 cambia qué es builtin vs
  método, el `selfhost/checker.ray` y `selfhost/interpreter.ray`/`vm.ray` deben
  reflejarlo, y sus fuentes migrarse, para no romper la **meta-circularidad**
  (`tests/selfhost_metacircular*.rs`). Es el punto de mayor riesgo; se aborda en M48.4e
  (o se difiere explícitamente si el corpus self-hosted no toca los builtins migrados
  como métodos, documentándolo).
- **Tests** (`tests/*.rs` + `#[cfg(test)]` en `src/`): los que construyen fuente raylang
  con `map_new`/`channel`/`len(x)` se migran junto a cada fase.
- **`book/`** (mdBook): capítulos de Map, concurrencia, stdlib — actualizar la sintaxis
  y añadir el literal `[:]`.
- **raydoc / prelude docs**: si `map_new`/`channel`/`len` dejan de ser builtins, su doc
  se mueve a la asociada / al `///` del método de trait en el prelude.
- **Editores** (`editors/vscode`, `editors/sublime`): podar la lista `builtins` de la
  gramática y bump de versión (M48.2 revisar `[:]`; M48.4 podar nombres migrados).

**Tests (preferencia del usuario — acotar siempre que se pueda)**: durante el
desarrollo y al cerrar cada sub-fase, correr **solo los tests de los archivos tocados**
(p. ej. `cargo test --lib checker`, `cargo test --test lsp_cli`, el binario de
integración afectado), **no** la suite completa —es lenta—. El barrido completo
(`cargo test`) se reserva para hitos puntuales donde el cambio es transversal de verdad
(típicamente el cierre de M48.4e, que toca prelude + self-hosting) o cuando un test
acotado deja dudas de regresión en otro módulo. Verificar además que los **ejemplos
migrados** corren (`ray run <archivo>`), pero solo los tocados en esa fase, no los 156.

## LSP: resumen de impacto por fase

| Fase | Completado | Hover | Signature help | Diagnóstico | Gramática |
|------|-----------|-------|----------------|-------------|-----------|
| M48.1 `Tipo.fn()` | `Map.`/`Channel.` → asociadas | `Map.new` → firma | `Map.new(` → firma | — | — |
| M48.2 `[:]` | — | claves/valores (ya) | — | no falsos positivos | revisar `[:]` |
| M48.3 footgun | — | — | — | error nuevo (gratis) | — |
| M48.4 traits | quitar de `methods_for` (evitar duplicados) | builtin→método de trait | builtin→firma manglada | — | podar builtins |

## Orden de ejecución

1. **M48.1** (funciones asociadas incorporadas) → **M48.2** (literal de Map).
2. **M48.3** (footgun → diagnóstico).
3. **M48.4a…e** (traits de contenedor, una sub-fase a la vez).

## Decisiones tomadas

- **Literal de Map**: `[:]` (vacío) y `[k: v, …]` (poblado), estilo Swift.
- **Transición `map_new()`/`channel()`**: migración de golpe (se retiran, sin alias).
- **M48.3**: error duro al redefinir un builtin.
- **D1 — Alcance de M48.4**: **COMPLETO** (Len + Push/Reverse + Contains + MapOps +
  StrOps/BytesOps).
- **D2 — `Contains`**: un solo trait genérico `Contains<T>` (unifica el nombre; conflación
  subcadena/pertenencia documentada).
- **D3 — `Push`**: trait genérico `Push<T>` (`impl<T> Push<T> for [T]`).
- **D-Index**: `a[i]` NO se traitifica (es sintaxis/operador; queda ad-hoc).
- **D4 — `to_string`**: diferido (se queda builtin; enrutar por `Show` es mejora aparte).
- **D5 — Self-hosting**: el self-hosted **sigue tratando los builtins como builtins**
  internamente (sus fuentes usan `.metodo()` que cae en la rama UFCS→builtin ya
  existente); la meta-circularidad se mantiene sin reimplementar los traits en raylang
  (oráculo conductual). Se confirma en M48.4e.
