# raylang · playground web (M44a)

Ejecuta raylang **en el navegador**: la VM compilada a WebAssembly (`wasm32-unknown-unknown`),
**sin `wasm-bindgen` ni dependencias nuevas** (ABI cruda a mano, como el resto del proyecto).

## Alcance

Es el **lenguaje núcleo**: lexer, parser, checker y VM, más el prelude
(`Option`/`Result`/`map`/`filter`/`fold`/`assert`/`sort`) y toda la stdlib pura (aritmética, strings,
arreglos, `Map`, structs, enums, `match`, closures, genéricos, traits). Un solo archivo (sin `import`).

**No disponible** en el playground (el navegador no lo permite, y respeta la invariante cero-deps):
red/TLS/HTTP, cripto (`ring`), FFI (`dlopen`), archivos, y el multicore (corre en un hilo, N=1). Un
programa que use esas features da un error claro.

## Construir y servir

```sh
./playground/build.sh                 # compila el .wasm release y lo deja en playground/
cd playground && python3 -m http.server 8000
# abre http://localhost:8000
```

> Hay que **servirlo por HTTP** (no `file://`): el navegador bloquea `fetch` del `.wasm` en local.

El `.wasm` es un artefacto de build (no se versiona; ver `.gitignore`). Con `wasm-opt` (paquete
`binaryen`) instalado, `build.sh` lo reduce con `-Oz`.

## Cómo funciona (ABI)

El módulo exporta tres funciones (`src/wasm.rs`):

- `alloc(len) -> ptr` — reserva `len` bytes en la memoria lineal.
- `run(ptr, len) -> u64` — compila y ejecuta el fuente en `[ptr, ptr+len)`; devuelve
  `(ptr_salida << 32) | len_salida` apuntando al texto de salida (stdout capturado + errores).
- `dealloc(ptr, len)` — libera un buffer.

El glue JS (`index.html`) reserva, escribe el fuente UTF-8, llama a `run`, lee la salida y libera.
