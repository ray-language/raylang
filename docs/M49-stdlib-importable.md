# M49 — stdlib importable: sacar familias de builtins a módulos `std/`

Continuación natural de **M48** (descongestionar el namespace de **valores**). Igual que M48 movió los
builtins de contenedor a métodos de trait, M49 mueve las familias **matemáticas / tiempo / criptografía**
del namespace global a **módulos importables** `std/…`, dejando globales solo las funciones verdaderamente
universales. Las **primitivas de concurrencia se quedan core** (son parte del modelo de ejecución, no una
librería).

> **Empieza por `std/math`** (el caso más fuerte: mayor liberación de nombres —`min`/`max`/`abs`/`round`—
> y ya está a medias: `std/math.ray` existe con extras enteros pero el núcleo sigue siendo builtin global).

## Por qué (el hilo de M48)

- **Libera nombres colisionables**: `min`/`max`/`abs`/`round`/`floor`/`ceil` son justo los nombres que un
  usuario quiere para su propio código. Hoy, como globales, colisionan (o los bloquea el footgun de M48.3).
  Con `math.min` el global `min` queda libre.
- **Cierra una inconsistencia actual**: `import std/math; math.gcd(…)` importado, pero `sqrt(…)` global —
  mismo dominio, dos convenciones.
- **Precedente fuerte**: Python (`import math`/`time`), Go (`math.Sqrt`, `time.Now`), Rust (`f64::sqrt`,
  `std::time`) namespacean matemáticas/tiempo, y **ninguno** namespacea `print`.

## El mecanismo YA existe (cero maquinaria nueva)

Dos piezas probadas se combinan; **no hay que tocar el compilador**:

1. **std embebida en el binario** (M40.5, `src/stdlib.rs`): una tabla `("std/math", include_str!(
   "../std/math.ray"))` que el loader consulta al ver `import std/math;`. Añadir/expandir un módulo = una
   fila + el `.ray`. Esto es lo "dentro del binario".
2. **Patrón `__x` + envoltorio** (la I/O ya lo usa: `read_file`/`__read_file`): un builtin se renombra a un
   **primitivo interno** `__sqrt` (mismo opcode) y `std/math.ray` lo expone con
   `pub fn sqrt(x: float) -> float { __sqrt(x) }`. `import std/math;` → `math.sqrt(…)`.

## Qué se mueve y qué se queda

| **Quedan globales** (prelude, auto-importado) | **Pasan a `std/…` importable** |
|---|---|
| `print` · `eprint` · `to_string` · `panic` · `assert`/`assert_eq` | **`std/math`**: `sqrt sin cos tan ln log10 exp floor ceil round pow` · `abs min max` · `PI E` · (+ los extras enteros ya presentes: `gcd lcm clamp sign iabs ipow`) |
| I/O de errores-como-valores (`read_file`/`input`/…) | **`std/random`**: `random`/`random_int` (no deterministas → aparte de `math`) |
| Primitivas de **concurrencia** (`spawn`/`send`/`recv`/`select`/`scope`/`close`/`Channel.*`) — atadas al modelo de ejecución | **`std/time`**: `now monotonic sleep` |
| | **`std/crypto`**: `sha256 sha512 sha1 hmac_sha256 ed25519_verify bytes_of` |

`char_code` (receptor `char`) y `join` (`[string]`/`Task`, ad-hoc) se quedan **builtins** por ahora (o van a
`std/char`/`std/text` en una fase posterior; fuera del alcance de M49).

## Decisiones de diseño (recomendadas; **confirmar antes de implementar**)

1. **Ad-hoc polimórficos `abs`/`min`/`max`** (hoy `int|float` sin firma raylang única → como `print`).
   **Recomendación: volverlos funciones genéricas puras en raylang** (no primitivos), como hizo M48 con los
   contenedores → traits:
   - `min`/`max` vía el trait **`Ord`** ya existente:
     `pub fn min<T: Ord>(a: T, b: T) -> T { if (a.menor(b)) { a } else { b } }` (sirve int/float/string/char).
   - `abs` vía un trait nuevo **`Signed { fn abs(self) -> Self; }`** con `impl` para int/float (cuerpos puros:
     `if (self < 0) { 0 - self } else { self }`) → `pub fn abs<T: Signed>(x: T) -> T { x.abs() }`.
   - **Consecuencia**: `abs`/`min`/`max` dejan de necesitar opcode (`Abs`/`Min`/`Max` quedan muertos → se
     podan). Coexisten sin colisión con `sort.min(xs: [T]) -> Option<T>` (otro módulo + otra aridad).
2. **`pi`/`e`**: pasar de builtins nularios (`pi()`) a **constantes** `pub const PI: float = 3.14159…;` /
   `pub const E: float = 2.71828…;`. Más limpio (`math.PI`), y son literales exactos. Opcodes `Pi`/`E` muertos.
3. **`random`/`random_int`**: a **`std/random`** (no deterministas → separados del `math` puro-determinista,
   como en Python). Naming a fijar (`random.float()` / `random.int(n)` vs conservar `random.random()`/…).
4. **Migración**: **corte en seco** (como M48.4e, "de golpe", sin alias) reusando el **reescritor AST**:
   `sqrt(x)` → `math.sqrt(x)` **+ auto-insertar `import std/math;`** en cada archivo que use algún nombre
   movido. Cubre corpus + las **fixtures de test embebidas** en Rust (a mano, como en M48.4e).

## Sub-fases

- **M49.1a — `std/math`, funciones float**: renombrar los builtins float→float a `__x` (mismo opcode
  `MathF(...)`), envoltorios `pub fn` en `std/math.ray`, `import std/math;`; migrar sus sitios de llamada.
  Determinista → **oráculo** VM↔intérprete + el oráculo de self-hosting (lib pura).
- **M49.1b — `abs`/`min`/`max` genéricos + `PI`/`E` const**: trait `Signed`, `min`/`max` sobre `Ord`, consts;
  podar los opcodes `Abs`/`Min`/`Max`/`Pi`/`E`. Migrar. **M49.1 = `std/math` COMPLETO.**
- **M49.2 — `std/time` + `std/random`**: `now`/`monotonic`/`sleep` y RNG. No deterministas → pruebas por
  subproceso (como M15.1b).
- **M49.3 — `std/crypto`**: `sha*`/`hmac`/`ed25519`/`bytes_of`. Deterministas → verificar contra los vectores
  ya existentes (reusa los tests de M20).
- **Concurrencia**: se queda core (no se toca).

## Verificación

- **Deterministas** (`std/math`, `std/crypto`): oráculo VM↔intérprete + oráculo de self-hosting.
- **No deterministas** (`std/time`, `std/random`): integración por subproceso.
- **Regresión**: suite completa verde tras cada sub-fase. Los ejemplos migrados corren idénticos
  (comportamiento) antes/después.

## Prerrequisitos y notas

- El **prelude** y el **compilador auto-alojado** (`selfhost/*.ray`) que usen alguna función movida deben
  migrarse igual (probablemente pocos: son enteros-intensivos). El self-hosted `is_known_builtin` deja de
  listar los movidos (o los pasa a `__x`); a verificar que su oráculo siga byte-idéntico.
- **DESIGN.md**: al implementar, abrir **§51** documentando M49 (el "cambiar el lenguaje = actualizar DESIGN
  primero" del método de trabajo).
- **MANUAL / playground / gramáticas**: actualizar a la forma `math.sqrt` al cerrar cada sub-fase.

## Descartado / fuera de alcance

- Mover `print`/`panic`/`assert` a un `std/io` (anti-idiomático para un lenguaje de aprendizaje; regla de
  Python: lo universal, global).
- "Módulos builtin" especiales del compilador (innecesario: el embedding + wrappers ya lo cubre).
- Un `use std::*` / prelude configurable (posible futuro; no ahora).
