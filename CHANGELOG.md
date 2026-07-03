# Changelog

Todas las versiones notables de raylang. El formato sigue el espíritu de
[Keep a Changelog](https://keepachangelog.com/) y el versionado es
[SemVer](https://semver.org/) (la versión del lenguaje y la de la stdlib van juntas; ver `SPEC.md` §12).

## 1.0.0 — 2026-07-03

Primera versión estable. raylang pasó de un lenguaje de juguete (un lexer + un intérprete tree-walking) a un
lenguaje **auto-alojado**, con una VM de bytecode como motor de producto, concurrencia multicore por actores,
un ecosistema de herramientas (`ray`, gestor de paquetes, LSP, formateador, doc) y un playground en el
navegador — manteniendo la invariante de **casi cero dependencias** (solo TLS/criptografía vía `rustls`/`ring`).

### El lenguaje

- **Estáticamente tipado, orientado a expresiones** (`if`/bloques/`match` producen valor; retorno implícito),
  sintaxis de llaves. `let` inmutable / `var` mutable; **sin `null`**.
- **Errores como valores**: `Option<T>`/`Result<T,E>` + el operador `?`.
- **Tipos suma** (`enum`) y **pattern matching** exhaustivo (`match`), con guardas, `if let`, patrones
  anidados y de struct.
- **Genéricos** (funciones y tipos, con inferencia y *erasure*) y **traits** con despacho estático, *bounds*
  (paso de diccionarios), impls genéricos, métodos por defecto y **trait objects** (`dyn A + B`, con
  upcasting).
- **UFCS** (`recv.f(args)`) y **pipelines** (`x |> f(a)`); closures con captura; **inferencia local**.
- **Protocolo `Iterator`** perezoso (`map`/`filter`/`take`/`skip`/`zip`/`enumerate` + `fold`/`collect`/`sum`)
  sobre el que se re-fundan las operaciones ansiosas.
- **Datos**: arreglos `[T]`, structs, `Map<K,V>`, `Set<T>`, `Deque<T>`, `char`, `bytes`, enteros con signo/
  tamaño y operadores bit a bit.
- **Módulos** multi-archivo por directorios, con cápsulas (`mod.ray`), `pub`, `import`/`from … import`,
  re-exports y tipos por módulo.
- **Anotaciones** (`@test`, `@derive(Eq, Show, Hash, Ord)`).

### El runtime

- **VM de bytecode** (pila y marcos explícitos) como **motor de producto**, con **GC mark-and-sweep**; el
  **intérprete** queda como oráculo de validación cruzada en desarrollo.
- **TCO** (recursión de cola en O(1) de pila) en ambos motores; recursión profunda robusta.
- **Confinamiento opcional** para embeber raylang: `--fuel` (límite de instrucciones) y `--heap` (tope de
  objetos vivos).

### Concurrencia

- Modelo **CSP → actores con aislamiento de heap**: `spawn`/canales tipados (`channel`/`send`/`recv`/
  `close`/`select`), *structured concurrency* (`scope`/`join`) y cancelación.
- **Scheduler M:N multicore real** (pool de hilos; ~3,84× en 4 tareas), con `--deterministic` para salida
  reproducible. *Data-race freedom* por construcción (heap por fibra; los canales transfieren la propiedad).

### Auto-alojado (self-hosting)

- El **lexer, parser, checker, intérprete y VM de raylang están escritos en raylang** (`selfhost/`), validados
  contra el toolchain de Rust como oráculo. **Meta-circularidad**: el compilador auto-alojado se ejecuta a sí
  mismo sobre el intérprete y la VM auto-alojados.

### Ecosistema y herramientas

- **CLI `ray`**: `new/run/build/test/fmt/doc/lsp/repl/version`.
- **Gestor de paquetes**: manifiesto `ray.toml`, lockfile `ray.lock` con hashes SHA-256 (supply-chain),
  dependencias git / ruta local / transitivas.
- **stdlib `std/`** embebida (matemáticas, texto, orden, colecciones, codificación, hashing…) + un paquete
  `net` (HTTP/HTTP2, DNS, WebSocket, TLS, gRPC, Postgres, Redis, OAuth2…) como paquete adicional.
- **LSP** (diagnósticos, hover, ir-a-definición, referencias, rename, completado, signature help),
  **formateador**, **raydoc**, y clientes de editor (VSCode, Sublime, Neovim/Helix).
- **FFI** con ABI C (`extern "lib" { fn … }`, sin `libffi`).

### Seguridad y calidad

- **Compilador sin pánicos**: toda entrada → error con posición o ICE reportable; **fuzzing continuo** del
  front-end (0 crashes). Política de seguridad en `SECURITY.md`.
- **Criptografía de producción** vía `ring` (SHA/HMAC/Ed25519/AEAD); las implementaciones puras en raylang
  quedan como demostración del lenguaje.
- Casi cero dependencias de Cargo (única excepción consciente: TLS/`ring`), auditadas en CI (`cargo audit`).

### Playground web

- La VM compilada a **WebAssembly** (`wasm32`, **sin `wasm-bindgen`**) → raylang corre **en el navegador**
  (`playground/`). Alcance: lenguaje núcleo (sin red/cripto/FFI/hilos).

### Distribución

- Instalador `curl | sh` (`install.sh`) y CI de releases con binarios por plataforma (macOS, Linux, Windows).
- Licencia **MIT OR Apache-2.0**.
