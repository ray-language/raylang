# regex — comparación con std/regex y plan (22 jul 2026)

El bench políglota `regex` (`~/Desktop/benchmarks/regex`) usa en su variante
`.ray` un **parseo manual** (nota en el propio archivo: `std/regex` daba ~59 s
VM / ~2.3 s nativo). Aquí la comparación pedida con la **librería real**
(`import std/regex`, patrón compilado una vez vía `Matcher`, checksum idéntico
al resto), el porqué medido con perfil, y el plan.

## 1. La comparación (misma tarea, mismo checksum; hyperfine, M3)

| variante | motor | tiempo |
|---|---|---|
| regex.rs | crate `regex` (DFA + literal opts, nativo) | **27.2 ms** |
| regex.ray (manual, nativo) | sin regex | 51.4 ms |
| regex.go | `regexp` (NFA nativo) | 77.3 ms |
| regex.php | PCRE2 (C, JIT) | 98.9 ms |
| regex.js | Irregexp (C++, JIT) | 103.4 ms |
| regex.py | `sre` (C) | 167.9 ms |
| **regex-std.ray nativo** | **Pike VM en raylang→Rust** | **1.95 s** (25× Go) |
| **regex-std.ray VM** | **Pike VM interpretada** | **55.9 s** (723× Go) |

Contexto honesto: todos los demás lenguajes **bindean un motor en C/C++/nativo**;
`std/regex` es el único motor escrito EN el propio lenguaje del bench (M29.1,
Thompson→Pike VM, tiempo lineal garantizado). La comparación es motor-contra-
motor, no lenguaje-contra-lenguaje. Aun así, 25× en nativo tiene margen real:

## 2. Atribución (perfil del binario nativo, top-of-stack)

| función (de std/regex) | muestras | qué hace hoy |
|---|---|---|
| `new_bools` | **224** | el set `seen` de la Pike VM se **re-aloca y re-llena con un bucle de push POR POSICIÓN del texto** (~7.4 M veces en el bench) |
| `add_thread` | 159 | siembra/propaga hilos; además `run_pike` **siembra un hilo nuevo en CADA posición** aunque el patrón esté anclado con `^` (muere en la aserción, pero ya pagó `new_saves` + la recursión) |
| `copy_saves` | 151 | el array de capturas se **clona entero por cada hilo añadido** (por posición × hilos), no solo cuando un `Save` escribe |
| allocador (malloc/free/realloc) | ~590 | consecuencia de lo anterior + `npcs`/`nsvs` frescos por posición |

## 3. Plan (library-level, raylang puro — mantiene la pureza del motor)

- **R1 — `seen` por generaciones**: un solo `[int]` asignado por ejecución + un
  contador; `seen[pc] == gen` en vez de re-alocar y re-llenar `[bool]` por
  posición. Mata el hotspot nº 1.
- **R2 — no sembrar bajo `^`**: si el programa arranca con la aserción BOL (y no
  es multiline), la siembra por posición es trabajo muerto — sembrar solo en
  `start`. En este bench elimina ~7.4 M `new_saves`+`add_thread`.
- **R3 — `copy_saves` copy-on-write**: clonar el array de saves solo cuando una
  instrucción `Save` escribe (el clásico de la Pike VM), no en cada `add_thread`.
- **R4 — reusar `pcs`/`npcs`/`svs`** entre posiciones (índices con longitud
  manual en vez de arrays frescos).
- Estimación conjunta (por el peso del perfil: R1+R2 cubren ~50 % de las
  muestras, R3 otro ~20 %): nativo **1.95 s → ~0.4–0.7 s** (de 25× a ~6–9× de
  Go); la VM bajará proporcionalmente (~15–25 s) pero seguirá lejos — para regex
  en caliente la respuesta de la VM sigue siendo **deploy = nativo** (o P2.a).
- **R5 (decisión de producto, si algún día hace falta la liga C)**: feature
  `regex` en `ray-runtime` (el crate `regex` de Rust) como motor acelerado del
  nativo — precedente ring/rusqlite/mimalloc. Pondría a raylang en los 27 ms de
  Rust, a cambio de que el motor deje de ser raylang puro en el binario nativo.
  No proponerlo hasta que un caso real lo pida.

## 4. La variante `regex-std.ray` usada (para el set del bench, si se quiere añadir)

```ray
// La MISMA tarea que regex.go/rs/js/…, pero con el motor real de la stdlib:
// import std/regex (Pike VM en raylang puro), patrón compilado UNA vez (Matcher).
import std/regex;

fn main() {
    let n = 200000;
    let rx = regex.compile("^user([0-9]+) GET /api/([0-9]+) ([0-9]+) ([0-9]+)ms$").unwrap();
    var checksum = 0;
    var match_count = 0;
    for i in 0..n {
        let status = if (i % 5 != 4) { 200 } else { 404 };
        let line = `user${i} GET /api/${i % 50} ${status} ${i % 250}ms`;
        match (rx.captures_str(line)) {
            Option.Some(caps) => {
                match_count = match_count + 1;
                let uid = parse_int(caps[1].unwrap_or("")).unwrap_or(0);
                let path = parse_int(caps[2].unwrap_or("")).unwrap_or(0);
                let st = parse_int(caps[3].unwrap_or("")).unwrap_or(0);
                let ms = parse_int(caps[4].unwrap_or("")).unwrap_or(0);
                checksum = (checksum * 31 + uid + path + st + ms) % 1000000007;
            },
            Option.None => {},
        }
    }
    print(match_count);
    print(checksum);
}
```

Sugerencia para el set: mantener `regex.ray` (manual) como la variante que entra
al ranking (mismo espíritu que `regex.rs`… que también podría tener su gemela
`regex-std`), y añadir `regex-std.ray` como variante informativa — mide el motor,
no el lenguaje.
