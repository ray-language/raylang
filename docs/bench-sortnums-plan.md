# sortnums — el caso "la VM gasta menos que el nativo" (22 jul 2026)

Caso especial del bench políglota `sortnums` (ordenar 1M de ints): la **VM usaba
menos memoria pico que el binario nativo** — 28.8 MB vs 34.1 MB (Go 11.1, Rust
17.1). Sorprendente hasta que se lee el Rust emitido; dos causas concretas, dos
fixes medidos.

## 1. Por qué pasaba

Leyendo el `--emit-rust` de sortnums:

1. **`for v in sorted` clonaba el `Vec` ENTERO para iterar**
   (`for v in sorted.clone().borrow().clone()`): +8.4 MB en un arreglo de 1M.
   La VM, en cambio, itera **por índice sobre el arreglo vivo** (longitud tomada
   al entrar, `emit_counted_loop`) — además de más barato, es la semántica de
   referencia; el clon era también una divergencia latente bajo mutación del
   arreglo dentro del cuerpo.
2. **`__ray_sort` usa el sort ESTABLE de Rust**, que asigna un buffer temporal de
   n/2 (+4.2 MB aquí). La VM, desde V5, ordena los primitivos con
   `sort_unstable` (para int/string/char es observacionalmente idéntico — los
   empates de primitivos son valores indistinguibles).

La VM "ganaba" porque su `IntArray` (8 B/elemento) + `sort_unstable` in place
sobre el clon no pagaban ninguno de esos dos extras.

## 2. Aplicado (medido, salida idéntica)

- **SN1 — `__ray_sort_unstable` para la forma fusionada `__sort_prim`**: el
  checker solo la genera para primitivos → inestable seguro; los tipos de
  usuario (estabilidad observable vía su `Ord`) siguen en `__ray_sort` estable.
- **SN2 — `for-in` sobre arreglos SIN clonar**: bucle por índice sobre el
  arreglo vivo con longitud al entrar (la semántica de la VM), soltando el
  préstamo por elemento antes del cuerpo (que puede mutar) y con el incremento
  antes del cuerpo (`continue` correcto). El caso string (`.chars()`) no cambia.

| sortnums nativo | antes | después |
|---|---|---|
| pico de memoria | 34.1 MB | **26.1 MB (−24 %)** |
| tiempo | 23.3 ms | **20.4 ms (−12 %)** |

El nativo vuelve a quedar por debajo de la VM (26.1 vs 28.8). Regresiones: cero
(wordcount neutro-positivo; jsonserialize/treealloc no usan estos caminos);
batería completa (102 suites) + corpus nativo entero + naming, verdes.

## 3. Lo que queda (consciente, sin acción)

- El resto del gap con Rust (26.1 vs 17.1 MB) es estructural y compartido con la
  VM: la semántica de `sort` (devuelve arreglo NUEVO → original + ordenado vivos
  a la vez, 16.8 MB) + el crecimiento por duplicación del push-loop. Un análisis
  de último-uso (liberar `arr` si no se usa tras el `sort`) sería el siguiente
  paso teórico — coste/beneficio dudoso hoy.
- La VM (28.8 MB, 210 ms) queda bien para su papel: 8 B/elemento (`IntArray`) y
  el sort nativo de V5; su distancia a Go es el clon semántico + slots.
