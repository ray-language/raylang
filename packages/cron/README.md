# packages/cron — expresiones cron + timers recurrentes (M86, v1 UTC)

```rust
import cron/cron;
import std/time;

let s = cron.parse("*/15 9-17 * * 1-5")?;         // 5 campos: min hora dom mes dow
let t = cron.next_after(s, time.now())?;          // siguiente disparo (epoch-ms UTC)

spawn(fn() { cron.run(s, fn() { print("tick"); }); });   // runner cooperativo
```

- Sintaxis: `*`, valores, rangos `a-b`, pasos `*/n` y `a-b/n`, listas `x,y`, alias
  `@hourly/@daily/@weekly/@monthly/@yearly`. `dow` 0-7 (0 y 7 = domingo).
- **Quirk de vixie-cron** (fiel): si `dom` Y `dow` están ambos restringidos, el día casa
  si CUALQUIERA casa (OR); si solo uno, manda ese.
- `next_after` es **puro** (testeable por golden); una expresión imposible (`0 0 30 2 *`)
  devuelve `Err` (búsqueda acotada a ~5 años).
- El runner duerme cediendo la fibra (`time.sleep`, M57.2) → convive con el resto del
  programa bajo `spawn`.
- **Hora local (M86b)**: `import cron/local;` (+ `packages/tz` como dependencia hermana) —
  `local.next_after_in(s, zona, ms)` / `local.run_in(s, zona, job)` evalúan el schedule en
  la hora CIVIL de la zona. Política DST: una hora del **hueco** de primavera dispara al
  acabar el hueco; una del **solape** de otoño dispara solo la PRIMERA vez. Módulo aparte
  para que el cron UTC no arrastre la dependencia tz.
