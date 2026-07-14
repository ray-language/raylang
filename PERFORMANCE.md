# PERFORMANCE.md — plan para llevar raylang a la liga de Node/Go

> **Decisión (14 jul 2026, fijada con el usuario)**: el lenguaje está completo y gusta;
> el perfil académico queda atrás del todo. **De ahora en adelante el objetivo nº 1 es
> RENDIMIENTO.** Este documento recoge todas las propuestas — de la más barata a la más
> radical — ordenadas en arcos ejecutables. La disciplina de siempre se mantiene:
> **incremental, midiendo** (benchmarks + oráculo), se conserva solo lo que supera el ruido.

## 1. Punto de partida (medido, jul 2026)

Benchmark poliglota del usuario (`~/Desktop/benchmarks`, hyperfine/best-of-N, M3) contra
node/php/lua/python/ruby/perl:

| Workload | Qué mide | ray vs líder | Puesto |
|---|---|---|---|
| arranque | binario nativo, sin warm-up | **~3 ms, top-3** | 🥉 |
| `fibrec` (fib 34) | llamada/despacho puros | 12.5× tras node | 6/7 |
| `loopsum` (10M) | aritmética | 8.3× tras php | 7/7 |
| `jsonserialize` | **servicios**: construir respuestas | 2.9× tras perl (bate a ruby 3×) | 6/7 |
| `logparse` | **servicios**: parsear entrada | 4.9× | 7/7 |
| `wordcount` | **servicios**: agregación en map | **9.7×** | 7/7 |

**Atribución medida** (experimento de aislamiento): en `wordcount`, el **Map se come el
68 %** (1002→321 ms al quitarlo); el resto es el impuesto general de intérprete (~2–3×,
el mismo múltiplo de `jsonserialize`). Node gana `fibrec` por JIT.

**Causas raíz confirmadas en el código** (no especulación):
- `m.get(k)` = **2 alocaciones de GC por acceso** (el opcode `MapGet` construye
  `Vec`+`Obj::Array`, `src/vm.rs:991`; el prelude lo envuelve en `Option` = otra) + clon
  de la clave (`heap_to_key`) + el `insert` re-hashea la misma clave. Un contador
  `m.insert(k, m.get(k).unwrap_or(0)+1)` paga ~4 allocs + 3 hashes por palabra.
- El Map usa el **SipHash** por defecto de Rust (`src/gc.rs:150`) — anti-DoS, 2–5× más
  lento que FxHash/aHash sobre claves string cortas.
- El lazo de la VM ya está exprimido en su forma actual: el ledger de IDEAS.md §11/§45
  refutó por medición los inlines (Opt.14), registerizar ip (Opt.17), etc. Lo que queda
  es **estructural**: representación de datos y codegen.

## 2. El activo estratégico: tipos estáticos + erasure

La carta que ray tiene y ningún rival del benchmark tiene: **es estáticamente tipado**.
Todo lo que hace difícil un JIT/optimizador de JS/Ruby/PHP — especular tipos, hidden
classes, inline caches polimórficas, deopt storms — aquí es **casi gratis o innecesario**:

- `fn fib(n: int)` puede compilar a enteros nativos con **cero guardas de tipo**.
- El offset de cada campo de struct se conoce **en compilación** (no hay shapes dinámicas).
- El checker ya calcula la sustitución σ en cada sitio de llamada genérica → puede
  **monomorfizar** selectivamente (hoy borra; podría especializar).
- `[int]` puede ser un `Vec<i64>` plano (unboxed), no un arreglo de valores etiquetados.

Además el proyecto tiene la infraestructura de validación perfecta para un backend
nuevo: **dos motores + oráculo conductual + goldens + self-hosting como test de estrés**.
Un backend nativo se valida contra todo eso "gratis". Y las **deps de Cargo son
aceptables** (precedente ring/rusqlite) → Cranelift/ahash están sobre la mesa.

## 3. Los arcos, por ROI

### Arco P0 — matar las alocaciones tontas del camino caliente (barato, semanas)

El 68 % de `wordcount` no es "el intérprete es lento", es **basura por acceso**. Nada de
esto toca la semántica; oráculo intacto.

| # | Propuesta | Detalle | Gana |
|---|---|---|---|
| P0.1 | **aHash en `Obj::Map`** ✅ **HECHA** (14 jul) | alias `MapStore` con `ahash::RandomState` en ambos motores (dep `ahash`, ya transitiva; runtime-rng → resistencia a hash-flooding intacta). **Medido**: neutra sobre el camino con-allocs (el hashing NO dominaba), pero **−4.5% aislada** sobre el camino sin allocs (get_or) — enmascarada por las allocs, aflora al quitarlas (crecerá con P0.3+). Se conserva | −4.5% (crece) |
| P0.2 | **`get_or` sin alocar** ✅ **HECHA** (14 jul) | opcode `MapGetOr` + primitivo `__get_or` + método de prelude `get_or(m,k,d) -> V`: lookup único, **cero allocs** (vs `get(k).unwrap_or(d)`, que aloca el `[V]` + el `Option`). Es la forma idiomática justa (= `dict.get(k,0)` de Python / `Hash.new(0)` de Ruby). **Medido**: wordcount 1011→618 ms SipHash, **590 ms con aHash (−42% del baseline)** | mata 2 allocs/acceso |
| P0.3 | **Upsert en 1 lookup** | el patrón contador `m.insert(k, m.get(k).unwrap_or(0)+1)` hace 2 lookups + 3 hashes → builtin `map_update`/entry-API (u opcode fusionado que detecte el patrón) | 2× en agregación |
| P0.4 | **Interning de strings del split** | `split` produce las mismas palabras millones de veces; internarlas (tabla global `Rc<str>`/símbolos) → comparación por puntero, hash memoizado en la string | grande en parse+map |
| P0.5 | **Hash memoizado** | guardar el hash junto a la string del heap (se calcula 1 vez) | compone con P0.1-4 |
| P0.6 | **Superinstrucciones ronda 3** | histograma dinámico sobre corpus **call-heavy** (fib/selfhost) y **map-heavy** (wordcount); fusionar los pares de `Call`/`Return` y de map. El patrón A4 ya dio −19/−28 % | 5–15 % general |

**Meta P0**: `wordcount` 9.7× → **~3–4×**; `logparse` 4.9× → **~3×**. Sin tocar el modelo.

### Arco P1 — representación de datos (estructural, el "B" del plan viejo)

| # | Propuesta | Detalle | Gana |
|---|---|---|---|
| P1.1 | **NaN-boxing / `HeapValue` en 8 bytes** | hoy 16 B; empaquetar int/float/bool/handle en un u64 NaN-boxed → mitad de tráfico de pila y de memoria, mejor caché | 10–20 % general |
| P1.2 | **Arreglos unboxed tipados** | el checker SABE que `[int]` es de ints → `Obj::IntArray(Vec<i64>)` sin etiqueta por elemento (ídem float). Indexar/sumar sin desempaquetar | grande en datos |
| P1.3 | **Structs por índice (B2)** | `GetField(String)` → `GetFieldIdx(u16)` anotado por el checker pre-erasure; instancia = `Vec<HeapValue>` sin nombres | el ROI del nicho |
| P1.4 | **Strings `Arc<str>` compartidas** | revivir Opt.3 como Arc (traba M38 resuelta); mover/clonar strings deja de copiar | string-heavy |
| P1.5 | **Monomorfización selectiva** | para funciones genéricas calientes (sort, map/filter/fold), emitir la versión especializada por σ en vez de despachar diccionarios | HOF + sort |

**Meta P1**: servicios a **~2× del líder** (liga php/lua); fib/loop ~2× mejor que hoy.

### Arco P2 — codegen nativo: la apuesta grande (la liga de Node/Go de verdad)

Sin esto, el techo es "liga de Python/Ruby buena". Con esto, el techo desaparece. Dos
rutas complementarias — y la (b) es la idea fuera-de-la-caja con mejor razón coste/beneficio:

**P2.a — Method-JIT con Cranelift (tiered)**
- Interpretar en frío; contador de calor por función; JITear las calientes.
- Gracias a los tipos estáticos: **sin especulación ni deopt por tipos** — el JIT de
  raylang es una fracción del coste del de un lenguaje dinámico.
- MVP pragmático: solo funciones **sin puntos de cesión** (hoja numérica: fib, bucles) —
  esquiva fibras-en-código-nativo, lo duro. La VM sigue siendo el motor de todo lo demás
  y el destino de fallback.
- Lo duro de verdad: **root maps** (el GC debe hallar raíces en marcos nativos →
  stackmaps de Cranelift en safepoints) y mantener trazas M79/fuel coherentes (los
  marcos JIT pueden reportar "función JIT" — degradación honesta).
- Esperado: **5–20× en aritmética/llamadas** → `fibrec`/`loopsum` a distancia de node.

**P2.b — AOT: transpilar a Rust (`ray build --native`)** ★ fuera de la caja
- raylang mapea casi 1:1 a Rust: estático, orientado a expresiones, sin null, Result y
  `?` nativos, closures, enums+match. El checker tiene TODA la información de tipos.
- Emitir un crate Rust + `rustc -O` → **binario nativo con la velocidad de Rust**, la
  liga de **Go**, sin escribir un codegen de máquina ni un JIT: rustc hace el 99 % del
  trabajo (registros, inlining, vectorización). GC: `Rc`/arena (el intérprete ya
  demuestra la semántica con `Rc`).
- Se valida con la infraestructura existente: **el oráculo conductual + los 33 ejemplos +
  self-hosting** corren idénticos por el tercer backend, igual que se validó la VM.
- Trade-off honesto: compilar deja de ser instantáneo (necesita rustc) y la concurrencia
  M12/M38 exige diseño (tokio/threads o "no en v1: los programas con spawn corren en la
  VM"). Pero para el nicho servicios —donde el binario se construye una vez y corre
  semanas— es el ajuste perfecto: **dev = VM (arranque 3 ms, ciclo rápido); deploy =
  nativo**. Exactamente el modelo dev/release de… Rust.
- Esperado: benchmarks CPU **por delante de node** (nativo sin warm-up); servicios en la
  liga de Go para cómputo.

**Recomendación**: P2.b primero. Menos I+D que el JIT (nada de root maps ni stackmaps),
reusa un optimizador de clase mundial, y deja el JIT (P2.a) solo si el ciclo
editar-correr nativo importara — que para servicios no.

### Arco P3 — runtime (cuando asome en el perfil)

- **GC**: nursery/bump-allocation para la basura joven (los `Option`/arrays temporales).
  *Nota: si P0 elimina esas alocaciones, puede no hacer falta.* Pausas ya resueltas
  (heap-por-fibra, 0.12 ms).
- **Canales**: Condvar vs busy-poll, sharding — solo con contención real (send_heavy).
- **Multicore ya existe** (M38, pool M:N 3.84×): los benchmarks poliglota son
  single-thread; para throughput de servicio real ray ya escala por fibras — contarlo
  (benchmark de servicio concurrente vs node single-thread sería favorable).

## 4. Más ideas fuera de la caja (backlog abierto)

- **Caché de bytecode `.rayc`**: serializar el chunk compilado → arranque de programas
  grandes aún más imbatible (hoy re-parsea todo).
- **Perfilador integrado** `ray run --profile`: histograma de opcodes/funciones del
  usuario (la instrumentación del histograma A4 ya existió; hacerla producto).
- **Arena por request** (estilo frameworks PHP): un scope/fibra cuya basura se libera de
  golpe al terminar — encaja con heap-por-fibra de M38.
- **Escape analysis en el checker**: structs que no escapan → stack, no heap.
- **`const fn` / eval en compilación**: plegar llamadas puras sobre literales (el
  plegado de constantes Opt.12 ya existe; subirlo a funciones).
- **Backend WASM**: mismo esqueleto que P2.b (transpilar) con target wasm32 — abre
  edge/browser y es otro mercado del nicho servicios.
- **BOLT sobre el binario PGO** (post-link layout): PGO ya dio −5/−9 %; BOLT suele
  añadir otro tanto en intérpretes.

## 5. Gobernanza: cómo se trabaja este objetivo

1. **El banco poliglota es el juez** (`~/Desktop/benchmarks`, hyperfine): fibrec ·
   loopsum · jsonserialize · logparse · wordcount + arranque. Tabla completa antes/después
   de cada arco.
2. **Presupuesto de arranque**: ≤ 5 ms es un activo de marca — gate de regresión.
3. **Oráculo y goldens siempre verdes**; cada optimización = commit propio con su
   medición (formato del ledger §11/§45 de IDEAS.md).
4. **Secuencia propuesta**: P0 (semanas, mata el 68 % del Map) → P1.2+P1.3 (datos) →
   **decisión P2.b** (el salto de liga) → P1.1/P2.a solo si aún hacen falta.

## 6. Metas numéricas (contra el benchmark del usuario)

| Hito | fibrec | loopsum | wordcount | jsonserialize | logparse |
|---|---|---|---|---|---|
| hoy | 12.5× | 8.3× | 9.7× | 2.9× | 4.9× |
| **P0.1+P0.2 (medido 14 jul)** | — | — | **5.4×** | 2.8× | **3.5×** |
| post-P0 (con P0.3+ interning) | ~11× | ~8× | **~3.5×** | ~2.5× | **~3×** |
| post-P1 | ~6× | ~4× | ~2.5× | ~2× | ~2× |
| post-P2.b (nativo) | **<1× (bate a node)** | **<1×** | **~1×** | **~1×** | **~1×** |

*(post-P2.b se compara el binario `--native`; el modo VM conserva su perfil para dev.)*
