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

> **Re-medido 19 jul 2026 (post-H6)**: la auditoría hizo la aritmética de int **checked por defecto**
> (paridad de semántica con la VM; commit e76c332) y eso cuesta ~25% justo en fib (una suma y dos restas
> por llamada). Hoy (hyperfine, M3 Pro, `ray build --native --release`): default **21.5 ms = 4.2× sobre
> node** (89.3 ms); con `--fast` (aritmética envolvente, la semántica del spike) **17.6 ms = 5.1×**. La
> tabla de arriba queda como registro del spike; la cifra vigente para el build por defecto es **4.2×**.

**Conclusión**: P2.b es EL camino, validado. La tesis "raylang mapea a Rust → velocidad nativa" es cierta
y espectacular para el núcleo de cómputo. Confirma el veredicto de P0/P1: lo único que el HW cobra es el
bucle de despacho, y P2.b lo **borra**.

#### 🏁 ARCO P2.b CONSOLIDADO (15 jul 2026) — 33 fases, todo medido / byte-idéntico a la VM

Del spike escalar al producto: `src/transpile.rs` (~2.6k líneas) + `ray build --native [--release]`. El
transpilador cubre el **lenguaje completo** del corpus (37/37 ejemplos deterministas byte-idénticos) +
**multi-módulo/cápsulas → un binario** + **toda la I/O de `std/fs`** (texto/binaria/handles/dirs/stdin/env)
+ **concurrencia CSP con hilos de SO reales** (spawn/canales/Task/join/scope/select/signals; canales de
int/float/bool/char/string/bytes). Método invariante en las 33 fases: **verificar contra la VM, no asumir**
(el oráculo cazó los bugs de multi-módulo namespacing, `$`-temps, concat-no-Display, colisión del `.rs`
temporal…), y **medir antes de optimizar** (LTO ~10% solo en alloc/Map; PGO descartado). Estado: **581
tests lib + 32 del transpilador + 9 `build_native` verdes**, working tree limpio. **Diferido consciente**
(proyectos grandes con tradeoffs, no huecos): canales de tipos MUTABLES (struct/arreglo/Map → el modelo de
valores thread-safe con el aislamiento de borrows), `try_join`/`Selected`/`cancel`/cancelación M12.5, y la
VM auto-alojada del propio transpilador. **Punto de consolidación**: se retoma el núcleo del lenguaje/VM;
el transpilador queda como un tercer backend de producto, estable y extensible fila-a-fila.

#### Fase 34 — cobertura de builtins de las librerías (eprint + string + bytes_of)

Tras consolidar, se retoma "las implementaciones nativas que faltan" para que **los ejemplos de librería**
(`stdlib/`/`web/`/`net/`/`io/`) transpilen. Escaneo de bloqueos → el nº1 era **`eprint`** (17 ejemplos):
añadido (→ `eprintln!`, stderr; comparte el arm con `print`). Luego **`bytes_of([int]) -> bytes`** (5) y
el resto de **string builtins** que estaban en la lista de skip pero nunca se emitían (el corpus no los
ejercitaba): `trim` (→ `str::trim`), `to_upper`/`to_lower` (Unicode, `to_uppercase`/`to_lowercase`),
`starts_with`/`ends_with` (→ bool), `repeat` (n≤0→""), `replace`, `substring` (corte por carácter con
clamp). Todos mapean 1:1 a métodos de `str`/`String` de Rust con la misma semántica que la VM (verificado
byte a byte). **Ejemplos que transpilan: 40 → 62** (de `stdlib/web/net/io`; los `missing main` restantes son
archivos-LIBRERÍA sin `main`, se importan, no aplican). Corpus 37/37 intacto. Restan bloqueos menores:
`indexar bytes` (`b[i]`), protocolo `Iterator<T>`, `spawn` no-literal, canales de tipos mutables.

#### Fase 35 — indexar bytes (`b[i]`)

`b[i]` sobre `bytes` → el octeto como **int** (checker: `Type::Bytes` indexado → `Int`). Como `bytes` es
`Rc<[u8]>` (sin `RefCell`), se indexa directo: `(<b>[<i> as usize] as i64)` — con paréntesis porque el
`as i64` no puede ir seguido de un método (`.ray_show()`), y sin `.borrow()` (a diferencia de arreglos).
OOB → pánico (equivale al error de runtime de la VM; el camino feliz casa byte a byte). Verificado
(recorrer `"Hi!".to_bytes()` → 72/105/33, suma 210). Librería emit OK: 62 → 63 (desbloquea el ejemplo de
hash que indexa bytes). Corpus 37/37 intacto.

#### Fase 36 — time/RNG + char_code (y `std/time` transpila entero)

Builtins de reloj y aleatoriedad, **no deterministas** → el nativo casa por PROPIEDADES (rango/tipo), no
byte a byte (como en la VM: la semilla del PRNG viene del reloj). `std::time::now`/`monotonic` → int
(millis; `SystemTime`/`Instant`), `sleep(ms)` → `thread::sleep`; `std::random::next` → float [0,1),
`below(n)` → int [0,n), `seed(n)` fija la semilla — con un **PRNG SplitMix64 propio** (el MISMO que la VM)
de estado global (`Mutex<u64>` sembrado del reloj), emitido bajo `needs_time_rng`. Interceptados por
prefijo como `std::math`/`std::fs`, pero **solo las funciones-primitivo**; el resto de `std/time`/
`std/random` (helpers de `DateTime` en raylang puro) se emiten normal. **Bug de cadena cazado**: `import
std/time` arrastra TODO el módulo; una función (`campo`) usaba **`char_code`** (no soportado), y por el
diseño de *silent-skip* (una función no-main que no transpila se salta callada) quedaba una **llamada
colgante** desde una función SÍ emitida (`parse_iso8601` → `campo`/`parse_iso8601_millis`) → `rustc`
fallaba. Fix: soportar **`char_code(c)` → `(c as u32 as i64)`** y **`char_from_code(n)` →
`char::from_u32(n as u32)`** (Option<char>). Con eso **`std/time` transpila y compila ENTERO** (verificado
end-to-end). Nota sobre la cola larga: de 63 ejemplos de librería que EMITEN, 18 COMPILAN — el resto tiene
llamadas colgantes de funciones con builtins aún no soportados (sockets/crypto/regex…), cola larga aparte.
Corpus 37/37 intacto.

#### Fase 37 — sockets TCP (`std/net`)

Red real: `std::net::{tcp_connect,tcp_listen,tcp_accept,socket_read,socket_write,socket_read_bytes,
socket_write_bytes,local_port}` → `std::net::TcpStream`/`TcpListener` de Rust (cero deps). Se **extiende el
registro de handles** (M11.8, compartido con archivos) con dos variantes `Tcp(TcpStream)`/
`Listener(TcpListener)`; el bloque del registro se refactoriza (base común `__RayHandle`/`__RayReg`/
`__ray_reg_insert`/`__ray_close` bajo `needs_handles||needs_net`; ops de archivo bajo `needs_handles`; ops
de socket bajo `needs_net`). Como la VM, los ops de socket **clonan el stream** (`try_clone`) para no
retener el lock del registro durante la I/O bloqueante; `socket_read` lee ≤64 KiB (UTF-8 lossy, EOF→"");
`socket_write` escribe todo (Ok(nº bytes)). `close` de un socket ya funcionaba (handle int → `__ray_close`
→ el `Drop` cierra). Interceptados por prefijo `std::net::` (solo las 8 TCP; TLS/rustls quedan fuera —
dep externa, imposible en un `.rs` standalone). **Verificado end-to-end con binarios nativos**: un servidor
transpilado (listen puerto 0 → imprime el puerto → accept → read → eco → close) y un cliente que conecta,
envía y lee la respuesta — real TCP, round-trip `eco: hola`. Test de integración
`build_native_servidor_tcp_hace_eco` (el test hace de cliente con `std::net`). Librería rustc OK: 18 → 21
(desbloquea los ejemplos TCP-sin-TLS). Corpus 37/37 intacto. Diferido: TLS (rustls), UDP,
`set_read_timeout`, y la cesión no-bloqueante de M15.5 (innecesaria con hilos de SO reales).

#### Fase 38 — `index_of` (desbloquea hex/base64/url/uuid/json…)

`index_of(s, sub) -> Option<int>`: índice **por CARÁCTER** de la primera aparición de la subcadena (sub
vacío → Some(0); no encontrado → None). Rust `str::find` da índice de BYTE, así que el helper de preámbulo
`__ray_index_of` compara por `char` (como la VM `char_index_of`). Estaba en la lista de skip pero nunca se
emitía en los sitios de llamada (como trim/substring). **El mayor desbloqueo de la cola larga hasta ahora**:
`index_of` es el bloque nº1 (18+ referencias) — lo usan hex/base64/url/uuid/json y sus demos para decodificar
caracteres (`match (index_of("0123…", c)) …`). Como esas funciones (`hex_val`, `dec_char_std`…) fallaban al
transpilar, el *silent-skip* dejaba llamadas colgantes en cadena. Con index_of, **librería rustc OK: 21 →
32** (+11). Verificado byte a byte (incl. por carácter: `index_of("murciélago","élago")` → 5;
`base64_demo` completo ≡ VM). `uuid` compila pero difiere (genera un UUID ALEATORIO → no determinista, como
random). Corpus 37/37 intacto.

#### Fase 39 — diagnóstico de la cola larga + contains/parse_float/set_read_timeout + fix ICE

Para atacar la cola larga con datos, se añade **`RAYLANG_TRANSPILE_DEBUG`**: reporta cada función no-main
que el *silent-skip* descarta y por qué (`[transpile skip] <fn> — <error>`). El escaneo reveló los bloqueos
top: `__tls_*` (fuera, rustls), `__contains` (19), `__socket_set_read_timeout` (19), `parse_float` (bug),
UDP, crypto. Añadidos: **`contains`** ad-hoc (string→subcadena `str::contains`, arreglo→pertenencia
`.iter().any(== )`, bytes→subsecuencia `windows`), **`parse_float`** (faltaba en emit_call —solo estaba en
type_of— → `.parse::<f64>().ok()`), **`set_read_timeout`** (socket op → `TcpStream::set_read_timeout`).
**Bug real (ICE) cazado y arreglado**: `get_or(q, "x")` con **2 args** (no el del prelude, que lleva 3)
hacía `panic` por `eff[2]` inexistente → ahora el arm se guarda con `eff.len() == 3` y una aridad distinta
cae al fallback (error limpio, no crash). Test de regresión. Librería rustc OK: 32 → 33 (los que usan
`contains` suelen usar además TLS/crypto). Corpus 37/37 intacto.

#### Fase 40 — UDP (`std::net::UdpSocket`)

Sockets UDP: los primitivos `__udp_bind`/`__udp_send_to`/`__udp_recv_from` (que envuelven los wrappers de
raylang de `udp.ray`) → `std::net::UdpSocket`. Se extiende `__RayHandle` con `Udp(UdpSocket)` (5.ª variante)
y `local_port` cubre las tres (Tcp/Listener/Udp). Los primitivos devuelven **arreglos etiquetados** (como
`__read_file`): `bind`/`send_to` → `["ok", n]`/`["err", msg]` (strings); `recv_from` → `[b"ok", host, port,
data]`/`[b"err", msg]` (bytes, que los wrappers decodifican a un `Packet`). `recv_from` es **BLOQUEANTE**
(con hilos de SO reales, un hilo bloqueado en `recv_from` es gratis; la VM usa no-bloqueante + scheduler
para no atascar su M:1 → mismo efecto observable). Clonan el socket (`try_clone`) para no retener el lock.
**Verificado end-to-end**: un servidor UDP transpilado (bind puerto 0 → imprime el puerto → recv → eco al
remitente) y un cliente `UdpSocket` — datagrama de ida y vuelta `hola udp`. Test de integración
`build_native_udp_hace_eco` (el test es el cliente). Librería rustc OK: 33 → 34. Corpus 37/37 intacto.
Diferido en red: TLS (rustls), crypto (`ring`) — deps externas, techo estructural de los ejemplos web.

#### Fase 41 — crypto en raylang puro (fix del doble-borrow en `push`)

Exploración: sha1/sha256/sha512/hmac/chacha20/poly1305 están escritos en **raylang PURO** (cero primitivos
`__`, cero `ring`) → deberían transpilar. `chacha20_demo` ya salía idéntico; pero **sha256/sha512/hmac
COMPILABAN pero PANICABAN en runtime** con `RefCell already mutably borrowed`. **Bug de corrección real
cazado**: `w.push(w[i] + w[j])` (patrón ubicuo en cripto) se emitía `w.borrow_mut().push(<arg>)`, y Rust
evalúa el **receptor primero** → `borrow_mut()` retiene el préstamo mientras el `<arg>` hace `w.borrow()`
→ pánico (doble borrow del RefCell). Fix (como en las asignaciones `a[i]=a[j]`): el valor de `push` se
evalúa a un **temp ANTES** del `borrow_mut` (`{ let __v = <arg>; a.borrow_mut().push(__v); }`). Con eso
**sha256/sha512/hmac/chacha20 → byte-idénticos a la VM** (hashes correctos): crypto de verdad en el binario
nativo, sin deps. Es un fix de RUNTIME (esos demos ya *compilaban*), no cambia el conteo de rustc pero sí la
CORRECCIÓN. Test de regresión `push_que_lee_el_mismo_arreglo_no_doble_borra`. Corpus 37/37 intacto.
Diferido: crypto vía `ring` (ed25519, aead con nonce del SO), TLS.

#### Fase 42 — enteros con tamaño (`u64`) sin corromper el prelude (colisión de posiciones)

`poly1305_demo` (MAC con aritmética `u64`, *showcase* de enteros con tamaño) COMPILABA mal: `rustc`
rechazaba `string#hash` del prelude (`let mut h = (17i64 as u64)` mezclado con `i64`). Causa raíz —
**colisión de posiciones**, misma clase que el bug de L3 (loader): la lowering de literales uint del
checker (M28.3b) indexa por `(línea, col)` sobre el programa **fusionado**. El loader ya da a cada
módulo de usuario una **banda de líneas disjunta** (`shift_program`), pero **el prelude se inyecta
DESPUÉS con sus líneas propias** (1..). Un literal `u64` de `poly1305.ray` (desplazado) caía en la misma
`(260, 17)` que el `17` de `var h = 17` de `string#hash` del prelude → la lowering por posición lo
envolvía como `u64`. El transpiler lo destapó porque **emite TODAS las funciones** (incluidas las
muertas: el `string#hash` del prelude nunca se llama en poly1305, así la VM lo toleraba). Era además un
bug latente en la VM/intérprete: si `string#hash` se usara (p. ej. `Set<string>`) con la colisión,
hashearía con semántica `u64` errónea. **Fix**: el prelude se desplaza a su **banda disjunta**
(`prelude::LINE_BASE = 1e9`, `loader::shift_program` hecho `pub`) al parsearse → posiciones globalmente
únicas; arregla de golpe TODAS las lowerings por posición (uint + UFCS/dicts/`dyn` de M9). Con esto
**poly1305 transpila byte-idéntico a la VM** → **todo el crypto en raylang puro nativo** (19/19 de los
web-demos deterministas no-servidor: sha256/512, hmac, chacha20(+poly1305 AEAD), ed25519, poly1305,
scram, sigv4). Tests: `el_prelude_va_en_su_banda_de_lineas_disjunta` (invariante) +
`build_native_enteros_con_tamano_no_corrompen_el_prelude` (poly1305 nativo ≡ VM). 591 tests lib + corpus
37/37 intactos. Restan sin transpilar (no crypto): json/jwt/jwt_eddsa/protobuf/framework (rustc) y
url_demo (emit).

#### Fase 43 — `RayShow` para `Map` (desbloquea JSON + JWT)

`json_demo`/`jwt_demo`/`jwt_eddsa_demo` no compilaban: `error: no method named 'ray_show' for
Rc<RefCell<HashMap<Rc<str>, Rc<json::Json>>>>`. Causa — el enum `Json` tiene la variante `JObject(Map<
string, Json>)`, y el transpiler **emite el `RayShow` de TODOS los enums de usuario** (aunque no se
impriman; mismo patrón de "código muerto emitido"): el de `Json` recurre a `.ray_show()` sobre el campo
Map, pero **no existía `impl RayShow for Map`**. `print(map)` directo lo veta el checker, PERO el
runtime SÍ renderiza un Map contenido en un struct/enum (`Value::Map` Display): `Map{k: v, …}` con los
pares (renderizados) **ordenados como cadena** → determinista pese al `HashMap`. **Fix**: emitir
`impl<K: RayShow + Hash + Eq, V: RayShow> RayShow for Rc<RefCell<HashMap<K,V>>>` con ese formato exacto
(clave vía `k.ray_show()` = string sin comillas, como `k.to_value().to_string()` de la VM). Verificado
conductualmente (imprimir `Json.JObject`/Map con claves string e int → byte-idéntico a la VM, no solo
que compile). Con esto **json/jwt/jwt_eddsa transpilan byte-idénticos** → 22/22 web-demos deterministas
no-servidor. Tests: `emite_rayshow_para_map` (unit) + `build_native_json_con_map_coincide_con_la_vm`
(json nativo ≡ VM). Corpus 37/37 intacto. Restan (no JSON/crypto): `framework`/`protobuf` (rustc),
`url_demo` (emit).

#### Fase 44 — protobuf + url (concat de bytes/arreglos, override de builtin, `len` por caracteres, Map de función, `<fn>`)

Cerrando el lote web. Cinco arreglos, cada uno **verificado aislado contra la VM** (no solo que compile):
- **Concat de bytes y arreglos** (`a + b`): caían al `+` genérico, que Rust rechaza (`Rc<[u8]>`/`Vec` no
  tienen `Add`). Bytes → `Rc::<[u8]>::from([&*a, &*b].concat())`; arreglos → arreglo nuevo con los
  elementos clonados de ambos. Desbloquea **`protobuf_demo`** (codec lleno de concat de bytes). ✓ ≡ VM.
- **Override de un builtin del prelude**: si el usuario **redefine** una función homónima de un builtin
  manejado (p. ej. su propio `get_or(m, k)` de 2 args), antes se descartaba como si fuera el del prelude
  → la llamada quedaba sin destino (`'get_or' no soportada`). Ahora `skip_fn_def` solo salta la versión
  del **prelude** (`line >= LINE_BASE`); una redefinición de usuario (línea normal) se emite. Los `std::*`
  (con `::`) siguen saltándose siempre.
- **`len(string)` cuenta CARACTERES, no bytes** (`.chars().count()`): con UTF-8 multibyte (`más`, `ñ`) el
  `.len()` en bytes hacía que `while i < len { s[i] }` se pasara e indexara fuera de rango (panic). Bytes
  sí cuenta octetos.
  Estos dos desbloquean **`url_demo`** (redefine `get_or` + recorre strings UTF-8). ✓ ≡ VM.
- **`get`/`remove`/… sobre un Map de una FUNCIÓN**: `type_of` devolvía el retorno guardado **sin
  normalizar** → `get(mkmap(), k)` veía `Struct("Map")` (no `Type::Map`) y hacía silent-skip. Se
  normalizan los tipos de la firma (`normalize_type`) al registrar `self.funcs`. Win general.
- **Campo/payload de tipo función → `<fn>`**: el `RayShow` generado de un struct/enum con un campo función
  llamaba `.ray_show()` sobre `Rc<dyn Fn…>` (inexistente). Ahora esos campos se renderizan con el literal
  `<fn>` (como el Display del runtime; las firmas variadas impiden un impl único). Win general.

Resultado: **24/25 web-demos deterministas no-servidor byte-idénticos** (0 DIFF, 0 CRASH). Único restante:
**`framework_demo`** — **techo estructural TLS**: usa `std::net::tls_accept`/`tls_connect` (rustls, crate
externo) → imposible en un `.rs` autónomo, como la nota TLS/ring. (Sus sub-gaps residuales —`ray_show`
de un `[fn]`, llamada a un handler-closure— quedan tras ese techo.) Tests: 4 unit (`concat_de_bytes_y_
arreglos`, `len_de_string_cuenta_caracteres`, `una_funcion_de_usuario_gana_a_un_builtin_del_prelude`,
`campo_funcion_se_muestra_como_marcador`) + 2 integración (`build_native_protobuf…`/`build_native_url…`
nativo ≡ VM). 597 tests lib + corpus 37/37 intactos.

#### Fase 45 — stubs que panican para funciones no transpilables (dead-code) — el mayor desbloqueo

Antes, una función no-main que no transpilaba (usa un primitivo TLS, `for` sobre iterador, etc.) se
**OMITÍA**, y sus llamadas quedaban **colgantes** → rustc fallaba **aunque el flujo real nunca la llamara**.
Caso testigo: `http_demo` habla **HTTP plano** (ya soportado) pero importa el módulo `http`, que arrastra
`std::net::tls_connect`; una sola referencia colgante a esa función tumbaba TODO el binario. **Fix**:
`emit_stub` emite la función con su **firma declarada** y cuerpo `panic!(...)` (el `!` de `panic!` encaja
con cualquier retorno). Así el programa **COMPILA**; si el flujo real no alcanza el código no soportado,
corre **idéntico a la VM**; si lo alcanza, **panica con un mensaje claro** en runtime (mejor que un error
críptico de rustc por llamada colgante). Si ni la firma es representable (raro), se omite como antes.
**Impacto medido** (web-demos deterministas): **de 24 → 38 byte-idénticos**, **0 DIFF, 0 CRASH** (ninguno
de los previamente-idénticos regresó → confirma que lo que se convierte en stub es genuinamente inalcanzable
en los caminos deterministas). Los 14 nuevos son "servidores"/clientes self-contained que en realidad
computan protocolo/encoding sin peer vivo: `http`/`http2`(×4)/`https`/`websocket`(+client)/`grpc_call`/
`oauth2`/`dns`(+cache)/`redis`/`postgres`/`udp`. Tests: `una_funcion_no_transpilable_queda_como_stub_que_
panica` (unit) + `build_native_http_con_tls_no_alcanzado_coincide_con_la_vm` (integración). Corpus 37/37
intacto. Restan fuera del subconjunto en su **`main`** (el stub no aplica): `metrics_server_demo` (canal de
struct no-Send), `udp_yield_demo` (`spawn` de fn nombrada), y por rustc: `framework`/`webserver`/
`https_server` (bug de captura `t` en spawn + `ray_show` de un `[fn]`; + techo TLS de `webserver`/`https`).

#### Fase 46 — closures que MUTAN su captura (celdas `Rc<RefCell>`, B1)

En raylang una closure captura POR REFERENCIA y puede **mutar** la variable capturada (patrón contador:
`var n` que la closure incrementa entre llamadas; `contador()` da contadores independientes). En Rust un
`move ||` es `Fn` inmutable → mutar una captura no compila (`cannot assign to captured variable`). **Fix**
(espejo de la semántica M4): una `var` **capturada por una closure** vive en una celda `Rc<RefCell<T>>` —
se lee con `.borrow().clone()`, se escribe con `.borrow_mut()` (el RHS a un temp antes, para no doble-
borrar en `n = n + 1`), y la closure **pre-clona** el `Rc` antes del `move` (`{ let n = n.clone(); Rc::new(
move || …) }`) → mutación **compartida bidireccional** (el enclosing ve la mutación de la closure y
viceversa). Análisis: `cell_vars(body)` = { `var` declaradas } ∩ { idents referenciados dentro de alguna
closure } (recorre `statements` **y el `tail`** del bloque; no desciende a cuerpos de closures anidadas,
que tienen su propio análisis). Una `var` **no capturada** sigue como `let mut` normal (cero coste).
Desbloquea `examples/stdlib/closures.ray` (contadores + captura transitiva) → byte-idéntico a la VM.
Verificado además: mutación compartida bidireccional + contadores independientes + var no-capturada
plana. Tests: `var_capturada_y_mutada_por_closure_va_en_una_celda`, `var_mutable_no_capturada_sigue_
siendo_local_normal` (unit) + `build_native_closures_con_estado_mutable_coincide_con_la_vm` (integración).
598 tests lib + corpus 37/37 + 38 web-demos idénticos, 0 regresiones.

#### Fase 47 — `for` sobre iteradores (B2): Iterator de usuario + adaptadores escalares del prelude

`for x in it` sobre un `impl Iterator<T>` (M40.2) no transpilaba. **Fix** en varias piezas, todas
verificadas contra la VM:
- **`ForIter::Iter`**: `for x in it` baja a `{ let __it = it; loop { match next(__it.clone()) { Some(x) =>
  {cuerpo}, None => break } } }`. El tipo del elemento sale de unificar el tipo del iterador con el `self`
  de `next` y sustituir en su retorno `Option<T>` (funciona para adaptadores genéricos). Variante de
  patrón-tupla para `Some((a, b))`.
- **Campo-closure** (`self.step()`): llamar un campo de tipo función → `(r.borrow().f.clone())(args)`
  (prioridad campo→método). Es el linchpin del `Iter<T>` del prelude (`{ step: fn() -> Option<T> }`, cuyo
  `next` es `self.step()`).
- **`Iter<T>` del prelude se EMITE** (antes se omitía): ahora `iter`/`range` (que capturan+mutan su cursor)
  son transpilables gracias a B1; se quitan de la lista de builtins-omitidos y los métodos `Iter#*` dejan
  de saltarse. Guardas `!name.contains('#')` en `map`/`filter`/`fold` para NO confundir la función libre
  sobre `[T]` con el método `Iter#map` (que cae al despacho de método).
- **`'static` en los genéricos** (`<T: Clone + RayShow + 'static>`): un valor genérico puede acabar en un
  `Rc<dyn Fn…>` (el `Iter<T>`) que lo exige; SIEMPRE cierto en raylang (todo es `Rc`/`Copy`, sin préstamos).
- **`RayShow` para tuplas** (2/3): satisface el bound de un `Iter<(k, v)>` (aun cuando `enumerate`/`zip`
  queden como stubs).
Cubre: `impl Iterator` de usuario + `.iter()`/`range`/`.map()`/`.filter()`/`.take()`/`.skip()` (escalares),
byte-idéntico a la VM (`examples/stdlib/iterador_escalar.ray`). **Diferido**: `.enumerate()`/`.zip()`
(entregan tuplas; la inferencia genérica a través de la cadena de adaptadores no resuelve el tipo del
elemento — el `iterador.ray` completo los usa). Tests: `for_sobre_iterador_de_usuario_baja_a_un_loop_con_
next`, `campo_closure_se_llama_desenvolviendolo` (unit) + `build_native_iteradores_coinciden_con_la_vm`
(integración). 602 tests lib + corpus 37/37 + 38 web idénticos, 0 regresiones. **B COMPLETO** (B1 + B2).

#### Fase 48 — enumerate/zip: inferencia genérica a través de la cadena de adaptadores

Cierra el diferido de la Fase 47. `for (i, x) in it.enumerate()` (y `zip`) fallaba porque el tipo del
elemento (`Struct("T", [])`, sin resolver) no cruzaba la cadena de adaptadores. **Causa raíz**:
`unify`/`subst_type` (la inferencia genérica del transpilador) **no recurrían en structs genéricos con
args, enums ni tuplas** — sólo en Array/Map/Fn. Así `unify(Iter<T>, Iter<(int, string)>)` no ligaba `T`,
y `subst_type(Option<(int, T)>)` no sustituía dentro de la tupla. **Fix**: ambas funciones recurren ahora
en `Struct(n, args)` (genérico), `Enum(n, args)`, `Tuple(ts)` (y `Channel`/`Task` en subst). Además se
emiten los terminales `sum`/`sum_float` (se quitan de la lista de builtins-omitidos). Con esto **el
`examples/stdlib/iterador.ray` COMPLETO transpila byte-idéntico** — `impl Iterator` de usuario + TODOS los
adaptadores del prelude: `.iter()`/`range`/`.map()`/`.filter()`/`.take()`/`.skip()`/`.enumerate()`/
`.zip()`/`.sum()`/`.fold()`/`.collect()`. El fix de `unify`/`subst_type` es en la inferencia CORE (afecta
toda llamada genérica) → verificado sin regresión: corpus 37/37, 38 web idénticos, 604 tests lib. Tests:
`for_sobre_enumerate_destructura_la_tupla` (unit) + `build_native_iteradores_coinciden_con_la_vm`
(integración, ahora sobre el `iterador.ray` completo). **B2 sin diferidos.**

#### Fase 49 — FFI (M41): llamar a funciones C nativas (`extern "lib" { … }`)

Un `extern "lib" { fn name(params) -> ret; }` declara funciones de una librería C (la VM las resuelve con
dlopen/dlsym). **Transpilación**: por cada extern se emite (1) una declaración `extern "C"` del símbolo
(`__ffi_name` con `#[link_name = "name"]`, tipos de ABI) agrupada por librería con su `#[link(name =
"lib")]` (libc va implícita), y (2) un **wrapper** `fn name(...)` con la firma raylang que **marshala**:
argumentos (string→`CString` VIVO durante la llamada, bool→`c_int`, bytes→`*const u8`, ptr→`*mut
c_void`), llama al símbolo en `unsafe`, y marshala el retorno (`c_int`→bool, `char*`→`Option<bytes>`/
`Option<string>` copiando hasta el NUL y validando UTF-8, `ptr`→`Option<ptr>`). rustc enlaza libm/libc.
**Gotcha de ABI cazado**: un retorno `int` es C **`int` de 32 bits** (como la VM, `RetMold::I32`), NO
`long` — declararlo `i64` deja los 32 bits altos de `rax` con basura y el **EOF -1 de `fgetc` se ve
positivo → bucle infinito**; se declara `c_int` y se extiende el signo (`__r as i64`). Cubre
`examples/ffi/libm.ray` (sqrt/pow/cbrt/hypot de libm + abs/strlen/atoi de libc, con string→char*) y
`examples/ffi/cstrings.ray` (strstr→`Option<string>`, strchr→`Option<bytes>`, fopen→`Option<ptr>`,
fgetc/fclose leyendo un archivo real), byte-idénticos a la VM. `Type::Ptr` → `i64` (dirección opaca).
Tests: `ffi_emite_extern_c_y_wrapper_con_marshalling`, `ffi_retorno_int_es_c_int_de_32_bits` (unit) +
`build_native_ffi_libc_libm_coincide_con_la_vm` (integración). 605 tests lib + corpus 37/37 intactos
(el FFI solo afecta a programas con `extern`). Diferido: `print` de un `ptr` (se vería la dirección, no
`<ptr>`), arreglos/structs por FFI (aridad 0..=3 primitivos, como M41).

#### Fase 50 — flecos no-crate (Paso previo del crate `ray-runtime`, 15 jul)

Antes de la extracción a `ray-runtime` (ver `docs/transpilador-nativo.md`), se cierran los flecos del
transpilador que **no** dependen de crates externos (la lista de la Fase 45). Tres fixes:

1. **Leak de estado al caer a stub**: cuando el cuerpo de una función no transpila, `emit_function`
   propagaba el `Err` con `?` **sin popear los scopes ni deshacer las cells** que ya había declarado →
   sus locales (p. ej. un `Task t`) se **filtraban** al siguiente `emit_function`, cuyo `spawn` capturaba
   ese `t` fantasma (`in_scope_channels`) y emitía `let t = t.clone()` con `t` inexistente en Rust
   (`cannot find value t`). Fix: `emit_function` envuelve a `emit_function_inner` y **restaura el estado
   (scopes/tparams/cells) en TODOS los caminos**. Bug de corrupción real (código roto emitido), no solo
   un hueco de cobertura.
2. **RayShow de un tipo-función dentro de un contenedor** (`[fn]`, `Map<_, fn>`, `(fn, …)`): el `impl`
   genérico de `Vec`/`Map`/tupla exige `T: RayShow` y no había RayShow para `Rc<dyn Fn>` (el caso
   directo `campo: fn` ya se renderizaba `<fn>`, pero no anidado). Fix: se emite un `impl RayShow`
   **concreto** (render `<fn>`, como el Display del runtime) por firma concreta distinta
   (`collect_fn_rayshow` recorre arrays/maps/tuplas/fn; `ty_mentions_tparam` salta las que mencionan un
   param de tipo, no representables como impl concreto). Desbloquea el RayShow de `framework` (campo
   `middlewares: [fn(Ctx, Res) -> bool]`).
3. **`spawn`/`scope` de una función NOMBRADA** de aridad 0 (`spawn(worker)`, no solo un literal
   `fn(){}`): se baja a `move || worker()` (sin captura del ámbito, es top-level); `type_of` resuelve el
   retorno desde la firma. Desbloquea `udp_yield` (que ahora compila y corre byte-idéntico en un caso
   determinista de prueba).

**Estado de los 4 demos del Paso previo**: `udp_yield` → **compila** ✓. `framework`/`webserver` → los dos
bugs nombrados (leak de `t` + RayShow de `[fn]`) **corregidos**; ahora topan con el **límite compartido**
de un `Rc<dyn Fn>` no-`Send` capturado por `spawn` (los closures `preparar`/`atender_conn` que recibe
`bucle_servidor`). `metrics_server` → sigue en el canal de un struct no-`Send`. Estos dos últimos son el
**modelo de valores/closures thread-safe** (mismo cubo), no flecos → se difieren conscientemente.
605 tests lib + 19 `build_native` (serial) verdes; test nuevo `build_native_spawn_de_funcion_nombrada`.
(El check `naming_policy` ya estaba rojo en la rama antes de esta fase — nombres de test en español del
arco P2.b; fuera de alcance aquí.)

#### Fase 51 — el crate `ray-runtime` + cripto nativa bajo demanda (Paso 0, 15 jul)

Primer paso del **crate `ray-runtime`** (docs/transpilador-nativo.md §4): el runtime con dependencias de
crates de producción, que consumen la VM y el binario transpilado con el **mismo código** → paridad
byte-idéntica por construcción, no verificada a mano. Quita el techo de los crates (rustls/ring/rusqlite)
que el `rustc` pelado no podía enlazar.

**Hallazgo que fijó el orden**: la doc asumía extraer crypto/tls/sqlite uniformemente, pero los handles
TLS y SQLite comparten el `enum OpenHandle` con archivos/TCP/UDP (registro común; `close(h)` no distingue)
→ extraerlos es un **split de registro** (diseño propio de sus Pasos). **Crypto, en cambio, es puro**
(10 funciones sin estado, único uso directo de `ring::`) → se extrae limpio y valida el mecanismo con
riesgo cero. Por eso el Paso 0 = crypto.

**0a — extracción** (commit `d199b93`): nuevo workspace member `crates/ray-runtime` con módulo `crypto`
tras la feature `crypto` (ring 0.17): sha256/sha512/sha1/hmac/ed25519_*/chacha20poly1305_*/crypto_random.
Firmas de tipos simples (`&[u8]`/`Vec<u8>`); sin la feature, stubs (compila sin ring). `builtins.rs`
**delega** (una línea por función) conservando su gating cfg (net-tls/wasm) → todos los call sites
(vm/interpreter/cli/index) intactos. `Cargo.toml`: `[workspace]`; ray-runtime opcional (no-wasm), enlazado
por `net-tls` vía `ray-runtime?/crypto` (deps target-gated en features); `ring` ya no se declara directo en
raylang. Verificado: build default + slim (`--no-default-features`) OK; wasm no arrastra ray-runtime/ring
(`cargo tree`: ausentes; el fallo getrandom/wasm es preexistente y ajeno).

**0b — la fontanería + cripto nativa** (commit `cf8a8e5`): `build_native` **bifurca**. `transpile()`
devuelve `Transpiled { source, rt_features }`: las features que el programa necesita, activadas **bajo
demanda** al interceptar un builtin con-crate. El transpilador intercepta los primitivos de cripto
(`__sha256`/…/`__chacha20poly1305_*`) → `ray_runtime::crypto::*` (marshalling: `bytes`=`Rc<[u8]>`, `&expr`
deref-coerce a `&[u8]`; `Vec<u8>`→`Rc<[u8]>`; `Option<Vec<u8>>`→`[bytes]` etiquetado) y activa
`needs_rt_crypto`. `build_native`: sin features → `rustc` pelado (camino rápido de siempre, sin red); con
features → genera un **proyecto Cargo** temporal (`src/main.rs` + `ray-runtime` con las fuentes
**incrustadas** vía `include_str!` → paridad con la VM por construcción) y compila con `cargo`, activando
SOLO lo detectado; `CARGO_TARGET_DIR` compartido → ring compila una vez por máquina. Gotcha cazado: la
resolución de nombre recorta el prefijo `__` antes del match → se interceptan los nombres pelados
(`sha256`) con guarda `name` empieza por `__` (no pisar un método de usuario homónimo).

**Vertical slice end-to-end**: un programa con `std/crypto` (sha256/sha512/hmac/ed25519/chacha20poly1305)
transpila a **NATIVO por primera vez**, byte-idéntico a la VM (antes: stub que panica). Camino rápido
intacto. 605 tests lib + 22 `build_native` (serial) verdes; tests nuevos
`build_native_crypto_de_produccion_via_ray_runtime` + `build_native_sin_crate_externo_usa_la_via_rapida`.
**Siguiente**: Paso 1 = `rustls` (TLS, el de mayor valor — desbloquea https/webserver + clientes de DB),
que estrena el split del registro de handles. Luego rusqlite, ring-extra.

#### Fase 52 — TLS nativo vía `ray-runtime` (Paso 1 — rustls, 15 jul)

Paso 1 del crate `ray-runtime`: **TLS de producción** en el binario transpilado (el subsistema de mayor
valor — desbloquea https/webserver + clientes de DB sobre TLS). Estrena el **split del registro de
handles** que el Paso 0 identificó (TLS comparte el `enum OpenHandle` con archivos/TCP/UDP).

**Decisión de diseño clave**: el binario transpilado usa **hilos de SO reales**, así que hace I/O TLS
**BLOQUEANTE** (`ray_runtime::tls::TlsStream` sobre `rustls::StreamOwned`) — mucho más simple que el pump
no-bloqueante + fibras de la VM (que necesita ceder el hilo del scheduler). El registro de handles inline
gana una variante `Tls` tras un **`Arc<Mutex<TlsStream>>` propio**: así el I/O TLS bloqueante NO retiene el
lock global del registro → varias conexiones concurrentes (cada una en su hilo) no se serializan ni
deadlockean (el hazard que el modelo bloqueante+lock-global habría causado).

- `crates/ray-runtime/src/tls.rs` (feature `tls`): `TlsStream` (enum cliente/servidor, porque
  `StreamOwned` exige el tipo de sesión CONCRETO — el enum unificado `rustls::Connection` no cumple los
  bounds) + `connect`/`connect_h2`/`upgrade`/`accept`. Espeja la config de la VM (raíces Mozilla +
  `SSL_CERT_FILE`). Sin la feature, stubs.
- El transpilador intercepta `__tls_connect`/`__tls_connect_h2`/`__tls_accept`/`__tls_upgrade` → helpers
  `__ray_tls_*` y activa `needs_rt_tls` (implica `needs_net`). Emite CONDICIONALMENTE la variante `Tls`
  del `__RayHandle` y el despacho TLS en `socket_read_bytes`/`socket_write` (**como la VM: solo las
  variantes `_bytes` desvían a TLS**; accept/upgrade parten de un handle TCP, sacan su `TcpStream` del
  registro y reinsertan la conexión TLS con el MISMO handle). `build_native` incrusta `tls.rs`.

**Verificado** (oráculo conductual — TLS es I/O de red no determinista, como los tests TCP): cliente TLS
**NATIVO** ↔ servidor TLS del VM = `eco: hola tls`; servidor TLS **NATIVO** ↔ cliente VM = ídem (tests
`build_native_tls_cliente_contra_servidor_vm` + `build_native_tls_servidor_contra_cliente_vm`, con los
fixtures `tests/fixtures/tls_*.pem`). 605 tests lib + 24 `build_native` verdes; slim + wasm sin regresión
(la VM compila rustls directo; `ray-runtime/tls` solo vive en el proyecto generado del binario transpilado).
**Siguiente**: Paso 2 = `rusqlite` (DB embebida, superficie pequeña, determinista → oráculo directo).

#### Fase 53 — SQLite nativo vía `ray-runtime` (Paso 2 — rusqlite) + fix de `type_of(match)` (15 jul)

Paso 2 del crate `ray-runtime`: **SQLite embebido** (rusqlite `bundled`) en el binario transpilado. Más
simple que TLS (Paso 1): la conexión es **propia** (no muta un handle TCP) y el I/O es local y rápido → se
opera reteniendo el lock global del registro (como la VM), sin el `Arc<Mutex>` por-conexión de TLS.
Resultados **deterministas** → oráculo por **byte-identidad** VM↔nativo (a diferencia de TLS, red no
determinista).

- `crates/ray-runtime/src/sqlite.rs` (feature `sqlite`, rusqlite 0.32 bundled): `Conn` + `open`/`exec`/
  `query` (celdas a texto: INTEGER/REAL decimal, NULL `""`, BLOB hex; ncols + celdas aplanadas fila a
  fila), espejando el host de la VM. Sin la feature, stubs.
- El transpilador intercepta `__sqlite_open`/`__sqlite_exec`/`__sqlite_query` → helpers `__ray_sqlite_*` y
  activa `needs_rt_sqlite`. El `__RayHandle` inline gana una variante `Sqlite` condicional; exec/query
  colectan los params `[string]` a `Vec<String>`. El registro se emite también con `needs_rt_sqlite`
  (sqlite no implica net). `build_native` incrusta `sqlite.rs`.

**Bug preexistente cazado y arreglado** (bloqueaba sqlite y cualquier `var x = match {…}` con struct):
`type_of(match)` tomaba el tipo del PRIMER brazo que resolviera, pero un brazo **divergente** (`Err(e) => {
…; return }`) resuelve a `Unit` y ganaba, mientras el brazo real (`Ok(conn) => conn`) fallaba porque el
binding `conn` no está en ámbito para `type_of` → el `var c` se declaraba `Unit`, no se clonaba al leer
(no-heap) → **move error** en rustc al pasar `c` a `exec`/`query` varias veces. Ahora `arm_type` salta los
brazos divergentes (`expr_diverges`) y resuelve el tipo de un binding del patrón desde el tipo del
escrutinio + el patrón (`pattern_binding_types`, reusa la sustitución de payload de `emit_pattern`).

**Verificado**: el demo real `examples/db/sqlite_demo.ray` (`:memory:`, determinista) transpila a **NATIVO**
y da salida **byte-idéntica** a la VM (test `build_native_sqlite_via_ray_runtime`). 605 tests lib + 25
`build_native` (serial) verdes; slim sin regresión (`ray-runtime/sqlite` solo vive en el proyecto generado).
**El techo de los crates está quitado**: crypto (ring) + TLS (rustls) + DB (rusqlite) transpilan a nativo,
bajo demanda, con paridad por construcción. Siguiente posible: `ring`-extra (crypto avanzada) donde los
ejemplos lo pidan, o retomar otros frentes.

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

#### Fase 16 — `@derive(Eq)` (14 jul)

Igualdad derivada + bound `T: Eq`. **Decisión clave**: el checker ya baja los bounds de traits de USUARIO
(`T: Valor`) por **paso de diccionarios** —params ocultos `T#Trait#m` (valores función) + el impl manglado
`Tipo#m` como función ordinaria—, y el transpilador **ya lo emite tal cual** (así funcionan bounds.ray/
impls_genericos.ray). Eq es un trait más: en vez de un trait `RayEq` a la medida (que obligaría a DESCARTAR
los diccionarios y rompería los traits de usuario), se **reusa esa misma maquinaria**. Solo hacía falta
**dejar de saltar la emisión de `eq`**: el impl derivado `Punto#eq`/`Color#eq` (que genera `@derive`) y el
`int#eq` del prelude (`self == other`) se emiten como funciones ordinarias; `x.eq(y)` deja de mapearse a
`==` (que fallaba sobre `Rc<Struct>`) y fluye como llamada normal/al diccionario. Se sigue saltando `eq`
sobre contenedores (`[]`/Map/Channel/Task/…: clave no-identificador o `impl` no transpilable). `show`/`less`
siguen directos (RayShow / `<`). `anotaciones.ray` (`@derive(Eq, Show)` sobre Color/Punto/Forma +
`cuenta_iguales<T: Eq>`) transpila y da salida byte-idéntica a la VM.

**34/37 ejemplos** (añade `anotaciones`) — nativo ≡ VM byte a byte.

#### Fase 17 — `From`-conversion en `?` (14 jul)

El `?` que **convierte el error** vía `impl From<E1> for E2` (`fn convert`). El checker ya baja `leer()?`
—cuando el error de la función difiere del devuelto— a un `match` que en el brazo `Err` llama a la
conversión manglada `AppError#from#string`, con **temporales sintéticos `$to`/`$te`** como bindings. El
transpilador ya emite la función de conversión (es un `impl` de usuario, no se salta) y baja el `match`;
solo faltaba que el `$` (que el checker usa para temps y no es identificador Rust válido) se **manglara**
a `_D_`. Dos ajustes: `mangle` reemplaza `$`→`_D_`, y `emit_match`/`emit_pattern` aplican `mangle` en los
**sitios de binding** (antes crudos; los usos ya manglaban → ahora casan). `conversion_error.ray` transpila
y da salida byte-idéntica a la VM.

**35/37 ejemplos** (añade `conversion_error`) — nativo ≡ VM byte a byte.

#### Fase 18 — operator-overloading + `impl Show` custom (14 jul)

`a + b`/`a - b`/`a * b`/`-a` con `impl Add/Sub/Mul/Neg for Vec2`. Los **valores** ya salían bien: el
checker baja el operador sobrecargado a una llamada al método (`Vec2#add`), que el transpilador ya emite
(como cualquier impl de usuario). El bug real era el **`impl Show` CUSTOM**: `.show()` se mapeaba a
`.ray_show()` (el render default auto-generado `Vec2 { x, y }`), ignorando el `impl Show for Vec2` del
usuario (`(x, y)`). Fix (espejo de Eq): dejar de saltar la emisión de `show` (impl custom/derivado
`Tipo#show` + prelude `int#show` = `to_string(self)`) y dejar de interceptar `.show()` EXPLÍCITO → fluye
como llamada al impl / al diccionario del bound. **Sutileza semántica clave** (verificada contra la VM):
`print(x)`/`to_string(x)` usan el **render default** (RayShow), NO el `impl Show`; solo `.show()`
explícito usa el impl — así que `print`/`to_string` se dejan en `.ray_show()` y solo se cambia `.show()`.
Los tipos `@derive(Show)` (cuyo `Tipo#show` derivado produce el mismo formato) siguen intactos; el impl
custom de `Vec2` ahora se respeta. `operadores.ray` transpila y da salida byte-idéntica a la VM.

**36/37 ejemplos** (añade `operadores`) — nativo ≡ VM byte a byte.

#### Fase 19 — enteros con tamaño (`u8`/`u32`/`u64`) — CIERRA el corpus (14 jul)

Enteros de ancho fijo → los nativos de Rust (`Type::UInt(w)` → `u8`/`u32`/`u64`). Tres piezas: (1)
**literales tipados por contexto** — Rust no coacciona `i64`→`u8`; el checker ya inserta un `Cast`
explícito por cada coerción de literal (`200`→`(200i64 as u8)`), que el transpilador emite (helper
`emit_typed` como red de seguridad para literales sin cast: sufijo `200u8`, propagación al tipo de
elemento de un arreglo, o `(e) as uW`). (2) **aritmética ENVOLVENTE**: `a + b` sobre `u8` con `a,b`
sized → `a.wrapping_add(b)` (Rust **deniega en compilación** el overflow constante `200u8 + 100u8`, y el
`-O` solo desactiva el chequeo en *runtime*; `wrapping_*` garantiza mod 2^N como la VM). Solo `+`/`-`/`*`
entre operandos sized; `^`/`&`/`|` (bitwise) y los casts no desbordan. (3) **casts** `x as u32`/`as int`
vía el `as` de Rust (trunca/envuelve a N bits, igual que la VM). `enteros.ray` (hash FNV-1a con
wrap a 32 bits, `~0u8`, `u64` de 10^12) transpila y da salida byte-idéntica a la VM.

**37/37 ejemplos** (añade `enteros`) — **el corpus determinista COMPLETO** transpila y da salida
byte-idéntica a la VM.

**Cobertura tras 19 fases — ARCO P2.b (transpilar-a-Rust) COMPLETO**: escalares · control · recursión ·
strings · arreglos · Map · structs/enums/match · Option/Result/`?` (+ From-conversion) ·
closures/map/filter/fold · genéricos (fns + tipos) · traits (estático + defaults + bounds) · tuplas ·
trait objects (`dyn`) · `std::math` · `args()` · `for` sobre Map · `@derive(Eq)` · operator-overloading +
`impl Show` custom · **enteros con tamaño** · const/char/cast · UFCS. **El transpilador cubre el lenguaje
COMPLETO** del corpus con salida byte-idéntica a la VM (**37/37 ejemplos deterministas**). Fuera del
corpus determinista quedan solo features no-transpilables por naturaleza (I/O no determinista: stdin/env;
concurrencia M12: `spawn`/canales → "caen a la VM" en v1) y limitaciones acotadas conocidas (captura
mutable en closures → `FnMut`, `.show()` de una función, protocolo `Iterator<T>`). La tesis de P2.b queda
**probada de punta a punta**: `ray build --native` transpila todo el lenguaje a Rust nativo, y la medición
del spike (fibrec 5.4× más rápido que node, 61× que la VM) se generaliza a programas reales.

#### Fase 20 — `ray build --native` (el subcomando de producto)

El flag oculto `--emit-rust` (spike) se cablea en el subcomando de producto: **`ray build --native [-o
<salida>]`** carga+chequea, transpila a Rust (`transpile::transpile`), escribe un `.rs` temporal e invoca
**`rustc -O -A warnings`** → binario nativo (nombre por defecto = *stem* del archivo; `-o` lo cambia).
Requiere `rustc` en el PATH; el test de integración lo salta limpiamente si no está. **Modelo dev=VM /
deploy=nativo**, como Rust: `ray run` (VM, arranque ≤5 ms, iteración rápida) para desarrollar; `ray build
--native` para el binario de producción.

**Medición end-to-end (14 jul, fib(35) recursivo, best-of-3, release+`-O`)**:

| motor | tiempo | vs node |
|-------|--------|---------|
| raylang **nativo** (`ray build --native`) | **0.02 s** | **5× más rápido** |
| node v26 | 0.10 s | — (referencia) |
| raylang VM (release) | 1.53 s | 15× más lento |

El nativo bate a node **5×** y a la propia VM **~76×**. **raylang pasa de peor-de-la-clase** (la VM era
12.5× más lenta que node en el banco original) **a mejor-de-la-clase** en cómputo — el giro ~68× del spike
se confirma en el pipeline real. Test de integración `build_native_produce_un_binario_que_corre_como_la_vm`
(`tests/cli_cli.rs`): el binario nativo da salida idéntica a la VM.

#### Fase 21 — el nicho de servicios en nativo + I/O de entrada transpilable

**El nicho de servicios, medido en nativo** (14 jul, best-of-3): los 3 benchmarks del banco poliglota
(wordcount/jsonserialize/logparse) **ya transpilaban** (generan datos sintéticos en memoria: split + Map +
sort, sin I/O). Compilados con `ray build --native` y comparados con los líderes del nicho:

| bench | ray-nativo | node | php | python | ray vs líder |
|-------|-----------|------|-----|--------|--------------|
| wordcount | **0.09 s** | 0.16 | 0.10 | 0.17 | ~1.1× (empata php) |
| jsonserialize | **0.04 s** | 0.11 | 0.08 | 0.15 | **2× más rápido** |
| logparse | **0.05 s** | 0.09 | 0.07 | 0.10 | **1.4× más rápido** |

El nicho de servicios era donde raylang iba **2.9–9.7× por detrás** del líder (banco original) → **1.9–2.5×**
tras P0 → ahora **el más rápido o empatado** en nativo. El giro se completa en los DOS perfiles (cómputo y
servicios).

**I/O de entrada transpilable** (para programas reales, no solo los benchmarks sintéticos): se añaden al
transpilador los builtins de **lectura**. Del prelude (nombre pelado): `input() -> Option<string>` (una
línea de stdin, quita `\n`/`\r` finales, `None` en EOF — byte-idéntico a la VM), `read_int() -> Option<int>`
(input + parse). Del módulo `std/fs` (cualificado `std::fs::*`, interceptado por prefijo como `std::math`):
`read_file -> Result<string,string>` (`std::fs::read_to_string`), `write_file -> Result<int,string>`,
`exists -> bool`. No deterministas → sin oráculo; se prueban por subproceso (nativo vs VM con el mismo
stdin/archivo) en `tests/io_cli.rs::native_io_de_entrada_coincide_con_la_vm`. Diferido (aún fuera):
handles de archivo (`open`/`read_line`/`write`/`close`), `env`/`args`-de-entorno, `list_dir`/`remove_file`.

#### Fase 22 — handles de archivo (`open`/`read_line`/`write`/`close`)

I/O bufferizada con handles (M11.8): un servidor/parser que streamea un archivo. `open`/`read_line`/`write`
viven en `std/fs` (cualificados `std::fs::*`, por `emit_fs`); `close(h)` es un builtin pelado ad-hoc (para
un handle de archivo, int → 0). La VM guarda los archivos abiertos en un **registro global de proceso**
(`Mutex<HashMap<i64, OpenHandle>>` + contador); el transpilador **emite el mismo registro** en Rust
(`__RayHandle` = Reader(BufReader)/Writer(File), `__RayReg` tras `Mutex`/`OnceLock`, `__ray_open/read_line/
write/close`) — mensajes de error **byte-idénticos** a la VM (`invalid open mode…`, `invalid file handle:
N`, `the handle is open for reading, not writing`). El registro se **anexa al final** del `.rs` (Rust
permite items top-level en cualquier orden) y **solo si el programa usa handles** (flag `needs_handles`,
activado al emitirlos) → los programas sin handles no lo arrastran. `open`→`Result<int,string>`,
`read_line`→`Option<string>` (bufferizada, sin `\n`), `write`→`Result<int,string>` (nº de caracteres,
como `s.len()` de la VM), `close`→`int` (0). Verificado nativo ≡ VM (escribir + releer línea a línea + los
3 casos de error) por subproceso en `tests/io_cli.rs::native_handles_de_archivo_coinciden_con_la_vm`.
Diferido (aún fuera): `env`/`args`-de-entorno no determinista, `list_dir`/`remove_file`, sockets/TLS.

#### Fase 23 — `list_dir`/`remove_file` (gestión de archivos)

Cierra la gestión de archivos común: `remove_file(path) -> Result<int,string>` (`std::fs::remove_file` →
`Ok(0)`/`Err(msg)`) y `list_dir(path) -> Result<[string],string>` (`std::fs::read_dir`, nombres
**ORDENADOS** → determinista, como la VM). Ambos en `std/fs` (cualificados, por `emit_fs`); `list_dir`
reconstruye el `[string]` en la repr del transpilador (`Rc<RefCell<Vec<Rc<str>>>>`). Mensajes de error
byte-idénticos a la VM (el `e.to_string()` del OS: `No such file or directory (os error 2)`, …). Verificado
nativo ≡ VM (listar ordenado + borrar ok/inexistente + dir inexistente) por subproceso en
`tests/io_cli.rs::native_list_dir_y_remove_file_coinciden_con_la_vm`.

#### Fase 24 — directorios + metadatos (`mkdir`/`rename`/`copy_file`/`is_dir`/`file_size`…)

Cierra el módulo `std/fs` (M67): `mkdir` (`create_dir_all`), `remove_dir` (solo vacío), `rename`,
`copy_file` (`std::fs::copy(..).map(|_|())`) → `Result<int,string>` (Ok(0)/Err); `is_dir`/`is_file` →
`bool` (`Path::is_dir`/`is_file`); `file_size` → `Result<int,string>` (`metadata`; un directorio es error
con el mensaje byte-idéntico a la VM `no es un file`). Los cuatro de resultado unitario comparten un solo
brazo parametrizado (nombre de la fn de Rust + aridad + si mapea el retorno). La **escritura atómica**
(temp + `rename`) ya es transpilable. Verificado nativo ≡ VM (ciclo completo mkdir→escribir→copiar→
renombrar→borrar + errores) por subproceso en `tests/io_cli.rs::native_directorios_coinciden_con_la_vm`.
**La I/O de archivos del transpilador queda COMPLETA — TODO `std/fs`**: leer/escribir entero
(`read_file`/`write_file`), consultar (`exists`/`is_dir`/`is_file`/`file_size`), streaming con handles
(`open`/`read_line`/`write`/`close`), stdin (`input`/`read_int`), gestión de archivos (`list_dir`/
`remove_file`) y de directorios (`mkdir`/`remove_dir`/`rename`/`copy_file`). Diferido (aún fuera):
`env`/`args`-de-entorno no determinista, I/O binaria (`*_bytes`), sockets/TLS.

#### Fase 25 — proyectos multi-módulo / cápsulas → UN binario nativo

`ray build --native` sobre un `main.ray` que importa otros módulos/cápsulas produce **un solo binario**:
el loader ya aplana `main` + sus `import`/`from`/cápsulas (transitivos) en **un `Program`** antes de
transpilar (mismo camino que `ray run`). **Bug encontrado y arreglado**: el loader namespaca los nombres a
`modulo::nombre`; el transpilador manglaba las FUNCIONES (`geo::area`→`geo_CC_area`) pero **no los TIPOS**
(`struct geo::Punto` → `::` ilegal en un identificador Rust → `rustc` fallaba). Fix: aplicar `mangle` a los
nombres de tipo en los ~9 sitios de emisión (def de struct/enum, referencias en `rust_ty`, literal de
struct, construcción de enum, target + arms de los impls de RayShow, patrones de match). **Sutileza de
render** (verificada contra la VM): el `print(x)` default (RayShow) muestra el nombre **COMPLETO**
namespacado (`geo::Punto { … }`), mientras que el `.show()` de `@derive(Show)` muestra el **LOCAL**
(`Punto`) — el derivado ya lo trae bien (string literal del codegen del checker), y RayShow usa el completo.
`@derive(Eq)` cruza módulos sin cambios (diccionarios). Los 3 proyectos reales (`examples/modulos`,
`examples/capsula`, `examples/proyecto`) transpilan y dan salida byte-idéntica a la VM. Archivo único
(sin `import`) intacto (`mangle` es la identidad sin `::`). Tests: unitario (los tipos se manglan, Display
completo) + integración (`build_native_de_un_proyecto_multi_modulo_es_un_solo_binario`, `tests/cli_cli.rs`).

#### Fase 26 — I/O binaria (el tipo `bytes`)

Primer tipo nuevo del transpilador desde los enteros con tamaño: **`bytes`** (secuencia INMUTABLE de
octetos) → **`Rc<[u8]>`** (clon barato/compartible, como `Rc<str>`). Piezas: (1) literal `b"..."` →
`Rc::<[u8]>::from(vec![…u8])`; (2) `impl RayShow for Rc<[u8]>` → **hex** minúsculas sin separador
(`{:02x}` por octeto, como `bytes_to_hex` de la VM) → `print`/`to_string` dan lo mismo; (3) `==` gratis
(`Rc<[u8]>` compara contenido); (4) builtins: `to_bytes` (UFCS, `s.as_bytes()`), `from_utf8`
(`std::str::from_utf8` → `Result<string,string>`), `sub_bytes` (corte `[i,j)` con clamp, nunca falla),
`len`; (5) **I/O**: `read_file_bytes` (`std::fs::read`) / `write_file_bytes` (`std::fs::write`, Ok(len)) /
`append_file_bytes` (`OpenOptions::append`) → abre parsear/generar formatos binarios en nativo. **Bug de
concatenación cazado y arreglado**: `"x: " + to_string(b)` inlinaba `b` como `{}` (Display), pero `[u8]`
(y struct/enum/array) NO son Display → ahora el inline solo aplica a primitivos Display; el resto pasa por
`ray_show`. Al cubrir `bytes` el `match` de `emit_expr`/`type_of` pasó a ser **exhaustivo sobre `ExprKind`**
(se quitaron los `_ =>` muertos → una variante nueva del AST rompe la compilación, mejor que un error en
runtime). Verificado nativo ≡ VM (hex, sub_bytes, from_utf8, round-trip de archivo) por subproceso en
`tests/io_cli.rs::native_io_binaria_coincide_con_la_vm`. Corpus 37/37 + multi-módulo intactos. Diferido:
`env`/`args`-de-entorno, sockets/TLS.

#### Fase 27 — `env` de entorno (cierra la I/O de proceso)

`env(name) -> Option<string>` → `std::env::var(&*name).ok().map(Rc::<str>::from)` (`Some` si está,
`None` si no — como la VM). Con `args()` (ya soportado, Fase 14) el **entorno de proceso queda cubierto**:
un programa nativo lee las mismas variables de entorno y argumentos posicionales que bajo la VM. Determinista
si el harness controla el entorno → test de integración `build_native_env_y_args_coinciden_con_la_vm`
(`tests/cli_cli.rs`): el binario nativo con `RAY_IT_VAR=hola` y args `uno dos` da la salida de `ray run
prog.ray uno dos` con el mismo env. El test `rechaza_` pasa a usar `spawn` (concurrencia, aún fuera). Corpus
37/37 intacto. **Diferido restante**: concurrencia (`spawn`/canales → "cae a la VM" en v1) y red
(sockets/TLS) — ambos requieren runtime propio, no I/O simple.

#### Fase 28 — concurrencia CSP (spawn + canales) — hilos de SO reales

**Decisión de modelo (con el usuario): hilos de SO reales** (paralelismo de verdad), no el scheduler
cooperativo determinista de la VM. La VM ya corre multicore no-determinista por default (M38); su salida
casa con `--deterministic` porque los programas concurrentes bien hechos son **deterministas por diseño**
(pipelines FIFO). Primera fase: el **slice CSP** (M12.1/M12.2). `Channel<T>` → **`__RayChan<T>`**, un canal
**MPMC thread-safe propio** (`Arc<(Mutex<VecDeque>, Condvar)>` — sin deps, el `.rs` es standalone) con
backpressure (bounded), cierre y FIFO. `Channel.new()` (no acotado) / `Channel.bounded(n)`; `send`/`recv`
(→ `Option<T>`) / `close` (ad-hoc: canal → `.close()`, handle de archivo → int); `spawn(fn(){…})` →
**`std::thread::spawn(move || …)`** (fire-and-forget; el `Task<T>` se descarta — `join` es la fase
siguiente). **Gotcha clave**: el closure captura por `move`, pero varios spawns/main comparten el mismo
canal → antes de cada `spawn` se **clonan** los canales en ámbito (`in_scope_channels`, Arc bump) para que
el closure mueva un clon y el original siga usable. `T: Send` (primitivos en v1; canales de string/struct →
diferido, necesitarían el modelo de valores thread-safe). **Verificado**: `examples/concurrency/
concurrencia.ray` (pipeline productor→cuadrador→main) transpila a un binario nativo con hilos reales y da
salida byte-idéntica a la VM (`1 4 9 16 25 55`), **estable en 10/10 corridas** pese al paralelismo real.
Test de integración `build_native_concurrencia_csp_coincide_con_la_vm` (`tests/cli_cli.rs`). El test
`rechaza_` pasa a usar `select`. Corpus 37/37 + multi-módulo intactos. **Diferido**: `select`, structured
(`scope`/`join`/`Task`), cancelación; canales de tipos no-Send.

#### Fase 29 — structured concurrency (`Task`/`join`/`scope`)

Segundo slice (M12.3): spawn tareas, recoge resultados. `Task<T>` → **`__RayTask<T>`** = un
`JoinHandle<T>` envuelto en `Arc<Mutex>` que **cachea el resultado** (join una vez ejecuta el hilo; joins
posteriores devuelven el clon cacheado → una tarea se une explícitamente **o** por el scope, no dos veces).
`spawn(fn()->T)` pasa de fire-and-forget a **`__ray_spawn`** → devuelve el `Task` **y** se registra en el
scope activo; `join(t)` → `t.join()` (espera + re-lanza si falló; ad-hoc con el `join(arr,sep)` de strings,
se distingue por el tipo del primer arg); `scope(fn()->R)` → **`__ray_scope`**, que corre el cuerpo y al
salir **une todas** las tareas lanzadas dentro. Mecánica: un **thread-local** `__SCOPES` = pila de frames;
cada frame acumula clausuras-de-join; `__ray_spawn` empuja la suya al frame más interno; `__ray_scope`
las ejecuta al salir. `T: Send + Clone` (primitivos). Verificado: `examples/concurrency/structured.ray`
(3 tareas `cuadrado(3/4/5)` unidas con `join`, + un scope que espera un `spawn` sin join explícito)
transpila con hilos reales y da salida byte-idéntica a la VM (`50`, `7`), estable 10/10. `spawn`
fire-and-forget (concurrencia.ray, sin scope) sigue funcionando (sin scope activo → tarea detached). Test
de integración `build_native_structured_concurrency_coincide_con_la_vm`. **Diferido**: `select` (M12.4),
cancelación (M12.5); canales de tipos no-Send.

#### Fase 30 — `select` sobre varios canales

Tercer slice (M12.4): `select(chs: [Channel<T>]) -> int` → índice del PRIMER canal listo para recibir
(cola no vacía ∨ cerrado). Runtime `__ray_select` = **poll con backoff** del índice menor listo (std no
tiene un select multi-condvar; un `sleep(50µs)` entre rondas evita el busy-spin; el resultado es correcto).
`select(chs)` → `__ray_select(&chs.borrow()[..])`. **Hallazgo importante** (verificado): el **orden** de
`select` es inherentemente **no-determinista bajo paralelismo real** — la VM misma, en su default multicore,
ya alterna entre `100 101 200 201` y `200 100 101 201` (solo `--deterministic` lo fija). Así que el nativo
**no** casa byte-a-byte con una corrida única, pero SÍ el **invariante**: el multiset de valores
`{100,101,200,201}`, el `total` (602) y el **exit code** (90 = 602&0xFF) — todos order-independientes.
El test de integración `build_native_select_invariante_coincide_con_la_vm` verifica ese invariante (multiset
ordenado + exit), no un orden fijo — honesto con la semántica. `examples/concurrency/select.ray` transpila.
Corpus 37/37 intacto. El test `rechaza_` pasa a usar `signals()` (canal de señales del SO, M88.1). **Queda
en concurrencia**: `signals()` (señales del SO — runtime de señales aparte), cancelación de hermanas M12.5
(automática), `try_join`/`Selected<T>`/`cancel(t)` explícito; y canales de tipos no-Send.

#### Fase 31 — `signals()` — el apagado ordenado (patrón microservicio)

Cierra el ejemplo `senales.ray` (M88.1): `signals() -> Channel<int>` = el canal de señales del SO
(SIGTERM=15/SIGINT=2 llegan como int; singleton). Réplica del truco del **self-pipe** de la VM, con
**FFI a libc sin crates** (el `.rs` es standalone; `pipe`/`write`/`read`/`signal` viven en libSystem/libc,
siempre enlazadas): el handler `__ray_on_signal` (async-signal-safe: solo un `write` + un store atómico)
escribe el nº de señal a un pipe; un **hilo lector** lo lee (bloqueante, sin poll) y lo `send`ea al
`__RayChan<i64>`. `signals()` → `__ray_signals()`, singleton vía `OnceLock` (instala los handlers una vez).
Unix-only (el diferido no-unix se documenta). Verificado nativo ≡ VM byte a byte: corriendo `senales.ray`,
procesa 3 items de `trabajo` y al recibir **SIGTERM** (o **SIGINT**) imprime `señal 15/2: drenando y
apagando` y sale limpio — estable en varias corridas. Test de integración (unix)
`build_native_signals_apagado_ordenado` (`tests/cli_cli.rs`): lanza el binario, envía `kill -TERM`, verifica
el apagado. **Los 4 ejemplos de concurrencia transpilan** (`concurrencia`/`structured` byte-idénticos;
`select`/`senales` con el invariante correcto). El `rechaza_` pasa a usar `try_join`. Corpus 37/37 intacto.
**Diferido restante en concurrencia**: `try_join`/`Selected<T>`/`cancel(t)` explícito, cancelación
automática de hermanas M12.5, canales de tipos no-Send.

#### Fase 32 — canales/tasks de `string` y `bytes` (tipos heap inmutables)

`Channel<string>`/`Channel<bytes>` (y `Task<…>`). **El obstáculo**: los valores del transpilador son
`Rc`/`RefCell` (mono-hilo, `!Send`), y cruzar un hilo exige `Send`. El **modelo de valores thread-safe
completo** (Arc/Mutex para todo) NO es un drop-in: `a[i] + a[j]` (dos lecturas del mismo arreglo) emite dos
`.borrow()`, legal en `RefCell` (lecturas compartidas) pero **deadlock** con `Mutex`/`RwLock` (re-lock
mismo hilo, no-portable) → requeriría aislar cada borrow. **Subconjunto limpio y seguro**: los heap
**INMUTABLES** (`string`/`bytes`) se mandan **copiando al BORDE** a una repr Send (`Arc<str>`/`Arc<[u8]>`)
y de vuelta al recibir — **sin tocar el modelo de valores** (todo sigue `Rc`, mono-hilo). Como son
inmutables, copiar es semánticamente idéntico. `send_type(T)` da la repr del canal (primitivos igual,
string/bytes→Arc, structs/arreglos/Map→error claro); `send` convierte con `Arc::from(&*v)`, `recv` con
`.map(|x| Rc::from(&*x))`, `spawn(fn()->string)` envuelve el retorno del closure (corre en otro hilo),
`join` desenvuelve. Verificado nativo ≡ VM: `Channel<string>` (pipeline de saludos), `Task<string>`,
`Channel<bytes>` — byte-idénticos, estables. Int/float/bool/char intactos (sin conversión); struct/arreglo
por canal → error `canal/tarea de tipo no-Send`. Test de integración
`build_native_canal_de_string_coincide_con_la_vm`. Corpus 37/37 intacto. **Diferido**: canales de
structs/arreglos/Map (mutables → el modelo thread-safe con sus hazards); `try_join`/`Selected`/`cancel`,
cancelación M12.5.

#### Fase 33 — tier de optimización `ray build --native --release` (medido)

Optimización del BINARIO nativo, con el método **medir-primero**. Medición (best-of-6, sobre el binario
transpilado):

| flags de rustc | fibrec (cómputo) | wordcount (Map/alloc) | compilación |
|----------------|-----------------|-----------------------|-------------|
| `-O` (opt2, default) | 0.10 s | 0.10 s | 0.21 s |
| `opt3 + lto=fat + codegen-units=1 + target-cpu=native` | 0.10 s | **0.09 s** | 1.83 s |

**Conclusión**: los flags agresivos dan **~10 %** en cargas de asignación/Map (wordcount), **nada** en
cómputo puro (fibrec — rustc ya lo deja óptimo en opt2, un solo archivo/crate → inlining completo), a
cambio de **~9× de tiempo de compilación** y un binario **no portable** (features de la CPU del host).
**PGO se DESCARTÓ** (como el NaN-boxing de P1): con `-Cprofile-generate`+entrenamiento+`llvm-profdata`+
`-Cprofile-use`, wordcount quedó en 0.08 s **igual que sin PGO** — sin ganancia medible y con alta
complejidad (requiere un run de entrenamiento con input representativo). Decisión de producto: un **tier**,
como cargo dev/release. `ray build --native` → `-O` (rápido, portable, el mejor equilibrio para dev/deploy
general); `ray build --native --release` → `opt3+lto+cgu1+target-cpu=native` (el ~10 % extra cuando importa,
asumiendo compilación lenta y binario no portable). Ambos dan salida byte-idéntica (solo cambian los flags
de rustc). Test de integración `build_native_release_produce_binario_correcto`. Ayuda + module-doc
actualizados.

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
