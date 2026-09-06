# Plan de mejoras impulsadas por `ray-remote` (sep 2026)

`ray-apps/ray-remote` es un cliente de escritorio remoto (RFB/VNC) para macOS escrito en raylang:
4.500 líneas, cuatro fibras, autenticación Diffie-Hellman de Apple, decodificadores Raw/CopyRect/
Hextile, `std/ui` + `ray bundle`. Su `docs/raylang-feedback.md` recoge 16 entradas de fricción,
todas con su rodeo. Este documento las convierte en trabajo ordenado para la toolchain.

**Verificación previa (6 sep 2026, `raylang 1.6.4`):** las entradas 2, 5, 6, 7, 8, 10, 11 y 13
se reproducen con ficheros de una línea. El resto son carencias de stdlib o de documentación
que no necesitan reproducción.

## 1. Criterio de orden

Tres preguntas por entrada: ¿es un bug (se arregla primero, sin debate)? ¿cambia el lenguaje
(SPEC primero, tres motores + selfhost)? ¿cuánto le quita a la app? Con eso salen cuatro lotes.
Cada lote termina **volviendo a la app**: se borra el rodeo correspondiente en `ray-remote`, se
mide y se anota en su feedback. Ese es el criterio de "hecho".

| Lote | Qué | Tamaño | Cambia SPEC |
|---|---|---|---|
| A | Bugs y mensajes: veredicto de `ray test`, `ray fmt` y comentarios, `close` con emisor bloqueado + `try_send`, cuatro mensajes de error, `ray run --` | S + M + S + S | no (salvo `try_send`, superficie nueva) |
| B | Ergonomía numérica y léxica: literales por tipo esperado, cuenta de desplazamiento `int`, `from` contextual | M | sí, tres cambios pequeños |
| C | Stdlib de protocolos: `std/inflate` incremental, `std/crypto/legacy`, `std/bigint` con `modpow` en el runtime | M + S + L | no |
| D | Decisiones de diseño que reabre la app: `break`/`continue`, sombra de builtins en módulos, closures sin anotación | — | sí; **las decide el usuario** |

Y transversal: documentación (§6).

## 2. Lote A — bugs y mensajes (primero)

### A1. `ray test` dice "all passed" cuando no ha corrido nada (feedback 5) — S ✅ M188

`src/test_runner.rs:146` imprime `result: N test(s), all passed ✓` con `ran == 0` aunque un
módulo no compile (el exit ya es 65, luego CI no lo cuela; el fallo es de presentación).
Cambio: si algún módulo del árbol no compila, el veredicto es
`result: no test ran — 1 module failed to compile ✗`; y `0 test(s)` sin fallo de compilación
se imprime como `result: 0 test(s) found` sin el tick. Test en `tests/test_runner_cli.rs`.

### A2. `ray fmt` desplaza los comentarios de línea y colapsa cadenas (feedback 13) — M ✅ M189

Reproducido: un comentario al final de la línea de una continuación de `&&` cae al final de la
sentencia junto a los demás, y una cadena `else if` de ocho casos queda en una línea de 300
columnas. Es el hallazgo que hace que el formateador **no sea seguro de aplicar**, así que va
antes que cualquier ergonomía.

1. **Comentarios**: un `//` que sigue a un token en su línea original queda pegado a ese token
   tras el reformateo (el formateador ya conserva los comentarios de línea completa; falta el
   caso "trailing en una continuación"). Si la expresión se reordena en una línea, los trailing
   se reparten uno por operando, no se apilan al final.
2. **Ancho**: las cadenas de `&&`/`||`, las de `else if` y las concatenaciones de strings
   respetan el límite de 100 columnas: una vez que no cabe, un operando por línea. Hoy parte la
   primera llamada en un argumento por línea y deja el resto en una línea de 98.
3. **Paréntesis redundantes**: el formateador deja de quitarlos. En código criptográfico son
   documentación deliberada; un formateador que reescribe la precedencia hace dudar al lector.

Golden: `tests/fixtures/fmt_comments.ray` (el fichero de doce líneas del feedback) y el propio
`ray-remote` con `ray fmt --check` en su CI.

### A3. Canales: `close` con emisor bloqueado y `try_send` (feedback 11 y 15) — S ✅ M190

Reproducido: `Channel.bounded(1)` lleno + fibra dormida en `send` + `close` →
`runtime error: close on a channel with a blocked sender`. Hoy "acotado" y "se cierra para
terminar" son incompatibles, que es justo lo que una app con productor/consumidor necesita.

- `close` despierta a los emisores bloqueados; su `send` termina con el error ya existente
  `send on a closed channel` (simétrico de los receptores, que reciben `None`).
- `try_send(ch, v) -> bool`: `false` si el canal está cerrado o lleno, sin panic. Es la vía
  para "la otra parte se fue antes que yo", que en un programa con fibras es normal, no
  excepcional. Superficie nueva → SPEC §concurrencia, REFERENCE, MANUAL, los tres motores.
- `ray_doc "Channel.bounded"` responde; el MANUAL deja de hablar de `channel(n)`, que no existe.

### A4. Cuatro mensajes de error que hoy apuntan mal (feedback 6, 7, 10, 12) — S ✅ M188 (`!`, `from`, `ray run --`; el de `break` lo sustituye D1)

| Hoy | Después |
|---|---|
| `'!' requires bool, not u32` | `… — for a bitwise NOT use '~'` |
| `expected a parameter name` (sobre `from`) | `'from' is a reserved word and cannot name a parameter` (hasta B3) |
| `name 'break' not declared` | `raylang has no 'break'/'continue': return from a function or use an iterator (MANUAL §4)` |
| `could not read module '--'` | `ray run <file> [args…]`: `--` tras el fichero se acepta y se descarta; antes del fichero, mensaje con la forma correcta |

Los mensajes de diagnóstico van en tándem con `selfhost/checker.ray` (byte-idénticos).

## 3. Lote B — ergonomía numérica y léxica (SPEC primero)

### B1. Literales enteros validados contra el tipo esperado, y sufijos (feedback 2) — M

`let c: u64 = 0xFFFFFFFFFFFFFFFF` es hoy `lex error: integer out of range`: el lexer valida
contra `i64` sin mirar el destino. Propuesta en SPEC: el lexer acepta cualquier literal que
quepa en `u64`; el checker valida el rango contra el tipo esperado (`int` sigue siendo i64) y
sin contexto el default es `int`. Sufijos `u8`/`u32`/`u64` para el caso sin contexto
(`0xFFFFFFFFFFFFFFFFu64`). Toca lexer, checker, selfhost y los tres motores (constantes).

### B2. La cuenta de un desplazamiento es un `int` (feedback 8) — S

`v << n` con `v: u32, n: int` es hoy error de tipos. SPEC: `<<`/`>>` aceptan como segundo
operando `int` o el mismo tipo sin signo; el resultado es el tipo del primero; cuenta fuera de
rango con el comportamiento ya definido. Quita un `as u32` de cada rotación en cualquier hash.

### B3. `from` como palabra clave contextual (feedback 10) — S

Solo tiene significado al inicio de sentencia (`from M import X;`). El parser la reserva
globalmente y no se puede llamar `from` a un parámetro. Cambio: reservada solo en posición de
sentencia; en cualquier otra es identificador. Parser + selfhost + LSP (coloreado).

## 4. Lote C — stdlib de protocolos

### C1. `std/inflate` incremental (feedback 4) — M

`std/inflate` es raylang puro (`examples/web/inflate.ray`, 661 líneas, embebido vía
`src/stdlib.rs`) con API de un golpe. Los encodings ZRLE/Zlib/Tight de RFB y el
`permessage-deflate` de WebSocket (RFC 7692) mantienen **un stream zlib durante toda la
sesión**. Refactor a un estado explícito, sin tocar el runtime:

```raylang
pub fn inflate_init() -> Inflater                     // ventana + estado del bloque en curso
pub fn inflate_push(z: Inflater, chunk: bytes) -> Result<bytes, string>   // lo que se pueda producir
pub fn inflate_end(z: Inflater) -> Result<bytes, string>                  // cola + verificación adler32
```

`zlib_inflate`/`gunzip` pasan a ser envoltorios de los tres. Golden con el stream troceado en
puntos arbitrarios (incluido dentro de un código Huffman) y comparado contra Python `zlib`.
Beneficiario inmediato: ZRLE en `ray-remote`; después `net` (WebSocket comprimido).

### C2. `std/crypto/legacy` (feedback 3) — S–M ✅ M194 (como `std/crypto/{md5,aes,des}`, con descifrado, CBC, AES-256 y 3DES)

MD5, DES/3DES y AES-128/256 en ECB/CBC, en raylang puro, con los vectores de RFC 1321,
FIPS 46-3 y FIPS-197. El código y los tests **ya existen en `ray-remote/src/crypto/`** (unas
900 líneas): se portan a la stdlib con la advertencia en la doc ("para hablar con protocolos
que lo exigen, no para diseñar"). SHA-1 ya está en `std/crypto`. Sin dependencia nueva.

### C3. `std/bigint` con `modpow` en el runtime (feedback 3 y 9) — L

Medido en la app: dos `modpow` de 4096 bits cuestan 14 s en la VM y 4,9 s en nativo, frente a
10–30 ms de una biblioteca decente. Es el único punto del feedback donde el código de la app no
puede hacer nada. Decisión de dependencia (SECURITY.md): `num-bigint` + `num-traits`
(Rust puro, sin `unsafe` relevante) en `crates/ray-runtime` y la VM, bajo feature `bigint`;
documentado que **no es de tiempo constante** (vale para DH/RSA de cliente, no para claves de
servidor bajo ataque de temporización). Superficie mínima: `BigInt` desde/hacia `bytes`
big-endian, hex y decimal; `+ - * / %`, comparación, `modpow`, `modinv`. Tres motores (el
nativo emite llamadas a `ray_runtime::bigint`). Desbloquea DH clásico, RSA y JWT RS256.

## 5. Lote D — decisiones que reabre la app (las toma el usuario)

### D1. `break`/`continue` (feedback 6) ✅ M191 — reabierto en forma mínima (decisión del usuario, 6 sep)

DESIGN §0 registra la decisión explícita (30 jul 2026) de **no** tenerlos: `return` +
extracción a función, iteradores, y el coste en el análisis de divergencia y en cuatro
implementaciones. La app aporta un dato nuevo: el rodeo con bandera **no corta el cuerpo del
bucle**, y eso produjo errores reales en tres bucles de red ("lee hasta que se cierre").
Opciones:

- **Mantener** la decisión y reforzar el mensaje (A4) y el MANUAL §4 con el patrón de canal
  (`recv` → `None`) que la propia app terminó usando.
- **Reabrir** con la forma mínima: `break;`/`continue;` como sentencias de tipo `unit`, solo
  dentro de `while`/`for`, sin valor ni etiquetas. Coste M: checker (divergencia: un `while`
  con `break` no diverge), compilador/VM/intérprete/nativo (salto), selfhost, SPEC.

Recomendación: reabrir en la forma mínima. La bandera es el patrón que la gente escribe de
verdad, y es el que falla en silencio.

### D2. Funciones `pub` de módulo con nombre de builtin (feedback 1)

`pub fn close(c: Conn)` en un módulo consumido cualificado se rechaza. Opción: permitirlo
cuando el módulo se importa cualificado, con `builtin.close(...)` como escape dentro del
módulo. Riesgo: el módulo que hace `import proto;` sin cualificar. Coste S–M en el checker,
más selfhost. Recomendación: sí, con el escape explícito.

### D3. Closure sin anotación cuyo cuerpo produce un valor (feedback 12)

`spawn(fn() { …; 0 })` es error. Opción: inferir el tipo de retorno de una función anónima
sin anotación a partir del cuerpo. Coste M (inferencia en el checker + selfhost). Baja
prioridad: el rodeo es una sentencia.

## 6. Documentación (transversal, S) ✅ M190 (captura/spawn, `bytes`, `Channel.bounded`; el patrón "canal que se cierra" queda con D1)

- **MANUAL, captura de closures**: en negrita, "entre fibras solo se comparten canales y
  handles; lo que `spawn` captura se copia" (feedback 14). Es un bug real que compila y arranca.
  La versión de compilador (aviso al capturar un struct que se muta fuera) queda como idea L.
- **REFERENCE `bytes`**: `+` sobre `bytes` es lineal amortizado (22 ms para 8 MB en 128 trozos);
  `[int]` + `bytes_of` octeto a octeto es dos órdenes más lento (feedback 16).
- **`Channel.bounded`** en `ray_doc`, MANUAL y REFERENCE (A3).
- **MANUAL §4**: el patrón "canal que se cierra como señal de parada" (feedback 14, D1).

## 7. Orden propuesto y numeración

| Hito | Contenido | Efecto en `ray-remote` |
|---|---|---|
| M188 | A1 + A4 + `ray run --` | `ray test` honesto; mensajes que orientan |
| M189 | A2 (`ray fmt`) | `ray fmt --check` entra en su CI |
| M190 | A3 + §6 docs | el canal de frames vuelve a ser acotado; `stop` sin `try_call` |
| M191 | D1 (`break`/`continue`) ✅ | los tres bucles de red con bandera vuelven a ser `break` |
| M192 | B1 + B2 + B3 | tablas de constantes copiadas del estándar; `rotl` sin casts |
| M193 | C1 (`std/inflate` incremental) | ZRLE/Zlib/Tight; usable por WAN |
| M194 | C2 (`std/crypto/legacy`) | `src/crypto/` se reduce a `bignum.ray` |
| M195 | C3 (`std/bigint`) | conectar pasa de 5 s a <100 ms; `src/crypto/` desaparece |
| D | D2–D3 | según decisión |

Cada hito: rama + PR, SPEC antes si cambia el lenguaje, DESIGN con el porqué, CHANGELOG "Sin
publicar", y la validación final es el diff en `ray-remote` que borra el rodeo.
