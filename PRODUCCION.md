# raylang — Análisis y plan de perfeccionamiento hacia producción

> Documento-contrato del **cambio de norte** anotado en DESIGN §21.1 e IDEAS ("raylang de
> producción"). Vive en la rama `feature/improvements`. Primera parte: **análisis a fondo** del
> lenguaje tal como está (post-§36, jul 2026) contra los cinco ejes que debe mantener — *moderno,
> flexible, ligero, seguro, elegante*. Segunda parte: el **plan** (arcos A–D, M33–M43) para llevarlo
> a una 1.0 de producción sin perder esos ejes.

---

## Parte I — Análisis

### 0. Fotografía del estado actual

| Métrica | Valor |
|---|---|
| Núcleo Rust | ~24.6k líneas (`checker` 6.0k, `vm` 4.6k, `parser` 2.2k, `interpreter` 2.2k) |
| Código raylang (stdlib/ejemplos/selfhost) | 164 archivos `.ray` |
| Tests | 665 verdes + 7 `#[ignore]` lentos; oráculo VM↔intérprete; meta-circularidad por ambos back-ends |
| Dependencias Cargo | 3 (rustls + webpki-roots + rustls-pki-types; la excepción TLS de §28.4) |
| Higiene | 16 warnings de clippy, 3 TODO, 6 `unsafe` (acotados a `poll.rs`) |
| Deuda anotada | ~95 menciones de "diferido" en DESIGN.md (honestas, localizadas) |
| `panic!`/`unwrap`/`expect` en el compilador | parser 90 · vm 59 · checker 37 · intérprete 20 |
| Rendimiento | VM de pila sin JIT; GC mark-and-sweep stop-the-world; banco `benchmarks/` con historial medido (§27) |
| Concurrencia | CSP M:1 cooperativo, determinista — **un solo núcleo** |

### 1. Moderno — ✅ fuerte, con tres huecos estructurales

**Lo que ya está al nivel de un lenguaje de 2026**: tipos suma + pattern matching exhaustivo,
genéricos con bounds, traits con métodos por defecto y trait objects (`dyn A + B` con upcasting),
`Option`/`Result`/`?` sin `null`, UFCS + pipelines, tuplas, `for`, interpolación `f"..."`,
sobrecarga de operadores, enteros con tamaño (`u8`/`u32`/`u64`), `@derive(Eq, Show)`, módulos con
cápsulas y re-exports, concurrencia estructurada CSP (spawn/canales/scope/select/cancelación), LSP
completo, formateador, runner de tests integrado. Pocos lenguajes nuevos llegan a 1.0 con esta lista.

**Huecos estructurales** (los que un usuario de 2026 notaría el primer día):
- **Sin protocolo de iteración.** `for` es ejecución directa por forma (arreglo/rango/string/Map);
  un tipo de usuario no es iterable. Falta un trait `Iterator` (o `next() -> Option<T>`) que unifique
  `for`, `map`/`filter`/`fold` y permita adaptadores perezosos.
- **Patrones planos.** `match` no anida patrones (`Some(Punto { x, .. })`), no matchea literales ni
  tiene guardas (`if` en el brazo), y no hay `if let`/`while let`. Es la aspereza ergonómica más
  visible que queda.
- **`Map` limitado a claves primitivas.** Falta un trait `Hash` derivable para claves de usuario; y
  faltan colecciones hermanas (`Set`, deque, string builder eficiente).

### 2. Flexible — ✅ en el lenguaje, ❌ en el ecosistema

El **lenguaje** es flexible: traits + genéricos + dyn cubren polimorfismo estático y dinámico;
las librerías M15–M32 demuestran que se puede escribir de todo (de un códec Huffman a un cliente
PostgreSQL) sin tocar el runtime. La arquitectura de builtins (registro único L1) hace barato
extender la frontera con el host.

La **inflexibilidad está fuera del lenguaje**:
- **Sin gestor de paquetes**: las "librerías" son archivos copiados de `examples/`. No hay
  manifiesto, versiones, ni resolución de dependencias. Es el bloqueo nº 1 para cualquier adopción.
- **Sin FFI**: la única vía de escape es escribir un builtin en Rust y recompilar el compilador.
  Producción necesita `extern fn` con frontera C documentada.
- **Sin metaprogramación**: `@derive` es un conjunto cerrado (Eq, Show). Sin macros ni derive de
  usuario, cosas como `@derive(Json)` exigen tocar el compilador.

### 3. Ligero — ✅ excepcional, con un coste oculto

Tres dependencias, binario único, runtime mínimo, arranque instantáneo. La invariante cero-deps
produjo un artefacto genuinamente ligero y auditable — esto es un **activo de producción**, no solo
pedagógico, y hay que conservarlo.

El coste oculto: **se compilan y mantienen dos motores** (intérprete + VM) y el producto final solo
necesita uno. El intérprete es el oráculo — valiosísimo para desarrollo del lenguaje, innecesario en
el binario que instala un usuario. Y la VM, aunque optimizada incrementalmente (§27: −5/−9% por
pase), sigue siendo una VM de pila con `HeapValue` de 32 bytes, sin inline caches ni
superinstrucciones: correcta, no competitiva.

### 4. Seguro — ✅ por diseño, ❌ en tres frentes de producción

**Por diseño**: sin `null`, errores como valores, tipado estático estricto, índices con
bounds-checking, sin desbordamiento de pila (límite compartido + TCO), data-race freedom por CSP
(trivial en M:1), TLS delegado a rustls (la decisión correcta), comparaciones en tiempo casi
constante en las libs cripto, `unsafe` acotado a 6 sitios en `poll.rs`.

**Los tres frentes que producción exige y hoy no están**:
1. **El compilador puede caerse (ICE).** ~200 `panic!`/`unwrap` en parser/checker/VM. La mayoría
   son invariantes internas legítimas, pero un compilador de producción **no hace panic con ninguna
   entrada**: todo camino debe acabar en diagnóstico. Corolario: falta *fuzzing* sistemático.
2. **La cripto en raylang no es cripto de producción.** SHA-256/ChaCha20/Ed25519 escritos en el
   lenguaje son un logro pedagógico verificado contra vectores RFC, pero no están endurecidos contra
   side-channels ni auditados. Producción debe delegarlas al proveedor nativo (ring, ya en el árbol
   por rustls) y conservar las puras como `edu/` o para verificación cruzada.
3. **Semántica de overflow sin política declarada.** `u8/u32/u64` son wrapping por diseño (M28.3),
   pero el `int` (i64) no tiene política escrita (¿wrap? ¿panic? ¿UB?). Producción exige definirla
   (propuesta: checked con panic en debug, wrapping documentado en release, `+%` explícito si se
   quiere wrap siempre) y también: límites de recursos (fuel/memoria para código no confiable).

### 5. Elegante — ✅ el activo principal; protegerlo es parte del plan

La elegancia de raylang no es cosmética, es **arquitectónica**: orientación a expresiones, un
núcleo que casi no creció en 32 módulos (erasure, azúcar de front-end, builtins registrados),
UFCS que hace que todo se lea igual, errores con posición siempre. La *tesis* del proyecto — núcleo
pequeño, todo lo demás encima — es exactamente lo que un lenguaje de producción quiere y casi
ninguno consigue.

Lo que la empaña hoy:
- **Diagnósticos de un solo punto** (`línea, col` sin spans): el subrayado es un `^` de un
  carácter; el LSP reporta **un** error por documento (fail-fast). Producción = spans + multi-error
  con recuperación + sugerencias ("¿quisiste decir…?").
- **La stdlib vive en `examples/`**: nombres surgidos por acreción (`index_of` vs `position`,
  `fetch` por colisión con `get`), sin revisión de API global, sin documentación generada.
- **DESIGN.md es una crónica, no una especificación**: 4.5k líneas de historia razonada (oro
  pedagógico) pero nadie puede implementar raylang desde ella. Falta la gramática EBNF normativa y
  la semántica por construcción.

### Síntesis: las siete brechas hacia producción

1. Compilador que nunca se cae + diagnósticos de calidad (spans, multi-error).
2. Un solo motor de producto, con rendimiento competitivo (la VM; el intérprete pasa a herramienta de desarrollo).
3. Paralelismo real (multicore) sin sacrificar la data-race freedom de CSP.
4. Gestor de paquetes + CLI unificado + stdlib versionada con API revisada.
5. Especificación normativa + versionado semántico + política de estabilidad.
6. Endurecimiento: cripto nativa, política de overflow, fuzzing, límites de recursos.
7. FFI y distribución (binarios, playground, marketplace).

---

## Parte II — El plan (M33–M43, arcos A–D)

**Principios que el plan hereda del proyecto** (no negociables):
- **Una fase a la vez**, cada una con sus tests y su commit; el oráculo VM↔intérprete sigue vigente
  *en desarrollo* aunque el producto embarque solo la VM.
- **Cero dependencias** salvo excepciones conscientes y acotadas (hoy TLS; el plan añade como mucho
  el proveedor cripto que rustls ya trae).
- **Medir antes de conservar** (§27): toda optimización pasa por `benchmarks/` y se queda solo si
  supera el ruido.
- **La elegancia es un requisito**, no un adorno: cada fase declara qué NO va a engordar.

### Arco A — Fundamentos de estabilidad (M33–M35)

**M33 — Compilador sin pánicos + diagnósticos de producción.** La fase más transversal.
- **a)** Spans `(inicio, fin)` en tokens y nodos (hoy solo `(línea, col)`); `diagnostic.rs` subraya
  rangos. Es el prerrequisito de todo lo demás del arco.
- **b)** ICE → diagnóstico: auditoría de los ~200 `panic!`/`unwrap` del front-end; los alcanzables
  por entrada del usuario se convierten en errores con posición; los de invariante interna se
  centralizan en un `ice!()` que pide reporte de bug con contexto.
- **c)** Multi-error con recuperación: el parser se resincroniza (en `;`/`}`), el checker acumula
  (hasta N) en vez de fail-fast; el LSP deja de reportar solo el primero.
- **d)** Fuzzing: `cargo-fuzz` sobre lexer/parser/checker; el corpus semilla son los 164 `.ray` del
  repo. Criterio de salida: cero crashes tras una campaña sostenida.

**M34 — Especificación y versionado.**
- `SPEC.md` normativo: gramática EBNF completa + semántica por construcción (DESIGN.md queda como
  la crónica de diseño; el libro como pedagogía). El parser auto-alojado de M14 es el validador
  perfecto de la gramática escrita.
- Versionado semántico del lenguaje y la stdlib; política de estabilidad (qué es 1.0-estable, qué
  es `unstable` detrás de flag); proceso de deprecación.
- Congelar la superficie 1.0-beta: revisión final de nombres/firmas ANTES de que exista ecosistema
  (después es tarde). Aquí entran las asperezas conocidas: `index_of`/`position`, `fetch`, etc.

**M35 — Un solo motor de producto.** La VM pasa a ser *el* motor; el intérprete queda como oráculo
de desarrollo (feature de Cargo `oracle`, fuera del binario release). El binario `ray` release
embarca VM + LSP + fmt + test runner. Suite de regresión de rendimiento en CI (los benchmarks
corren y fallan si degradan >5%).

### Arco B — Rendimiento y paralelismo (M36–M38)

**M36 — Optimización profunda de la VM.** Continúa §27 con los pendientes ya identificados y los
estructurales: `HeapValue` 32→16 B (o NaN-boxing), dedup de constantes, superinstrucciones +
peephole ampliado, inline caches para `ODispatch`/campos, dispatch del bucle (tail-call dispatch o
computed-goto estable). Presupuesto honesto: **3–5× acumulado** en el banco, medido pase a pase.

**M37 — GC de producción.** Generacional (nursery + copia) o incremental (tri-color con write
barrier) — decidir midiendo pausas con `gcnested`-style workloads reales. Objetivo: pausas acotadas
(<1 ms p99 en el banco), no throughput máximo. Nota: si M38 elige heap-por-actor, este GC se
simplifica (heaps pequeños e independientes) — por eso M37 y M38 se **diseñan juntos** aunque se
implementen en ese orden.

**M38 — Paralelismo M:N con aislamiento por actores.** El multicore es la brecha de producción más
profunda, y CSP ya eligió el camino: **heap por fibra + transferencia de propiedad en `send`**
(move para valores únicos, copia profunda si hay aliasing — semántica Erlang/Pony-lite). Resultado:
scheduler M:N sobre un pool de hilos, **sin GC global concurrente** (cada actor recolecta su heap
en sus yields) y data-race freedom **preservada por construcción**, no por ownership en el tipo.
El determinismo del scheduler M:1 se conserva como modo (`--deterministic`) para tests. Es la fase
de más riesgo técnico del plan; entra después de que A estabilice y M36/37 den la base de memoria.

### Arco C — Ecosistema (M39–M41)

**M39 — CLI unificado `ray` + gestor de paquetes.**
- `ray new/build/run/test/fmt/doc/add/publish` (hoy: binario + flags sueltos).
- Manifiesto `ray.toml` (¡el parser TOML de M32.2 se come su propia comida!), lockfile con hashes
  (supply-chain desde el día uno), resolución semver mínima-versión (estilo Go, más simple y
  reproducible que NP-completo estilo Cargo), registro **git-first** (URLs + tags; un índice
  central es fase posterior).

**M40 — stdlib 1.0.** Promover `examples/web` + `examples/stdlib` a un árbol `std/` versionado que
el gestor trae por defecto. Con la revisión de API de M34 aplicada: protocolo `Iterator` (cierra el
hueco nº 1 de "moderno"; `for` y `map`/`filter`/`fold` se re-fundan sobre él), trait `Hash`
derivable (claves de usuario en `Map`), `Set`/deque/string-builder, patrones anidados + guardas +
`if let` en el lenguaje (la deuda ergonómica que quedó), y `raydoc` (documentación generada desde
comentarios `///`).

**M41 — FFI.** `extern fn` con ABI C: declarar, cargar (`dlopen` con el mismo molde `extern "C"`
sin deps de `poll.rs`), marshalling de primitivos + bytes. Frontera documentada como *la* zona
insegura del lenguaje (todo lo demás sigue siendo seguro). Es lo que permite que el ecosistema
crezca sin tocar el compilador.

### Arco D — Endurecimiento y distribución (M42–M43 → 1.0)

**M42 — Endurecimiento de seguridad.**
- Política de overflow de `int` escrita en SPEC y aplicada (checked+panic en debug, definida en release).
- Cripto de producción: `sha256`/`hmac`/`chacha20poly1305`/`ed25519` delegan al proveedor nativo
  (ring, ya enlazado por rustls — cero deps nuevas); las implementaciones raylang puras se mueven a
  `edu/` y quedan como verificación cruzada y material del libro.
- Límites de recursos opcionales (fuel de instrucciones + tope de heap) para embeber raylang como
  lenguaje de scripts confinado — un nicho de adopción natural para un runtime así de ligero.
- Fuzzing continuo en CI + `cargo audit` + revisión de los 6 `unsafe`.

**M43 — Distribución y lanzamiento.**
- Releases con binarios por plataforma (macOS/Linux/Windows) + instalador (`curl | sh` + brew).
- **Playground web**: la VM compilada a WASM (Rust→wasm32 es directo; el poller cae a no-disponible
  honesto como ya hace en Windows).
- Extensión VSCode publicada en el marketplace; libro y sitio publicados; `SECURITY.md` + proceso
  de reporte.
- **raylang 1.0** cuando: cero ICEs en el corpus de fuzzing, SPEC publicada, `ray` + paquetes
  funcionando, multicore estable, benchmarks dentro del presupuesto, política de seguridad vigente.

### Orden y dependencias

```
A (M33 spans/no-ICE → M34 SPEC/API-freeze → M35 un-motor)
   └─→ B (M36 VM ─ M37 GC ⇄ M38 M:N)          [B necesita la estabilidad de A]
   └─→ C (M39 ray+pkg → M40 std/ → M41 FFI)   [C necesita el API-freeze de M34]
B + C ─→ D (M42 endurecer → M43 lanzar 1.0)
```

A es prerrequisito de todo (no se optimiza ni se congela API sobre un compilador que hace panic).
B y C pueden avanzar **en paralelo** tras A (tocan capas distintas: runtime vs tooling). D cierra.

### Qué se sacrifica, dicho honestamente

- **El intérprete como producto** (queda como oráculo de desarrollo): dos motores en el binario es
  un lujo pedagógico.
- **El absolutismo cero-deps** se relaja a "cero deps salvo excepción consciente": TLS (ya hecha) y
  el proveedor cripto (que ya estaba en el árbol). Nada más.
- **La cadencia libro-por-fase** se afloja en A–B (fases de infraestructura); el libro gana en su
  lugar capítulos de cierre por arco.
- **No entran en 1.0**: JIT/backend nativo (M18 sigue aparcado; el transpile-a-Rust queda como
  investigación post-1.0), macros de usuario, algebraic effects, reflection. Anotados en IDEAS.md.
