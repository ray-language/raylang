# Estrategia de subdivisión de los archivos grandes

> Documento de diseño (17 jul 2026; **actualizado 20 jul 2026** — cifras refrescadas, el plan de
> partición sigue sin ejecutarse). Objetivo: mejorar la mantenibilidad y la organización del
> repo dividiendo los archivos que han crecido más allá de lo razonable, **sin cambiar ni una línea
> de lógica ni la API pública**. Este documento presenta el diagnóstico, las opciones evaluadas, la
> recomendación y el mapa de partición archivo por archivo. La ejecución es un trabajo aparte
> (por lotes, un archivo por commit).
>
> **Estado (20 jul 2026): nada de esto se ha ejecutado todavía** — los cuatro archivos grandes
> siguen siendo archivos únicos (`src/checker.rs` etc., no `src/checker/`); no hay branch ni PR
> previos sobre este tema. En los tres días transcurridos crecieron ~1–2 % más (self-hosting +
> concurrencia M12 siguieron tocándolos) pero el diagnóstico y el mapa de partición del §4 siguen
> vigentes tal cual.

## 1. Diagnóstico

Tamaños actuales (líneas) y frecuencia de cambio (commits que tocan el archivo, últimas 6 semanas):

| Archivo | Líneas | Churn | De ellas, tests | Núcleo del problema |
|---|---:|---:|---:|---|
| `src/checker.rs` | 7 849 | 124 | ~1 710 | Un `impl Checker` de ~3 100 líneas + 9 secciones de lowering |
| `src/vm.rs` | 7 221 | **188** | ~3 100 | Bucle de despacho + scheduler + GC + **la mitad es tests** |
| `src/transpile.rs` | 6 220 | 86 | ~810 | Emisión + runtime embebido (strings) + análisis + tipos, todo junto |
| `src/lsp.rs` | 4 987 | 79 | ~1 340 | 7 features LSP + transporte JSON-RPC + docs en un archivo |
| `src/parser.rs` | 2 598 | — | — | Aceptable (descenso recursivo cohesivo) |
| `src/builtins.rs` | 2 571 | — | — | **No dividir**: es el registro único (L1), su valor es estar junto |
| `src/interpreter.rs` | 2 444 | — | — | Aceptable (oráculo, cohesivo) |
| `src/cli.rs` | 2 174 | — | — | Aceptable |
| `src/fmt.rs` | 1 600 | — | — | Por debajo del umbral; vigilar |
| `src/loader.rs` | 1 543 | — | — | Por debajo del umbral |
| `src/compiler.rs` | 1 423 | — | — | Por debajo del umbral |

(Churn no recalculado para las filas sin variación de diagnóstico — ver §4.5 para la lista completa
de "no se divide". `transpile.rs` tiene además dos bloques `#[cfg(test)]` pequeños en cabecera,
líneas 184 y 222, además del `mod tests` grande en 5415 — no cambia el mapa de partición del §4.3.)

Observaciones que guían la estrategia:

- Los cuatro grandes ya tienen **costuras internas marcadas** (cabeceras `// ====` en checker,
  structs del scheduler en vm, secciones de análisis/emisión/tipos en transpile, una función por
  feature en lsp). La partición no hay que inventarla: hay que **materializarla en archivos**.
- En `vm.rs` y `checker.rs`, entre el 22 % y el 45 % del archivo son los `mod tests`. Solo
  extraerlos ya cambia la experiencia de edición.
- Los cuatro usan el patrón "un struct con un `impl` gigante" (`Checker`, `Vm`, `Transpiler`).
  Rust permite **varios bloques `impl` del mismo tipo repartidos por submódulos** del mismo
  crate — es la técnica estándar (así están hechos rustc y rust-analyzer) y no cambia nada
  semánticamente.

## 2. Opciones evaluadas

### Opción A — módulo-directorio con `impl` repartidos (RECOMENDADA)

Cada archivo grande pasa de `src/foo.rs` a `src/foo/` con un `mod.rs` que conserva **exactamente
la misma API pública** (`crate::foo::X` sigue funcionando; `lib.rs` y los clientes no se tocan).
Los métodos del struct central se reparten en submódulos temáticos como bloques `impl` adicionales;
los helpers libres van con la sección que los usa; los tests, a `tests.rs` del propio módulo.

- **Pros**: archivos de 300–1 500 líneas navegables; el diff de un cambio futuro toca el archivo
  de su tema; cero cambios de API ni de comportamiento; compatible con la convención de tests del
  proyecto (siguen viviendo *dentro del módulo de su fase*, solo que en `foo/tests.rs`);
  reversible; se puede hacer archivo a archivo.
- **Contras**: churn puntual de `git blame` (mitigable: commits de movimiento puro + `git log
  --follow`); algunos ítems privados pasan a `pub(super)`/`pub(crate)` (único cambio textual
  permitido).

### Opción B — extraer solo los `mod tests` (paso 1 de A)

Mover cada `#[cfg(test)] mod tests` a `src/foo/tests.rs` (o `foo_tests.rs` con `#[path]`).
Gana de golpe: `vm.rs` −3 100 líneas, `checker.rs` −1 700, `lsp.rs` −1 300, `transpile.rs` −800.

- **Pros**: riesgo casi nulo, una tarde, beneficio inmediato.
- **Contras**: no resuelve la organización del código de producción. **No es una alternativa
  sino la primera fase de A** — así se adopta aquí.

### Opción C — workspace multi-crate (`raylang-front`, `raylang-vm`, …) (DESCARTADA por ahora)

Separar fases en crates del workspace (como ya se hizo con `crates/ray-runtime`).

- **Contras decisivos**: rompe la simplicidad de build de la toolchain (un binario, un crate);
  las fronteras reales del proyecto no son limpias entre crates (`runtime.rs` lo comparten
  intérprete y VM; `builtins.rs` lo consultan checker/compilador/intérprete; los tests oráculo
  cruzan motores); obligaría a publicar como `pub` mucha superficie hoy interna; y no aporta
  nada que A no dé para el objetivo declarado (mantenibilidad). Reconsiderar solo si algún día
  se quiere publicar el front-end como librería independiente.

**Recomendación: A, ejecutada de forma incremental (B es su primer paso), en el orden §5.**

## 3. Mecánica segura (reglas de ejecución)

1. **Movimiento puro**: un commit de partición no cambia lógica, mensajes ni firmas. Solo se
   permiten: `mod`/`use`, visibilidad mínima necesaria (`pub(super)`/`pub(crate)`) y docs de
   cabecera `//!` en cada archivo nuevo (reciclando las cabeceras `// ====` actuales).
2. **Un archivo grande por commit** (`refactor(vm): divide vm.rs en módulo vm/ …`), con
   `cargo test` verde entre commits. Nada de partir dos a la vez.
3. **API congelada**: `mod.rs` mantiene los `pub use`/ítems públicos con los mismos paths. Guardia:
   `git grep 'checker::\|vm::\|transpile::\|lsp::' src tests` antes y después debe compilar sin
   tocar ningún consumidor.
4. **Tests**: siguen siendo `#[cfg(test)]` del módulo (convención del proyecto), en
   `src/foo/tests.rs` (`#[cfg(test)] mod tests;` en el `mod.rs`). Los tests oráculo de la VM no
   cambian de nombre (los filtros `cargo test oracle_` siguen funcionando).
5. **Tamaño objetivo**: ningún archivo nuevo > ~1 500 líneas; no fragmentar por debajo de ~200
   (demasiados archivos también cuesta).
6. **Blame**: anotar en el mensaje de commit "movimiento puro; usar `git log --follow`".
7. Después de cada partición, correr los tests dirigidos del módulo + una pasada de
   `cargo clippy` (no debe aparecer ningún warning nuevo).

## 4. Mapa de partición por archivo

Las líneas citadas son las actuales (17 jul 2026), para localizar cada bloque al ejecutar.

### 4.1 `src/checker.rs` (7 849) → `src/checker/` (7 archivos)

| Archivo nuevo | Contenido (líneas actuales) | Tamaño estimado |
|---|---|---:|
| `mod.rs` | API pública (`check`, `semantic_index`, `prepare_program`/`check_program`), structs `TypeError`/`FnSig`/`VarInfo`/`Checker`/`SemanticIndex` (39–592) y declaración de submódulos | ~700 |
| `core.rs` | El grueso del `impl Checker`: expr/stmt/llamadas/ámbitos/divergencia (593–3700) — si al moverlo se ve una costura natural (p. ej. `check_call` y la resolución por punto), partir en `core.rs` + `calls.rs` | ~1 500 ×2 |
| `aux.rs` | "Auxiliares libres" (3700–3941): `subst`/`unify`/helpers puros | ~250 |
| `enums.rs` | Resolución de construcción de enums, M5 (3942–4080) | ~150 |
| `traits.rs` | Auxiliares de traits M9 + `@derive` M10.1 (4081–5163) | ~1 000 |
| `lowering.rs` | Las 6 bajadas post-check: UFCS/dicts/dyn/uint/`?`-conv/operadores (5164–6053). Son secciones de ~150 líneas: UN archivo con las cabeceras actuales como separadores | ~900 |
| `tests.rs` | `mod tests` (6141–7849, actualizado 20 jul) | ~1 700 |

Nota: el espejo selfhost (`selfhost/checker.ray`) **no se toca** — la partición es solo del host
Rust y no cambia ningún mensaje.

### 4.2 `src/vm.rs` (7 221) → `src/vm/` (6 archivos)

| Archivo nuevo | Contenido (líneas actuales) | Tamaño estimado |
|---|---|---:|
| `mod.rs` | API (`run_program`/`run`/`set_deterministic`, `num_workers`), struct `Vm` y el **bucle de despacho** (el `match` de opcodes, 285–~3000) | ~1 500 |
| `sched.rs` | El scheduler: `Fiber`/`ScopeFrame`/`Waiting`/`Parked`/`IoParked`/`Shared` (161–284) + los métodos de scheduling del `impl Vm` (`poll_next`, `wake_*`, `cancel_task`, `fail_current_fiber`, aparcamiento de E/S, M38 workers) | ~1 200 |
| `values.rs` | Conversión y formato de valores: `const_to_heap`/`values_equal`/`format_value`/`to_value`/`heap_to_key` (3490–3650, 3741–3812) | ~250 |
| `transfer.rs` | M38.1a transferencia de subgrafo entre heaps (3650–3740) | ~100 |
| `tests.rs` | `mod tests` (4120–7221, actualizado 20 jul) — si estorba, segunda pasada: `tests/oracle.rs` (los `oracle_*`) vs `tests/unit.rs` | ~3 100 |

El corte exacto despacho-vs-scheduler dentro del `impl Vm` se decide al ejecutar (regla: si el
método toca `Shared`/`parked`/`ready`, va a `sched.rs`).

### 4.3 `src/transpile.rs` (6 220) → `src/transpile/` (7 archivos)

| Archivo nuevo | Contenido (líneas actuales) | Tamaño estimado |
|---|---|---:|
| `mod.rs` | API (`transpile`/`transpile_with`/`transpile_with_opts`), struct `Transpiler`/`Transpiled`, orquestación del pipeline (581–1400 sin el runtime) | ~600 |
| `names.rs` | Identidad y clasificación de nombres: `mangle`, `is_rust_keyword`, `is_prelude_impl`, `is_handled_builtin`, `skip_fn_def`, `resolve_callee`, guardia H11 `NATIVE_TRACKED_BUILTINS` (26–257) | ~250 |
| `analysis.rs` | Análisis previo: `spawn_fn_param_marks` (punto fijo N5c), walkers `visit_exprs_*`/`idents_of_*`/`captured_idents_*`/`cell_vars` (258–580) | ~350 |
| `runtime.rs` | El **runtime embebido** (los bloques `concat!` de canales/Task/scope/select/signals/PRNG/handles/FFI, hoy ~700–1400 dentro de `transpile_with_opts`): función(es) que devuelven/emiten el preámbulo. Es texto, no lógica — el candidato más limpio a archivo propio | ~700 |
| `emit.rs` | `impl Transpiler`: funciones/stmts/exprs/match/literales (1399–2860 aprox.) | ~1 400 |
| `calls.rs` | `emit_call` (~735 líneas) + `emit_call_arg` + emisión de builtins + `emit_send_convs`/spawn-captures N5 (2288, 2861–4620 aprox.) | ~1 500 |
| `types.rs` | Sistema de tipos del backend: `type_of`, `classify`, `normalize_type`, `rust_ty`, `send_type`/`send_is_tree`, `unify`/`subst_type`, helpers FFI (4203–5070) | ~900 |
| `tests.rs` | `mod tests` (5415–6220, actualizado 20 jul; hay además dos bloques `#[cfg(test)]` pequeños en cabecera, 184/222, que se mueven junto con ellos) | ~810 |

### 4.4 `src/lsp.rs` (4 987) → `src/lsp/` (6 archivos)

| Archivo nuevo | Contenido | Tamaño estimado |
|---|---|---:|
| `mod.rs` | `run`/`serve` + dispatch de mensajes | ~300 |
| `json.rs` | El `mod json` interno (3304–3651, actualizado 20 jul; parser+serializador) — ya es un módulo, solo cambia de sitio; su `mod tests` interno (3611) le acompaña | ~350 |
| `protocol.rs` | Framing (`read_message`/`send`), extracción de params, helpers de rangos/posiciones | ~300 |
| `features.rs` | hover, definición, referencias+rename, completion, signature help — si supera ~1 500, partir en `hover_def.rs` / `refs_rename.rs` / `completion.rs` | ~1 800 |
| `docs.rs` | Documentación de símbolos (`doc_of_symbol`, docs de builtins/prelude) | ~400 |
| `tests.rs` | `mod tests` (3652–4987, actualizado 20 jul) | ~1 340 |

### 4.5 Lo que NO se divide (y por qué)

- **`src/builtins.rs` (2 571)**: es el **registro único** de L1 — su valor es precisamente que
  cada builtin sea UNA fila en UN sitio. Dividirlo recrearía el problema que L1 eliminó.
- **`src/parser.rs` (2 598)**: descenso recursivo; la jerarquía de precedencia se lee de arriba
  a abajo — partirla rompería la narrativa. Vigilar si supera ~3 500.
- **`src/interpreter.rs` (2 444)**: el oráculo; cohesivo y estable.
- **`src/cli.rs` (2 174), `src/fmt.rs` (1 600), `src/loader.rs` (1 543), `src/compiler.rs`
  (1 423)**: por debajo del umbral (20 jul).

## 5. Orden propuesto y esfuerzo

Prioridad = tamaño × churn × riesgo bajo. Cada paso es un commit independiente con suite verde.

1. **`vm.rs`** (el más caliente: 188 commits/6 semanas y 45 % tests). Empezar por extraer
   `tests.rs` (riesgo ~0), luego `sched.rs`/`values.rs`/`transfer.rs`. — ~medio día.
2. **`transpile.rs`** (foco actual del trabajo nativo). `runtime.rs` y `tests.rs` primero
   (texto y tests), luego `analysis.rs`/`names.rs`/`types.rs`, y por último el corte
   `emit.rs`/`calls.rs`. — ~medio día.
3. **`checker.rs`** (el más grande; churn medio). `tests.rs` + `lowering.rs` + `traits.rs`
   primero; el corte fino de `core.rs` al final (es el de más criterio). — ~1 día.
4. **`lsp.rs`** (menos crítico: cliente externo). — ~medio día.

Total estimado: **2–3 días** repartibles; cada paso deja el repo mejor sin bloquear nada.

## 6. Riesgos y mitigaciones

| Riesgo | Mitigación |
|---|---|
| Romper lógica al mover | Commits de movimiento puro + suite dirigida por módulo entre commits |
| Perder `git blame` | `git log --follow`; anotarlo en cada mensaje de commit |
| Visibilidad: privados usados entre submódulos | `pub(super)` mínimo; nunca `pub` nuevo en la API del crate |
| Choque con ramas abiertas | La rama nativa (PR #18) ya se fusionó (20 jul); no hay ramas largas abiertas ahora mismo — buen momento para ejecutar sin choque. Revisar `git branch -a`/PRs abiertos antes de arrancar cada paso, por si hay trabajo nuevo en curso sobre alguno de los cuatro archivos |
| Convención "cada fase lleva sus tests en su archivo" | Se conserva en espíritu: los tests viven en `foo/tests.rs`, dentro del módulo de su fase (actualizar la línea de CLAUDE.md al ejecutar) |
