<div align="center">

<!-- PNG y no SVG: la app móvil de GitHub no renderiza SVGs del repo (y menos en privados) -->
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/raylang-lockup-horizontal-dark.png">
  <img src="assets/raylang-lockup-horizontal.png" alt="raylang" width="380">
</picture>

**Un lenguaje de programación estáticamente tipado, orientado a expresiones y auto-alojado — escrito en Rust, con una superficie de dependencias mínima y deliberada.**

[![CI](https://github.com/ray-language/raylang/actions/workflows/ci.yml/badge.svg)](https://github.com/ray-language/raylang/actions/workflows/ci.yml)
[![Licencia: MIT OR Apache-2.0](https://img.shields.io/badge/licencia-MIT%20OR%20Apache--2.0-blue.svg)](#licencia)
[![Versión](https://img.shields.io/badge/versión-1.3.1-brightgreen.svg)](CHANGELOG.md)

**[raylang.dev](https://raylang.dev)** · [Instalación](#instalación) · [Un vistazo](#un-vistazo-al-lenguaje) · [Playground](https://raylang.dev/playground/) · [Documentación](#documentación) · [Lo notable](#lo-notable)

</div>

---

**raylang** es un lenguaje enfocado a **producción real**: genéricos, traits, pattern matching,
concurrencia multicore por actores, un ecosistema de herramientas y **tres motores que coinciden byte a
byte** — una VM de bytecode para desarrollar, un **binario nativo** para desplegar y un intérprete como
oráculo de validación. Además se **compila a sí mismo** (self-hosting) y corre **en el navegador** vía
WebAssembly.

El anfitrión es **Rust**. La política de dependencias es *mínima y deliberada*: una dependencia entra
solo cuando hacerla a mano sería peor ingeniería (TLS/`rustls`, cripto/`ring`, SQLite/`rusqlite`, el
cambio de contexto de las fibras) o cuando la mejora está **medida** (`mimalloc`, `ahash`). Todo lo
demás —HTTP/1.1 y HTTP/2, HPACK, JSON, TOML, DNS, WebSocket, protobuf, los clientes de BD, el LSP, el
poller de E/S— está escrito en raylang o en el Rust del propio proyecto. Detalle en
[`SECURITY.md`](SECURITY.md#política-de-dependencias).

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
  concurrente con keep-alive, TLS y apagado ordenado. El nativo corre sobre **fibras M:N** (jul 2026):
  el framework da **~188k req/s de techo — 93% de axum, con p50/p99.9 empatadas (0,48/1,05 ms vs
  0,47/1,04) y 1,5× Go+chi** (escalón `json`, generador de carga dedicado), sirviendo con **14 hilos y
  ~21 KB por conexión**. Guía: [`docs/web-framework.md`](docs/web-framework.md).
- **Procesos del SO sin sorpresas.** `std/process` lanza comandos con **argv tipado, sin shell**
  (`run`, un builder con plazo y topes, y *streaming* por canales acotados con contrapresión). El hijo
  va en su propio grupo de procesos y es **hijo de scope**: nadie se queda huérfano.
- **Auto-alojado.** El lexer, parser, checker, intérprete y VM de raylang están escritos **en raylang**.
- **Compila a binario nativo.** `ray build --native` transpila el programa a Rust y lo compila a un
  ejecutable, con paridad byte-idéntica (*dev = VM / deploy = nativo*). En el banco poliglota de 14
  programas (29 jul 2026, M3 Pro) **le gana a node en 9 de los 10 de cómputo** (1,1×–20×), **a Go
  en seis** y **a `rustc -O` en cuatro** (empatando con ambos en otros dos), y arranca en
  **1,80 ms — el más rápido de la mesa**. En tiempo×memoria queda **#1 o #2 en 11 de los 12
  programas** contra 9 lenguajes. Frente a la propia VM:
  3–4× en cargas de servicio y 28–57× en cómputo puro. Tablas:
  [`benchmarks/poly/README.md`](benchmarks/poly/README.md).
- **Corre en el navegador.** La VM compilada a WebAssembly, sin `wasm-bindgen`.

## Instalación

### `curl | sh` (macOS / Linux)

```sh
curl -sSfL https://raylang.dev/install.sh | sh
```

Instala los binarios `ray` (y su alias `raylang`) en `~/.local/bin`.

### PowerShell (Windows)

```powershell
irm https://raylang.dev/install.ps1 | iex
```

Deja `ray.exe` (y `raylang.exe`) en `%LOCALAPPDATA%\Programs\raylang\bin` y lo añade al PATH de
usuario (sin administrador; abre una terminal nueva). Mismas variables que el `.sh`
(`RAYLANG_VERSION`, `RAYLANG_BIN_DIR`, …). Solo x86_64: Windows ARM lo ejecuta por emulación.
Qué funciona y qué no en Windows: [`PRODUCTION.md`](PRODUCTION.md#windows).

Para actualizar a la última versión (o consultar si hay una nueva):

```sh
ray upgrade            # descarga la última release y reemplaza los binarios instalados
ray upgrade --check    # solo informa (0 = al día, 1 = hay versión nueva)
ray toolchain install  # Rust privado para `ray build --native` en un equipo sin Rust (+ vendor: primer build sin red)
```

### Desde el código

```sh
git clone https://github.com/ray-language/raylang
cd raylang
cargo build --release          # target/release/ray
```

> Rust se instala vía [rustup](https://rustup.rs/). Para un binario mínimo (sin TLS/cripto, SQLite ni
> carga de código nativo): `cargo build --release --no-default-features --features interp`. Ver
> [`docs/build.md`](docs/build.md).

## Uso

```sh
ray new hola           # crea un proyecto (ray.toml + src/main.ray)
cd hola
ray run                # ejecuta src/main.ray en la VM
ray dev                # modo desarrollo: recompila y reinicia ante cambios (+ live-reload del navegador)
ray build              # chequea y compila sin ejecutar
ray build --native     # transpila a Rust y compila un binario nativo (3–57× la VM, según la carga)
ray test               # corre las funciones @test
ray fmt src/main.ray   # formatea
ray doc src/main.ray   # genera documentación desde /// 
ray build --templates-only vistas/      # compila templates .ray.html a funciones raylang tipadas (SSR)
ray repl               # REPL interactivo
ray lsp                # servidor LSP (diagnósticos, hover, definición, refs, rename, completado, formateo, símbolos…)
ray mcp                # servidor MCP para agentes LLM (check/run/test/fmt/doc, con el código confinado)
```

**Gestor de paquetes** (manifiesto `ray.toml` + lockfile `ray.lock` con hashes SHA-256):

```sh
ray add textutils@^1.2 # añade una dependencia del registro y la descarga
ray remove textutils   # la quita (y su caché si nadie más la usa)
ray search json        # busca en el registro
ray fetch              # descarga a .ray-deps/ lo que declara ray.toml
ray update             # re-resuelve a las más nuevas compatibles
ray registry publish            # publica TU paquete (valida + chequea + hashea; --sign lo firma)
```

**Modo desarrollo** (`ray dev`): vigila los fuentes, recompila en ms y **solo reinicia si el cambio
compila** (un error a medio escribir no tira el servidor que funciona). Con `--port 8080` el supervisor
**retiene el socket** entre reinicios (cero conexiones rechazadas) e inyecta **live-reload** en el
navegador. Detalle en [`MANUAL.md`](MANUAL.md#17-herramientas).

El código de salida de `ray run` es el `int` que devuelve `main` (0 si es `unit`). Para embeber raylang
confinado: `ray run --fuel N` (límite de instrucciones) y `--heap N` (tope de objetos). Concurrencia
reproducible: `--deterministic`.

**Deploy nativo** (*dev = VM / deploy = nativo*, como Rust): `ray build --native prog.ray` produce un
ejecutable de código máquina, byte-idéntico a la VM, cuya concurrencia corre sobre un scheduler **M:N de
fibras**. Los subsistemas con crate de producción (TLS, criptografía, SQLite, regex acelerada) se enlazan
**solo cuando el programa los usa**; `--release` sube el tier de optimización, `--target` cross-compila y
`--without crypto,tls,sqlite,regex,mimalloc,ahash,fibers,process` (o `[native] without = [...]` en
`ray.toml`) excluye lo que no quieras dentro — quitando `mimalloc,ahash,fibers` se vuelve a la vía rápida
de `rustc` pelado. Ver [`docs/transpilador-nativo.md`](docs/transpilador-nativo.md).

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

Hay **173 ejemplos** en [`examples/`](examples/): desde `fib`/`fizzbuzz` hasta trait objects, structured
concurrency, un servidor web, WebSockets, y el propio compilador auto-alojado en [`selfhost/`](selfhost/).

## Playground web

**Pruébalo ya, sin instalar nada: [raylang.dev/playground](https://raylang.dev/playground/)** —
editor real (CodeMirror) con el LSP de raylang corriendo dentro del wasm: diagnósticos,
autocompletado, hover y formateo en el navegador.

Para correrlo en local, raylang corre en el navegador (la VM compilada a `wasm32`, **cero `wasm-bindgen`**):

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
- **Dependencias contadas y justificadas.** La pila de red y formatos (HTTP/2, HPACK, JSON, TOML, DNS…)
  está **escrita en raylang** (`packages/`), y el runtime (LSP, poller `kqueue`/`epoll`, SHA del gestor
  de paquetes, transpilador) en el propio Rust del proyecto, sin crates. Los crates que sí entran —TLS,
  cripto, SQLite, carga de librerías, fibras, allocador y hasher— están enumerados con su porqué y su
  alcance en [`SECURITY.md`](SECURITY.md#política-de-dependencias); un build *slim* deja fuera los tres
  primeros.
- **Robusto ante entrada arbitraria.** Compilador sin pánicos + **fuzzing continuo** del front-end.

## raylang y los agentes LLM

raylang trae de serie las dos piezas para que un agente de código escriba raylang correcto:

- **[`llms.txt`](llms.txt)** — el contexto destilado (~250 líneas): el delta contra Rust, las
  formas canónicas y los mensajes de error exactos. Pégalo en tu prompt / `CLAUDE.md` (o deja
  que tu cliente MCP cargue el resource `raylang://llms.txt`).
- **`ray mcp`** — un servidor [MCP](https://modelcontextprotocol.io) embebido en el binario, que
  da al modelo el bucle **escribir → verificar → corregir**: tools `ray_check` (diagnósticos
  exactos), `ray_run`, `ray_test`, `ray_fmt` y `ray_doc`, con el código del modelo **confinado**
  (fuel + heap + plazo, en subproceso). Con Claude Code: `claude mcp add raylang -- ray mcp`.
  Guía completa: [`docs/mcp.md`](docs/mcp.md).

## Documentación

| Documento | Qué es |
|-----------|--------|
| [`MANUAL.md`](MANUAL.md) | La **guía práctica**: cómo usar el lenguaje, idiomas, y mejores prácticas. |
| [`REFERENCE.md`](REFERENCE.md) | El **catálogo exhaustivo**: palabras clave, operadores, builtins, prelude, `std/` y CLI, con firmas. |
| [`PUBLISH.md`](PUBLISH.md) | La guía del **publicador**: empaquetar, versionar y publicar en el registro. |
| [`SPEC.md`](SPEC.md) | La **especificación normativa** del lenguaje (gramática + semántica). |
| [`llms.txt`](llms.txt) | **raylang para LLMs**: el contexto destilado (delta vs Rust, formas canónicas, errores exactos) para que un modelo escriba raylang correcto. Pégalo en tu prompt/CLAUDE.md. |
| [`docs/mcp.md`](docs/mcp.md) | El **servidor MCP** (`ray mcp`): las tools check/run/test/fmt/doc para agentes LLM, con el código confinado (fuel/heap/plazo). |
| [`docs/web-framework.md`](docs/web-framework.md) | La guía del **framework web** (estilo Express): rutas, middleware, SSR, deploy. |
| [`docs/build.md`](docs/build.md) | La guía de **builds**: features slim, PGO, binario nativo. |
| [`docs/transpilador-nativo.md`](docs/transpilador-nativo.md) | El **backend nativo** por dentro: cómo se transpila a Rust y cómo se garantiza la paridad. |
| [`docs/diseno-concurrencia-nativa.md`](docs/diseno-concurrencia-nativa.md) | El **scheduler de fibras M:N** del binario nativo: corrutinas, reactor y decisiones. |
| [`PERFORMANCE.md`](PERFORMANCE.md) | La **crónica de rendimiento**: cada arco de optimización, medido. |
| [`PRODUCTION.md`](PRODUCTION.md) | El **contrato de producción**: ejes, invariantes y criterios de calidad vigentes. |
| [`book/`](book/) | El **libro** (mdBook): cómo se **construyó** el lenguaje, fase a fase. |
| [`DESIGN.md`](DESIGN.md) | La **crónica de diseño**: cada decisión y su porqué. |
| [`IDEAS.md`](IDEAS.md) | Backlog de features y su clasificación de impacto. |
| [`docs/organizacion-codigo.md`](docs/organizacion-codigo.md) | Cómo está **organizado el código** del compilador (módulos-directorio, dónde viven los tests). |
| [`SECURITY.md`](SECURITY.md) | Política de seguridad, política de dependencias y modelo de amenazas. |
| [`CHANGELOG.md`](CHANGELOG.md) | Qué cambió en cada versión. |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | La **guía de contribución**: flujo de PRs, principios no negociables y batería de admisión de módulos. |
| [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) | Código de conducta (Contributor Covenant 2.1) que rige la participación en el proyecto. |
| [`RELEASE-1.0.md`](RELEASE-1.0.md) | Checklist histórica del lanzamiento 1.0 (las releases se publican desde la v1.1.0). |

Editores: extensión de [VSCode](editors/vscode/) (con cliente LSP), paquete de [Sublime Text](editors/sublime/),
extensión de [Zed](https://github.com/ray-language/zed-raylang) (tree-sitter + `ray lsp`)
y config para Neovim/Helix (usan `ray lsp` directo).

## Estado

**raylang 1.3.1** (última release publicada; las versiones salen como Releases de GitHub con binarios
por plataforma), con trabajo continuo sobre esa línea (rendimiento, concurrencia nativa, framework web,
procesos del SO). Motor de producto = la VM; el binario nativo es el destino de despliegue y el
intérprete, el oráculo de desarrollo. La suite tiene **703 tests unitarios** + **123 archivos de tests de
integración** (incluido un fuzzer del front-end, los oráculos VM↔intérprete y el corpus de paridad del
binario nativo). Lo publicado y lo que está en camino, en [`CHANGELOG.md`](CHANGELOG.md).

El foco es **producción real**, con el alcance dicho de frente: lo hace un solo mantenedor y no ha pasado
una auditoría externa (ver [`SECURITY.md`](SECURITY.md#alcance)).

## Contribuir

La guía contractual está en **[`CONTRIBUTING.md`](CONTRIBUTING.md)**: el flujo (rama + PR contra
`main`, Conventional Commits en español, CI en verde con las guardas `fmt`/`naming`/`module`), los
principios no negociables (la SPEC manda, byte-identidad de los tres motores, idioma, errores como
valores) y la **batería de admisión** para módulos nuevos de `std/`/`packages/` (doc `///` en inglés,
fila en `REFERENCE.md`, tests por ambos motores, uso real demostrado). Antes de tocar comportamiento,
lee los documentos-contrato: [`SPEC.md`](SPEC.md) manda sobre la semántica y [`DESIGN.md`](DESIGN.md)
cuenta el porqué. Para reportar una vulnerabilidad, ver [`SECURITY.md`](SECURITY.md).

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
