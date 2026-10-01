# Plan: auditoría de la documentación y handbook técnico

Estado: **PROPUESTA** (1 oct 2026, sobre 1.27.26). Parte A es la auditoría de lo que hay;
parte B es el plan del handbook. Nada de esto está aplicado.

---

## A. Auditoría de la documentación actual

### A.1 El problema de fondo

La documentación creció **por acreción**: cada arco (M-número) dejó su párrafo en el sitio donde
cayó. El resultado es un `MANUAL.md` de 3407 líneas correcto en los detalles pero sin **caminos**:
quien llega desde la web queriendo «hacer una app móvil» no encuentra un capítulo que lo lleve
del `ray new` al `.ipa`. Las piezas existen, repartidas en cinco secciones y escritas en tono de
changelog (89 referencias a M-números, «desde 1.12.1», «IDEAS §97 #28»).

En paralelo, el **posicionamiento** cambió en la web (agentes LLM + escritorio/móvil + binario
nativo) y los demás documentos no lo siguieron.

### A.2 Hallazgos de posicionamiento y mensajes

| # | Hallazgo | Dónde |
|---|---|---|
| P1 | La web dice «Escribe como script, despliega como binario; nativo para agentes LLM, apps de escritorio y móvil». README y PRODUCTION siguen con «enfocado a **producción real**». | `README.md:23,329`, `README.en.md:23,332`, `PRODUCTION.md:1-3`, `DESIGN.md:41` |
| P2 | El prefacio del libro dice «no para producción», lo contrario del README. | `book/src/prefacio.md:4` |
| P3 | `PRODUCTION.md` no tiene eje para LLM/MCP ni para escritorio/móvil, que son la mitad del pitch actual. Su «estado medido (jul 2026)» está obsoleto (53k líneas → ~82k; 15 módulos std → 30; 123 tests de integración → 172). | `PRODUCTION.md:17-24,47-54` |
| P4 | Cifras de rendimiento cruzadas: README «2,6–4× / 14–28×» frente a web, MANUAL y PERFORMANCE «3–4× / 28–57×»; README «3–57×» mezcla ambas. Go/rustc: README gana «a Go en 5, rustc en 4», PERFORMANCE al revés. | `README.md:73-77,150`, `landing:565`, `MANUAL.md:3134`, `PERFORMANCE.md:17` |
| P5 | La landing dice que `net` y `db` están «escritos en raylang puro» con TLS y SQLite; README y PRODUCTION dicen rustls y rusqlite. | `landing:645-649`, `README.md:29-31` |
| P6 | `Cargo.toml` description: «casi cero dependencias». | `Cargo.toml:11` |
| P7 | «Nativo para agentes LLM» = `llms.txt` + `ray mcp` (ayudar a un LLM a *escribir* raylang). No hay nada para *escribir agentes* en raylang: ni `std/llm`, ni paquete `mcp`; raycode lleva su propio cliente Anthropic/OpenAI/MCP como código de app. | `site/landing.ray.html:290,376-430`; `ray-apps/raycode/src/{anthropic,openai,mcp}.ray` |
| P8 | `RELEASE-1.0.md` lista como pendiente publicar la SPEC (ya en raylang.dev/spec.html) y la extensión (ya publicada). | `RELEASE-1.0.md:35-37,62-65,76` |

### A.3 Hallazgos de enlaces y puntos de entrada

| # | Hallazgo | Dónde |
|---|---|---|
| E1 | **El enlace «Guía paso a paso» de Desktop y móvil apunta a la raíz de `MANUAL.md`** (3407 líneas, sin ancla). Es el hueco que motivó esta revisión. | `landing.ray.html:454,473` |
| E2 | La web solo tiene una entrada de aprendizaje: «Guía» → `docs/getting-started.md` en GitHub. No enlaza MANUAL ni REFERENCE (salvo E1 y REFERENCE.en desde la spec inglesa). No hay «learn / handbook». | `layout.ray.html:9`, `landing:367-368` |
| E3 | `getting-started.md` no cubre escritorio ni móvil: una celda de tabla para `ray bundle`. | `docs/getting-started.md:335` |
| E4 | La landing promete «config para Neovim/Helix en editors/»; no existe ahí (vive en `book/src/m10/lsp.md:125-137`). | `landing:486,496` |
| E5 | `README.en.md` «Full guide» del MCP enlaza la versión española. | `README.en.md:281` |
| E6 | El sitio publica solo index/spec/bench/playground. `pages.yml:10` dice que publica «el libro, los docs»: falso. `book/` no se publica ni se enlaza renderizado. | `site/site.ray`, `.github/workflows/pages.yml:10` |
| E7 | `examples/README.md` manda a `cargo run`, no cubre `db/`, `ffi/`, `term/`, y presenta `examples/web/*.ray` como librerías cuando son copias viejas de `packages/net` (http.ray 875 vs 1402 líneas; webserver 982 vs 2642). | `examples/README.md:5-8` |

### A.4 Hallazgos de cobertura (features recientes sin guía)

Leyenda: **ejemplo** = explicado con código · **mención** = una línea/celda · **ausente**.

| Feature | MANUAL | REFERENCE | Calidad |
|---|---|---|---|
| Firma y notarización (`--sign/--notary`, `[app] sign`, `RAY_SIGN_IDENTITY`, signtool; M249) | 2352-2356 dentro de `std/update`; **1556 dice «en v1 no hay firma/notarización»** | 566: misma celda se contradice | **contradictoria** |
| `std/keychain` (M266) | ausente; 2195-2218 enseña `process.run("security", …)`, lo que el módulo sustituye | 1 fila | **ausente / consejo obsoleto** |
| Pools (`net/pool`, `db.pool_*`, `redis.pool`, `http.pool`; M318/M320) | un comentario en 2587 | 474-496 | **ausente en MANUAL** |
| gRPC servidor/cliente (net 0.5.0, M321) | «HTTP/2 + gRPC» en 2546 | 470 | sin ejemplo en ninguna guía |
| `rpc.pool_call_with`, `serve_on*` (M306/M314) | — | — | **ausente** (solo README del paquete) |
| `web app.gzip()` (M306), `serve_file[_with]`, `static_mount_with` (M271-M283) | — | parcial | **ausente** (tampoco en `docs/web-framework.md`) |
| Paquetes `tz` y `cron` | — | — | **ausentes** en MANUAL, REFERENCE y llms.txt |
| `main` como fibra en nativo (M329), `RAYLANG_SPIN_US` | — | — | **ausente** |
| `ui.replace_menu`, evento `focused`, `Cmd.spawn_detached`, `process.self_command`, `arch()` | — | sí | **ausentes en MANUAL** |
| Código de salida 74 (firma no verifica) | — | §15 no lo lista | **ausente** |
| `ray dev-client` / `ray dev-lib` | de pasada | sin fila | mención |
| Flags de CLI: `run --devtools`; `build --native --lib/--devtools/--no-stubs/--embed`; 10 variables de entorno (`RAY_SIGN_IDENTITY`, `RAY_DEV_LIB`, `RAY_UI_BACKEND`, `RAY_AUDIO_SINK`, `RAY_KEYCHAIN_FILE`…) | — | ausentes en §14 | **ausente** |
| Catálogo de std en MANUAL §12 (1095-1097) | omite io, term, kv, bigint, keychain, audio, ui, embed, image, ffi, zip, update, markdown | — | obsoleto |
| `docs/mcp.md` | tabla de tools solo con `code`; el servidor acepta `path` (proyecto en disco) en las cinco tools; «~300 líneas» (son 1181) | — | obsoleto |
| Hot reload móvil, Vite, diálogos, menús, ventanas, `std/update`, `ray release`, audio en segundo plano | ejemplo | sí | **bien**, pero mal ubicado (ver A.5) |

Positivo: REFERENCE.md y REFERENCE.en.md están en sincronía (mismas 15 secciones), y los 45
módulos de `src/stdlib.rs` están todos en REFERENCE §10.

### A.5 Estructura del MANUAL: el cajón de sastre

§13 «I/O y sistema» ocupa 1131-2509 (el 40 % del manual) y mezcla: consola, terminal, imagen,
audio, embed, **bundle (desktop+iOS+Android)**, **hot reload móvil**, **ventanas `std/ui`** (300
líneas), Vite, markdown, fs, procesos, **auto-update + firma**, crypto, time. El índice no contiene
las palabras «escritorio» ni «móvil». Caminos evaluados:

- **App móvil: no hay camino.** Bundle iOS/Android (1414-1469), hot reload (1471-1559, con un
  párrafo de escritorio huérfano en 1554-1559), puente JS↔raylang (1619-1696), Vite en el
  teléfono (1906), servidor local cerrado (§14 + SECURITY). Sin secuencia `new → ui.open → dev
  --device → bundle --ios → firma → tienda`.
- **App de escritorio: a medias.** Bundle antes que ventana; el patrón de app está en §14 (2607);
  live-reload y devtools en §17; firma contradicha.
- **API HTTP: aceptable para empezar** (§14 + `docs/web-framework.md`); para producción faltan
  pools, gzip, streaming, gRPC.
- **CLI: pobre.** `args()` en 8 líneas (2109-2117); nada de flags, códigos de salida,
  `std/term`, ni `ray build --native` como entregable.
- **Agente LLM / MCP: nada**, salvo `ray mcp` como servidor para la toolchain.

### A.6 Inventario de lo que sí sirve (base del handbook)

- `MANUAL.md` §1-§12 (lenguaje) — bueno, se conserva como referencia del lenguaje.
- `MANUAL.md` §13 ventanas/bundle/hot-reload/Vite/update — contenido correcto, a **reordenar**.
- `docs/getting-started.md` (bilingüe), `docs/web-framework.md`, `docs/build.md`,
  `docs/windows.md`, `docs/mcp.md` (bilingüe, desactualizado).
- `packages/*/README.md` (net 208 l, db 258 l, rpc 113 l; web/cron/tz cortos).
- `examples/web/desktop_window/`, `examples/web/framework/`, `examples/db/` como proyectos reales.
- Apps dogfood con README útil: raydesk (escritorio+iOS+Android, diagrama IPC), raynote, raycode
  (cliente Anthropic/OpenAI + cliente MCP stdio/HTTP), ray808 (audio móvil).
- Guardas CI reutilizables: `tests/docs_root.rs` (enlaces + huérfanos + tabla CLI),
  `tests/docs_i18n.rs` + `tools/docs_sync.py` (traducciones), `tests/llms_signatures.rs`
  (firmas citadas vs `ray doc`), `tests/fmt_policy.rs`.

---

## B. Plan: el handbook técnico

### B.1 Principios

1. **Organizado por lo que el desarrollador quiere construir**, no por módulo: cada capítulo es
   un camino completo de `ray new` a entregable instalado/desplegado.
2. **Un proyecto de ejemplo real por capítulo**, en `examples/apps/<nombre>/` con `ray.toml`,
   `src/`, tests `@test`, compilado en CI. El capítulo cita el ejemplo; el ejemplo es la verdad.
3. **Sin historia**: ningún M-número, ninguna «desde 1.x», ningún «#findings». Lo que no
   funciona aún se dice en presente («no soportado»). La historia queda en CHANGELOG/DESIGN.
4. **Cada snippet compila**: todo bloque ```raylang del handbook pasa por `ray check` en CI (con
   el proyecto de ejemplo del capítulo como contexto para imports y dependencias).
5. **Publicado en raylang.dev**, no solo en GitHub: el mismo pipeline `site/site.ray` +
   `std/markdown` que ya renderiza la SPEC. Bilingüe ES/EN con la guarda `docs_i18n` existente.
6. **Reparto de responsabilidades claro**: handbook = caminos (cómo construir X); MANUAL =
   referencia del lenguaje (§1-§12 + concurrencia + FFI); REFERENCE = catálogo de firmas;
   SPEC = normativa; book = crónica histórica (marcada como tal).

### B.2 Dónde vive

```
handbook/
  SUMMARY.md              # índice (fuente para la navegación del sitio)
  00-empezar.md           # instalar, ray new, run/build/test, editor + MCP
  10-cli.md               # herramienta de línea de comandos
  20-api.md               # API HTTP de producción
  30-escritorio.md        # app de escritorio
  40-movil.md             # app iOS / Android
  50-multiplataforma.md   # un fuente → escritorio + móvil
  60-llm-mcp.md           # usar un LLM para escribir raylang; escribir agentes y MCP
  70-servicios.md         # rpc/gRPC, colas, cron, resiliencia, tracing
  80-distribuir.md        # bundle, firma, notarización, update, release, CI
  90-rendimiento.md       # perfilar, nativo, flags, memoria
  *.en.md                 # traducciones, sincronizadas con docs_sync
examples/apps/
  todo-cli/  notes-api/  notes-desktop/  notes-mobile/  notes-everywhere/  agent/  mcp-server/
```

Publicación: `site/site.ray` renderiza `handbook/*.md` → `_site/handbook/<slug>.html` y
`_site/en/handbook/...`, con una plantilla `handbook_page.ray.html` (barra lateral desde
`SUMMARY.md`). `pages.yml` asevera los HTML generados como ya hace con spec/bench. La landing
cambia «Guía paso a paso en el manual» por el enlace al capítulo concreto (`/handbook/movil.html`),
y la navegación gana «Handbook».

Alternativa descartada: mdbook. Ya hay un generador propio en raylang que produce el sitio;
añadir una toolchain Rust externa para la mitad de las páginas duplica estilos y navegación.

### B.3 Índice de capítulos y qué resuelve cada uno

**0. Empezar** (reescritura de `getting-started`): instalar, `ray new`, el bucle
`run/test/fmt/build`, editor (VS Code/Sublime/Zed/Neovim), conectar `ray mcp` al asistente,
«qué leer después» según lo que quieras construir.

**1. Herramienta CLI** (`examples/apps/todo-cli`): `args()` y parseo de flags (receta propia,
no hay módulo), `std/io`/`std/term` (colores, teclas, barra de progreso), stdin por bytes,
`exit`/códigos de salida, `std/fs` con errores como valores, `std/process` para hablar con
otros programas, `ray build --native` como entregable, binarios por plataforma en CI.

**2. API HTTP de producción** (`examples/apps/notes-api`): `web` (rutas, middleware, JSON,
validación), `db` con **pools** y transacciones, estado compartido (actor, nunca captura),
gzip, streaming/SSE, `listen_graceful` + señales, TLS, `net/trace`, `std/resilience`, tests
`@test` con cliente HTTP, despliegue como binario nativo (systemd, contenedor slim, `--without`).

**3. App de escritorio** (`examples/apps/notes-desktop`): arquitectura (ventana `std/ui` +
backend raylang en el mismo proceso), `ray://app` sin puerto, puente `window.ray.request` /
`ui.reply`, menús/roles de edición/`replace_menu`, diálogos, tipos de ventana, `intercept_close`,
`std/kv`/`std/keychain` para datos y secretos, `std/embed`, live-reload y devtools, frontend con
Vite (React/Vue/Svelte) y `ray dev`, backends por plataforma (WKWebView / WebKitGTK / WebView2).

**4. App móvil** (`examples/apps/notes-mobile`): qué cambia en el teléfono (`window = 0`, sin
`process`, cwd, ciclo de vida), `ray bundle --ios` (proyecto Xcode, firma persistente,
simulador/dispositivo), `ray bundle --android` (Gradle, ABI, firma release), **hot reload**
`ray dev --device` + shell `--dev` + QR, devtools en el móvil, audio y segundo plano,
publicar en App Store / Play (hasta donde la toolchain llega, y dónde termina).

**5. Multiplataforma** (`examples/apps/notes-everywhere`, modelado sobre raydesk): un `src/`
para las cinco plataformas, `platform()`/`arch()` y ramas por plataforma, assets compartidos,
layout responsivo en el webview, `ray.toml` con `[app]`/`[ios]`/`[android]`/`[native]`, matriz de
CI que produce `.app`/`.exe`/`.desktop`/Xcode/Gradle.

**6. LLM y MCP** (dos mitades, `examples/apps/agent` y `examples/apps/mcp-server`):
(a) *usar* un LLM para escribir raylang: `llms.txt`, `ray mcp` con el argumento `path`, el
bucle escribir→verificar→corregir, CLAUDE.md de un proyecto raylang;
(b) *escribir* un agente en raylang: cliente de la API de mensajes con `net/http` streaming +
`net/sse` + `std/json`, bucle de tool-use, `std/keychain` para la clave, cliente MCP por stdio
(`std/process` sesión) y por HTTP, y un servidor MCP propio en raylang (JSON-RPC por stdio).
Ver decisión abierta D3.

**7. Servicios** (`examples/apps/notes-api` extendido): `rpc` y gRPC servidor/cliente, pools de
RPC, `cron`, `tz`, colas (patrón rayq), resiliencia, tracing distribuido, observabilidad.

**8. Distribuir**: `ray bundle` por plataforma, **firma y notarización** (macOS `--sign/--notary`,
Windows signtool, Linux), `std/update` + `ray keygen`/`ray release`, códigos de salida (incl. 74),
`install.sh` propio, GitHub Actions de release, builds reproducibles.

**9. Rendimiento**: VM vs nativo (cifras **únicas** y actualizadas, fuente `benchmarks/poly`),
`ray profile`, PGO, mimalloc/ahash/fibras, memoria en servidores, `RAYLANG_THREADS`,
determinismo.

### B.4 Correcciones inmediatas (independientes del handbook, PR 1)

1. Landing: enlace de Desktop y móvil → `MANUAL.md#empaquetar-la-app-ray-bundle` hoy, al
   capítulo del handbook cuando exista. Quitar «config en editors/» para Neovim/Helix o añadir
   `editors/nvim-helix.md`. `README.en.md:281` → `mcp.en.md`.
2. Un solo lema en todas partes: README (ES/EN) y PRODUCTION adoptan el pitch de la web y
   retiran «producción real»; `Cargo.toml` description sin «casi cero dependencias»; PRODUCTION
   gana ejes «Apps (escritorio y móvil)» y «Agentes LLM» y refresca el estado medido.
3. Cifras de rendimiento: una sola fuente (`benchmarks/poly/README.md`) y README/landing/MANUAL
   /PERFORMANCE citan lo mismo.
4. MANUAL: borrar «en v1 no hay firma/notarización» (1556) y la celda contradictoria de
   REFERENCE 566; añadir `std/keychain` y retirar la receta con `security`; añadir `tz`/`cron`,
   código 74, flags y variables de entorno ausentes de REFERENCE §14; catálogo de §12 completo.
5. `docs/mcp.md` (+ EN): argumento `path`, recuento real, instrucciones del `initialize`.
6. `examples/README.md`: `ray run`, cubrir db/ffi/term, marcar `examples/web/*.ray` como
   copias históricas (o moverlas a `examples/historico/`).
7. `pages.yml:10` y `RELEASE-1.0.md`: decir la verdad sobre qué se publica.
8. `book/`: prefacio con aviso «crónica histórica de la construcción (M1-M40); para usar
   raylang ve al handbook».

### B.5 Fases

| Fase | Entregable | PR |
|---|---|---|
| 0 | Correcciones inmediatas B.4 (solo docs + landing) | 1 |
| 1 | Infraestructura: `handbook/` + `SUMMARY.md`, plantilla y renderizado en `site.ray`, aserciones en `pages.yml`, guarda `tests/handbook_snippets.rs` (cada bloque ```raylang → `ray check` con el proyecto de ejemplo como contexto), extensión de `docs_root.rs` a `handbook/` (enlaces y huérfanos), `docs_i18n` cubre `handbook/*.en.md`. Capítulo 0 migrado desde getting-started. Navegación «Handbook» en la landing. | 2 |
| 2 | **Capítulo 3 Escritorio + Capítulo 4 Móvil** con `notes-desktop` y `notes-mobile`; la landing enlaza a ellos. Es el hueco que motivó la revisión: va primero. | 3-4 |
| 3 | Capítulo 2 API + Capítulo 7 Servicios con `notes-api` (pools, gzip, gRPC, cron). Absorbe `docs/web-framework.md` (queda como redirección). | 5 |
| 4 | Capítulo 6 LLM y MCP con `agent` y `mcp-server`. Absorbe `docs/mcp.md`. | 6 |
| 5 | Capítulo 1 CLI, Capítulo 5 Multiplataforma (`notes-everywhere`), Capítulo 8 Distribuir, Capítulo 9 Rendimiento (absorbe `docs/build.md` en lo que es guía). | 7-8 |
| 6 | Poda del MANUAL: §13 se queda con io/term/fs/process/time/crypto; ventanas, bundle, hot reload, Vite, update y firma se mueven al handbook y el MANUAL los enlaza. Barrido de M-números del MANUAL. | 9 |
| 7 | Traducción EN de todos los capítulos (sincronizada con `docs_sync.py`); `llms.txt` apunta a los capítulos; `ray mcp` expone `raylang://handbook/<cap>.md` como resources. | 10 |

Cada PR es solo-docs salvo la fase 1 (tests + `site.ray`) y los proyectos de `examples/apps/`
(compilan en CI). Las fases 2-5 pueden ir en paralelo si hace falta.

### B.6 Guardas de CI nuevas

- `tests/handbook_snippets.rs`: extrae bloques ```raylang de `handbook/*.md`; los que llevan
  `// project: examples/apps/<x>` se chequean con ese proyecto como raíz; el resto, aislados.
  Bloques marcados ```raylang,ignore se saltan (fragmentos). Falla el CI si algo no compila.
- `tests/docs_root.rs` extendido: enlaces relativos de `handbook/` resuelven; todo capítulo está
  en `SUMMARY.md`; ningún M-número (`\bM\d{2,3}\b`) en `handbook/`.
- `pages.yml`: `test -f _site/handbook/movil.html` etc. para cada entrada de `SUMMARY.md`.
- `tests/docs_i18n.rs` ya cubre `.en.md` en raíz y `docs/`; se añade `handbook/`.
- Los proyectos `examples/apps/*` entran en la batería de ejemplos que ya compila el CI
  (`ray check` + `ray test`); los de escritorio/móvil al menos `ray check` y `ray bundle --dry-run`
  si existe, o `ray build --native --lib` para iOS/Android en el job de macOS.

### B.7 Decisiones abiertas (del usuario)

- **D1. Nombre y URL**: `handbook/` → raylang.dev/handbook/ (propuesto) frente a «guides» o
  integrarlo como MANUAL v2.
- **D2. Idioma de origen**: ES original + EN traducido (como hoy, con `docs_sync`) o EN original
  (la web y las apps apuntan a público internacional; el MCP y llms.txt ya son EN).
- **D3. LLM/MCP como código, no solo docs**: el capítulo 6b necesita un cliente de LLM y un
  cliente/servidor MCP. Opciones: (a) el capítulo enseña a hacerlo a mano con `net/http` +
  `std/json` + `std/process` y cita raycode; (b) extraer de raycode un paquete `llm` (Anthropic +
  OpenAI, streaming, tool-use) y otro `mcp` (cliente stdio/HTTP + servidor) a `packages/`, y el
  handbook los usa. (b) hace verdad el «nativo para agentes LLM» de la landing; es un arco de
  código con su propio IDEAS. Recomendación: empezar por (a) en el handbook y abrir (b) como
  arco aparte.
- **D4. Destino de `book/`**: dejarlo en el repo como crónica con aviso (propuesto) o publicarlo
  en raylang.dev/book con mdbook. Publicarlo cuesta un job más y expone un texto que dice «no
  para producción».
- **D5. Qué absorbe el handbook**: `getting-started`, `web-framework`, `mcp` y la parte guía de
  `build.md` (propuesto); `windows.md` y `transpilador-nativo.md` siguen como contratos/diseño.
