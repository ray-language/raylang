# Plan: auditoría de la documentación y handbook técnico

Estado: **APROBADO con decisiones** (1 oct 2026, sobre 1.27.26). Parte A es la auditoría de lo que
hay; parte B es el plan del handbook, ya con las decisiones del usuario (B.7). La fase 0 (B.4) se
ejecutó en la PR #448 (fusionada el 1 oct 2026); la fase 1 va en `docs/handbook-fase1`.

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
4. **Cada snippet compila**: todo bloque ```rust del handbook pasa por `ray check` en CI (con
   el proyecto de ejemplo del capítulo como contexto para imports y dependencias). Convención:
   el código raylang se etiqueta **`rust`** en todo Markdown (coloreado en GitHub y en el sitio
   hasta que raylang tenga soporte propio); nunca `raylang` ni sin etiqueta.
5. **Publicado en raylang.dev**, no solo en GitHub: el mismo pipeline `site/site.ray` +
   `std/markdown` que ya renderiza la SPEC. Bilingüe ES/EN con la guarda `docs_i18n` existente.
6. **Reparto de responsabilidades claro**: handbook = caminos (cómo construir X); MANUAL =
   referencia del lenguaje (§1-§12 + concurrencia + FFI); REFERENCE = catálogo de firmas;
   SPEC = normativa; book = crónica histórica (marcada como tal).

### B.2 Dónde vive

```
handbook/
  index.md                # portada; el ORDEN de sus enlaces es el de la barra lateral
  empezar.md              # instalar, editor, ray new, el lenguaje, concurrencia, ray mcp, herramientas
  10-movil.md             # app iOS / Android con frontend react-ts        · persistencia: std/kv
  20-multiplataforma.md   # la misma app en escritorio (continuación)       · persistencia: SQLite (db)
  30-api.md               # API web con el framework `web`                  · persistencia: Postgres (db) + pools
  40-ssr.md               # sitio SSR con plantillas .ray.html compiladas   · persistencia: archivos (std/fs + JSON)
  50-web-frontend.md      # sitio con frontend react-ts embebido             · persistencia: Redis (db)
  60-llm-mcp.md           # usar un LLM para escribir raylang (ray mcp); el patrón de agente a mano
  70-distribuir.md        # bundle, firma, notarización, update, release, CI
  80-cli.md               # herramienta de línea de comandos (más adelante)
  90-rendimiento.md       # perfilar, nativo, flags, memoria (más adelante)
  *.en.md                 # cada capítulo existe en ES y EN, sincronizados con docs_sync
examples/apps/
  notes-mobile/  notes-everywhere/  notes-api/  notes-ssr/  notes-web/  agent-cli/
```

Publicación: **raylang.dev/handbook/** (decisión D1). `site/site.ray` renderiza `handbook/*.md` →
`_site/handbook/<slug>.html` y `_site/en/handbook/...` (módulo `site/handbook.ray`), con una
plantilla `handbook_page.ray.html` y la barra lateral sacada de los enlaces de `index.md` (el
título de cada capítulo es su primer `# `). Los nombres de archivo son el slug de la URL, sin
número de orden: renombrar un capítulo no rompe enlaces. Los enlaces entre capítulos pasan a
`.html`, y los que salen del handbook (`../MANUAL.md#…`) apuntan al repositorio en GitHub. `pages.yml` asevera los HTML generados como ya hace con
spec/bench. La landing cambia «Guía paso a paso en el manual» por el enlace al capítulo concreto
(`/handbook/movil.html`), y la navegación gana «Handbook».

Alternativa descartada: mdbook. Ya hay un generador propio en raylang que produce el sitio;
añadir una toolchain Rust externa para la mitad de las páginas duplica estilos y navegación.

### B.3 Índice de capítulos y qué resuelve cada uno

Decisión del usuario: las guías son **apps completas encadenadas** (una libreta de notas que crece),
cada una con una **persistencia distinta** para cubrir toda la gama de la stdlib y de `db`.

**0. Empezar** (reescritura de `getting-started`): instalar, `ray new`, el bucle
`run/test/fmt/build`, editor (VS Code/Sublime/Zed/Neovim), conectar `ray mcp` al asistente,
«qué leer después» según lo que quieras construir.

**1. App móvil iOS/Android con react-ts** (`examples/apps/notes-mobile`, persistencia `std/kv`):
`ray new notes --frontend react-ts`, arquitectura (webview `std/ui` + backend raylang en el mismo
proceso), puente `window.ray.request` / `ui.reply`, `app://` en dev y `ray://app` empaquetado,
qué cambia en el teléfono (`window = 0`, sin `process`, ciclo de vida), `ray bundle --ios`
(Xcode, firma persistente, simulador/dispositivo), `ray bundle --android` (Gradle, ABI, firma de
release), **hot reload** `ray dev --device` + shell `--dev` + QR, devtools en el móvil, audio y
segundo plano, hasta dónde llega la toolchain para App Store / Play.

**2. App multiplataforma** (continuación: `examples/apps/notes-everywhere`, persistencia SQLite
vía `db`): el mismo `src/` en macOS, Linux, Windows, iOS y Android; `platform()`/`arch()` y ramas
por plataforma; menús, roles de edición, diálogos, tipos de ventana, `intercept_close`;
`std/keychain` para secretos; `std/embed`; layout responsivo; `ray.toml` con
`[app]`/`[ios]`/`[android]`/`[native]`; `ray bundle` por plataforma y una matriz de CI que
produce `.app`/`.exe`/`.desktop`/Xcode/Gradle.

**3. API web con el framework** (`examples/apps/notes-api`, persistencia Postgres vía `db` con
**pools** y transacciones): rutas, middleware, JSON y validación, estado compartido (actor, nunca
captura), gzip, streaming/SSE, `listen_graceful` + señales, TLS, `net/trace`, `std/resilience`,
tests `@test` con cliente HTTP, despliegue como binario nativo (systemd, contenedor slim,
`--without`).

**4. Sitio SSR con plantillas de raylang** (`examples/apps/notes-ssr`, persistencia en archivos
con `std/fs` + JSON): `vistas/*.ray.html` compiladas con firma tipada (`ray build
--templates-only`), layouts y parciales, formularios y sesiones, estáticos con ETag, `std/markdown`
para contenido, live-reload con `ray dev`, binario único con las vistas dentro.

**5. Sitio web con frontend react-ts embebido** (`examples/apps/notes-web`, persistencia Redis vía
`db`): `[frontend]` + Vite en dev (`ray dev` arranca ambos), build embebido con
`ui.mount_embed_at`/`static_embedded` en producción, API JSON + SSE para la SPA, autenticación con
sesiones, un solo binario que sirve todo.

**6. LLM y MCP** (`examples/apps/agent-cli`): (a) *usar* un LLM para escribir raylang: `llms.txt`,
`ray mcp` con el argumento `path`, el bucle escribir→verificar→corregir, CLAUDE.md de un proyecto
raylang; (b) *escribir* un agente en raylang **a mano**, sin citar raycode (decisión del usuario):
cliente de la API de mensajes con `net/http` streaming + `net/sse` + `std/json`, bucle de
tool-use, `std/keychain` para la clave, cliente MCP por stdio (`std/process` sesión). Si se aprueba
IDEAS §100 (paquetes `llm`/`mcp`), el capítulo se reescribe sobre ellos.

**7. Distribuir**: `ray bundle` por plataforma, **firma y notarización** (macOS `--sign/--notary`,
Windows signtool, Linux), `std/update` + `ray keygen`/`ray release`, códigos de salida (incl. 74),
`install.sh` propio, GitHub Actions de release, builds reproducibles.

**8. Herramienta CLI** y **9. Rendimiento**: después de los anteriores (CLI con `args()`,
`std/io`/`std/term`, `std/process`, `ray build --native`; rendimiento con cifras únicas de
`benchmarks/poly`, `ray profile`, PGO, memoria, `RAYLANG_THREADS`).

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
8. `book/`: **se ignora por ahora** (decisión D4); solo se corrige el comentario de `pages.yml`
   que decía publicarlo.

### B.5 Fases

| Fase | Entregable | PR |
|---|---|---|
| 0 | Correcciones inmediatas B.4 (solo docs + landing). **Hecha**: PR #448, publicada. | 1 |
| 1 | **Hecha en local** (rama `docs/handbook-multiplataforma`; la PR #449 se cerró: el arco va en local hasta la revisión). Infraestructura `handbook/` + `index.md`, `site/handbook.ray` y plantilla, coloreado del código (`site/highlight.ray`), guarda `tests/handbook.rs` (marcas `check: skip` / `check: project=`), `docs_i18n`/`docs_sync` sobre `handbook/`, workflow `handbook.yml`. Capítulo Empezar, ES + EN. | 2 |
| 2 | **Hecha en local.** Capítulo App móvil (`notes-mobile`, react-ts + `std/kv`), ES + EN. Probado en el simulador de iOS y el emulador de Android, incluida la recarga en caliente; destapó M334 y M335 (arreglados en ramas locales `fix/android-insets` y `fix/dev-device-embed`, fusionadas en la rama de trabajo). | 3 |
| 3 | **Hecha en local.** Capítulo App multiplataforma (`notes-everywhere`, SQLite, menús y diálogos nativos, diseño adaptable), ES + EN. Probado: `.app` de macOS, simulador de iOS, emulador de Android y diseño de escritorio. | 4 |
| 4 | **Hecha en local.** Capítulo API web (`notes-api`: `web` + Postgres con pool compartido, token Bearer en tiempo constante, gzip, logs JSON, apagado ordenado). Probado contra un Postgres en Docker, con tests que se saltan sin base de datos. | 5 |
| 5 | **Hecha en local.** Sitio con plantillas (`notes-ssr`, archivos) y sitio con frontend React (`notes-web`: SPA embebida, API JSON, notas en Redis con scripts Lua atómicos; probado con un Redis en Docker, `ray dev` con el proxy de Vite y el binario nativo). ES + EN. | 6-7 |
| 6 | **Hecha en local.** Capítulo LLM y MCP (`agent-cli`: Messages API por HTTP con `claude-opus-5-5`, cliente MCP por stdio contra `ray mcp`, tests sin red). No se llamó a la API real (gastaría de la cuenta del usuario). | 8 |
| 7 | **Hecha en local.** Capítulo Distribuir: empaquetado y firma por sistema, tiendas móviles (AAB firmado verificado) y actualizaciones de escritorio con `std/update`, ensayadas de punta a punta (0.1.0 → 0.2.0, y rechazo de un manifiesto alterado). La firma y notarización de Apple no se pudo probar: no hay identidad «Developer ID Application». | 9 |
| 8 | Poda del MANUAL: §13 se queda con io/term/fs/process/time/crypto; ventanas, bundle, hot reload, Vite, update y firma se mueven al handbook y el MANUAL los enlaza. Barrido de M-números. `llms.txt` y `ray mcp` apuntan a los capítulos (`raylang://handbook/<cap>.md`). | 10 |
| 9 | Capítulos 8 CLI y 9 Rendimiento. | 11 |

Cada capítulo sale **con su traducción EN en la misma PR** (decisión D2). Cada PR es solo-docs
salvo la fase 1 (tests + `site.ray`) y los proyectos de `examples/apps/` (compilan en CI).

### B.5b Orden real de los capítulos

En el sitio el orden lo da `handbook/index.md`: Empezar, App móvil, App multiplataforma, Sitio con
plantillas, API web, Sitio con frontend React, LLM y MCP, y Distribuir. Los nombres de archivo son
el slug de la URL (`movil`, `multiplataforma`, `ssr`, `api`, `web-react`, `llm-mcp`, `distribuir`).
Todos los ejemplos usan los paquetes del registro (`ray add`), nunca rutas al repositorio. Los hallazgos de cada capítulo van a IDEAS §101.

### B.6 Guardas de CI nuevas

- `tests/handbook.rs` (hecho en la fase 1): todo bloque ```rust de `handbook/*.md` pasa
  `ray check`. Dos marcas invisibles, un comentario HTML en la línea anterior al bloque, cambian
  eso: `<!-- check: skip (motivo) -->` lo salta (fragmentos de varios archivos, código con
  dependencias) y `<!-- check: project=examples/apps/<x> -->` exige que el bloque aparezca tal
  cual en un `.ray` de ese proyecto y que el proyecto compile. Además: ningún bloque `raylang`,
  enlaces relativos que resuelven, cada capítulo enlazado desde `index.md` y con su `.en.md`, y
  ningún M-número fuera del código.
- `tests/docs_i18n.rs` y `tools/docs_sync.py` cubren `handbook/*.en.md` (hecho).
- `.github/workflows/handbook.yml` (hecho): `ci.yml` ignora `**.md` y `site/**`, así que una PR
  que solo toque capítulos no correría la guarda. Este workflow compila `ray`, corre
  `tests/handbook.rs` y `docs_i18n`, y genera el sitio aseverando una página por capítulo y
  idioma. Se espera verde antes de fusionar cualquier PR del handbook.
- `pages.yml` asevera las páginas del handbook en el despliegue (hecho).
- Los proyectos `examples/apps/*` entran en la batería de ejemplos que ya compila el CI
  (`ray check` + `ray test`); los de escritorio y móvil, al menos `ray check`, y
  `ray build --native --lib` para iOS/Android en el job de macOS.

### B.7 Decisiones (tomadas el 1 oct 2026)

- **D1. Nombre y URL**: `handbook/` → **raylang.dev/handbook/**.
- **D2. Idioma**: **ES y EN** desde el primer capítulo, sincronizados con `docs_sync`.
- **D3. LLM/MCP**: el capítulo enseña el patrón **a mano** y **no cita raycode** hasta que se
  decida el arco de paquetes `llm`/`mcp`, registrado en **IDEAS §100** (evaluación: qué
  estandarizar, de dónde extraerlo, impacto). La landing conserva «nativo para agentes LLM»
  apoyada en `llms.txt` + `ray mcp` mientras tanto.
- **D4. `book/`**: **se ignora por ahora**; no se publica ni se toca.
- **D5. Qué absorbe el handbook**: `getting-started`, `web-framework`, `mcp` y la parte guía de
  `build.md`; `windows.md` y `transpilador-nativo.md` siguen como contratos/diseño.
- **D6. Guías**: apps completas encadenadas con una persistencia distinta cada una (ver B.3):
  móvil iOS/Android con react-ts → multiplataforma → API web → sitio SSR con plantillas → sitio
  con frontend react-ts embebido.
