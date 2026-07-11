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
- **M85b**: pasado el horizonte de transiciones explícitas rigen las **reglas perpetuas
  del footer** TZ-string (`CET-1CEST,M3.5.0,M10.5.0/3`; solo la forma `Mm.w.d[/hora]`,
  la que emite zic — un footer no soportado degrada a extrapolar el último tipo).
- `fixtures/` trae TZif commiteados (tzdata es dominio público) para tests deterministas.
