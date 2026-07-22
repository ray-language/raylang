# jsondeserialize — investigación y plan (22 jul 2026)

El benchmark nuevo del proyecto políglota (`~/Desktop/benchmarks/jsondeserialize`)
dejó a raylang último en tiempo: **VM 998 ms (20.8× Go)** y **nativo 224 ms (4.7×
Go)**, con Go en 48 ms y Rust en 52 ms. Investigación con el método de siempre:
reproducir, atribuir, prototipar midiendo. Los prototipos D1/D2 de este documento
están **implementados y medidos** (PR `perf/jsondeserialize-ascii`); D3–D5 son el
plan restante.

## 1. ¿Está bien escrito el bench?

Razonablemente sí — es idiomático y del mismo espíritu que las otras lenguas
(búsqueda de substrings + parse_int, sin librería JSON). Pero tenía dos
desventajas objetivas frente al Go de referencia:

1. **Materializa `after_name`** (`line.substring(name_start, line.len())`): una
   copia de toda la cola del string por iteración, solo para buscar `tail` en
   ella. Go busca sobre un *slice* sin copiar. Como `tail` aparece UNA vez, se
   puede buscar directo en `line` y el resultado es idéntico.
2. **Constantes dentro del bucle** (`id_prefix`/`mid`/`tail` se redefinen por
   iteración: 3 pushes de constante extra).

**Variante B** (medida, salida byte-idéntica): busca `mid`/`tail` directo en
`line`, constantes fuera del bucle, sin `after_name`. → **VM −8.6 %, nativo
−12 %**. Recomendación: adoptarla como el `.ray` del bench (está en
`§5`). Con eso el bench queda parejo al resto; el gap restante es del motor.

## 2. Atribución del gap (motor)

### La causa dominante: `index_of` y `substring` eran O(n) **con allocs** por llamada

Ambos operan por índice de **carácter** (semántica correcta, consistente con
`len`/`chars`/`s[i]`), y su implementación materializaba **`Vec<char>` del string
completo** en cada llamada, con búsqueda por ventana ingenua (sin memchr):

- `builtins::char_index_of` / `builtins::substring_chars` (compartidos por VM e
  intérprete): 2 `Vec<char>` por `index_of`, 1 por `substring`.
- El transpilador emitía lo mismo (`__ray_index_of` con doble collect; el
  `substring` inline con collect + clamp).

El bench hace ~6 de estas llamadas por iteración × 400k: ~8 allocs de `Vec<char>`
+ escaneos redundantes por iteración, **en los dos backends**.

### La segunda capa (solo VM): el impuesto del envoltorio `Option`

`index_of`/`parse_int` son primitivos que devuelven un **arreglo etiquetado en el
heap** (`[]`/`[v]`), que un envoltorio del prelude traduce a `Option` (construye
el enum en el heap), y `.unwrap_or(d)` es otra llamada con su marco y su `match`.
Por iteración: 4 pares primitivo+wrapper ≈ **~6 allocs de heap + ~10 marcos de
llamada** que Go/Rust no pagan. Es exactamente la forma que P0.2 mató para Map
con `get_or` (medido entonces: wordcount −40 %).

El resto es el bucle de despacho del intérprete (techo estructural conocido,
~50 %+ de lo que queda; solo lo mueve P2.a/JIT).

## 3. Lo hecho (D1+D2, medido, PR `perf/jsondeserialize-ascii`)

**Fast-path ASCII** en las cuatro implementaciones: si el string es ASCII
(`is_ascii()`, barrido vectorizado ~gratis), índice de byte == índice de carácter
→ `str::find` (memchr) para `index_of` y corte por bytes con el mismo clamp para
`substring`. El caso no-ASCII sigue por el camino por carácter (semántica
intacta; test de contrato `index_of_and_substring_ascii_fast_path_matches_char_semantics`
en builtins — los helpers son compartidos, así que el oráculo VM↔intérprete no
cubriría una divergencia aquí).

| | baseline | D1+D2 | D1+D2 + variante B |
|---|---|---|---|
| VM | 992 ms | 853 ms (−14 %) | **784 ms** |
| nativo | 231 ms | **95.3 ms (−59 %)** | **83.7 ms** (Go 48, Rust 52) |

El nativo pasa de 4.7× a **~1.7× de Go**. Lo que le queda al nativo vs Rust puro:
cada `substring` de raylang produce un `Rc<str>` nuevo (el `.rs` usa slices
zero-copy) — inherente a la semántica de valores, aceptable.

## 4. Plan restante

- **D3 — fusión del envoltorio `Option`** ✅ **HECHA** (22 jul; PERFORMANCE.md
  Fase 61): la pasada `lower_prelude_fusions` (generaliza la de V5, misma guardia
  `PreludeOrigin`) reescribe `Option#unwrap_or(index_of(s,sub), d)` →
  `__index_of_or(s,sub,d)` y `Option#unwrap_or(parse_int(s), d)` →
  `__parse_int_or(s,d)` (opcodes `IndexOfOr`/`ParseIntOr`): cero arreglo
  etiquetado, cero Option, cero marcos. **Medido (muy por encima del estimado
  −25–40 %): VM 853 → 413 ms (jd) y 784 → 321 ms (jd-b) — −52/−59 %**; nativo
  neutro (su Option ya era barato); oráculo `option_unwrap_or_fusion_oracle`
  (fusionados + no-fusionados + override del usuario). El techo restante de la
  VM es el despacho (P2.a).
- **D4 — adoptar la variante B en el bench** (medida: −8.6 % VM / −12 % nativo,
  misma salida; más pareja con lo que hacen Go/JS/Rust).
- **D5 — diferidos conscientes**: `index_of_from(s, sub, start)` (búsqueda con
  offset; API nueva — solo si aparece demanda real: con D1 el re-escaneo ASCII es
  barato); substring zero-copy (vistas/ropes) = cambio de representación,
  **descartado** (Opt.3/P1.4/interning, refutado 3×); `std/json` como librería
  (existe la ruta, pero este bench compara a propósito el parsing manual).

## 5. La variante B del bench (propuesta para `jsondeserialize.ray`)

```ray
fn main() {
    let n = 400000;
    var checksum = 0;
    var total_name_len = 0;
    let mid = ",\"name\":\"";
    let tail = "\",\"score\":";
    for i in 0..n {
        let line = `{"id":${i},"name":"user${i}","score":${i % 100}}`;
        let id_end = line.index_of(mid).unwrap_or(0);
        let id_val = parse_int(line.substring(6, id_end)).unwrap_or(0);
        let name_start = id_end + mid.len();
        let name_end = line.index_of(tail).unwrap_or(0);
        let name_val = line.substring(name_start, name_end);
        let score_val = parse_int(line.substring(name_end + tail.len(), line.len() - 1)).unwrap_or(0);
        checksum = (checksum * 31 + id_val + score_val) % 1000000007;
        total_name_len = total_name_len + name_val.len();
    }
    print(checksum);
    print(total_name_len);
}
```

## 6. Dónde debería aterrizar

| | baseline (imagen) | D1+D2 (+B) | **D1+D2+D3 (+B), medido** |
|---|---|---|---|
| nativo | 224 ms (4.7× Go) | ~84 ms (~1.7×) | **84 ms (~1.7×)** |
| VM | 998 ms (20.8×) | ~784 ms | **321 ms (6.7×; 413 con el .ray original)** |

Total del arco jsondeserialize: **VM 3.1× más rápida** (998→321) y **nativo
2.7×** (224→84) — todo medido con binarios reales y salida byte-idéntica.
