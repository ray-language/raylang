<div align="center">

<!-- PNG y no SVG: la app móvil de GitHub no renderiza SVGs del repo (y menos en privados) -->
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/raylang-lockup-horizontal-dark.png">
  <img src="assets/raylang-lockup-horizontal.png" alt="raylang" width="380">
</picture>

**Un lenguaje de programación estáticamente tipado, orientado a expresiones y auto-alojado — escrito en Rust, con casi cero dependencias.**

[![CI](https://github.com/roberto-ayala/raylang/actions/workflows/ci.yml/badge.svg)](https://github.com/roberto-ayala/raylang/actions/workflows/ci.yml)
[![Licencia: MIT OR Apache-2.0](https://img.shields.io/badge/licencia-MIT%20OR%20Apache--2.0-blue.svg)](#licencia)
[![Versión](https://img.shields.io/badge/versión-1.0.0-brightgreen.svg)](CHANGELOG.md)

[Instalación](#instalación) · [Un vistazo](#un-vistazo-al-lenguaje) · [Playground](#playground-web) · [Documentación](#documentación) · [Lo notable](#lo-notable)

</div>

---

**raylang** es un proyecto de aprendizaje llevado hasta sus últimas consecuencias: construir un lenguaje de
programación de principio a fin, tocando **todas** las fases y problemáticas. El resultado es un lenguaje real
—con genéricos, traits, pattern matching, concurrencia multicore por actores y un ecosistema de herramientas—
que además se **compila a sí mismo** (self-hosting) y corre **en el navegador** vía WebAssembly.

El anfitrión es **Rust** (una VM de bytecode con GC como motor de producto; un intérprete tree-walking como
oráculo de validación). La invariante de diseño es **casi cero dependencias de Cargo**: la única excepción
consciente es TLS/criptografía (`rustls`/`ring`).

```rust
enum Arbol { Hoja, Nodo(Arbol, int, Arbol) }

fn suma(a: Arbol) -> int {
    match (a) {
        Arbol.Hoja => 0,
        Arbol.Nodo(izq, v, der) => suma(izq) + v + suma(der),
    }
}

fn main() -> int {
    let t = Arbol.Nodo(Arbol.Nodo(Arbol.Hoja, 1, Arbol.Hoja), 2, Arbol.Nodo(Arbol.Hoja, 3, Arbol.Hoja));
    print("suma del árbol: ${t.suma()}");    // UFCS + interpolación  → 6

    let s = [1, 2, 3, 4, 5]
        .iter()
        .map(fn(x: int) -> int { x * x })
        .sum();                              // iteradores perezosos + closure  → 55
    print("suma de cuadrados: ${s}");
    0
}
```

## Por qué mirarlo

- **Norte de diseño sin `null`.** Los errores son valores: `Option<T>`/`Result<T,E>` + el operador `?`.
- **Orientado a expresiones.** `if`, bloques y `match` producen valor; retorno implícito.
- **Sistema de tipos rico.** Genéricos con inferencia, **traits** (despacho estático, *bounds*, impls
  genéricos, métodos por defecto, `dyn A + B`), tipos suma y **pattern matching** exhaustivo (con guardas,
  `if let`, patrones anidados).
- **Ergonomía moderna.** UFCS (`x.f()`), pipelines (`x |> f()`), closures, e iteradores perezosos
  (`map`/`filter`/`take`/`zip`/`fold`/`collect`/…).
- **Concurrencia de verdad.** Modelo de **actores con aislamiento de heap** + canales tipados, sobre un
  scheduler **M:N multicore** con *data-race freedom* por construcción.
- **Web de producción.** Un **framework estilo Express** (`web/framework`: rutas con parámetros,
  middleware, CORS, estáticos con ETag, cookies, JSON tipado vía `ToJson`) sobre un servidor HTTP/1.1
  concurrente con keep-alive, TLS y apagado ordenado — el webserver nativo da **~107k req/s** (M3 Pro,
  p99 = 2,2 ms al 60% de capacidad). Guía: [`docs/web-framework.md`](docs/web-framework.md).
- **Auto-alojado.** El lexer, parser, checker, intérprete y VM de raylang están escritos **en raylang**.
- **Compila a binario nativo.** `ray build --native` transpila el programa a Rust y lo compila a un
  ejecutable: **24–61× más rápido que la VM**, y en cómputo puro **le gana a node (V8) por 4,2×**
  (5,1× con `--fast`, que cambia la aritmética chequeada por envolvente; medido 19 jul 2026). Modelo
  *dev = VM / deploy = nativo*, con paridad byte-idéntica.
- **Corre en el navegador.** La VM compilada a WebAssembly, sin `wasm-bindgen`.

## Instalación

### `curl | sh` (macOS / Linux)

```sh
curl -sSfL https://raw.githubusercontent.com/roberto-ayala/raylang/main/install.sh | sh
```

Instala los binarios `ray` (y su alias `raylang`) en `~/.local/bin`. En **Windows**, descarga el `.zip` de
la [última release](https://github.com/roberto-ayala/raylang/releases).

### Desde el código

```sh
git clone https://github.com/roberto-ayala/raylang
cd raylang
cargo build --release          # target/release/ray
```

> Rust se instala vía [rustup](https://rustup.rs/). Casi cero dependencias: solo `rustls`/`ring` (TLS/cripto).

## Uso

```sh
ray new hola           # crea un proyecto (ray.toml + src/main.ray)
cd hola
ray run                # ejecuta src/main.ray en la VM
ray dev                # modo desarrollo: recompila y reinicia ante cambios (+ live-reload del navegador)
ray build              # chequea y compila sin ejecutar
ray build --native     # transpila a Rust y compila un binario nativo (24–61× la VM)
ray test               # corre las funciones @test
ray fmt src/main.ray   # formatea
ray doc src/main.ray   # genera documentación desde /// 
ray templ vistas/      # compila templates .ray.html a funciones raylang tipadas (SSR)
ray repl               # REPL interactivo
ray lsp                # servidor LSP (diagnósticos, hover, definición, refs, rename, completado, formateo, símbolos…)
```

**Gestor de paquetes** (manifiesto `ray.toml` + lockfile `ray.lock` con hashes SHA-256):

```sh
ray add textutils@^1.2 # añade una dependencia del registro y la descarga
ray search json        # busca en el registro
ray update             # re-resuelve a las más nuevas compatibles
ray publish            # publica TU paquete (valida + chequea + hashea; --sign lo firma)
```

**Modo desarrollo** (`ray dev`): vigila los fuentes, recompila en ms y **solo reinicia si el cambio
compila** (un error a medio escribir no tira el servidor que funciona). Con `--port 8080` el supervisor
**retiene el socket** entre reinicios (cero conexiones rechazadas) e inyecta **live-reload** en el
navegador. Detalle en [`MANUAL.md`](MANUAL.md#17-herramientas).

El código de salida de `ray run` es el `int` que devuelve `main` (0 si es `unit`). Para embeber raylang
confinado: `ray run --fuel N` (límite de instrucciones) y `--heap N` (tope de objetos). Concurrencia
reproducible: `--deterministic`.

**Deploy nativo** (*dev = VM / deploy = nativo*, como Rust): `ray build --native prog.ray` produce un
ejecutable de código máquina, byte-idéntico a la VM. Los subsistemas con crate de producción (TLS,
criptografía, SQLite) se enlazan **solo cuando el programa los usa** (vía un proyecto Cargo generado); si
no toca ninguno, se compila con `rustc` pelado en ~0,2 s. `--release` sube el tier de optimización;
`--without crypto,tls,sqlite` (o `[native] without = [...]` en `ray.toml`) excluye un subsistema para
builds herméticos/cross-compile. Ver [Compilación a binario nativo](#documentación).

## Un vistazo al lenguaje

**Traits + genéricos:**

```rust
trait Mostrable { fn mostrar(self) -> string; }

struct Punto { x: int, y: int }
impl Mostrable for Punto {
    fn mostrar(self) -> string { "(${self.x}, ${self.y})" }   // interpolación de strings
}

fn imprime<T: Mostrable>(v: T) { print(v.mostrar()); }
```

**Errores como valores + `?`:**

```rust
fn dividir(a: int, b: int) -> Result<int, string> {
    if (b == 0) { Result.Err("división por cero") } else { Result.Ok(a / b) }
}

fn calc() -> Result<int, string> {
    let x = dividir(10, 2)?;   // desempaqueta o retorna el Err
    Result.Ok(x + 1)
}
```

**Concurrencia (actores + canales):**

```rust
fn main() -> int {
    let ch: Channel<int> = channel();
    spawn(fn() { var i = 0; while (i < 5) { send(ch, i * i); i = i + 1; } close(ch); });
    var total = 0;
    var seguir = true;
    while (seguir) {
        match (recv(ch)) {
            Option.Some(v) => { total = total + v; },
            Option.None => { seguir = false; },
        }
    }
    print("total: ${total}");   // 0+1+4+9+16 = 30
    0
}
```

Hay **171 ejemplos** en [`examples/`](examples/): desde `fib`/`fizzbuzz` hasta trait objects, structured
concurrency, un servidor web, WebSockets, y el propio compilador auto-alojado en [`selfhost/`](selfhost/).

## Playground web

raylang corre en el navegador (la VM compilada a `wasm32`, **cero `wasm-bindgen`**):

```sh
./playground/build.sh
cd playground && python3 -m http.server 8000   # → http://localhost:8000
```

Cubre el lenguaje núcleo (todo el lenguaje + prelude + stdlib pura). Ver [`playground/`](playground/).

## Lo notable

- **Self-hosting + meta-circularidad.** raylang lexea/parsea/chequea/ejecuta raylang, con el toolchain de
  Rust como oráculo. El compilador auto-alojado se ejecuta a sí mismo sobre el intérprete y la VM
  auto-alojados.
- **Multicore por actores.** Heap por fibra + transferencia de propiedad en `send` → *data-race freedom*
  sin *ownership* en el tipo. Scheduler M:N con speedup real medido; `--deterministic` para tests.
- **Tres motores que coinciden.** Un oráculo VM↔intérprete blinda cada cambio de runtime, y el binario
  nativo (`ray build --native`) verifica salida **byte-idéntica a la VM**.
- **Casi cero dependencias.** La pila de red y formatos (HTTP/2, HPACK, JSON, TOML…) está **escrita en
  raylang** (`packages/`), y el runtime (LSP, `dlopen`, `kqueue`/`epoll`, SHA del gestor de paquetes) en
  el propio Rust del proyecto, sin crates; la única excepción es TLS/`ring`.
- **Robusto ante entrada arbitraria.** Compilador sin pánicos + **fuzzing continuo** del front-end.

## Documentación

| Documento | Qué es |
|-----------|--------|
| [`MANUAL.md`](MANUAL.md) | La **guía práctica**: cómo usar el lenguaje, idiomas, y mejores prácticas. |
| [`REFERENCIA.md`](REFERENCIA.md) | El **catálogo exhaustivo**: palabras clave, operadores, builtins, prelude, `std/` y CLI, con firmas. |
| [`PUBLICAR.md`](PUBLICAR.md) | La guía del **publicador**: empaquetar, versionar y publicar en el registro. |
| [`SPEC.md`](SPEC.md) | La **especificación normativa** del lenguaje (gramática + semántica). |
| [`docs/web-framework.md`](docs/web-framework.md) | La guía del **framework web** (estilo Express): rutas, middleware, SSR, deploy. |
| [`docs/build.md`](docs/build.md) | La guía de **builds**: features slim, PGO, binario nativo. |
| [`PERFORMANCE.md`](PERFORMANCE.md) | La **crónica de rendimiento**: cada arco de optimización, medido. |
| [`book/`](book/) | El **libro** (mdBook): cómo se **construyó** el lenguaje, fase a fase. |
| [`DESIGN.md`](DESIGN.md) | La **crónica de diseño**: cada decisión y su porqué. |
| [`IDEAS.md`](IDEAS.md) | Backlog de features y su clasificación de impacto. |
| [`SECURITY.md`](SECURITY.md) | Política de seguridad y modelo de amenazas. |
| [`RELEASE-1.0.md`](RELEASE-1.0.md) | Checklist del estado hacia la 1.0. |

Editores: extensión de [VSCode](editors/vscode/) (con cliente LSP), paquete de [Sublime Text](editors/sublime/),
y config para Neovim/Helix (usan `ray lsp` directo).

## Estado

**raylang 1.0.0.** Motor de producto = la VM; el intérprete es el oráculo de desarrollo. La suite tiene
**610 tests unitarios** + **95 archivos de tests de integración** (incluido un fuzzer del front-end, los
oráculos VM↔intérprete y el corpus de paridad del binario nativo). Ver [`CHANGELOG.md`](CHANGELOG.md).

Es un **proyecto de aprendizaje**: real y cuidado, pero no pensado para producción crítica.

## Contribuir

Cada fase del proyecto es un commit con sus tests (Conventional Commits en español). El código y la
documentación están en español. Antes de tocar comportamiento, lee los documentos-contrato (`SPEC.md`,
`DESIGN.md`). Para reportar una vulnerabilidad, ver [`SECURITY.md`](SECURITY.md).

## Licencia

Doble licencia, a tu elección:

- **MIT** ([`LICENSE-MIT`](LICENSE-MIT))
- **Apache-2.0** ([`LICENSE-APACHE`](LICENSE-APACHE))

`SPDX-License-Identifier: MIT OR Apache-2.0`

---

<div align="center">
<img src="assets/raylang-mascot.png" alt="la mascota de raylang: una manta raya sonriente" width="130">
<br>
<sub>La identidad de marca (logo, variaciones, colores) vive en <a href="assets/"><code>assets/</code></a> · <a href="assets/branding/raylang-brand.pdf">libro de marca</a>.</sub>
</div>
