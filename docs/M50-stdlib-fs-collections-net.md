# M50 — cerrar la descongestión del namespace: `std/fs` + `std/collections` + `std/net`

Continúa M48/M49: mueve del **prelude global** (auto-inyectado) a **módulos `std/` opt-in** los tres
grupos grandes que aún ensucian el namespace de valores — **sistema de archivos**, **colecciones** y
**red** —. Misma filosofía y mecanismo que M49 (`__x`+envoltorio; migración dirigida por errores de
compilación). Cierra el arco de espacios de nombres.

**Lo que se QUEDA global** (los esenciales del prelude, regla de Python): `Option`/`Result` + `?`,
`map`/`filter`/`fold`, `print`/`eprint`/`panic`/`assert`/`assert_eq`, y `to_string`. **`close`** se queda
global (ad-hoc: cierra canal **o** handle de archivo — como `join`, no se parte). **stdin** (`input`/
`read_int`) y **`env`** NO son archivos → se dejan globales por ahora (decisión aparte: `std/io`/`std/env`).

## M50.1 — `std/fs` (sistema de archivos)

Todo lo que toca **disco** → `fs.X` (con `import std/fs;`). Es un *capability hint* suave: importar
`std/fs` señala "este archivo toca el sistema de archivos".
- **Envoltorios del prelude** (sobre `__x`, devuelven `Result`/`Option`): `read_file`, `write_file`,
  `read_file_bytes`, `write_file_bytes`, `append_file`, `remove_file`, `list_dir`, `open`, `read_line`,
  `write` → se **cortan del prelude** a `std/fs.ray`.
- **Builtin** `exists` → `__exists` + envoltorio `fs.exists`.
- Los primitivos `__read_file`/etc. se quedan builtins.
- **No determinista** (disco) → tests por subproceso (reusa `io_cli`/`bytes_io_cli`).
- Uso en el corpus: moderado (ejemplos de I/O + tests).

## M50.2 — `std/collections` (`Set`/`Deque`/`StringBuilder`)

Estructuras de datos **puras en raylang** (sin `__x`) → se **cortan del prelude** a submódulos bajo
`std/collections/` (ver la decisión de naming abajo): las funciones **y** los `struct Set`/`Deque`/
`StringBuilder` se namespacan a su submódulo.
- **Set**: `set_new`/`set_add`/`set_has`/`set_remove`/`set_size`/`set_items` (+ helpers internos
  `set_bucket`/`set_en_bucket`, a ocultar como no-`pub`).
- **Deque**: `deque_new`/`deque_push_back`/`deque_push_front`/`deque_pop_back`/`deque_pop_front`/
  `deque_peek_front`/`deque_len`/`deque_is_empty`.
- **StringBuilder**: `sb_new`/`sb_push`/`sb_build`/`sb_count`.
- **Determinista** → **oráculo** VM↔intérprete (+ el de self-hosting).
- Uso en el corpus: **muy pequeño** (~2 archivos).

**Naming — DECIDIDO: submódulos bajo `std/collections/`** (lo mejor de las dos vías, sin maquinaria nueva):
`std/collections/set`, `std/collections/deque`, `std/collections/stringbuilder`. Se usan con leaf-binding
(M11.5): `import std/collections/set;` → `set.new()`/`set.add(s, x)`; `import std/collections/deque;` →
`deque.push_back(d, x)`; `import std/collections/stringbuilder;` → `sb.push(b, s)` (leaf `stringbuilder`, o
`as sb`). **Agrupa bajo `std/collections/` Y cae el prefijo redundante** (el leaf da la desambiguación que
dentro de un solo módulo exigía `set_`/`deque_`/`sb_`). Espeja `Map.new()`. Mecanismo: `stdlib::embedded`
hace match exacto por nombre (`("std/collections/set", include_str!("../std/collections/set.ray"))`, etc.) +
el leaf-binding de directorios ya probado (`import geo/formas/circulo;`). Los `struct Set`/`Deque`/
`StringBuilder` se namespacan a `set.Set`/`deque.Deque`/`stringbuilder.StringBuilder` (o vía from-import).
Descartadas: un solo `std/collections` con prefijos (`collections.set_new`, redundante) y `std/set` plano
(sin agrupar).

## M50.3 — `std/net` (transporte)

TCP/TLS/UDP/sockets → `net.X`. Es el grupo mayor pero acotado.
- **Envoltorios del prelude**: `tcp_connect`/`tcp_listen`/`tcp_accept`, `tls_connect`/`tls_connect_h2`/
  `tls_accept`, `socket_read`/`socket_write`/`socket_read_bytes`/`socket_write_bytes` (+ los UDP).
- **Builtin** `local_port` → `__local_port` + `net.local_port`.
- Se conserva la distinción en los nombres (`net.tcp_connect`/`net.tls_connect`/`net.udp_*`).
- **No determinista** (red) → tests por subproceso (reusa `net_cli`/`tls_cli`/etc.).
- Uso: **~15 archivos** (el stack web: `http`/`websocket`/`webserver`/`redis`/`dns`/… en examples +
  packages). **Ninguno embebido usa red** → sin embebido-importa-embebido; migración directa.

## Mecanismo y verificación (los tres)

- **Patrón M49**: renombrar el builtin a `__x` (o cortar el envoltorio del prelude) → crear `std/X.ray`
  con los envoltorios → registrar en `src/stdlib.rs` → **migración dirigida por errores de compilación**
  (compilar el corpus, migrar lo que falle: `import std/X; X.fn`).
- **Verificación**: deterministas (collections) → oráculo VM↔intérprete + self-hosting; no deterministas
  (fs/net) → integración por subproceso, ambos motores.
- **MANUAL + playground + DESIGN §52** al cerrar cada sub-fase. Código nuevo ya en inglés (regla nueva).

## Orden y sub-fases
- **M50.1** `std/fs` (limpio, molde directo).
- **M50.2** `std/collections` (el más pequeño; fija antes A vs B).
- **M50.3** `std/net` (el mayor; ~15 archivos, ninguno embebido).

## Fuera de alcance (decisión aparte)
stdin (`input`/`read_int`) y `env` (→ posible `std/io`/`std/env`); `close`/`print`/`eprint` se quedan
globales. La **limpieza de identificadores a inglés** (`docs/limpieza-nombres-en-ingles.md`) sigue diferida.
