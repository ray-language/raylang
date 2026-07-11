# packages/tz — hora local IANA (M85)

Zonas horarias sobre los **TZif** del sistema (`/usr/share/zoneinfo`), en raylang puro
(RFC 8536, v1/v2/v3; cero deps). La moneda es la de `std/time`: instantes en epoch-ms UTC.

```rust
import tz/tz;
import std/time;

let mad = tz.load("Europe/Madrid")?;          // o tz.system() / tz.load_file(ruta)
let dt = tz.to_local(mad, time.now());        // hora civil local
let off = tz.offset_at(mad, time.now());      // offset en ms
match (tz.to_utc(mad, civil)) {               // la inversa NO es total (DST):
    tz.LocalResult.Single(ms) => …,           //   lo normal
    tz.LocalResult.Ambiguous(antes, despues) => …,  // solape de otoño (ocurre 2 veces)
    tz.LocalResult.Gap => …,                  //   hueco de primavera (no existe)
}
```

- **Windows**: sin zoneinfo, `load` devuelve `Err` claro (UTC de `std/time` sigue).
- **Alcance v1**: transiciones explícitas del archivo (tzdata las trae hasta ~2037);
  después se extrapola el último tipo. El footer TZ-string (reglas perpetuas) es M85b.
- `fixtures/` trae TZif commiteados (tzdata es dominio público) para tests deterministas.
