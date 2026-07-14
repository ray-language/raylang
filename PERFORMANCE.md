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
| P0.3 | **Upsert en 1 lookup** ✅ **HECHA** (14 jul) | opcode `MapAdd` + builtin público `add_to(m, k, delta)` (ad-hoc int/float, entry-API): `m[k] += delta` en UN lookup, frente a `get_or(k,0)+insert(k,...)` que hashea/busca/clona-la-clave dos veces. Es la acumulación de servicios (contar/sumar), = `h[k]+=1` de Ruby / `$h{k}++` de Perl. **Medido**: wordcount 572→338 ms (**−41%**), logparse 214→169 ms (−21%). Oráculo `map_add_to_oracle` | 2× en agregación |
| P0.4 | **Allocador mimalloc** ✅ **HECHA** (14 jul) — *el interning se descartó por medición* | Atribución tras P0.3: en `wordcount` el **`split` es el 82%** (279 de 340 ms) y el Map solo 30 ms → el interning (que ayuda a comparar/almacenar en el Map, no a trocear) **no** era el lever. El `split` es puro **malloc churn** (1.8M `String` pequeños; el libmalloc de macOS es lento). `#[global_allocator] mimalloc` (cfg no-wasm) lo ataca de raíz, **sin cambio semántico** (oráculo intacto). **Medido**: wordcount −18%, logparse −17%, **jsonserialize −21%** (todos), arranque intacto (3.8 ms) | −17 a −21% global |
| P0.5 | **Hash memoizado** | guardar el hash junto a la string del heap (se calcula 1 vez) | compone con P0.1-4 |
| P0.6 | **Superinstrucciones ronda 3** ✅ **HECHA** (14 jul) | Histograma dinámico de pares ejecutados (instrumentación temporal, revertida): el par MÁS caliente era la **guarda** `[GetLocalConst, CmpJump]` (fib: **18.5M**; A4 fusionó `Cmp;JumpIfFalse;Pop`→`CmpJump` pero no el `GetLocalConst`). Nuevo opcode `GetLocalConstCmpJump` (lee local+const, compara y salta sin apilar) + pase `fuse_guard_round3` (tras `fuse_round2`, mismo remapeo de saltos). Solo VM → oráculo intacto (test `guard_fusion_round3_oracle`). **Medido (A/B fusión on/off)**: **fibrec +11%**, loopsum +2.5%; servicios neutros (±2% ruido de LAYOUT del match — el op apenas se ejecuta ahí; PGO lo absorbe) | +11% fib |

**Meta P0**: `wordcount` 9.7× → **~3–4×**; `logparse` 4.9× → **~3×**. Sin tocar el modelo.

### Arco P1 — representación de datos (estructural, el "B" del plan viejo)

| # | Propuesta | Detalle | Gana |
|---|---|---|---|
| P1.1 | **NaN-boxing / `HeapValue` más pequeño** ❌ **EVALUADO y DESCARTADO** (14 jul, medido) | Dos hallazgos: (1) **inviable** — `HeapValue` mide **32 B** (no 16; lo domina el `String`/`Vec` inline de `Str`/`Bytes`), y raylang tiene **`int` de 64 bits** → un `i64` NO cabe en los ~48 b del payload NaN (habría que romper la semántica a 63 b o mover `Str` al heap, que *sube* el GC en el nicho string-heavy). (2) **no ayudaría** — experimento de sensibilidad (variante de padding, `HeapValue` 32→40 B): **agrandarlo NO ralentizó** (fib incluso −4%, resto ±ruido). El tamaño del valor NO es el cuello: mover 32 B por push/pop es gratis en el M3 (L1 caliente), la misma verdad de HW que refutó Opt.17. |
| P1.2 | **Arreglos unboxed tipados** | el checker SABE que `[int]` es de ints → `Obj::IntArray(Vec<i64>)` sin etiqueta por elemento (ídem float). Indexar/sumar sin desempaquetar | grande en datos |
| P1.3 | **Structs por índice (B2)** | `GetField(String)` → `GetFieldIdx(u16)` anotado por el checker pre-erasure; instancia = `Vec<HeapValue>` sin nombres | el ROI del nicho |
| P1.4 | **SSO de strings (`compact_str`)** ❌ **EVALUADO y DESCARTADO** (14 jul, medido) | Se implementó entero: `HeapValue::Str(CompactString)` (VM), inline ≤24 B, `Send` (sortea la traba 4 de Opt.3). **Medido (A/B vs no-SSO, best-of-15)**: wordcount **+2.7%**, split-aislado +5.9%, pero **jsonserialize −9%**. Neto negativo. **Causa**: **mimalloc (P0.4) ya se comió el almuerzo** — el malloc ya es barato (~30 ns), así que evitarlo apenas gana; y el branching inline-vs-heap de `CompactString` penaliza los strings **medianos** (los registros JSON de ~40 B van al heap igual y se construyen más lento). Espeja el rechazo de Opt.3 (`Rc<str>`). Revertido (churn de 119 sitios + dep, sin premio). |
| P1.5 | **Monomorfización selectiva** | para funciones genéricas calientes (sort, map/filter/fold), emitir la versión especializada por σ en vez de despachar diccionarios | HOF + sort |

**Meta P1**: servicios a **~2× del líder** (liga php/lua); fib/loop ~2× mejor que hoy.

**VEREDICTO DEL ARCO P1 (14 jul, medido)**: la **representación de datos es un callejón sin salida en
este hardware**. P1.4 (SSO) salió neto-negativo (mimalloc ya comió la asignación) y P1.1 (encoger
`HeapValue`) no ayuda (el tamaño del valor no es el cuello; mover 32 B es gratis en el M3) además de
ser inviable con `i64`. Confirmado tres veces (Opt.17, P1.4, P1.1): **en Apple Silicon los accesos a
memoria caliente y el tráfico de valores son gratis**; lo único que mueve la aguja es ejecutar **MENOS
instrucciones** (superinstrucciones — P0.6 dio fib +11%, el único win real de este tier) o cambiar el
**modelo de ejecución** entero. Quedan como *quizá* solo los que cambian el MECANISMO, no el tamaño:
- **P1.2 arreglos unboxed** (`Obj::IntArray(Vec<i64>)`): evita construir/destruir un `HeapValue` por
  elemento en bucles numéricos sobre `[int]`/`[float]` — pero eso es cómputo, no el nicho de servicios.
- **P1.3 structs por índice** (`GetFieldIdx`): cambia el acceso a campo de buscar-por-nombre a
  indexar — mecanismo distinto, no tamaño; el único P1 con ROI plausible para servicios.

Pero el salto real de aquí en adelante es **P2** (nativo/JIT): borra el bucle de despacho entero, que
es lo único que el hardware SÍ cobra. **Recomendación**: no invertir más en P1 salvo P1.3 puntual;
evaluar P2 (transpile-a-Rust primero, menor I+D que el JIT).

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

#### ✅ SPIKE de P2.b COMPLETO (14 jul 2026) — tesis PROBADA

Prueba de concepto en `src/transpile.rs` (+ flag `ray --emit-rust <archivo>`, + 3 tests): transpila el
**subconjunto escalar** de raylang (funciones, `int`/`float`/`bool`, aritmética, `if`/`while`/`for`-rango,
recursión, `print`) a Rust, se compila con `rustc -O` y se ejecuta. Salida byte-idéntica a la VM.

**Medido** (best-of-7, M3; ray-native = transpilado+`rustc -O`):

| | ray-native | node | php | ray-VM |
|---|---|---|---|---|
| **fibrec** | **16 ms 🥇** | 89 ms (5.4×) | 272 ms (17×) | 1000 ms (**61×**) |
| **loopsum** | **33 ms 🥇** | 324 ms (9.8×) | 93 ms (2.8×) | 808 ms (**24×**) |

El nativo es **24–61× más rápido que la propia VM** y en fib **le gana a node (V8 JIT) por 5.4×**. raylang
pasa de peor-de-la-clase (fib 12.5× tras node) a **mejor-de-la-clase** — un giro de ~68×. `rustc -O` de
un programa pequeño: **0.03 s** (el modelo dev=VM / deploy=nativo, como Rust). Es código máquina real.

**Conclusión**: P2.b es EL camino, validado. La tesis "raylang mapea a Rust → velocidad nativa" es cierta
y espectacular para el núcleo de cómputo. Confirma el veredicto de P0/P1: lo único que el HW cobra es el
bucle de despacho, y P2.b lo **borra**.

#### Fase 2 — strings (14 jul, arco P2.b en marcha)

Extendido el transpilador a **strings** (`Type::String` → `Rc<str>`; concat, `to_string`, `len`, params/
retorno). El modelo de valores: escalares *unboxed*, heap en `Rc` (clon-al-leer = bump O(1), *sound* para
la semántica de valor de raylang sobre la de movimiento de Rust). Entorno de tipos propio (params + `let`
inferido) decide qué clonar. UFCS/métodos manglados (`__len`, `string#…`) se normalizan al método real.

**Lección de codegen (medir-primero también aquí)**: la bajada NAÍF de strings (`Rc::from(format!())` por
cada `+`, ~2N allocs por cadena) salió **0.7× — más LENTA que la VM** (mimalloc + opcodes de string son
eficientes). El fix decisivo: **aplanar la cadena de concat en UN solo `format!`** (`"user"+to_string(i)+
"-" → format!("user{}-", i)`, inlineando `to_string`) → 2 allocs por cadena en vez de ~2N. Resultado:

| `strbuild` (300k cadenas) | naíf | **aplanado** |
|---|---|---|
| ray-native vs VM | 0.7× (más lento) | **2.4× más rápido** (32 vs 76 ms) |

Un giro de 3.4× por el codegen. **Los strings van nativos y más rápidos que la VM** → P2.b sirve también
al nicho de servicios, no solo al cómputo — PERO requiere lowering LISTO (folding, y a futuro move-analysis
para evitar clones, SSO). El coste de P2.b completo está en esa optimización de codegen, no en la viabilidad.

#### Fase 3 — arreglos → `jsonserialize` ENTERO en nativo (14 jul)

`Type::Array(T)` → `Rc<RefCell<Vec<T>>>` (semántica de referencia + mutación, como el intérprete). Cubre:
literal `[…]`/`[]`, índice lectura/escritura (`a[i]` / `a[i]=v` → `borrow`/`borrow_mut`), `push`, `len`
(rama por tipo), `split`/`join` (helpers del preámbulo generado), `for x in <arreglo>` (itera una copia
del Vec para no retener el borrow) y `for c in <string>` (chars). Bug latente cazado: los *lvalues* ident
no deben clonarse (el `Assign` ahora los emite crudos). Escapado correcto de `format!` (`{`/`}`/`"`/`\`).

**Con esto se transpila `jsonserialize` ENTERO** (arreglos + strings + `to_string` + `join` + for-rango,
sin Map). Salida byte-idéntica a la VM. **Medido — un workload de SERVICIO real (serializar respuestas):**

| `jsonserialize` | ray-native | perl | node | ray-VM | ruby |
|---|---|---|---|---|---|
| tiempo | **41 ms 🥇** | 65 ms | 118 ms | 139 ms | 611 ms |
| vs líder | **1.0× (#1 de 8)** | 1.6× | 2.9× | 3.4× | 14.7× |

**El nativo transpilado es #1 de los 8 lenguajes** —bate a perl/node/php— y **3.4× la propia VM**.

#### Fase 4 — Map → LOS TRES benchmarks de servicio en nativo, #1 de 8 (14 jul)

`Map<K,V>` → `Rc<RefCell<HashMap<K,V>>>`. Cubre: `Map.new`, `insert`, **`add_to`** (`*entry(k).or_insert(0)
+= d`, el upsert de la VM), `get`/`get_or` (→ `Option` nativo, fusionado con `unwrap_or`), `contains_key`,
`keys`/`values` (ordenadas por clave → deterministas, helpers del preámbulo), `sort`, `len`, y
`parse_int(x).unwrap_or(d)` (→ `x.parse::<i64>().ok().unwrap_or(d)`). Gotcha: el parser deja `Map<K,V>`
como `Struct("Map",[K,V])` (el checker lo reclasifica en su tabla, no en la anotación) → `normalize_type`.

**Con esto se transpilan `wordcount` y `logparse` ENTEROS.** Salida byte-idéntica a la VM. **Medición
final del nicho de servicios completo (best-of-7, ray-native = transpilado + `rustc -O`):**

| Benchmark | ray-native | mejor rival | ray-VM | inicio (VM) |
|---|---|---|---|---|
| **jsonserialize** | **41 ms 🥇 #1** | perl 65 | 139 (3.4×) | 2.9× |
| **wordcount** | **93 ms 🥇 #1** | php 108 | 291 (3.1×) | **9.7×** |
| **logparse** | **53 ms 🥇 #1** | perl 63 | 145 (2.7×) | 4.9× |

**LOS TRES benchmarks de servicio: ray-native es #1 de los 8 lenguajes.** `wordcount` —el PEOR al empezar
(9.7× tras php)— ahora **le gana a php**: un giro de ~9×. P2.b lleva el nicho de servicios **entero** de
peor-de-la-clase a mejor-de-la-clase, con salida idéntica a la VM y tests verdes.

**Cobertura del transpilador tras 4 fases**: escalares · control (`if`/`while`/`for`) · recursión ·
strings (`Rc<str>`, concat aplanado) · arreglos (`Rc<RefCell<Vec>>`) · Map (`Rc<RefCell<HashMap>>`) ·
UFCS/métodos manglados · entorno de tipos propio.

#### Fase 5 — structs + enums + match (14 jul)

Tipos de usuario: **struct** → `Rc<RefCell<S>>` (referencia + mutación); **enum** → `Rc<E>` (inmutable,
permite recursión). Cubre: definiciones (Rust `struct`/`enum`), literal de struct, acceso/asignación de
campo (`p.x` / `p.x = v` → `borrow`/`borrow_mut`), construcción de variante (`Rc::new(E::V(..))`),
**`match`** (sobre `&*scrutinee`, bindings clonados a valores propios al inicio del brazo, patrones
variante anidados, `_`, binding-total), y **`impl Display`** por tipo (= el `Show` de raylang: `Name {
f: v }` / `Name.Variant(payload)`) para `print`. Gotcha: el parser deja el tipo enum como `Struct(n)` →
`declare` lo reclasifica a `Enum(n)`. Diferido: `@derive(Eq/Ord)`, patrones struct, guardas de `match`.

**Verificado — 9 ejemplos reales transpilan y dan salida byte-idéntica a la VM**: `structs` (campos,
mutación, **aliasing** por referencia, structs anidados), `inventario` (structs en arreglos), **`enums`**,
**`match_figuras`** (match con payload), **`lista_enlazada`/`lista_recorrido` (enums RECURSIVOS = listas
enlazadas)** + fib/gcd/primes/fizzbuzz. Los ejemplos que aún fallan son de OTRAS features (print de
arreglos, `const` de nivel superior, indexar strings), no de structs/enums.

**Cobertura tras 5 fases**: escalares · control · recursión · strings · arreglos · Map · structs/enums/
match · UFCS.

#### Fase 6 — cierre de flecos (14 jul)

Barrido de huecos pequeños para que **casi todos los ejemplos** transpilen y den salida byte-idéntica a
la VM: literales `char` (`{:?}` de char), casts `as` (int↔float, char↔int), `const` de nivel superior
(→ funciones `NAME()`), **print de arreglos** (`[e0, e1, …]` recursivo para anidados, vía `show_expr`),
indexar strings `s[i]`→char, `chars()`, `assert`/`assert_eq`, literal de Map `[k: v]`, `remove`. Y **dos
bugs reales de aliasing/inferencia**: (1) `p.x = p.x + 1` generaba `borrow_mut()… = …borrow()…` sobre el
MISMO RefCell → doble borrow en runtime; fix: el RHS a un temporal antes del `borrow_mut`. (2)
`type_of(match)` fallaba si el primer brazo usaba un binding del patrón; fix: primer brazo que resuelva.

**Resultado: 15 de 24 ejemplos** (`examples/data` + `examples/basics`) transpilan y coinciden con la VM
byte a byte — TODOS los de estructuras de datos (arreglos, matrices, structs, enums, pilas, **listas
enlazadas**) y básicos (casts, constantes, fizzbuzz, gcd, palíndromo, primes). Los 9 restantes necesitan
**fases de features grandes, no flecos**: Option/Result como enums reales (match sobre `get()`), closures,
genéricos de usuario (monomorfizar `Caja<T>`), tuplas, interpolación de strings, builtins de `std::math`,
`args()`, y el `for (k,v)` sobre Map.

#### Fase 7 — Option/Result + `?` (14 jul)

Estilo idiomático de errores en nativo, **mapeando `Option<T>`/`Result<T,E>` a los NATIVOS de Rust**
(genéricos gestionados por rustc, sin monomorfizar): `Option.Some(x)`→`Some(x)`, `Option.None`→`None`,
`Result.Ok/Err`→`Ok/Err`; `match` sobre Option/Result (patrones `Some`/`None`/`Ok`/`Err`, sobre `&opt`
sin `Rc`); operador **`?`** → el `?` de Rust; `get`/`remove`/`parse_int` devuelven el `Option` nativo;
`unwrap`/`unwrap_or` nativos. Gotcha clave: el checker inyecta las funciones del prelude (`parse_int`,
`read_int`, `get`, …) en `program.functions`; con `?` ya soportado, `read_int` transpilaba y referenciaba
`input()` (no soportado) → **`is_handled_builtin`** salta TODA función del prelude (lista extraída de
`src/prelude.ray`). Y las colecciones vacías (`[:]`/`[]`/`Map.new()`) no infieren K/V → se **emite la
anotación del `let`** (`let x: T = …`) para pinar la inferencia de Rust.

**17 de 24 ejemplos** transpilan y coinciden con la VM (añade `mapa`, `mapa_literal`). `?` verificado
end-to-end (mismo exit que la VM). Los 7 restantes necesitan **fases grandes**: genéricos de usuario
(`Caja<T>`), closures, tuplas, interpolación de strings, `std::math`, `args()`, `for (k,v)` sobre Map.

#### Fase 8 — closures + map/filter/fold (14 jul)

Estilo funcional en nativo: función-valor `fn(int)->int` → **`Rc<dyn Fn(i64)->i64>`** (invocable directo,
clon barato); función anónima → **closure `move` de Rust** (captura por valor: para los `Rc<RefCell>`
comparte el estado como las celdas del intérprete; los escalares se copian). Llamar a un closure en ámbito
→ `f(args)`. **`map`/`filter`/`fold`** (prelude) → **iteradores de Rust** (`iter().map/filter/fold`, la
closure ligada una vez a `__f`). `print` de una función → `<fn>` (como la VM).

**18/24 ejemplos** (añade `funciones`) + `stdlib` (map/filter/fold) — nativo ≡ VM byte a byte. **Diferido**:
la captura MUTABLE de un escalar (`contador()` que muta `n` capturado, `closures.ray`) diverge — necesita
el análisis de celdas (boxear en `Rc<RefCell>` los locales capturados-y-mutados), como el `captured_slots`
de la VM.

#### Fase 9 — genéricos (funciones + tipos) (14 jul)

**Genéricos vía los genéricos de Rust** (rustc monomorfiza → nativo, sin erasure): `fn id<T>(x: T) -> T`
→ `fn id<T: Clone + Display>(mut x: T) -> T`; `struct Par<A,B>`/`enum Caja<T>`/`enum Lista<T>` (recursivo)
→ structs/enums genéricos de Rust; `Struct(T)` con `T` en ámbito → el genérico `T` (no Rc-envuelto).
**Inferencia de llamadas genéricas por unificación** (`unify`/`subst_type`: liga los params de tipo de la
firma con los tipos de los args, sustituye en el retorno) — para los sitios sin anotación. **Función como
valor** (`aplicar(negar, …)`) → `Rc::new(fn)` (coerciona a `Rc<dyn Fn>`). Sustitución de los params de
tipo del enum/struct en los bindings de `match` y en el acceso a campos (`Caja<int>` → T=int). Bounds de
raylang (`T: Show`) → se emiten `Clone + Display`; los bounds de usuario (traits) → fase futura.

**23/37 ejemplos** (añade `genericos`, `tipos_genericos`, `inferencia`, `errores`, `opcional`) — nativo ≡
VM byte a byte. Los restantes necesitan el **sistema de traits** (`bounds`, `impls_genericos`, `traits`,
`trait_objects`, `metodos_por_defecto`, `operadores`, `anotaciones`/`@derive`) o misc (tuplas,
interpolación, `std::math`, `args()`, enteros con tamaño).

#### Fase 10 — traits (despacho estático + RayShow) (14 jul)

**Traits vía la ERASURE de M9**: el checker ya baja los métodos de trait a funciones mangladas
(`Punto#valor`) + paso de diccionarios (params función-valor); el runtime no sabe de traits. El
transpilador **emite el programa bajado** renombrando `#`/`::` a idents Rust (`mangle`) — los métodos son
funciones, los diccionarios son closures (`Rc<dyn Fn>`, ya soportados). Cubre: traits de usuario, impls
(sobre structs/enums Y builtins como `int`), **despacho estático**, **métodos por defecto**, **bounds**
(dictionary-passing), **impls genéricos**. Se saltan los impls del PRELUDE (Len/StrOps/MapOps sobre
builtins, el struct `Iter`); `eq`/`show`/`less` (Eq/Show/Ord) → `==`/`ray_show`/`<` nativos (guard
`name.contains('#')` para no chocar con una función de usuario homónima). Pieza clave: **`RayShow`** —
un trait propio (Display no vale: los structs son `Rc<RefCell<..>>` y `RefCell` no es Display, y un bound
`T: Display` fallaría) impl'd para todo tipo (builtins + structs/enums generados, recursivo, genérico-
consciente); `print`/`to_string` lo usan y los genéricos lo llevan como bound.

**27/37 ejemplos** (añade `traits`, `bounds`, `metodos_por_defecto`, `impls_genericos`) — nativo ≡ VM byte
a byte. Diferido (features avanzadas): `@derive(Eq)` con diccionarios, operator-overloading (`operadores`),
trait objects (`dyn`, `trait_objects`), `From`-conversion (`?`); + misc (tuplas, interpolación,
`std::math`, `args()`, enteros con tamaño, captura mutable, concurrencia).

#### Fase 11 — tuplas (14 jul)

`(a, b, …)` → **tuplas NATIVAS de Rust** (`(A, B,)`, heterogéneas, por valor — a diferencia del `Vec<T>`
homogéneo; el checker las borra a arreglos en runtime, pero el AST conserva `TupleLit`/`LetTuple`/
`Type::Tuple`, que bajo directo). Cubre: retorno de tupla, literal, acceso `t.0`/`t.1` (llega como `Field`
con nombre numérico → campo nativo, sin borrow), y **desestructuración** `let (q, r) = e;` (`_` descarta,
`var`→`mut`). **29/37 ejemplos** (añade `tuplas`, `interpolacion`).

#### Fase 12 — trait objects (`dyn`) (14 jul)

**El reto arquitectónico del arco**: el checker baja `dyn Trait` a un struct sintetizado `__dyn_T { data,
métodos… }` donde **`data` está tipado `Unit`** (placeholder; en runtime guarda el valor concreto — la VM
lo maneja con su `Value` universal). Ese **borrado de tipos por runtime choca de frente con los tipos
estáticos de Rust** (`data: ()` no puede guardar un `Cuadrado`). Es el ÚNICO sitio donde "emitir el
programa bajado" no basta. **Solución limpia (sin `Box<dyn Any>` ni downcast)**: el objeto dinámico se baja
a un **struct de CLOSURES que capturan el concreto** — un campo `Rc<dyn Fn(args)->ret>` por método (sin
`data`); la coerción envuelve cada método `m: { let __c=<concreto>; move |a| m_concreto(__c.clone(), a) }`,
y el despacho `(r.m)(r.data, a)` → `(r.borrow().m.clone())(a)` (se descarta el arg `data`). Firmas de los
métodos desde `prog.traits`. Cubre métodos por defecto sobre el objeto y `[dyn Trait]`.

**30/37 ejemplos** (añade `trait_objects`) — nativo ≡ VM byte a byte.

#### Fase 13 — `std::math` (14 jul)

El módulo `std/math` (`import std/math; math.sqrt(x)`, `math.PI`) mapea 1:1 a `f64` de Rust — **la VM ya
usa la impl de Rust** (`__sqrt`→`f64::sqrt`), así que emitir el método nativo da el MISMO resultado por
construcción. Las funciones float (`sqrt`/`pow`/`floor`/`sin`/`ln`/…) son wrappers `pub fn` sobre
primitivos `__*`; abs/min/max son **genéricas** (`T: Signed`/`Ord`, con diccionarios). Solución:
**interceptar `std::math::*` en el sitio de llamada** (`emit_math`) → método de `f64`
(`(x).sqrt()`, `(b).powf(e)`, `(x).abs()`, `(a).min(b)`; abs/min/max preservan int|float, ambos con esos
métodos en Rust), y las constantes `PI`/`E` → `std::f64::consts::{PI,E}`. Se **saltan** las funciones/
consts del módulo (`is_handled_builtin`/la emisión de const-fns) — sus wrappers llamarían al primitivo
`__sqrt` inexistente y las genéricas arrastrarían params-diccionario. `type_of` también mapea
`std::math::*` **antes** de la ruta genérica (su FnSig lleva el arg-diccionario `int#less`, que no
tiparíamos). `matematicas.ray` transpila y da salida byte-idéntica a la VM.

**31/37 ejemplos** (añade `matematicas`) — nativo ≡ VM byte a byte.

#### Fase 14 — `args()` (14 jul)

Argumentos de línea de comandos: `args() -> [string]`. Mapea a
`std::env::args().skip(1)` → `Vec<Rc<str>>` en la repr de arreglo (`Rc<RefCell<…>>`). El `skip(1)`
salta el nombre del binario; la VM salta el binario **y** el `.ray` (`argv` tras el archivo) → **equivalen**
(el usuario pasa los mismos argumentos posicionales en ambos). Verificado con `fib.ray` (que lee
`args()[0]` como nº de pasos vía `parse_int`): idéntico a la VM **sin** args (default 10) y **con** arg
(`fib 5`). El resto (`a.len()`, `a[0]`, `parse_int`, `match`) ya estaba soportado.

**32/37 ejemplos** (añade `fib`) — nativo ≡ VM byte a byte.

#### Fase 15 — `for` sobre Map (14 jul)

`for (k, v) in map` (destructuring de pares). El reto es el **orden**: la VM itera un Map por **clave
ordenada** (determinista, como keys()/values()), así que el nativo debe hacer lo mismo. Helper de
preámbulo **`__ray_pairs`** que materializa un `Vec<(K, V)>` ordenado por clave (suelta el `borrow` antes
del cuerpo, que podría mutar el Map — como el `for` sobre arreglo). El patrón tupla baja a `for (k, v) in
__ray_pairs(&map)`; un binder `_` (wildcard) → `_` de Rust. `for_bucles.ray` transpila y da salida
byte-idéntica a la VM (kiwis/manzanas/peras en orden alfabético).

**33/37 ejemplos** (añade `for_bucles`) — nativo ≡ VM byte a byte.

**Cobertura tras 15 fases**: escalares · control · recursión · strings · arreglos · Map · structs/enums/
match · Option/Result/`?` · closures/map/filter/fold · genéricos (fns + tipos) · traits (estático +
defaults + bounds) · tuplas · **trait objects (`dyn`)** · `std::math` · `args()` · **`for` sobre Map** ·
const/char/cast · UFCS. **El transpilador cubre CASI TODO el lenguaje** con salida byte-idéntica a la VM
(33/37 ejemplos). Restan solo 4, cada una una extensión acotada: `@derive(Eq)` con diccionarios,
operator-overloading, `From`-conversion, enteros con tamaño. Sin incógnitas de viabilidad.

**Lo que el spike NO cubre (= el trabajo real de P2.b completo)**: strings, arreglos, structs, enums,
closures, genéricos, `Map`, y sobre todo la **semántica de referencia + GC** (raylang: mark-sweep;
Rust: `Rc<RefCell>`/arena) y la **concurrencia** M12/M38. raylang mapea 1:1 en lo estructural
(struct→struct, enum→enum, `?`→`?`, genéricos→genéricos); lo duro es el GC/aliasing y el runtime de
fibras. Estimación: es un arco grande (comparable a la VM o al self-hosting), pero **sin incógnitas de
viabilidad** tras el spike — es ingeniería, no investigación. Orden sugerido del arco completo: escalares
(hecho) → datos con `Rc` → control (match/closures/`?`) → genéricos (monomorfizar) → GC/aliasing →
concurrencia (o "spawn cae a la VM en v1"). Validación gratis: oráculo conductual + 33 ejemplos +
self-hosting por el tercer backend.

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
| **P0.1+P0.2+P0.3 (medido 14 jul)** | — | — | **3.2×** | 2.7× | **2.7×** |
| **+P0.4 mimalloc (medido 14 jul)** | — | — | **2.7×** | **2.1×** | **2.0×** |
| **+P0.6 guarda + PGO release (medido 14 jul)** | **10.1×** | **8.0×** | **2.5×** | **1.9×** | **2.0×** |

**Arco P0 CERRADO** (release/PGO, cada lenguaje con su acumulador idiomático): en la niche de
servicios ray va **1.9–2.5×** y **bate a Ruby en los tres**, a Python/Lua en dos (puestos 5–6 de 7).
Los micro-CPU (fibrec 10.1×, loopsum 8.0×) mejoraron pero siguen capados: solo los mueve **P2**
(nativo/JIT). Seis pasos, todos medidos, oráculo intacto: aHash · get_or · add_to · mimalloc ·
superinstrucción de guarda (P0.5 no llegó a existir). El método medir-primero redirigió dos de ellos
(aHash no era el cuello; el interning de P0.4 se descartó por `split`=malloc).
| post-P1 | ~6× | ~4× | ~2.5× | ~2× | ~2× |
| post-P2.b (nativo) | **<1× (bate a node)** | **<1×** | **~1×** | **~1×** | **~1×** |

*(post-P2.b se compara el binario `--native`; el modo VM conserva su perfil para dev.)*
