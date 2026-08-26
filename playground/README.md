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

El `.wasm` y el `editor.bundle.js` son artefactos de build (no se versionan; ver `.gitignore`).
Con `wasm-opt` (paquete `binaryen`) instalado, `build.sh` reduce el wasm con `-Oz`; el bundle
del editor lo produce npm/esbuild desde `editor/` (la única parte del playground con deps JS,
como la extensión de VSCode).

## El editor real (IDEAS §74)

El editor es **CodeMirror 6** (`editor/editor.mjs` → `editor.bundle.js`) con el **LSP de
raylang corriendo dentro del wasm**: el MISMO despacho del `ray lsp` de tu editor
(`src/lsp/mod.rs::handle_message`), compilado a wasm32. Eso da diagnósticos en vivo
(publishDiagnostics → subrayado + gutter), **autocompletado** (símbolos del documento +
builtins + snippets con placeholders), **hover** con tipos, **signature help** (la firma de la
llamada en curso con el parámetro activo resaltado) y **formateo** (`ray fmt`, botón `fmt` o
Shift+Alt+F), sin reimplementar nada en JS: el cliente (`index.html`) solo habla JSON-RPC con
la función exportada `lsp`.

## Cómo funciona (ABI)

El módulo exporta cuatro funciones (`src/wasm.rs`):

- `alloc(len) -> ptr` — reserva `len` bytes en la memoria lineal.
- `run(ptr, len) -> u64` — compila y ejecuta el fuente en `[ptr, ptr+len)`; devuelve
  `(ptr_salida << 32) | len_salida` apuntando al texto de salida (stdout capturado + errores).
- `lsp(ptr, len) -> u64` — despacha UN mensaje LSP (JSON-RPC, sin framing: la llamada es el
  transporte) y devuelve el **array JSON** de mensajes emitidos, empaquetado igual que `run`.
- `dealloc(ptr, len)` — libera un buffer.

El glue JS (`index.html`) reserva, escribe los bytes UTF-8, llama a `run`/`lsp`, lee la salida
y libera.
