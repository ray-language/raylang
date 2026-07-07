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

### M48.1 — Funciones asociadas a tipos (`Tipo.fn()`)

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
- **Compat**: mantener `map_new()`/`channel()` como alias **deprecados** durante la
  transición (o migrar los ejemplos de golpe; decidir con el usuario). Los tests que
  usan `map_new` se migran.
- **Runtime**: intacto (mismo opcode `MapNew`/`ChannelNew`; solo cambia cómo el
  front-end reconoce la llamada). Sin oráculo nuevo (el comportamiento no cambia).
- **Diferido**: funciones asociadas **definidas por el usuario** (`impl Tipo { fn
  new() {…} }` sin `self`) → M48.x posterior o parte de C. En M48.1 solo las
  incorporadas.
- **LSP**: `Map.` / `Channel.` deberían completar `new`/`bounded` (aprovechar el
  trabajo reciente de completado de miembros).

### M48.2 — Literal de Map

Mata `map_new()` de raíz para el caso común.

- **Sintaxis** (decidir con el usuario): candidata `[:]` (vacío) y `[k: v, …]`
  (poblado), estilo Swift, para no chocar con `{}` (bloque/struct-literal). Alternativa
  `#{…}`. **Punto de decisión de diseño** (clasificar en IDEAS.md).
- **Semántica**: `[:]` es indeterminado (como `map_new()`), su tipo lo fija el
  esperado; `[1: "a", 2: "b"]` infiere `Map<int,string>` de los pares (unificando
  claves y valores, como el literal de array infiere `[T]`).
- **Parser**: extender el literal de array; distinguir `[a, b]` (array) de `[a: b]`
  (map) por el `:` tras el primer elemento; `[:]` es el mapa vacío.
- **Runtime**: baja a `MapNew` + `MapInsert` por par (o un opcode nuevo si conviene;
  medir). Asigna heap ⇒ **oráculo con estrés de GC**.
- **DESIGN.md**: nueva sección de literal de Map.

---

## Fase 2 — Footgun: diagnóstico en vez de silencio (barato, alto valor)

### M48.3 — Colisión de nombre con un builtin → error claro

Antes del refactor grande, eliminar la sorpresa.

- **Regla**: si el usuario declara una función de nivel superior cuyo nombre es un
  builtin (`fn len(…)`), el checker emite un **error claro** en la declaración:
  *"'len' es un builtin y no puede redefinirse"* (o, si se decide permitir override,
  darle prioridad al usuario con un aviso — **punto de decisión**, ver abajo).
- **Recomendación**: por ahora **error** (conservador y honesto); el override real
  llega gratis con C (cuando `len` deje de ser un builtin, redefinirlo será legal).
- **Alcance**: solo funciones top-level. No afecta locales (una local que tape a una
  función libre ya es válido y deseado; y un builtin no se tapa por una local — eso
  se mantiene).
- **Runtime**: intacto (puro checker). Sin oráculo.
- **Tests**: unitarios del checker (declarar `fn len` → error con la redacción
  exacta).

---

## Fase 3 — Builtins de contenedor → traits (el arreglo de fondo)

### M48.4 — La familia de traits de contenedor

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

**El diseño real está en cómo se cortan los traits** (clasificar cada decisión en
IDEAS.md). Catálogo tentativo:
- `Len` (len) → `[T]`, `string`, `Map`, `bytes`.
- `Push` / `Pop` → `[T]`.
- `Index` (`a[i]`, `s[i]`, `m[k]`) → `[T]`, `string`, `Map` (esto ya es sintaxis del
  lenguaje; decidir si se traitifica o se queda como operador con reglas ad-hoc).
- `MapOps<K,V>` (get/insert/remove/keys/values/contains_key) → `Map` (**trait
  genérico**; ya existe la maquinaria, cf. `Iterator<T>` de M40.2).
- `Contains` → hoy ad-hoc (subcadena en string, pertenencia en array): con traits hay
  que **comprometerse** (¿uno o dos traits?). Punto de decisión.
- string-específicos (trim/split/replace/…): decidir si van a un `StrOps` o siguen
  como métodos sueltos.

**Fuera de alcance de C** (se quedan como builtins de verdad — lección honesta "no
todo es un método"):
- `print`/`eprint` (universal/variádico de facto).
- `to_string` → puede enrutar por el `Show`/`mostrar` que ya existe.
- `map`/`filter`/`fold`/`sort` ya son **funciones del prelude** (no builtins), ya van
  por UFCS; opcionalmente moverlas a un trait tipo `Iterator` por coherencia total.

**Sub-fases sugeridas de M48.4** (una a la vez, oráculo por cada una que toque heap):
- **M48.4a** — `Len` (el caso más simple y multi-tipo; valida el patrón end-to-end).
- **M48.4b** — `Push`/`Pop` sobre `[T]` (mutación por referencia).
- **M48.4c** — `MapOps<K,V>` (trait genérico; get/insert/remove/keys/values).
- **M48.4d** — decidir e implementar `Contains`/`Index` (los ambiguos).
- **M48.4e** — limpieza: quitar los alias deprecados, migrar ejemplos/stdlib,
  actualizar el self-hosted si procede, DESIGN.md.

**Transición sin romper**: durante M48.4, un builtin puede coexistir con su trait
(el trait gana por prioridad de resolución) para no romper los ~35 ejemplos de golpe;
se migran y se retira el builtin en M48.4e.

**Riesgo self-hosting**: el checker/intérprete/VM auto-alojados (M14) replican el
manejo de builtins. Traitificar cambia qué es builtin y qué es prelude → hay que
reflejarlo en `selfhost/*.ray` para no romper la meta-circularidad. Evaluar el
alcance en M48.4e (puede quedar diferido si el corpus self-hosted no usa esos
builtins como métodos).

---

## Orden de ejecución

1. **M48.1** (funciones asociadas incorporadas) → **M48.2** (literal de Map).
2. **M48.3** (footgun → diagnóstico).
3. **M48.4a…e** (traits de contenedor, una sub-fase a la vez).

## Decisiones abiertas (para el usuario, clasificar en IDEAS.md)

- Sintaxis del literal de Map: `[:]`/`[k:v]` vs `#{…}`.
- `map_new()`/`channel()`: ¿alias deprecados durante la transición o migración de
  golpe?
- M48.3: ¿error duro al redefinir un builtin, o override con aviso?
- Corte de traits en M48.4: `Contains` (uno o dos), `Index` (traitificar o no),
  string-ops (trait o sueltos).
- Alcance self-hosting en M48.4e.
