# regex — comparación con std/regex y plan (22 jul 2026)

El bench políglota `regex` (`benchmarks/poly/regex`) usa en su variante
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

- **R1 — `seen` por generaciones** ✅ **HECHA** (22 jul): un solo `[int]` por
  ejecución + contador; `seen[pc] == gen` en vez de re-alocar/re-llenar `[bool]`
  por posición.
- **R2 — no sembrar bajo `^`** ✅ **HECHA**: `is_anchored(prog)` (cadena
  Save/Jmp desde pc 0 hasta AssertStart, sin Splits — una alternancia con brazo
  no anclado sigue sembrando) apaga la siembra por posición.
- **R3 — `copy_saves` copy-on-write**: al leer el código resultó que **ya estaba
  implementada** (el clon solo ocurre en `Save`); su peso en el perfil era
  volumen, no clones de más.
- **R4a — `copy_saves` por concatenación nativa** ✅ **HECHA**: `s + []` (un
  opcode que clona en Rust) en vez del bucle de push por elemento.
- **R4b — reusar las listas de hilos entre posiciones** ❌ **EVALUADA y
  DESCARTADA por medición**: struct con longitud manual + swap dio nativo −9 %
  pero **VM +17 %** (el acceso a campos por nombre por hilo cuesta más que las
  2 allocs/posición ahorradas). Anotado en el código; reevaluar solo si la VM
  gana acceso a campos por índice (P1.3 `GetFieldIdx`).

**Medido (bench regex 200k líneas, checksum intacto en cada paso)**:

| | antes | R1+R2 | +R4a (final) | total |
|---|---|---|---|---|
| VM | 55.9 s | 20.8 s | **17.6 s** | **3.2×** |
| nativo | 1.95 s | 696 ms | **580 ms** | **3.4×** (de 25× a **7.5× de Go**) |

El micro de `find_all` (~300 KB) bajó también: 2.64 → ~1.9 s en la VM. Lo que
queda en el perfil es el bucle central de la Pike VM (match de instrucción +
`add_thread` + los `Save` inherentes al patrón): territorio del despacho de la
VM (P2.a) o de R5. Para regex en caliente, la respuesta de la VM sigue siendo
**deploy = nativo**.
- **R5 — crate `regex` como motor del NATIVO** ✅ **HECHA** (22 jul; PERFORMANCE.md
  Fase 63). Diseño que preserva la paridad: el **parseo/validación** del patrón
  sigue siendo el parser raylang de std/regex (errores byte-idénticos a la VM);
  el `Prog` retiene el patrón fuente y el transpilador intercepta las 7 funciones
  internas `run_*` → `ray_runtime::regex` (feature detectada POR USO, como
  crypto). La ejecución traduce el DIALECTO (clases ASCII fijas `\d\w\s`, escapes
  literales `\b`→b, `.` que casa `\n`, índices por carácter, y los matches
  VACÍOS con el bucle exacto de std/regex vía `find_at` — el iterador del crate
  omite el vacío adyacente y std no). `--without regex` NO stubbea: el fallback
  es la Pike VM transpilada (la implementación real). **Medido: nativo 570 →
  70.7 ms — LE GANA A GO (76.1); 2.6× tras el crate de Rust a pelo (27.1)**.
  Oráculo e2e `build_native_regex_via_ray_runtime_matches_the_vm` (tortura del
  dialecto VM↔nativo byte a byte + fallback). La VM sigue con la Pike VM
  (motor raylang puro, R1–R4a).
- **R6 — el borde de capturas, desde rangos** ✅ **HECHA** (29 jul; PERFORMANCE.md
  Fase 68). `captures_str` construía `Vec<Option<String>>` y lo recopiaba a
  `Rc<str>` (2 allocs por grupo); ahora el borde corta los `Rc<str>` DIRECTO de
  los rangos de bytes del match (`captures_byte_ranges`, 1 alloc por grupo).
  **Medido: 74.1 → 65.2 ms — pasa a ganar a Go (76.4); node conserva un 5%
  (0.95×)**, y esa comparación sí es motor contra motor (la variante rust del
  bench parsea a mano: con el crate, Rust puro cuesta ~49 ms). R6b (reusar
  `CaptureLocations` por patrón) midió PEOR (+3 ms intercalado) y se descartó.

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

## 5. Addendum R7 (29 jul 2026): el crate también en la VM

R5 dejó el nativo en la liga del crate; la VM seguía interpretando la Pike VM (~18 s en el
bench). El perfil del 29 jul descompuso ese gap en ~9× de algoritmo × ~33× de interpretación
(sin hotspot único: despacho ~50%, push/pop ~15%, get_local ~10%; el GetField por nombre solo
~4%). R7 cierra por el mismo borde: la feature `regex` de la toolchain hace que el compilador de
bytecode compile las 7 `run_*` a `[RegexNative, Return]` — el opcode lee `Prog.pat` (ya validado)
y el texto de los locales sin clonar y llama a `ray_runtime::regex::*`. Medido (arnés del banco,
ray PGO): **18.05 s → 347.8 ms (52×; 5.5× del nativo; combinado #10 → #8)**. La Pike VM queda
como motor del intérprete (`--interp`), de los
builds slim y de `RAYLANG_REGEX_PIKE=1`; el oráculo continuo del dialecto es
`regex_cli::regex_native_vm_matches_pike_interp` (VM↔interp, sin rustc). Detalle:
PERFORMANCE.md Fase 69.
