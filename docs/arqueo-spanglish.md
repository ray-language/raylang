# Arqueo completo: identificadores en español/spanglish (20 jul 2026)

> **Nota (21 jul 2026)**: documento HISTÓRICO — el arqueo se aplicó completo y los scripts
> (`tools/arqueo_spanglish.py`, `tools/spanglish.py`) fueron retirados; viven en el historial de
> git. La detección quedó absorbida por `tests/naming_policy.rs` (que hoy cubre también `tests/`
> y `tools/`, parte por `_`/CamelCase y compara case-insensitive).

> **Disparador**: la sospecha del usuario de que hay "más lugares" que los 83 sitios que reporta
> `tests/naming_policy.rs`. **Confirmada con creces: 1219 declaraciones con español confirmado**
> (14× lo que ve el test), más un residuo de ~2000 marcas de baja confianza a curar. Este documento
> registra la metodología, todos los hallazgos, las inconsistencias de las reglas, y el plan por
> lotes — **sin aplicar ningún cambio de código** (decisión pendiente del usuario).

## 1. Metodología (reproducible)

Script: **`tools/arqueo_spanglish.py`** (python3, cero deps; corre en segundos). Más amplio que el
test en dos ejes:

- **Qué extrae**: el test solo mira `fn`/`let`/`var`; el arqueo añade `struct`/`enum`/`trait`/
  `const`/`static`, **campos** de struct y **parámetros** — en `.rs` y `.ray` de los cinco
  directorios bajo la política (`src`, `tests`, `selfhost`, `packages`, `benchmarks`; los snippets
  raylang embebidos en strings de test caen solos, el escaneo es por línea).
- **Cómo detecta**: (a) la wordlist curada del repo (248 tokens) **+ expansión** (~700 palabras
  españolas comunes en código, con los falsos amigos inglés≡español excluidos: error/total/final/
  color/…); (b) heurística de diccionario — token que no es inglés (web2 + flexiones) ni jerga
  técnica conocida → "sospechoso" (baja confianza, para curación).

Los identificadores se parten en tokens (snake_case y CamelCase); `// es-ok` exime la línea, como
en el test. Detalle completo del cubo confirmado: **`docs/arqueo-spanglish-detalle.txt`**
(1219 líneas: `archivo:línea → tipo de declaración → identificador → tokens españoles`).

## 2. Resultados

**1219 declaraciones con español confirmado** en 163 archivos (el test reporta 83 — ve solo
fn/let/var y su wordlist se quedó corta en ~350 tokens).

| Corte | es confirmado |
|---|---:|
| Por directorio | src 598 · tests 537 · packages 75 · selfhost 6 · benchmarks 3 |
| Por tipo de declaración | fn 825 · let 134 · struct 88 · param 64 · enum 46 · trait 33 · var 21 · campo 8 |
| Por lenguaje | Rust 1135 · raylang 84 |

**El corte que ordena el plan — dónde vive el español de `src/` (598):**

- **Solo 39 en código de producción** (`compiler.rs` 10, `checker.rs` 8, `cli.rs` 7, `templ.rs` 6,
  `builtins.rs`/`lib.rs`/`lsp.rs` 2, `loader.rs`/`vm.rs` 1) — la limpieza L1 sí se hizo; esto es
  goteo posterior.
- **559 dentro de `#[cfg(test)] mod tests`**: nombres de funciones de test
  (`transpila_fib_recursivo`, `oraculo_*`…) y **fixtures raylang embebidas** (`struct Caja<T>`,
  `Punto`, `Figura`, `Lista`, `trait Mostrable`… — el corpus histórico de los tests de genéricos/
  traits).

En `tests/` (537) el patrón es el mismo: nombres de test en español (404 fn) + fixtures. En
`packages/` (75) son **helpers privados** de `net/http.ray` y `net/webserver.ray`
(`leer_con_plazo`, `conectar`, `loop_iter_servidor`…) — la API `pub` de cara al usuario está
esencialmente limpia.

Top tokens: `valor` 65 · `punto` 49 · `servidor` 40 · `por` 40 · `caja` 40 · `transpila` 39 ·
`linea` 28 · `como` 24 · `leer` 23 · `los` 17 · `plazo` 16 · `figura` 14 · `etiqueta` 14.

**Residuo de baja confianza**: 2039 marcas "sospechoso" adicionales — mayormente jerga inglesa que
web2 no trae (payload/enums/callee…), pero con españoles reales intercalados que la curación de
este arqueo ya promovió al cubo confirmado en su mayoría; una pasada final de curación va incluida
en el lote D del plan.

> **Decisión del usuario (20 jul 2026)**: el directorio **`tests/` queda excluido del arqueo** — sus
> 537 declaraciones confirmadas (404 fn + fixtures) no se tocan en ningún lote. El total accionable
> baja de 1219 a **682** declaraciones (163 archivos → los de `tests/` salen del recuento). La wordlist
> del futuro `naming_policy.rs` (lote E) tampoco debe exigir inglés ahí.

## 3. Inconsistencias en las reglas y su enforcement

1. **CLAUDE.md §Convenciones dice "limpieza … COMPLETA"** (y `docs/limpieza-nombres-en-ingles.md`
   dice "Estado: COMPLETA" en el título). Era cierto para su alcance original (L1 core Rust + L2
   core raylang + L3 métodos de trait), pero **hoy es falso como afirmación general**: el mismo
   CLAUDE.md amplió el alcance a "TODO el código … incluidos los nombres de funciones de test y los
   snippets raylang embebidos", y bajo ese alcance hay 1219 violaciones. Los dos documentos deben
   dejar de decir COMPLETA o acotar a qué lotes aplica.
2. **El test se quedó corto para la regla que debe defender**: cubre 3 de los ~8 tipos de
   declaración (sin struct/enum/trait/param/campo/const) y su wordlist (248 tokens) no conoce
   `valor`, `punto`, `servidor`, `leer`, `plazo`… Por eso ve 83 donde hay 1219.
3. **El enforcement está doblemente roto**:
   - `tests/naming_policy.rs` **falla en main** (los 83 que sí ve) — la política se viola en verde
     aparente.
   - **CI no corre desde ~14 jul**: GitHub Actions no arranca ningún job ("account payments have
     failed or spending limit needs to be increased" — facturación, acción del usuario en
     Settings → Billing). 100 runs seguidos fallidos en 2–4 s; **los PRs #19–#27 se fusionaron sin
     CI**. Esto explica que nadie viera el test rojo.
4. **Causa raíz del goteo**: el código nuevo se escribe imitando el estilo del código vecino (los
   tests históricos en español), y sin gate operativo nada lo frena. Aplica también al asistente:
   varios tests de esta semana (M97/M98) se nombraron en español siguiendo el patrón local.
5. Menor: el encabezado del test dice que la wordlist se debe alimentar ("si aparece un token
   español nuevo… añádelo") — proceso manual que en la práctica nadie ejecuta; conviene que la
   wordlist del test absorba la lista curada del arqueo.

## 4. Plan por lotes (COMPLETO — lotes A–E aplicados, ver §6)

Ordenado por riesgo/beneficio; cada lote compila y pasa la suite. Los renombres son mecánicos
(regex/`tools/spanglish.py` como en la migración original), el riesgo vive en dos gotchas:

> ⚠️ **Gotcha 1 — mensajes byte-idénticos**: los fixtures embebidos (`Caja`, `Punto`, `Mostrable`)
> aparecen en **asserts de mensajes de error** (`'Caja' no implementa…`) y en goldens de `show()`.
> Renombrar el fixture obliga a tocar el assert en tándem, y donde el espejo selfhost asevera el
> mismo mensaje, **en tándem con el espejo** (`selfhost/checker.ray`).
> ⚠️ **Gotcha 2 — nombres de test**: `cargo test <filtro>` y scripts/documentación que filtren por
> nombre dejan de casar. Barrido de referencias antes de cada lote.

- **Lote A COMPLETO (PR #28)** — producción `src/` (39 sitios, 33 renombrados + 6 falsos positivos
  documentados: `indices`/`variable` en compiler.rs, `regen` en cli.rs — jerga inglesa válida).
- **Lote B COMPLETO (PR #29)** — helpers privados de `packages/` (75) + selfhost (6) + benchmarks
  (3). Verificado con suites de red + oráculo.
- **Lote C COMPLETO (PR #30)** — nombres de funciones de test en `src/` mod tests (~395 fn),
  ejecutado por 6 agentes en paralelo sobre archivos disjuntos. **`tests/` (404 fn) quedó FUERA del
  alcance** por decisión del usuario (20 jul 2026) — no se toca en ningún lote.
- **Lote D COMPLETO (PR #31)** — fixtures raylang embebidas (`Caja`→`Box`, `Punto`→`Point`,
  `Figura`→`Shape`, `Valor`→`Value`, `Lista`→`List`… 164 sitios en 10 archivos), ejecutado por 3
  agentes. Gotcha 1 (mensajes byte-idénticos) resuelto propagando cada rename a construcciones,
  impls, match y strings de assert dentro del mismo fixture; se recalcularon offsets de columna en
  tests de hover del LSP cuando el nuevo nombre cambió de longitud.
- **Lote E COMPLETO** — (1) `naming_policy.rs` reescrito: gana los declaradores que faltaban
  (struct/enum/trait/const/static/param/campo), **enmascara comentarios y contenido de strings/raw
  strings** antes de buscar declaraciones (la causa raíz de varios falsos positivos: fixtures
  raylang embebidas y mensajes de assert se leían como código real), y absorbe la wordlist curada
  completa del arqueo (`wordlist` + `expansion` + `curados` de `tools/arqueo_spanglish.py`, 248→1109
  tokens) más un set `FALSOS_AMIGOS` (`variable`, `indices`, `regen`, `configurable`, `bitops`,
  `ancount`, `saslname` — palabras iguales en inglés y español que el propio arqueo había
  clasificado mal). (2) `docs/limpieza-nombres-en-ingles.md` corregido en este lote: ya no afirma
  "COMPLETA" sin acotar a qué lotes aplica; `CLAUDE.md` ya se había corregido en el lote C (su
  claim "L1+L2+L3 está COMPLETA" es correcto tal cual — está acotado a ese alcance original — y el
  bullet "Alcance del inglés" ya documenta la exclusión de `tests/` y enlaza este arqueo). (3)
  **Reactivar el billing de GitHub Actions queda pendiente — es acción del usuario** (Settings →
  Billing); sin eso el gate de CI no corre pese a estar arreglado.

**Sobre el costo en tokens** (preocupación explícita del usuario): los lotes B–D son puro
find/replace dirigido por `docs/arqueo-spanglish-detalle.txt` — se pueden ejecutar con scripts
(estilo `tools/spanglish.py`) leyendo al contexto solo los diffs conflictivos (asserts de mensajes,
espejos), no los archivos enteros. El arqueo mismo se hizo así (script + resúmenes). Estimación:
lotes A+B+E en una sesión corta; C en 1–2 sesiones mecánicas; D en 1–2 sesiones con verificación
de oráculo.

## 5. Reproducir

```sh
python3 tools/arqueo_spanglish.py /tmp/detalle.txt   # resumen a stdout, detalle al archivo
```
