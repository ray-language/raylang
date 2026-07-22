# Bench políglota — investigación de optimización (22 jul 2026)

Investigación sobre los tres benchmarks de servicios del proyecto políglota
(`~/Desktop/benchmarks`): **wordcount**, **jsonserialize** y **logparse**, con dos
objetivos: bajar los tiempos (VM y nativo) y bajar el **pico de memoria de
jsonserialize**. Todo lo de aquí está **medido en esta máquina** (M3, macOS), no
estimado, salvo donde se indica.

## 1. Punto de partida (números del usuario, reproducidos)

| bench | ray VM | ray native | Go (líder) | native/Go | pico mem VM | pico native | pico Go |
|---|---|---|---|---|---|---|---|
| wordcount | 297 ms | 93.7 ms | 46.3 ms | 2.03× | 5.2 MB | 1.5 MB | 7.7 MB |
| jsonserialize | 145 ms | 42.9 ms | 29.6 ms | 1.45× | **83.9 MB** | **73.8 MB** | 47.0 MB |
| logparse | 156 ms | 55.9 ms | 26.5 ms | 2.11× | 4.3 MB | 1.2 MB | 7.5 MB |

Dos observaciones de contexto antes de tocar nada:

- **El script `build-rays.sh` compila el nativo SIN `--release`** (usa el tier dev
  `-O`/opt-level 2). Medido: `--release` da wordcount −9%, logparse −5%,
  jsonserialize ±0.
- El `.ray` de jsonserialize llama `to_string(i)` **dos veces** por iteración; el
  Go de referencia hace `s := strconv.Itoa(i)` una vez y lo reusa. Para comparar
  motores en igualdad, o se iguala el fuente o se acepta el sesgo (es pequeño,
  pero existe).

## 2. Atribución (perfilado con `sample` + `atos`, binario release con símbolos)

### VM (los tres benchmarks comparten el mismo perfil de fondo)

Top de pila, ×10 iteraciones para estabilizar:

- **~50–60 % es el bucle de despacho en sí** (`run_loop` + `run_worker` +
  `Vm::push`/`get_local`). Es el coste estructural del intérprete; solo lo mueven
  las superinstrucciones o el nativo (conclusión ya conocida del arco P1).
- El resto es **tráfico de strings**:
  - `String::clone` — cada `GetLocal` de un string **clona el buffer entero**
    (`vm/mod.rs:2850`), y cada push de una constante string la clona al heap
    (`const_to_heap`, caliente en los tres).
  - `Add` par a par (`apply_binary` → `a + &b`, `vm/mod.rs:2981`): `line = a+b+c`
    son N−1 strings intermedios; no hay opcode de concatenación n-aria.
  - `TwoWaySearcher::next` — el `split(" ")` usa el matcher genérico de patrones
    de `str` aunque el separador sea de 1 carácter (wordcount/logparse).
  - `to_string(int)` — 1 alloc por llamada, sin escritura directa al buffer
    destino (caliente en jsonserialize: `core::fmt::num`).
  - mimalloc (malloc/free) — churn de strings pequeños.
  - `degrade_int_array` aparece por volumen (es el embudo barato de acceso a
    arreglos, no un cuello real).

### Nativo (informe del transpilador, `src/transpile/`)

| causa | detalle | evidencia |
|---|---|---|
| **Allocador del sistema** | el binario generado NO lleva mimalloc (el `#[global_allocator]` de `src/lib.rs` es solo del binario `ray`) | **medido abajo: −18 a −40 %** |
| **HashMap std (SipHash)** | `types.rs:67` / `runtime.rs:73` — sin ahash, hasher lento para claves string | medido: −8.5 % extra en wordcount |
| Concat = `format!` + `Rc::<str>::from` | `emit.rs:1344`: 2 allocs + 1 copia completa por expresión (el `format!` además no pre-dimensiona); Go hace 1 alloc | perfil + código |
| `split` copia cada trozo a `Rc<str>` nuevo | `runtime.rs:61`; Go devuelve slices sin copiar | código |
| `join` = 3 allocs con copia final a `Rc<str>` | `runtime.rs:62-65`: el `String` unido se **recopia** entero a `Rc<str>` → +17 MB transitorios en jsonserialize | código |
| panic=unwind | sin `-C panic=abort` (el modelo de errores usa `catch_unwind`, no trivial de quitar) | código |

### El pico de memoria de jsonserialize, descompuesto

- **VM (~84–88 MB)**: arreglo de 400k `HeapValue` (12.8 MB) + payload de los
  strings (~19 MB con redondeo de malloc) + `out` (17 MB) + **el `Vec<String>`
  intermedio del `Join`, que CLONA los 400k elementos** (~27 MB transitorios,
  con el arreglo aún vivo) + retención del allocador. El GC es irrelevante aquí:
  dispara por **número de objetos** (`gc.rs:257`), y este programa tiene ~1 objeto
  vivo (el arreglo); los bytes de los strings son invisibles para el umbral.
- **Nativo (~74–77 MB)**: 400k `Rc<str>` (~25 MB con cabeceras) + `Vec` (6.4 MB) +
  `Vec<&str>` del join (6.4 MB) + `String` unido (17 MB) + **la recopia a
  `Rc<str>`** (otros 17 MB transitorios).
- **Go (47 MB)**: mismo residente estructural, sin la recopia final (su `Join`
  aloca una sola vez el resultado).

## 3. Experimentos medidos (evidencia dura)

Todos con hyperfine (w2), binarios `--release` como base:

| experimento | antes | después | delta |
|---|---|---|---|
| nativo wordcount + **mimalloc** en el binario generado | 86.8 ms | **52.5 ms** | **−40 %** |
| nativo wordcount + mimalloc + **ahash** | 86.8 ms | **48.0 ms** | **−45 %** (≈ Go: 46.3) |
| nativo logparse + mimalloc | 50.3 ms | **30.4 ms** | **−40 %** (Go: 26.5) |
| nativo logparse + mimalloc + ahash | 50.3 ms | 30.1 ms | ahash ≈ neutro aquí |
| nativo jsonserialize + mimalloc | 42.4 ms | **34.6 ms** | **−18 %** (Go: 29.6) |
| nativo `--release` vs dev (`-O`) | — | — | wc −9 %, lp −5 %, js ±0 |
| **VM jsonserialize, `Join` sin clonar** (prototipo en `vm/mod.rs`, revertido) | 88.0 MB pico | **76.0 MB** | **−14 % pico**, tiempo ~igual |

El experimento de mimalloc+ahash se hizo parcheando el `.rs` emitido
(`ray --emit-rust`) y compilándolo como crate con las mismas flags de release —
es exactamente lo que emitiría el transpilador.

## 4. Plan paso a paso (por ROI, cada paso = un commit/PR)

### Fase 0 — higiene del bench (sin tocar raylang) · 10 min

1. `build-rays.sh`: compilar con `ray build --native --release`.
2. (Opcional, decidir) igualar `jsonserialize.ray` al Go: `let s = to_string(i);`
   una vez. Documentarlo si se hace: cambia el programa, no el motor.

### Fase N — el nativo a la altura de Go (transpilador) · el ROI grande

3. **N1 — mimalloc en el binario generado** ✅ **HECHA** (22 jul; PERFORMANCE.md Fase 54:
   feature `mimalloc` de `ray-runtime` por defecto, `--without mimalloc` de escape;
   medido: −40/−40/−18 %).
   El transpilador ya sabe generar un proyecto Cargo cuando hay features con
   crate (TLS/cripto/SQLite); la vía natural es tratar mimalloc igual: si está
   disponible, el build nativo emite `#[global_allocator]` + dep `mimalloc` y
   compila por la ruta Cargo. Decisión de diseño a fijar: ¿mimalloc por defecto
   (recomendado; el precedente ring/rusqlite ya rompió el cero-deps) con
   `--without mimalloc` para el rustc pelado, o detrás de un flag? Con esto,
   wordcount nativo queda a ~1.15× de Go y logparse a ~1.15×.
4. **N2 — ahash en los Map generados** ✅ **HECHA** (22 jul; PERFORMANCE.md Fase 55:
   feature `ahash` de `ray-runtime` por defecto vía el alias `__RayMap`, escape
   `--without ahash`; medido: −8.5 % extra en wordcount, neutro donde el map no
   domina). Nota colateral: el ajuste de Fase 0 (`let s = to_string(i)`) ayudó a la
   VM pero costó ~9 ms al nativo (el inline en `format!` era gratis) → anotado en N4.
5. **N3 — `__ray_join` sin recopia** ✅ **HECHA** (22 jul, junto a V1; PERFORMANCE.md
   Fase 56): `Rc<str>` construido una vez. Medido: pico nativo jsonserialize
   **75.5 → 51.4 MB (−32 %**, Go: 47.0), tiempo −3 %.
6. **N4 — concat pre-dimensionado** ✅ **HECHA** (22 jul; PERFORMANCE.md Fase 59:
   temps + `String::with_capacity(exacta/cota)` + `write!`). Medido (A/B
   estricto): logparse **−10.6 %**, jsonserialize **−5.6 %**, wordcount −2.8 %.
   Los tres nativos a ~1.1× de Go.
7. (Menor) **N5 — `Vec::with_capacity` en `__ray_split`** por conteo previo de
   separadores, y evaluar `-C panic=abort` NO (el modelo de errores depende de
   unwind — descartado salvo rediseño).

### Fase V — la VM (tiempo y memoria)

8. **V1 — `Join` sin clonar** ✅ **HECHA** (22 jul, junto a N3; PERFORMANCE.md
   Fase 56): pico VM jsonserialize **88 → 76 MB (−14 %)**, oráculo intacto.
9. **V2 — opcode `ConcatN` (concatenación n-aria)** ✅ **HECHA** (22 jul;
   PERFORMANCE.md Fase 57: bajada `lower_concat` → primitivo `__concat` →
   opcode `ConcatN(n)`; oráculo `concat_chain_lowering_oracle`). Medido (A/B
   estricto): jsonserialize **−27 %** (143→105 ms), logparse **−10 %**,
   wordcount −3 %; nativo neutro. La fusión de `ToString` en el buffer quedó
   fuera (siguiente refinamiento si el perfil aún la señala).
10. **V3 — `split` con separador de 1 byte** ❌ **EVALUADA y DESCARTADA** (22 jul,
    medido; PERFORMANCE.md Fase 58): el patrón `char` es +9 % MÁS LENTO que el
    `&str` (el `StrSearcher` de std ya usa memchr; `CharSearcher` decodifica
    UTF-8), y el preconteo para preasignar, +4 %. El camino actual ya era el
    rápido. Anotado en el opcode `Split`.
11. **V4 — `heap_to_key` que consuma el valor** ✅ **HECHA** (22 jul; Fase 58):
    la clave se mueve en vez de clonarse en los 6 sitios de Map. Medido:
    wordcount **−4.3 %** (313.4 → 300.0 ms), micro de inserciones −3 %.
12. **V5 — `sort` nativo para `[string]`**: `keys().sort()` corre el merge sort
    del prelude (raylang), con 2 clones de String por comparación vía `Index`.
    Un builtin que ordene el `Vec<HeapValue>` in situ comparando `&str`. En estos
    benchmarks las claves son pocas (~1000) — impacto menor aquí, pero es la
    pieza que falta para cargas de ordenación reales (los `Ord` de usuario siguen
    por el camino del prelude).
13. **V6 — GC consciente de bytes (memoria, no tiempo)**: el umbral dispara por
    número de objetos; sumar `String::capacity` (contabilidad incremental en
    allocate/sweep) para que cargas string-heavy con pocos objetos no queden
    fuera del radar. En estos 3 benchmarks no cambia nada (no hay basura de heap
    que recolectar), pero gobierna el pico en programas con churn de arreglos de
    strings. Junto a esto: `shrink_to_fit` de `slots`/`free` tras barridos
    grandes (hoy el pico de handles es permanente).
14. **(Evaluar con prudencia) V7 — constantes string sin clon por push**: cada
    iteración clona la constante `base` (`const_to_heap` caliente). Opciones:
    handle de heap pre-alocado por constante (raíz permanente) o `Rc<str>` solo
    para constantes. Ojo: `Rc<str>` global (Opt.3) y SSO (P1.4) ya se midieron
    y descartaron; esta variante es más estrecha (solo el borde constante→pila),
    pero hay que medirla A/B antes de comprometerla.

### Lo que NO hacer (ya refutado por medición, no reabrir)

- SSO/`compact_str` (P1.4: neto negativo con mimalloc), `Rc<str>` global (Opt.3),
  encoger `HeapValue` (P1.1), PGO del nativo (fase 33: sin ganancia).

## 5. Dónde debería quedar cada benchmark si el plan aterriza

| bench | native hoy | native esperado (N1–N4) | VM hoy | VM esperado (V1–V4) |
|---|---|---|---|---|
| wordcount | 93.7 ms (2.03× Go) | **~48 ms (~1.05× Go)** — medido con N1+N2 | 297 ms | ~240–260 ms |
| jsonserialize | 42.9 ms / 73.8 MB | **~34 ms (~1.15× Go)** / **~57 MB (< Go)** | 145 ms / 84 MB | ~110–125 ms / **76 MB** (V1 medido) |
| logparse | 55.9 ms (2.11×) | **~30 ms (~1.15× Go)** — medido con N1 | 156 ms | ~130–140 ms |

Los "esperado" de la columna nativa salen de binarios ya compilados y medidos en
esta máquina (§3); los de la VM son estimaciones de perfil salvo V1 (medido).

## 6. Orden de ejecución recomendado

1. Fase 0 (bench) → re-medir baseline honesto.
2. N1 mimalloc (el −40 %) → N2 ahash → re-medir.
3. V1 join (memoria, ya validado) + N3 join nativo (memoria) en el mismo PR
   temático "join sin recopia".
4. V2 ConcatN (el lever de tiempo de la VM) → medir A/B.
5. V3 memchr-split → V4 clave sin clon → medir.
6. V5/V6/V7 según lo que diga el perfil tras lo anterior.

Cada cambio de runtime va con su oráculo VM↔intérprete y por rama+PR; los
números se re-miden con el bench políglota y se anotan aquí o en PERFORMANCE.md
(crónica).
