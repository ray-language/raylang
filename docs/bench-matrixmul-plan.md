# matrixmul — análisis de la VM y optimizaciones (22 jul 2026)

El bench políglota `matrixmul` (200×200, el único con aritmética flotante):
**nativo 31 ms** (3.2× Go 9.7 / 4.3× Rust 7.3 — bueno, con margen), **VM 1.43 s**
(~150× Go), memoria sana (8.8 MB). Análisis, tres optimizaciones aplicadas y
medidas, y lo que queda.

## 1. Atribución (perfil simbolizado)

El bucle interno (`s = s + a[i][k] * b[k][j]`, 8M iteraciones) ejecutaba ~15-16
despachos del intérprete por iteración, y además:

- La aritmética **flotante caía SIEMPRE a `apply_binary`** (el fast-path del
  bucle, Opt.4, solo cubría Int/Int): doble match + ~30 combinaciones, dos veces
  por iteración.
- Cada `a[i]` eran 3 despachos (`GetLocalLocal` + `Index`), y cada nivel extra
  otros 2 (`GetLocal` + `Index`).
- `[float]` se almacenaba como `Vec<HeapValue>` (32 B/elemento): 4× menos
  densidad de caché justo donde `b[k][j]` recorre por columnas (la P1.2 del plan
  de rendimiento, diferida entonces por "cómputo, no servicios" — este bench ES
  cómputo).

## 2. Aplicado (todo con A/B contra baseline de main del mismo perfil)

| # | qué | medido (matrixmul) |
|---|---|---|
| **MM1** | fast-path FLOTANTE en el bucle (gemelo del entero de Opt.4; semántica IEEE idéntica al camino general) | 1.41 → 1.30 s (**−7.6 %**) |
| **MM2** | fusión de INDEXACIÓN (ronda 4): `[GetLocalLocal, Index]` → `IndexLL` y `[GetLocal, Index]` → `IndexLocal`, con la semántica completa compartida en `do_index` (arrays/IntArray/FloatArray/string/bytes + bounds) | acumulado 1.42 → 0.98 s (**−31 %**) |
| **MM3** | **`FloatArray`** (la P1.2): arreglo homogéneo de floats a 8 B/elemento, gemelo de `IntArray` (nace en literal homogéneo o promoción del vacío al primer push; degrada por el mismo embudo; GC O(1), sin hijos) | acumulado 1.43 → **0.93 s (−35 %)**; la matriz pasa de 3.8 MB a ~1 MB |

Bonus medido en el resto de la suite (la fusión de indexado es general):
wordcount **−5 %**, sortnums **−4.3 %**, logparse −2.5 %; treealloc neutro.
Checksum idéntico en todo momento; batería completa (102 suites) + corpus nativo
entero + naming, verdes.

## 3. Lo que queda

- **VM**: el perfil restante es ~64 % bucle de despacho + marcos (`run_loop`/
  `run_worker`) — el techo estructural del intérprete: ~9 despachos por
  iteración que solo borra P2.a (JIT) o más superinstrucciones de grano grueso
  (p. ej. fusionar `Mul;Add` flotante o un opcode `MulAddLocals`; ROI decreciente,
  medir antes). `do_index`+`get_local`+`bounds_check` ≈ 25 % — micro-margen en
  evitar el doble `heap.get` del camino genérico del índice.
- **Nativo**: ✅ **N6 EJECUTADO** (29 jul 2026, PERFORMANCE.md Fase 67). El gap eran los
  `borrow()` de `RefCell` por elemento (bloqueaban la vectorización): N6a elide el
  `Rc::clone` del receptor en lecturas indexadas y N6b iza los `borrow()` fuera de los
  `for` de rango con cuerpo **puro-escalar** (análisis conservador: llamadas, mutación
  de arreglos, strings o control de flujo lo desactivan — el cuidado con el aliasing
  que pedía este plan). Medido: nativo **11.24 → 5.72 ms (1.97×)**, byte-idéntico —
  **#1 de la mesa** (empate estadístico con `rustc -O`, Go 1.35× detrás).
- Descartado implícito: cambiar la forma del bench (ya es idiomático y paralelo
  al resto de lenguas).

## 4. Dónde queda

| variante | antes | ahora |
|---|---|---|
| ray VM | 1.43 s | **0.93 s (−35 %)** |
| ray nativo | 31 ms (sin cambios; 3.2× Go) | — |
| Go / Rust | 9.7 / 7.3 ms | — |
