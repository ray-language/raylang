# treealloc — análisis de la VM y plan (22 jul 2026)

El bench políglota `treealloc` (binary-trees: construir/contar/descartar árboles
pequeños — presión pura de allocator/GC) dio: **nativo 20 ms (¡por delante de Go,
30 ms, y de Rust, 30 ms!)**, pero **VM ~900 ms (30-45×)** y un pico de memoria de
decenas de MB donde Rust usa 2 y Go 9. Análisis con perfil + métricas
deterministas, optimizaciones aplicadas (TA1/TA2/TA4, todas medidas) y lo que
queda.

## 1. Lección metodológica: el RSS mentía

El pico RSS y hasta el `peak commit` de mimalloc resultaron **adaptativos**: el
mismo binario daba 78 MB en una tanda y 45 MB en otra (purga/arenas dependientes
del estado de la máquina), y los A/B salían "idénticos al byte" por
compensaciones. La medición de memoria de la VM se hace ahora con una **sonda
interna determinista** (`RAY_HEAP_STATS=1`, permanente): picos exactos de
objetos/bytes vivos, capacidad del vector de slots y total de
asignaciones/bytes. Con ella, cada cambio se atribuye con precisión.

## 2. Atribución

- **Tiempo (perfil simbolizado)**: ~65 % es el bucle de despacho + marcos de
  llamada (`run_loop`/`run_worker`/`push`/`new_locals`) — binary-trees es
  recursión pura y ese es el techo estructural del intérprete (P2.a). El resto:
  clones del `MakeStruct` (~8 %), la contabilidad V6 `obj_bytes` (~4 %), GC.
- **Memoria (sonda)**: pico de **313 k objetos** (cada nodo lógico eran TRES
  objetos: struct + `Some` + los `None` de las hojas), **slots de 88 B por
  objeto** (el enum `Obj` lo dimensionaba la variante `Map` de 64 B), y cada
  instancia de struct cargaba **metadatos clonados** (nombre + nombre de cada
  campo, Strings por instancia).
- **Además**: la V6 (GC por bytes) **se auto-compensa** — una mejora de
  representación reduce también los bytes contabilizados, el disparo se retrasa
  y el equilibrio de pico apenas se mueve; el pico real lo gobiernan el umbral y
  el overhead por objeto (slots + bloques), no solo el payload.

## 3. Aplicado (medido con la sonda + hyperfine, A/B baseline-main mismo perfil)

| # | qué | medido |
|---|---|---|
| **TA1** | instancias de struct **sin metadatos** (`struct_idx` + valores en orden; los nombres viven en la tabla del programa — acceso/Show/borde los consultan ahí) | bytes por struct 112→64 contabilizados; churn total de treealloc 230→154 MB (−33 %) |
| **TA2** | **Slot 88→48 B**: `Obj::Map` boxeado (la variante grande ya no dimensiona el enum), ids de `VmEnum` a u32, `Slot.bytes` a u32 saturado; guardia de regresión `slot_stays_small` | vector de slots −45 % por ranura |
| **TA4** | **singleton por variante de enum sin payload** (`Option.None`…): inmutable y sin identidad observable → un handle canónico por fibra (raíz del GC) en vez de una asignación por construcción | **allocs totales 4.82 M → 3.20 M (−34 %)**; pico 313 k → 251 k objetos; slots 524 k → 262 k entradas (**46 → 12.6 MB, −73 % acumulado**) |

**Tiempo** (hyperfine, misma tanda): treealloc **~945 → ~805 ms (−15 %)**.
**Coste en los benches de servicios**: +1–2 % (borde del ruido; la indirección
del Map boxeado + los contadores de la sonda) — asumido frente a lo anterior.
Batería completa verde (102 suites + corpus nativo + concurrencia + naming); la
salida de treealloc y de todos los oráculos, intacta.

## 4. Lo que queda (por ROI, sin aplicar — cada uno exige su medición)

- **TA5 — niche de `Option`/enums de 1 variante-con-payload**: colapsar
  `Some(x)` al propio `x` y `None` a un centinela cuando el payload es un objeto
  de heap y el tipo no anida Option (el checker lo sabe; erasure-nivel). Pasaría
  de 2 objetos por nodo a 1 → el siguiente −40-50 % de objetos/allocs de este
  perfil. Es un cambio de representación GRANDE (checker + ambos motores +
  transfer + match) — hacerlo solo con un diseño cuidado del caso
  `Option<Option<T>>` y midiendo el barrido completo.
- **TA6 — marcos/locales sin asignar por llamada**: `new_locals` aparece en el
  perfil (un Vec de locales por llamada). Un slab/stack contiguo de locales
  (estilo clox) atacaría el 65 % estructural… pero es cirugía del corazón de la
  VM (celdas/upvalues M4.2 viven ahí). Evaluar junto a P2.a (JIT), no antes.
- **TA7 — payload inline de enums de aridad 1** (`Some` carga un `Vec` de 1:
  un malloc por Some): variante `payload: SmallVec1` (inline hasta 1 elemento).
  Menor que TA5 y sin sus riesgos semánticos; medir si TA5 no se acomete.
- **El nativo**: sin acción — 20 ms, por delante de Go y de la variante Rust del
  bench (`Rc<RefCell>` + drop recursivo incluidos). El modelo dev=VM /
  deploy=nativo cubre este perfil con nota alta.

## 5. Dónde queda el bench

| variante | tiempo | pico (RSS orientativo) |
|---|---|---|
| ray nativo | **20 ms** 🥇 | 4.7 MB |
| Rust / Go | 30 ms | 2 / 9.3 MB |
| ray VM (antes) | ~900 ms | 45–78 MB (adaptativo) |
| ray VM (TA1+2+4) | **~805 ms** | slots −73 %, allocs −34 % (sonda) |

El gap restante de la VM es el despacho+marcos (P2.a/JIT o TA5/TA6); la memoria
restante la gobiernan el equilibrio del GC (V6) y los ~48 B/objeto de slot — con
TA5 el número de objetos caería a la mitad otra vez.
