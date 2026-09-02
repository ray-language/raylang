# Changelog

Todas las versiones notables de raylang. El formato sigue el espíritu de
[Keep a Changelog](https://keepachangelog.com/) y el versionado es
[SemVer](https://semver.org/) (la versión del lenguaje y la de la stdlib van juntas; ver `SPEC.md` §12).

## Sin publicar

- **El puente IPC se endurece** (M159): la cola de eventos gana una **cota dura** (65536; al
  llenarse cae el `"message"` más viejo — nunca un `"closed"` — con aviso en stderr), solo el
  **main frame** puede hablar con el programa en macOS/iOS (un iframe de terceros ya no
  alcanza el puente ni con `postMessage` a mano; en Linux/Android el shim sigue siendo
  main-frame-only pero el handler no discrimina — nota de seguridad en el manual), y
  **`ui.split_events() -> (messages, other)`** parte el stream en dos canales — los
  `"message"` por uno, el resto por el otro — para consumirlos con `recv`/`select` sin
  filtrar a mano.

- **std/audio v2** (M158, §79b): `open_latency(rate, channels, latency_ms)` — el hint de
  latencia dimensiona anillo, buffers del dispositivo y chunk del alimentador (un juego
  rítmico pide 20-50 ms; una radio tolera 200-1000) — y **`played_ms(h)`**: la posición REAL
  de reproducción según el backend (AudioQueueGetCurrentTime / snd_pcm_delay /
  AAudioStream_getFramesRead), para sincronizar visuales con el audio. Y **Android gana
  audio**: backend **AAudio** por dlopen (≤50 ms pide el modo LOW_LATENCY) — `ray bundle
  --android` ya no excluye `audio`. Validado en dispositivo real de macOS (posición real por
  detrás de lo escrito) y batería de 3 motores sobre el sumidero de tiempo real.

- **El puente IPC gana payload JSON y petición-con-respuesta** (M157): `window.ray.send(v)`
  acepta no-strings (viajan como su JSON — parsear con `std/json`), y
  `window.ray.request(v)` devuelve una **Promise** que el programa resuelve con
  `ui.as_request(e)` + `ui.reply(window, id, valor)`. Cero maquinaria nativa nueva: el sobre
  viaja por el canal de mensajes existente y la respuesta por el `eval_js` fire-and-forget —
  el mismo contrato en macOS, Linux, iOS y Android (los 3 shims actualizados). Verificado el
  ciclo real en una ventana de macOS.

- **raylang corre en Android** (M156 fase 3 — §80b COMPLETO): `ray bundle --android` genera el
  proyecto Gradle (shell Java + WebView + el programa como `libray_app.so` con los símbolos
  JNI dentro). Validado en el emulador: webserver embebido sirviendo la UI, `window.ray.send`
  → evento `"message"` → `eval_js` de vuelta (el puente IPC completo), stdout en logcat (tag
  `ray`). `--android-abi arm64|x86_64|all` con preservación; `[android] application_id`;
  mismas exclusiones que iOS (process/audio). El MISMO fuente que escritorio/iOS.

- **El programa raylang compila a `.so` de Android** (M156 fase 2): `ray build --native --lib
  --target aarch64-linux-android` emite un **cdylib** con los símbolos JNI dentro
  (`JNI_OnLoad`, `Java_org_raylang_shell_RayBridge_*`) — el puente vive en el runtime (vtable
  JNI a mano, cero deps; MUTF-8 para los strings; stdout/stderr → logcat con tag `ray`). El
  build localiza el NDK solo (sin `cargo-ndk`) y alinea a 16KB (Android 15+). Verificado:
  `.so` arm64 de ~700KB con todos los símbolos en la tabla dinámica.

- **El runtime es Android-ready** (M156 fase 1): Android es unix pero no "linux" — nueve
  sitios con `cfg(target_os = "linux")` cuyo brazo contrario asumía Darwin quedaron en
  `any(linux, android)`; tres eran bugs latentes reales (el símbolo `__error` inexistente en
  bionic = error de link, y el RLIMIT emitido que en bionic habría subido el límite de
  PROCESOS en vez del de fds). Verificado: `ray-runtime` entero compila a
  `aarch64-linux-android` con todas las features (fibras epoll, tls, sqlite, shell).

- `ray bundle --ios` gana **`--ios-target device|sim|both`** (both por defecto): iterando
  contra un solo destino, el otro build (~15-20 s) sobra — y el `.a` del lado no construido
  se **preserva** del proyecto anterior (como la firma). El log además nombra cada build
  (`[bundle] device (aarch64-apple-ios)…`) — parecían un doble accidental.

- **El panel About nativo es personalizable** (M155): `ui.set_about(name, version,
  description, copyright)` llena el panel estándar de macOS que abre el item
  `"role:about"` — nombre en negrita, línea "Version …", descripción (como créditos, al
  estilo del Finder) y copyright; `""` omite un campo. Y en el `.app` de `ray bundle`,
  `[app] copyright = "…"` del ray.toml se escribe en el Info.plist (el panel lo muestra
  sin código). Declarado en ray_doc/REFERENCE/MANUAL — visible desde el MCP.

- **`web.state` — estado de aplicación compartido entre handlers** (M154, raydesk #8): el
  gemelo de `sessions` sin cookies — `state(path)` (persiste bajo `ray dev`, memoria pura en
  producción) o `state_memory()`, con `state_get/put/delete` y **`state_incr` atómico**. La
  base: `std/kv` gana `incr(key, delta)` y `set_if(key, expected, new)` (CAS) en `StoreOps` —
  sobre el `SharedStore` el read-modify-write corre ENTERO en la fibra dueña del actor
  (asertado: N incrementos concurrentes suman exacto N). El MANUAL §15 gana la receta del
  patrón actor que la referencia prometía. `web` sube a 0.3.0.

- **La tupla (o cualquier literal) en cola tras un `if`/`while`/`match` de sentencia ya
  funciona** (M153, SPEC §5): en posición de sentencia una forma-con-bloque termina en su
  `}` — `if (c) { … }` seguido de `(a, b)` es el `if` como sentencia y la tupla como valor
  del bloque, no una llamada. Muere el `return` defensivo del gotcha §55. En posición de
  expresión nada cambia (`let x = if (c) { f } else { g }(1);` sigue siendo llamada), y un
  `?`/`.` colgando de una forma-de-sentencia da un error dirigido ("wrap the expression in
  parentheses or bind it with 'let'").

- **El puente IPC llega a Linux y al iPhone** (M152 parte 2): el webview GTK se crea con un
  user content manager (shim + handler `"ray"`; con WebKitGTK sin los símbolos — <2.22 — la
  ventana nace sin puente, jamás falla `ui.open`), y el shell iOS instala el mismo puente en
  cada conexión de escena (`window` = 0 ahí: el shell no conoce el handle). Mismo contrato en
  las tres plataformas.

- **Puente IPC JS→raylang en `std/ui`** (M152, raydesk §81.1): cada webview trae
  `window.ray.send(text)` — el texto llega como evento
  `kind == "message"` (`tag` = texto, `window` = handle de la ventana) por el mismo stream de
  `next_event()`/`events()`; con `eval_js` de vuelta, una app chica ya no necesita webserver.
  Solo strings (otros tipos se ignoran; NULs eliminados); la vía de bajo nivel es
  `window.webkit.messageHandlers.ray.postMessage`. En headless, `RAY_UI_MSG` inyecta un
  mensaje por ventana (batería de 3 motores byte-idéntica). Sin opcodes nuevos: cero cambios
  en VM/intérprete/transpilador.

- **`ui.app_menu(name, items)`** (M151, raydesk #10): el menú de **aplicación** de macOS deja
  de ser intocable — tus items van encima de Hide/Quit (con separador), el tag `"role:about"`
  instala el **About nativo** (panel estándar), y un `name` no vacío re-titula el menú (bajo
  `ray run` salía "ray"; el `.app` ya mostraba su nombre). En Linux — sin menú de app global —
  los items van como menú normal y todos emiten el evento `"menu"` con su tag. Headless: no-op
  `Ok` (paridad de 3 motores en la batería).

- **La firma iOS sobrevive a `ray bundle --ios`** (M151, raydesk #9): declara tu team una vez
  con `[ios] development_team = "…"` en el ray.toml y cada regeneración lo escribe en el
  `App.xcconfig`; sin declararlo, el bundle **preserva** el `DEVELOPMENT_TEAM`/`CODE_SIGN_STYLE`
  que Xcode dejó en el xcconfig anterior (antes cada bundle los borraba y había que reponerlos
  a mano). Además `ray_doc` con un **nombre de módulo a secas** (`"std/kv"` o `"kv"`) lista la
  superficie pública completa — tipos, funciones y métodos de trait con firma — en vez de
  responder "usa module.function".

- **El terminal ya no queda "en escalera"** cuando una app en modo crudo muere por señal:
  `term.raw` arma un handler de `SIGHUP`/`SIGINT`/`SIGTERM` (solo si tenían la disposición por
  defecto — `signals()` manda si tu programa las maneja) que restaura el termios y re-lanza la
  señal, en los tres motores. Era la causa del terminal envenenado tras `ray dev` con una TUI:
  el relanzamiento mata con SIGTERM, `atexit` no corre, y el hijo siguiente guardaba el termios
  crudo como su "original". `ray dev` y `ray test --watch` además reponen su baseline del
  terminal si un hijo murió dejándolo cambiado (cinturón para SIGKILL/crash), y las suites de
  dev/watch corren con stdin nulo (no tocan el terminal del desarrollador y sus asserts de
  mensajes son deterministas con o sin tty).

- El shell iOS de `ray bundle --ios` adopta el ciclo de vida **`UIScene`** (manifest en el
  Info.plist + `SceneDelegate` con el webview y `ray_start`): desaparece el aviso de
  deprecación "`UIScene` lifecycle will soon be required" y la app queda a prueba del assert
  futuro de Apple. Los eventos `lifecycle` (`background`/`foreground`) llegan igual; si iOS
  reconecta la escena, el webview nuevo recarga la última URL de `ui.open` sin re-arrancar el
  programa.

- **Ronda 2 del dogfood raydesk** (M150): `web.listen_on(build, listener)` +
  `webserver.serve_on[_limits]/serve_with_on` — el **split bind/serve**: bindear al puerto 0
  primero (`net.local_port` da el puerto SIN carrera close/re-bind; el backlog acepta desde el
  bind) y servir después — el patrón de apps de escritorio, aplicado a los ejemplos. `ray_doc`
  acepta **`path`** y resuelve símbolos del PROYECTO y de sus paquetes de `.ray-deps`
  (`web.listen` con firma y doc — el agente leía el fuente a mano). REFERENCE: sección del
  paquete `web` en §11 (faltaba entera) y `std/kv` completado (`get_string`/`set_string`).
  **Republicados `net@0.2.0` y `web@0.2.0`** en el registro público (repos espejo + índice):
  `ray add web` entrega por fin `static_embedded`, sesiones, gzip y `listen_on`.

- **iOS** (§80b fase 1, M149): `ray build --native --lib` emite una **librería estática** con
  la entrada C `ray_start()` (el programa corre en su hilo; el shell posee el proceso), y
  `ray bundle --ios` genera el **proyecto Xcode** completo — shell WKWebView en ObjC,
  staticlibs de dispositivo y simulador elegidos por SDK, Info.plist con local networking.
  El MISMO fuente de escritorio corre en el iPhone (`ui.open` entrega la URL al webview del
  shell; ciclo de vida por eventos `lifecycle`). Verificado en el simulador Y en un iPhone
  físico. Android queda diferido (IDEAS §80b).

- **Quick-wins del dogfood raydesk** (un LLM construyendo una app solo con el MCP): la **coma
  final** vale ahora también en llamadas y parámetros de `fn` (consistencia con arrays/structs;
  SPEC §6); `ray_doc` cubre **métodos de trait** de la stdlib (la superficie de `std/kv` era
  invisible); el choque de un tipo de usuario con un builtin genérico lo dice claro (`'Task' is
  a builtin type (Task<T>) and shadows your struct…`); REFERENCE corrige `std/kv` (estilo
  método) y añade `fs.mkdir`.

- **MCP: modo PROYECTO** (la limitación de un-solo-archivo desaparece): `ray_check`/`ray_run`/
  `ray_test` aceptan **`path`** (archivo o directorio) y corren con el proyecto real como
  contexto — módulos propios y `[dependencies]` resuelven como en `ray run`; un directorio
  corre la entrada por defecto / la suite entera. Las instructions y llms.txt enseñan el flujo
  de DESARROLLADOR (proyecto + índice público github.com/ray-language/ray-index + `ray add`),
  no el de snippets; el modo `code` queda para experimentos autocontenidos. Lo estructural
  restante, clasificado en IDEAS §81.

## 1.4.0 — 2026-08-30

- **`std/ui`: menús y diálogos de archivo** (M148, F3 de IDEAS §80): el menú ESTÁNDAR
  (App/Edit) se instala solo — arregla que ⌘C/⌘V/⌘X no funcionaran en los campos de texto del
  webview en macOS (los atajos viajan por el menú) y añade ⌘Q/⌘W. `ui.menu(title, items)`
  declara menús custom cuyos clicks llegan como eventos `"menu"` con su `tag` (`UiEvent` gana
  el campo `tag`); `ui.pick_file()`/`pick_folder()`/`save_file(sugerido)` abren los diálogos
  nativos modales (`Ok(None)` = canceló; en headless los conduce `RAY_UI_PICK`). Linux por el
  mismo dlopen (menubar por-ventana, click-only en v1; chooser nativo exige GTK ≥ 3.20).

- **`std/ui` en Linux** (M147d): backend **GTK3 + WebKitGTK por `dlopen`** en runtime (cero
  headers de build, cero crates — patrón ALSA): mismo contrato que macOS (hilo principal
  capturado, eventos, `closed` exactamente una vez, cierre por el WM incluido). Sin las libs o
  sin display, `ui.open` da `Err` claro en ≤5 s. Nota honesta: validado por compilación y
  headless en CI; la ventana GTK real queda pendiente de un desktop Linux.

- **`ray bundle`** (M147c): empaqueta la app de escritorio — build nativo `--release` con los
  assets embebidos → `.app` en macOS (Info.plist mínimo + icns generado con sips/iconutil +
  codesign ad-hoc best-effort) o directorio + `.desktop` en Linux (Exec absoluto). Avisa si el
  proyecto no embebe assets (un .app lanza con cwd=/). Sin firma/notarización en v1.

- **`std/embed`** (M147): **assets del proyecto** — `[native] embed = ["assets"]` en ray.toml
  define un espacio de nombres idéntico en los tres motores (`read`/`list`; claves con `/`
  relativas a la raíz, orden lexicográfico, sin `..` ni ocultos). En `ray run` se leen en vivo
  del disco; `ray build --native` los hornea en el binario (`--embed dirs` ad-hoc) → binario
  autocontenido que corre desde cualquier cwd. El framework web gana
  `app.static_embedded(prefix, dir)` (ETag de contenido + 304 + Range, byte-idéntico entre
  motores). `ray dev` vigila también los dirs de embed: un cambio en un asset recarga el
  navegador SIN reiniciar el programa (la lectura en dev es en vivo). Y cerrar la VENTANA de
  una app `std/ui` bajo `ray dev` cierra también el modo dev (el contrato de las TUI: salida
  limpia = el usuario cerró la app).

- **`std/ui`** (M146): **ventana + webview** — la primitiva de apps de escritorio (IDEAS §80
  F1): `open(title, url, w, h)` abre una ventana nativa con el webview del SISTEMA cargando la
  URL (el patrón: tu webserver embebido; el IPC JS↔raylang es el framework web), `eval_js`
  (fire-and-forget), eventos con la fibra aparcada (`next_event`/`next_event_timeout`/
  `events() -> Channel<UiEvent>`; v1: `closed`, exactamente uno por ventana), `close(h)` cierra.
  Sin `ui.run()`: el runtime captura el hilo principal solo. macOS (AppKit/WKWebView por objc a
  mano, cero crates); `RAY_UI_BACKEND=headless` para tests/CI en cualquier OS (`ray test` lo usa
  por defecto); `--without ui` lo excluye del nativo. Ejemplo: `examples/web/desktop_window/`.

- **Fix** (`ray build --native`): `--without audio` era rechazado con exit 64 — `audio` faltaba
  en la lista de subsistemas válidos desde M145 aunque el help y los docs lo anunciaban.

- **Docs**: patrón "app de escritorio sin ventana propia" (fase 0 de IDEAS §80) — puerto libre
  del SO + abrir el navegador cuando el servidor ya acepta + salir desde la UI; el framework web
  es el IPC. Sección nueva en el MANUAL (§14) y ejemplo ejecutable `examples/web/desktop/`
  (VM y nativo, verificado end-to-end).

- **Fix** (nativo, hallazgo de raycode): `term.raw` drena el hilo escritor de `print` (M96f)
  antes de tocar el termios — la salida encolada en modo cocido ya no se escribe dentro de la
  siguiente sesión cruda (`\n` sin `\r`: la "escalera" intermitente). Cubre también
  `capabilities` y `read_hidden` (mismo punto único).

- **Fix** (`std/term`/nativo, hallazgo de rallyx): `raw()` es ahora **reentrante** —
  `capabilities()` (o cualquier helper que use raw) dentro de una sesión `term.raw` ya no deja
  el terminal cocinado a mitad (teclas muertas). De paso destapó y arregló una inversión en el
  `io.read_timeout` del binario nativo (M107.2): tras un primer vencimiento, los siguientes
  jamás entregaban el dato.

- **`std/audio`** (M145): salida PCM al dispositivo — `open`/`write`/`drain` + `close(h)`,
  s16le entrelazado; `write` **aparca la fibra** cuando el dispositivo va lleno (la
  contrapresión es el pacing). Backends sin crates: AudioQueue (macOS) y ALSA por dlopen
  (Linux); `RAY_AUDIO_SINK=null` para tests/CI. `--without audio` lo excluye del nativo.

- **`std/image`** (M144): `decode_png` — PNG a **RGBA8** en raylang puro sobre `std/inflate`:
  tipos de color 0/2/3/4/6, profundidades 1/2/4/8/16, paleta + `tRNS`, los cinco filtros, CRC
  por chunk y tope anti-bomba; corrupto/truncado = `Err`. "Cargo un sprite.png" funciona.

- **Terminal gráfico** (`std/term`, M143): `size_px()`/`cell_px()` — el área del terminal y el
  tamaño de una celda en píxeles (`ws_xpixel`/`ws_ypixel`, que ya se leían y se descartaban;
  un terminal que reporta 0 es `None`) — y `capabilities()` (`truecolor`/`colors_256`/`sixel`/
  `kitty_graphics`: entorno + query DA1 respondida por el propio terminal, con parser puro
  `parse_device_attributes`). La base para gráficos sixel/kitty escalados al layout.

- **MCP orientado desde el primer mensaje**: el `initialize` lleva *instructions* (el "system
  prompt" del servidor) que dirigen al modelo a leer `raylang://llms.txt` y el nuevo resource
  `raylang://reference.md` (el catálogo completo de firmas) antes de asumir que una feature
  falta — la stdlib va embebida y una búsqueda de archivos no la encuentra.

- **`ray test --watch`** (M140+M141): el bucle de desarrollo aplicado al runner — re-corre
  **solo las suites cuyo grafo de imports alcanza el archivo cambiado** (ray.toml/borrados/
  desconocidos re-corren todo), con eventos de kernel, debounce y guardados idénticos ignorados;
  un cambio a mitad de corrida la corta y re-corre, pantalla limpia entre corridas en un
  terminal, `q` sale. `ray test` acepta varias suites explícitas (`ray test a.ray b.ray`).

- **`ray dev` por eventos de kernel** (M139): el watcher del modo desarrollo deja el sondeo de
  mtimes (~200 ms de escaneo continuo) y reutiliza la maquinaria de `fs.watch`
  (FSEvents/inotify): reinicio a decenas de ms del guardado y cero coste en reposo, con
  fallback a sondeo en builds `--without watch`. Debounce, confirmación por contenido,
  check-before-restart, socket-activation y live-reload quedan idénticos. Además: con el
  programa terminado, `q` (o Ctrl-D) sale de `ray dev` limpio — útil tras cerrar una TUI con su
  propia tecla — y la línea del live-reload aclara que solo aplica a apps que sirven HTML.
- **Extensión para Zed**: gramática tree-sitter oficial
  ([ray-language/tree-sitter-raylang](https://github.com/ray-language/tree-sitter-raylang),
  validada en CI contra los 284 `.ray` del repo) + extensión
  ([ray-language/zed-raylang](https://github.com/ray-language/zed-raylang)): highlighting,
  outline, indentación y el servidor `ray lsp` cableado; enviada a la galería oficial
  (zed-industries/extensions#7361).
- **El sitio, publicado** (IDEAS §74 fase 3): deploy automático a GitHub Pages en cada push a
  main (`pages.yml`: wasm + bundle del editor + generación con `site/site.ray` + verificación
  del sitio completo) → https://ray-language.github.io/raylang/.
- **Benchmarks en el sitio** (IDEAS §74): sección "Medido, no prometido" en la landing y
  página `bench.html` — la gráfica de barras se **genera de la tabla real** de
  `benchmarks/poly/README.md` vía `std/markdown` (una sola fuente de verdad: re-medir el
  banco regenera el sitio), con fecha/hardware del propio README y el banco completo
  renderizado con enlace a la metodología reproducible.
- **Playground con editor real y LSP embebido** (IDEAS §74 fase 1b): el editor pasa a
  CodeMirror 6 y el **LSP de raylang corre dentro del wasm** (el mismo despacho de `ray lsp`,
  exportado como `lsp(msg)`): diagnósticos en vivo, autocompletado con snippets, hover con
  tipos, signature help y formateo (`ray fmt`), en el navegador y sin servidor. `playground/build.sh` produce ahora el wasm y el
  bundle del editor (npm/esbuild; artefactos no versionados).
- **Fix**: la compilación a `wasm32` llevaba rota desde M100 v3/M124/M126/M131 (usos de
  `ray_runtime` sin guarda de target); stubs honestos en wasm para procesos, hasher
  incremental, `tls_peer_cert` y normalización Unicode.

- **El sitio del lenguaje, fase 1** (IDEAS §74): `site/`, un generador estático en raylang
  puro — las páginas se autoran con los templates nativos `.ray.html` y siguen el manual de
  marca (paleta océano, fuentes woff2 embebidas → 100% autocontenido, símbolo y Manta) — que
  produce la landing (hero con efecto de tipeo, sección de agentes LLM/MCP, playground WASM
  embebido, ecosistema de apps y librerías) y la SPEC renderizada, con salida determinista y
  byte-idéntica por ambos motores. Previsualización: servir la salida con cualquier servidor
  estático.

## 1.3.1 — 2026-08-26

- **Fix LSP**: abrir un fuente de una dependencia descargada (`.ray-deps/<pkg>/…`) ya no
  marca "module not found" en los imports de sus dependencias hermanas (M138): la caché
  plana del proyecto es raíz de módulos aunque el paquete declare su `entry` publicable.

## 1.3.0 — 2026-08-26

- **`ray upgrade`** (M137): autoactualización del toolchain — descarga la última release
  (o un tag explícito) y reemplaza los binarios instalados, verificando el binario nuevo
  antes de tocar nada. `--check` solo informa (exit 0 = al día, 1 = hay versión nueva).

## 1.2.0 — 2026-08-26

- **El índice oficial de paquetes es el default** (M136): `ray add web` funciona en un
  proyecto recién creado sin configurar nada. Precedencia: `RAY_INDEX` → `[registry] index`
  → `git+https://github.com/ray-language/ray-index@main`. Opt-out explícito con
  `index = ""` (o `RAY_INDEX` vacía); el índice solo se contacta al resolver deps por
  nombre — `ray run`/`build` con deps git/`path:` siguen sin tocar la red.
- **La extensión de VSCode está en el Marketplace**: `ext install ray-language.raylang`
  (resaltado + cliente LSP). Publicación automatizable con el workflow `vscode-publish`
  (manual o tags `vscode-v*`; Open VSX opcional vía `OVSX_PAT`).

## 1.1.0 — 2026-08-25

La primera versión desde la casa nueva: el ecosistema entero vive en
[github.com/ray-language](https://github.com/ray-language) — el toolchain, los seis paquetes
instalables (`ray add net|db|web|rpc|tz|cron` contra `ray-index`), la gramática y las 17 apps de
dogfood, todo público. Los ejes del periodo: un **tercer motor** (el binario nativo), un salto de
**rendimiento** medido arco por arco, la capa de aplicación (framework web, procesos del SO,
herramientas de desarrollo), el **dogfood de 14 apps** que produjo M115–M133 (fs de producción,
timeouts de red, identidad del peer, half-close, `exit`, Unicode, correo…), la superficie entera
en **inglés**, y el **manejador de paquetes validado contra GitHub real**.

### Añadido — un tercer motor

- **Compilación a binario nativo** (`ray build --native`, arco P2.b): **transpila el programa a Rust** y
  lo compila a un ejecutable de código máquina — modelo *dev = VM / deploy = nativo*. Byte-idéntico a la
  VM (verificado con un corpus de paridad) y **3–4× más rápido que ella en cargas de servicio, 28–57× en
  cómputo puro**. En el banco poliglota (29 jul 2026) **gana a node en 9 de los 10 programas de cómputo**,
  a Go en seis y a `rustc -O` en cuatro (empatando con ambos en otros dos), y arranca en 1,80 ms — el
  más rápido de la mesa; en tiempo × memoria queda **#1 o #2 en 11 de los 12 programas**. Cubre el lenguaje completo (genéricos,
  traits, `dyn`, tuplas, closures
  con captura mutable, iteradores) + `std/fs`, sockets TCP/UDP, TLS, SQLite, procesos, FFI y toda la
  concurrencia.
- **Concurrencia nativa sobre fibras M:N** (arco F, `docs/diseno-concurrencia-nativa.md`): scheduler de
  corrutinas de pila propia (`corosensei`) + reactor `kqueue`/`epoll`, **el default** desde F-cierre
  (`--without fibers` recupera el hilo-por-tarea). Cubre sockets, TLS, UDP, `spawn`, `select`,
  cancelación de hermanas y esperas con plazo. Frente al hilo-por-conexión: de ~265 KB a ~21 KB por
  conexión y 350× menos CPU en esperas ociosas.
- **Crates de producción bajo demanda**: TLS (`rustls`), criptografía (`ring`), SQLite (`rusqlite`) y la
  regex acelerada se enlazan **solo cuando el programa los usa** (proyecto Cargo generado sobre el crate
  compartido `crates/ray-runtime`, del que también depende la VM → paridad por construcción).
  `mimalloc`, `ahash`, las fibras y los procesos van **por defecto**.
- **Control del build nativo**: `-o`, `--release` (opt3+lto+target-cpu=native), `--fast` (aritmética
  envolvente), `--target` (cross-compilation, con `Cargo.lock` reproducible) y
  `--without crypto,tls,sqlite,regex,mimalloc,ahash,fibers,process` — o `[native] without = […]` en
  `ray.toml` como política estable del proyecto.

### Añadido — lenguaje y stdlib

- **El repo vive en la organización** — `raylang` se transfirió de `roberto-ayala/raylang` a
  **`ray-language/raylang`** (M136): todo el ecosistema — toolchain, paquetes espejados,
  `ray-index`, apps — bajo un mismo techo. GitHub redirige las URLs viejas de forma permanente;
  los enlaces del repo, del instalador (`install.sh`), de los espejos y de las apps quedaron
  saneados al nuevo owner.
- **El ecosistema instalable: los seis paquetes espejados** (M135b) — `ray add
  net|db|web|rpc|tz|cron` funciona desde cualquier proyecto contra `ray-index`, con transitivas
  resolviendo solas (las path-deps entre hermanos se reescriben a git pinneado en el espejo).
  De paso: `cron` declara al fin su dependencia de `tz`, y el check de `ray registry publish`
  valida con una entrada sintética que IMPORTA la cara (la geometría real del consumidor —
  cazado porque el `fn send` privado de `db/postgres`, legal para todo consumidor, fallaba en
  falso como redefinición de builtin).
- **El piloto de espejos de paquetes** (M135) — decisión de forma del ecosistema: los paquetes
  viven en el monorepo (fuente de verdad, tándem con el lenguaje) y se PUBLICAN como espejos de
  solo-lectura en `github.com/ray-language` con tag inmutable por versión + entrada con hash en
  `ray-index` (`tools/publish-packages.sh`). Piloto: `rpc` — instalable con `ray add rpc` desde
  cualquier proyecto; e2e live en `deps_live_cli`.
- **El manejador de paquetes, validado contra GitHub real + la batería de admisión** (M134) —
  el flujo completo (dep `git+https@tag`, índice remoto, `ray add` por nombre con verificación
  de hash, lockfile reproducible) probado contra la organización `ray-language` (cápsula
  `greeting` + índice `ray-index`); harness vivo en `deps_live_cli` (nightly). De paso:
  **arreglado `ray registry publish`**, que validaba la cápsula con identidad de módulo
  equivocada y rechazaba imports internos calificados. Y el proceso de admisión de módulos queda
  ejecutable: `module_policy` en CI (doc `///` total, fila en REFERENCE, tests por módulo,
  README/manifest de paquetes), `CONTRIBUTING.md` y templates de PR (general + módulo nuevo).
- **El cliente gRPC, validado contra grpc-go real** (M133) — el dogfood pendiente de la única
  superficie de red sin peer real cazó dos bugs en la primera llamada: el decoder **HPACK ya
  acepta literales Huffman** (grpc-go comprime siempre sus cabeceras; la tabla del RFC 7541
  vivía en `std/huffman` sin puentear — vectores §C.4 como guarda) y una respuesta
  **trailers-only** (el error típico: UNIMPLEMENTED, sin DATA) ya devuelve su `grpc-status` en
  vez de romper como "frame too short". Arnés reproducible: servidor Go real como fixture
  (`cargo test --test grpc_real_cli -- --ignored`).
- **Los errores de la stdlib, en inglés** (M132) — el barrido final de la política de
  superficie-en-inglés: los ~110 `Result.Err` en español que quedaban en la stdlib embebida
  (json, toml, inflate, base64, hex, url, huffman, template, protobuf, time/ISO-8601, fs) pasan
  al inglés, alineados al estilo asentado ("unterminated string", "unexpected end of input",
  "gzip header truncated (unterminated FNAME)"…). De regalo: arreglado `exit(code)` en NATIVO,
  que bajo carga podía perder la salida pendiente de `print` (flusheaba el buffer equivocado —
  ahora drena el hilo escritor antes de terminar el proceso).
- **El lote D del dogfood** (M131) — **normalización Unicode en `std/text`**: `nfc`/`nfd`/
  `nfkc`/`nfkd` (crate `unicode-normalization` vía ray-runtime; VM por defecto, nativo por USO,
  slim = error claro) — el slug accent-insensitive por fin se escribe (NFD + descartar
  combinantes); y **`net/mail`**: las codificaciones de correo en raylang puro — `encoded_word`
  (RFC 2047), `header` (plegado a 78, RFC 5322), `base64_body` (76 columnas, RFC 2045),
  `dot_stuff` (RFC 5321) y `address` (mailbox con display-name).
- **El lote C del dogfood** (M130) — dos primitivos de motor en los TRES engines:
  **`net.shutdown_write(h)`** (half-close `shutdown(SHUT_WR)`: el peer ve EOF y este lado sigue
  leyendo — el idiom netcat/HTTP-1.0; solo TCP, errores estables byte-idénticos) y
  **`exit(code)`** (termina el proceso desde CUALQUIER fibra, flusheando salida; diverge como
  `panic` — `Option.None => exit(3)` cuadra de tipo — y `try_call` no lo captura).
- **El lote B del dogfood** (M129) — `admit(b)`/`report(b, ok)` en `std/resilience` (las
  transiciones del breaker sueltas, componibles con actores; `guard` queda como azúcar; los campos
  `abierto_hasta`/`hasta` pasan a `open_until`/`until`); **`ray test` aísla de verdad**: al
  terminar cada `@test` se cierran TODOS los handles del SO que dejó vivos — los LISTENERS
  incluidos, que antes sobrevivían y envenenaban el siguiente test (los procesos hijos no se
  matan); y **`webserver.gzip(req, resp)`** + `accepts_gzip(req)`: negociación de
  `Accept-Encoding` explícita por handler (umbral 512 octetos, `q=0` respetado, `Vary` correcto,
  streaming y `Content-Encoding` previos intactos).
- **El lote de menores de std** (M128) — cuatro cierres de dogfood en raylang puro:
  `std/regex` con **grupos con nombre** (`(?P<n>…)`/`(?<n>…)` + `group_names` +
  `captures_map(re, s) -> Option<Map<string, string>>`; la vía acelerada por crate sigue intacta);
  `std/csv` **incremental** (`parser`/`feed`/`finish` — los trozos pueden cortar por medio de un
  campo, de un `""` o de un CRLF; `parse_csv` reescrito encima); `std/toml` con **arreglos de
  tablas `[[ruta]]`** (aplanados como `ruta.N.clave` + `toml_array_len`); y
  **`jwt_verify_claims(secret, token, now_ms)`** en `packages/net` (firma + `exp`/`nbf` en una
  llamada; errores estables `"token expired"`/`"token not yet valid"`). De pasada, los errores de
  regex/csv pasan a inglés (`"regex: missing ')'"`…).
- **Pool de conexiones en `packages/rpc`** (M127) — `pool(host, port, size)` +
  `pool_call[_deadline/_full]` + `pool_close`: hasta `size` llamadas EN VUELO a la vez (una
  conexión por hueco = paralelismo real del lado servidor), checkout que aparca al agotarse
  (backpressure por canal acotado) y **reconexión automática** tras un timeout — el "disconnect
  y reconecta" manual desaparece.
- **Hasher incremental en `std/crypto`** (M126) — `sha256_init`/`sha512_init` + `hash_update` +
  `hash_final`: hashear un archivo grande POR TROZOS sin cargarlo entero, con digest idéntico al
  de una pasada. El patrón que tres apps copiaban a mano (cada una con su encadenado casero
  incompatible); el estado vive en el runtime compartido → byte-idéntico en los tres motores.
- **`term.read_hidden(prompt)`** (M125) — una línea SIN eco para passphrases: prompt a stderr
  (como getpass(3)), Backspace borra un carácter UTF-8 COMPLETO (la versión artesanal que las apps
  copiaban rompía una "ñ" a medias), Ctrl-C cancela con `Err("interrupted")`. Con su núcleo PURO
  `hidden_feed` (la filosofía de `term.decode`): probeable sin tty y byte-idéntico en los tres
  motores.
- **`net.tls_peer_cert(h)`** (M124) — el certificado del peer de una conexión TLS como
  `PeerCert { subject, issuer, not_before_ms, not_after_ms, san }`: **"expira en N días"** — EL
  check de TLS que todo operador quiere — deja de ser inexpresable
  (`(cert.not_after_ms - time.now()) / 86400000`). Conduce el handshake pendiente si hace falta
  (acotado a 10 s); el parseo X.509 (`x509-parser`, dependencia nueva anotada en SECURITY.md) es el
  mismo código en los tres motores → resumen byte-idéntico.
- **`Request.remote` + `net.peer_addr`** (M123) — el webserver expone por fin la dirección remota
  del cliente (`req.remote` = `"ip:puerto"`, `webserver.remote_ip(req)` sin el puerto, consciente
  de IPv6): rate-limit por IP, `X-Forwarded-For` y logs de acceso con origen dejan de ser
  inexpresables. Debajo, `net.peer_addr(h)` da la dirección del peer de cualquier conexión TCP o
  TLS en los tres motores.
- **`net.tcp_connect_timeout(host, port, ms)`** (M122) — el connect con plazo: un host que descarta
  los SYN (firewall, ruta negra) retenía el connect ~75 s (el timeout del SO); ahora el intento
  vencido falla con el error estable `"connect timeout"`. Espera acotada pero bloqueante, en los
  tres motores.
- **Timeout de lectura UDP** (M121) — `net.set_read_timeout(h, ms)` aplica ahora también a los
  sockets UDP en los tres motores: un `udp.recv_from` que espere más del plazo falla con el error
  estable `"read timeout"` en vez de esperar para siempre (UDP no retransmite: un datagrama
  perdido colgaba la fibra sin remedio). `net/dns` acota su espera a 5 s — una consulta perdida
  responde `Err("recv: read timeout")`, no un cuelgue del monitor. De paso, docs al día: la nota
  "UDP bloquea todas las fibras" estaba rancia (la VM cede desde M20.11 y el nativo-con-fibras
  desde F4; el hueco real era solo el timeout).
- **Harness diferencial de motores** (M120) — programas raylang **generados** (interacciones de
  features que los ejemplos no ejercitan: builtin×tipo, mutación-en-constructor, return-en-closure,
  valores cruzando fibras…) corren en intérprete, VM y binario nativo y deben producir exactamente
  el mismo stdout+exit; bisección automática al divergir, semillas reproducibles. Su primera
  corrida cazó y dejó corregidos **tres bugs del backend nativo**: `print` de `u8`/`u32`/`u64` no
  compilaba, los **genéricos acotados** (`fn largest<T: Ord>`) no compilaban (el diccionario
  `int#less` no se emitía), y llamar un método de un `dyn Trait` **como argumento**
  (`print(x.tag())`) fallaba el build. Corre en cada `cargo test` (humo), en cada push de CI
  (campaña) y en la nocturna con presupuesto alto.

- **Literales enteros en hex/octal/binario y escapes por code point** (M118, IDEAS §67, §71.6) —
  `0xFF`, `0o755`, `0b1010` (y en mayúsculas) para escribir máscaras de bits y permisos como lo que
  son (`fs.chmod("vault.db", 0o600)`, no `384`); y en cadenas/caracteres, `\0` (NUL), `\xNN` (octeto
  hex) y `\u{H…H}` (code point Unicode: `"\u{1F680}"` → 🚀, `'\u{00E9}'` → é). **`ray fmt` conserva
  la base** en que se escribió el literal en vez de canonizarlo a decimal — la base carga intención.
- **`std/term`: ancho en celdas de terminal** (M117, IDEAS §67) — la pieza que todo TUI copiaba a
  mano: `term.width(s)` da el ancho real en **celdas** (un `日` ocupa 2, un combinante 0), donde
  `s.len()` (cuenta caracteres) desalinea las columnas; `term.char_width(c)` para un carácter, y
  `term.fit(s, cells)`/`term.fit_right(s, cells)` truncan sin partir un carácter ancho y rellenan a
  la anchura exacta. wcwidth pragmático (control/combinantes → 0, CJK/kana/fullwidth/emoji → 2,
  resto → 1); raylang puro → byte-idéntico en los tres motores, y portable (no necesita tty).

- **Concurrencia: `select_timeout` — `select` con plazo** (M116.1, IDEAS §64) — `select_timeout(chs,
  ms)` devuelve `Some(i)` con el canal listo o `None` al vencer los `ms` milisegundos; `ms = 0` es
  un poll no bloqueante del conjunto. Es **event-driven** (despierta al llegar un canal, no sondea)
  y cierra el hueco "espera datos O una orden de control **con un límite de tiempo**": reemplaza el
  rodeo de un timer que enviaba a un canal solo para poder salir del `select`. Byte-idéntico entre
  VM y nativo (el scheduler M:N despierta el select aparcado por canal-listo **o** por deadline).

- **Concurrencia: `try_recv` — recepción de canal sin bloquear** (M116, IDEAS §64) — la pieza que
  tres apps rodearon con fibras lectoras + timers que enviaban bytes vacíos. `try_recv(ch)`
  devuelve el enum del prelude `Received<T>` (`Got(v)` / `Empty` / `Closed`), distinguiendo los
  tres estados que `recv` (que bloquea) colapsa a `Option`. Permite "haz trabajo O atiende una
  orden de control sin quedarte esperando" en una sola fibra. Solo VM/nativo (como el resto de la
  concurrencia); byte-idéntico entre ambos, con la misma conversión de valor que `recv`.

- **`std/fs`: watch de filesystem por eventos de kernel** (M115.4, IDEAS §69) — la pieza que
  CINCO apps reimplementaban sondeando mtimes (ray dev, raycode-dev, raylogs `--follow`, raysync
  `--watch`, raysite serve). `fs.watch(path)` (directorio → recursivo) + `fs.next_event(h)` — la
  fibra **aparca** hasta el cambio: el proceso duerme de verdad, no sondea — y
  `fs.next_event_timeout(h, ms)` para agrupar ráfagas. Detrás: FSEvents en macOS / inotify en
  Linux (crate `notify` en ray-runtime, feature `watch` por defecto; `--without watch` /
  build slim → error claro). Tres motores byte-idénticos; `close(h)` detiene el watch.

- **`std/fs`: metadatos y permisos** (M115.3, IDEAS §69/§71) — `fs.stat(path)` devuelve
  `Stat { kind, mode, size, mtime_ms }` **sin seguir symlinks** (lstat): un symlink por fin se
  puede DETECTAR (`kind "symlink"`) en vez de seguirse a ciegas — lo que un sync/backup fiel
  necesita; los helpers totales (`is_dir`/`is_file`/`mtime`) siguen resolviendo como siempre. Y
  `fs.chmod(path, mode)` cambia los bits de permiso (384 = 0o600): una bóveda de secretos ya
  puede restringirse a su dueño. Tres motores byte-idénticos.

- **`std/fs`: candados consultivos de archivo** (M115.2, IDEAS §66) — `fs.try_lock(h)` (candado
  EXCLUSIVO sin bloquear, flock; `Ok(true)` = adquirido, `Ok(false)` = lo tiene otro) y
  `fs.unlock(h)` (`close` también lo suelta). El patrón LOCK-file del proceso único: dos brokers
  sobre el mismo directorio ya no se doble-entregan en silencio. El lock bloqueante queda fuera a
  propósito (congelaría todas las fibras del proceso). Tres motores byte-idénticos.

- **`std/fs`: escritura binaria sobre handle + fsync** (M115.1, IDEAS §66/§68) — las dos piezas que
  el dogfood señaló como techo del eje almacenamiento. `fs.write_bytes(h, data)` es el gemelo
  binario de `fs.write` (octetos crudos en la posición actual; compone con `seek`; desbloquea WAL/
  AOF/formatos binarios en disco, que solo podían escribirse por ruta con `append_file_bytes`). Y
  `fs.sync(h)` fuerza lo escrito a **almacenamiento estable** (fsync): un append al page cache
  sobrevive al crash del proceso pero no a un corte de luz — con `sync` un programa raylang puede
  por fin **prometer durabilidad** real. Tres motores byte-idénticos; receta WAL en MANUAL.

- **`std/crypto`: acuerdo de claves X25519 + HKDF** (M114, IDEAS §62) — la pieza que faltaba para
  cifrar entre pares. Había **identidad** (Ed25519) y **cifrado** (ChaCha20-Poly1305), pero no había
  con qué unirlos: sin acuerdo de claves solo se podía cifrar con claves precompartidas fuera de
  banda. Ahora `crypto.x25519_public_key(secret)` y `crypto.x25519_shared_secret(secret, peer_public)`
  dan el secreto común sobre un canal público, `crypto.hkdf_sha256(salt, ikm, info, len)` (RFC 5869) lo
  convierte en claves usables —y las **separa** por `info`, que es como se tiene una clave por sentido
  y ningún nonce repetido— y `crypto.constant_time_eq(a, b)` compara secretos sin filtrar por
  temporización. Las privadas son 32 octetos cualesquiera (`random_bytes(32)`) y se pueden
  **persistir**: la identidad de un nodo sobrevive al reinicio. `x25519_shared_secret` devuelve `None`
  ante una clave pública de **orden pequeño** (forzaría un secreto todo-ceros que el atacante conoce).
  Detrás va `x25519-dalek` y no `ring`, cuya API solo entrega claves efímeras (ver `SECURITY.md`).
  Vectores de RFC 7748 §6.1 y RFC 5869 A.1/A.3 clavados en los tres motores; la receta completa de
  canal seguro —firmar la efímera, clave por sentido, nonce contador— en `MANUAL.md` §13 y en
  `examples/stdlib/key_agreement.ray`.

- **`std/process`: stdin escribible sobre un hijo VIVO** (M100 v3, IDEAS §53.10) — lo que faltaba
  para una **sesión persistente** por stdio (cliente MCP/LSP, driver de REPL): hasta ahora
  `.stdin(bytes)` escribía y CERRABA en el spawn, y `Proc` solo exponía `out`/`err`, así que
  "petición → respuesta → petición" era imposible. Ahora `.stdin_pipe()` deja el pipe abierto y
  `Proc.write(bytes) -> Result<int, string>` / `Proc.close_stdin()` lo alimentan mientras el hijo
  vive. `write` coloca TODO el dato y **aparca la fibra** si el pipe se llena (contrapresión real,
  reusando el aparcado por interés de escritura que ya tenían los sockets); si el hijo cerró su
  stdin o murió, devuelve **`Err`** (EPIPE visible — el error que un cliente de sesión necesita,
  y la razón de elegir métodos con `Result` en vez de un canal, que se lo tragaría);
  `close_stdin` ES el EOF que el hijo espera. En la VM y en el binario nativo (fibras e
  hilo-por-tarea), byte-idénticos.


- **`std/fs`: lectura por trozos + `seek`** (M113): `fs.read_bytes(h, max) ->
  Result<Option<bytes>, string>` lee hasta `max` octetos del handle desde su posición actual
  (trozos **exactos** salvo cerca del final; `None` = EOF; la memoria queda acotada por lo
  leído — la primitiva para transferir/trocear archivos grandes sin cargarlos enteros, que
  antes obligaba a lanzar un `cat` externo vía `std/process`) y `fs.seek(h, pos) ->
  Result<int, string>` mueve la posición absoluta (con `read_bytes` da transferencias
  **reanudables**). En los tres motores, byte-idénticos.

- **`std/markdown`: diagramas Mermaid** (M111.c): una cerca ` ```mermaid ` emite el contenedor
  que `mermaid.js` busca en la página (`<pre class="mermaid">`, texto escapado) en vez de
  `<pre><code class="language-mermaid">` — el render del diagrama es client-side, como en todos
  los generadores; en el AST sigue siendo `Code("mermaid", …)` para quien renderice distinto.

- **`std/markdown`: tablas GFM** (M111.b): cabecera + fila separadora (`|---|:--:|`) →
  `Block.Table(aligns, header, rows)` y `<table>` con `align`; `\|` escapado, filas cortas se
  rellenan, cabecera y separadora con distinto número de columnas no es tabla (párrafo intacto).

- **`std/markdown`** (M111): parser de Markdown en raylang puro — `parse(md) -> [Block]` (AST
  tipado: encabezados, párrafos, código cercado con lenguaje, listas anidadas, citas, regla;
  inline: énfasis/negrita/código/enlaces/imágenes/escapes) y `to_html(md)`. Subconjunto CommonMark
  pragmático con dos decisiones de seguridad: el HTML embebido se **escapa** (no se interpreta) y
  las URLs `javascript:`/`vbscript:`/`data:` no-imagen se neutralizan — la salida se puede servir
  sin sanitizador. Determinista: golden byte-idéntico en los tres motores.

- **Streaming del webserver + Range/206 en estáticos** (M110, el gemelo servidor del streaming de
  M108): `webserver.stream_response(status, ch)` — el cuerpo son los trozos que lleguen por un
  `Channel<bytes>` (el patrón de actores: el handler `spawn`ea al productor y devuelve ya), escritos
  al cliente en chunked según llegan, con backpressure vía canal acotado; la conexión cierra tras
  el stream y un HEAD drena el canal para no colgar al productor. Y `static_mount` gana **HTTP
  Range**: `Accept-Ranges` en los 200, `206 Partial Content` con `Content-Range` (rango cerrado,
  abierto y sufijo), `416` con el tamaño total, multi-rango → 200 completo (RFC), `If-Range`
  honrado contra el ETag y el `304` ganando al Range. De paso, `status_text` aprende 206 y 416.

### Corregido (bloque siguiente)

- **Los 4 bugs de divergencia VM/nativo del dogfood de IDEAS-APPS** (IDEAS §§63–72) — la clase
  "código válido en la VM que no compila o falla en nativo", cerrada de una tanda:
  - **`sort([float])` no compilaba en nativo** (§63): `f64` no es `Ord` en Rust y el
    `__ray_sort<T: Ord>` genérico moría con E0277 en el `cargo build` del usuario. El caso float
    va ahora por un helper que replica el merge sort bottom-up del prelude comparando con `<` —
    byte-idéntico a la VM incluso con NaN.
  - **`return` dentro del literal de `spawn` no compilaba** (§68): el `return;` fijaba `()` como
    retorno del closure de hilo y chocaba con la cola de conversión Send (E0308); `return s;` de
    un string era la misma familia. El cuerpo se emite como closure inmediatamente invocado: el
    `return` del usuario retorna de esa frontera y la conversión Send se aplica al resultado.
  - **`E.V(b.campo, f(b))` panicaba con "RefCell already borrowed"** (§64): la clase
    RefCell-en-args alcanzaba a TODOS los literales compuestos (variante, struct, arreglo, tupla,
    map), no solo a las llamadas ya arregladas: con 2+ exprs cada valor se iza a un temporal y los
    guards mueren entre argumentos, mismo orden de evaluación que la VM.
  - **`close(h)` cross-fibra con un lector aparcado era un no-op** (§64): ni FIN al peer ni
    despertar (el lector re-aparcaba para siempre). `__ray_close` hace ahora `shutdown(Both)` del
    stream TCP (el FIN llega y el fd despierta al lector vía el reactor) y el bucle de lectura
    re-verifica el registro al despertar y en EOF → `Err("invalid handle: h")`, byte-idéntico a la
    VM; el camino caliente con datos no paga ningún lock nuevo. Los 4 repros de rayrelay quedan en
    paridad.

- **Nativo: `http.stream_read` panicaba con "RefCell already borrowed"** — y con él toda la clase
  `f(s, s.campo)` donde `f` muta `s` (la destapó `stream_take(s, s.remaining)` del cliente HTTP
  en raycode; era LO ÚNICO que impedía usar el binario nativo allí). En Rust, el guard del
  `borrow()` de un argumento vive hasta el final de la sentencia — es decir, durante la llamada —
  y el `borrow_mut` del callee revienta. Arreglo estructural: **todos los argumentos de una
  llamada de usuario se izan a temporales** (cada `let` cierra sus guards; mismo orden de
  evaluación) en las DOS rutas de emisión (mismo módulo y calificada `mod::fn`), y el clon de un
  campo-closure/vtable-dyn también se iza antes de llamar. E2e nativo de streaming como guarda.

- **Nativo: un `match` sobre un método de trait con brazos compuestos se emitía como stub** —
  `let status = match (p.wait()) { Exit.Code(c) => "exit ${c}", … }` caía a "could not infer the
  type of the match" y la función entera panicaba al llamarse (raycode lo esquivaba extrayendo el
  match a otra función). Dos raíces, dos arreglos en el `type_of` del transpilador: el tipo del
  escrutinio se **clasifica** (un retorno `Type::Struct` crudo del AST ahora se reconoce como el
  enum que es) y los **bindings del patrón entran en ámbito** al tipar el cuerpo del brazo (antes
  solo el cuerpo-identificador pelado resolvía). De paso, la llamada a un **campo-closure**
  (`b.f(x)`) también se tipa (espejo del branch de emisión que ya existía).


- **El LSP resuelve los imports de un `tests/*.ray` como `ray test`** (M113b): el editor marcaba
  "module 'fileread' not found" sobre un test de integración que corría en verde — `ray test`
  añadía a mano la raíz de la entrada (`src/`) como raíz extra del loader y el LSP no. La regla
  vive ahora en `deps::dependency_roots_for` (compartida por CLI y LSP): en un proyecto-aplicación
  con `ray.toml`, el directorio de la entrada entra como raíz de **respaldo** — el editor y
  `ray run tests/x.ray` resuelven igual que `ray test`.


- **`std/markdown`: el `_` intra-palabra ya no crea énfasis** (regla 17 de CommonMark):
  `snake_case_name` en prosa quedaba mutilado a `snake<em>case</em>name` — especialmente dañino
  para agentes de código, que escriben identificadores en prosa constantemente. Un delimitador
  de `_`/`__` ahora solo abre si lo anterior no es "de palabra" y solo cierra si lo siguiente
  tampoco lo es (`intra__word__no` va literal; `__init__` sigue siendo negrita; `*` conserva el
  énfasis intra-palabra, como CommonMark).

- **`std/markdown`: la lista ordenada conserva su número de inicio**: `2. dos` emitía `<ol>`
  (se renderizaba como 1). El AST gana el campo — `Block.List(ordered, start, items)` (cambio
  de firma; `start` es el número del PRIMER marcador, los demás se ignoran como en CommonMark) —
  y el render emite `<ol start="2">` cuando no empieza en 1.


- **`ray fmt` borraba código en `"x ${n}" + " tail"`** (la misma familia de colisión de posición
  que el ConcatN de abajo, ahora en el formateador): el `+` exterior hereda la (línea, col) de la
  interpolación y el resurfacing del azúcar lo tomaba por la raíz de la cadena — imprimía solo la
  interpolación y **el resto de la expresión desaparecía**; con paréntesis (que RE-posicionan el
  nodo al `(`) el azúcar se duplicaba, y un pipeline parentizado (`(a |> to_string) + "!"`) se
  corrompía a `(a |> to_string)(a)`. Arreglo estructural: la tabla de interpolaciones se clava a
  la **hoja izquierda** del spine de `+` (inmune a los paréntesis) y se verifica por **conteo de
  piezas**; el pipeline verifica que el receptor guardado sea el primer argumento del `Call`. El
  corpus adversarial de `tests/fmt_policy.rs` (que asevera AST idéntico y comentarios intactos)
  gana los casos de toda la familia.

- **`("a" + "b").len() + 3` reventaba la VM** ("the checker guarantees strings"): los paréntesis
  son transparentes en posición, así que un `+` exterior no-string heredaba la (línea, col) del
  `+` de strings interior registrado para la superinstrucción `ConcatN` (V2) y `lower_concat`
  aplanaba la cadena equivocada. El sitio colisionado se des-registra (corrección antes que
  optimización: la cadena interior queda como `Add` normal). Lo destapó el framing chunked del
  streaming del webserver.

- **Streaming del cliente HTTP + cliente SSE** (M108, la otra mitad de "pintar mientras llegan
  tokens"): `net/http.stream[_with]` devuelve el status y las cabeceras en cuanto llegan y
  `stream_read` entrega el cuerpo **a trozos según llegan** (des-chunkeado incremental, plazo de
  ocio por lectura, truncado = error, fin limpio = `Ok(None)`; la petición no anuncia gzip — no
  hay gunzip incremental). Y `net/sse`: cliente Server-Sent Events sobre ese stream, con el
  decodificador **puro** `sse.decode` (bytes → evento, mismo patrón que `term.decode`): data
  multilínea, comentarios keep-alive, finales CR/CRLF/LF, y trozos que parten un evento — incluso
  un carácter UTF-8 — por cualquier octeto. El test de incrementalidad va por handshake: el
  servidor retiene el final del cuerpo hasta ver el primer trozo impreso por el cliente.

- **`signals()` entrega también SIGWINCH (28)** (M107.4, cierre del arco de terminal): con
  `select` sobre `signals()` + `term.size()`, una TUI se re-maqueta al redimensionar la ventana.
  El 28 coincide en macOS/BSD y Linux; VM y binario nativo (de paso se corrige la doc rancia que
  decía "solo VM": el nativo tiene el self-pipe desde M88.1).

- **`std/term` — el terminal** (M107.3): `is_tty(fd)`, `size() -> Option<(int, int)>`,
  `raw(f)` (modo crudo con restauración garantizada: al salir de `f` aunque falle, y al salir el
  proceso vía `atexit`), `read_key() -> Option<Key>` y el decodificador **puro**
  `decode(bytes) -> Option<(Key, int)>` — flechas, Home/End/PageUp/PageDown/Insert/Delete,
  F1..F12 (CSI y SS3), Ctrl+letra, Shift-Tab y UTF-8 multibyte, con los prefijos incompletos
  señalados para resolver un ESC suelto por plazo. Sin crates: termios como buffer opaco +
  `cfmakeraw(3)`/`ioctl(TIOCGWINSZ)`/`isatty(3)` declarados a mano, en la VM y en el binario
  nativo. Demo: `examples/term/keys.ray`.

- **`std/io` — lectura de stdin por bytes, que aparca la fibra** (M107.2): `io.read(max) ->
  Option<bytes>` (`None` = EOF) e `io.read_timeout(max, ms) -> ReadResult`
  (`Data`/`Eof`/`TimedOut`). En la VM, una lectura sin datos **aparca la fibra en el poller**
  (patrón de los sockets, sin tocar los flags del fd: `poll(2)` responde "¿hay algo ya?" y solo
  entonces se lee) — un programa puede animar/servir mientras espera teclas. En el nativo con
  fibras, por el reactor (`wait_readable`); sin fibras, lectura bloqueante en su hilo. El plazo
  reusa la maquinaria de deadlines de los sockets (M56.4) con el pseudo-handle 0 de stdin.

- **`std/io` — escritura sin salto de línea + flush** (M107.1, primera pieza del arco de terminal):
  `io.write(s)` / `io.ewrite(s)` / `io.write_bytes(b)` → `Result<int, string>` y `io.flush()`.
  Cubre prompts, barras de progreso y secuencias de escape (antes: abrir `/dev/stdout` en append
  por cada escritura). `write_bytes` entrega los octetos intactos, sin pasar por UTF-8. En el
  binario nativo, los writes van por el mismo canal que el `print` asíncrono (M96f) → el orden
  entre `print` e `io.write` es el de programa en los tres motores, verificado byte a byte.

- **Constructores de duración en `std/time`**: `millis seconds minutes hours days` convierten a la
  moneda de duración de la stdlib (int en **ms**) y, importados sin calificar, se leen en UFCS:
  `sleep(30.seconds())`, `2.hours() + 30.minutes()`. Sin sintaxis nueva: funciones ordinarias.
- **`std/units`**: constructores de tamaño a **bytes** en convención binaria (1 KB = 1024) — `kb mb
  gb`, la misma lectura UFCS: `64.kb()`, `16.mb()`.
- **FFI `blocking`**: `extern "lib" blocking { … }` marca llamadas C **bloqueantes de verdad** (E/S,
  C-libs lentas). En el binario nativo con fibras (el default) la llamada se descarga a un pool
  bloqueante y la fibra espera aparcada — el worker M:N no se vara; mismos tipos y valores en todos
  los motores (donde no hay scheduler que proteger, la marca es inerte). `blocking` es contextual
  (sigue valiendo como identificador).
- **FFI: aridad 0..=6 con paridad entre motores**: el catálogo de firmas de la VM se genera por macro
  hasta `MAX_ARITY` = 6 (antes 0..=3 a mano; cubre `mmap`/`sendto`/`recvfrom`) y el checker rechaza en
  compilación cualquier `extern fn` que lo exceda — antes un extern de 4+ argumentos compilaba en el
  binario nativo y fallaba en runtime solo en la VM.
- **FFI: pila de fibra dimensionada para C**: en el binario nativo con fibras, un programa con externs
  fija solo un default de **1 MiB** de pila por fibra (reserva virtual; antes 128 KiB, que el código C
  podía desbordar con SIGSEGV mudo). `RAY_FIBER_STACK_KIB` siempre gana al default.
- **`std/ffi` con `errno()`**: el `errno` del hilo tras una extern C estilo POSIX (`fopen` fallido →
  `ffi.errno()` = ENOENT), en los tres motores. Regla: leerlo inmediatamente tras la llamada. Con una
  extern `blocking`, el runtime nativo trae de vuelta el errno del hilo del pool — misma regla.
- **`try_call(f) -> Result<T, string>`** (M97): recuperación de un `panic`/error de ejecución **en la
  misma fibra**, el fallo como valor. En los tres motores. `try_join` hace lo propio con una tarea.
- **Cadenas plantilla con backticks** (M95): `` `…` `` es multilínea y admite `"` literal, con la misma
  interpolación que cualquier cadena.
- **`std/process`** (M100): ejecución de procesos del SO **sin shell** (argv tipado). `run` para el caso
  simple, un builder (`dir`/`env`/`stdin`/`timeout_ms`/`max_output`/`merge_output`) y **streaming** por
  canales acotados con contrapresión. `Err` solo significa "no se pudo lanzar"; el plazo devuelve la
  salida **parcial** con `timed_out`. El hijo corre en su propio grupo de procesos y es **hijo de
  scope**: una hermana que falla lo mata y lo cosecha, y uno que nadie esperó no sobrevive al scope.
- **`std/kv`** (G.1/G.2): estado clave-valor persistente en raylang puro, con un `SharedStore` por actor
  CSP — sobrevive al hot reload.
- **`std/collections/dict`** (M82): mapas con claves de **usuario** vía `Hash` + `Eq`.
- **`std/resilience`** (M88.2): reintentos con backoff y jitter, *circuit breaker* y plazos.
- **Señales del SO** (M88.1): `signals() -> Channel<int>` para apagado ordenado, y `serve_graceful` en el
  servidor web.
- **Stack trace de errores de ejecución** (M79) con la posición del llamador, en ambos motores.
- **Más superficie**: literales float con exponente (M80), `@derive(ToJson)` (M93.5), `monotonic_nanos`,
  `random.seed/between/choice/shuffle`, `crypto.random_bytes`, UUID v7, directorios y metadatos en
  `std/fs`, escapes `\uXXXX` en `std/json`, `find`/`chain`/`min`/`max` en iteradores, `clamp` genérica y
  trigonometría inversa en `std/math`.
- **Regex (M81/M59.2)**: Pike VM con grupos de captura, `{n,m}` y cuantificadores perezosos;
  `compile -> Result` y el trait `Matcher`.

### Añadido — ecosistema y aplicaciones

- **Tests a nivel proyecto** (M101): `ray test` ahora pasa por el loader — las `@test` pueden vivir
  en **cualquier módulo** (corren calificadas: `math.suma_ok`) y usar `import`; cada `tests/*.ray`
  junto al `ray.toml` corre como **suite de integración** que importa los módulos del proyecto. Un
  fallo reporta su **ubicación** (`at módulo:línea:col`, apuntando al assert del usuario) y cada
  prueba su duración. El código de salida pasa a **0/1** (antes era el número de fallos: 256 fallos
  daban exit 0 — un falso verde en CI); 65 si alguna suite no compila. `ray test <filtro>` filtra
  sin necesidad de dar el archivo.
- **Paquete `web`** (M93): framework de aplicación estilo Express sobre el servidor HTTP/1.1 —
  enrutado (parámetros, catch-all, `mount`, rutas regex, 405 + `Allow`), middleware componible, contexto
  (`header_of`/`cookie_of`/`form`/`json_body`), presets de CORS, `Cache-Control`, trace-id en el log y
  respuestas JSON tipadas vía `ToJson`. Compila también a binario nativo.
- **Servidor web de producción** (M56): límites anti-DoS, timeouts de lectura (anti-slowloris),
  keep-alive, HTTPS de servidor, `chunked` entrante, HEAD, estáticos con saneo, `static_mount` y caché
  ETag/304, varias cookies por respuesta y apagado ordenado.
- **Templates compilados `.ray.html`** (M55): `{% %}` con composición (`import`/`include`/`extends`/
  `block`/`let`), formateador propio y soporte completo de editor (completion, hover, references,
  rename, outline). `run`/`build`/`test` regeneran solos los desactualizados.
- **`ray dev`** (M92): modo desarrollo con watcher, *check-before-restart* (un error a medio escribir no
  tira el servidor que funciona), debounce, confirmación por contenido, retención del socket entre
  reinicios y **live-reload** del navegador por SSE.
- **`ray mcp`** (IDEAS §51): servidor MCP con las tools `check`/`run`/`test`/`fmt`/`doc` para agentes
  LLM, ejecutando el código confinado (fuel + heap + plazo). Junto a `llms.txt`, el contexto destilado
  del lenguaje.
- **Registro de paquetes multi-publicador** (M83/M84/M90.1): dueños de nombre y firmas Ed25519
  (`ray registry publish --sign`, `keygen`, `verify`), UI web estática generada **en raylang**, mirrors
  (`[registry] mirror`/`RAY_MIRROR`) y `ray remove`/`ray search`.
- **Clientes de base de datos** (`packages/db`, M53–M54, M76–M77): PostgreSQL (protocolo extendido,
  TLS), MySQL (prepared statements binarios, TLS, caching_sha2), SQLite (sobre `rusqlite`) y MongoDB
  (OP_MSG, SCRAM, BSON en raylang puro, cursores).
- **Más red**: RPC raylang↔raylang (`packages/rpc`, M88.4), tracing distribuido W3C (M88.3), keep-alive
  del cliente HTTP (M90.2), `tls_upgrade` (STARTTLS de cliente), cliente SNTP (M90.7), HTTP/2 con flow
  control y `grpc-status` (M58.3), lector de tramas WebSocket robusto (M58.1).
- **Tiempo local y planificación**: `packages/tz` (IANA/TZif, incluido el footer de reglas DST
  perpetuas) y `packages/cron` (expresiones cron y timers, UTC y hora local).
- **Builds a medida**: features `sqlite`, `net-tls` y `ffi` (activas por defecto) → un build *slim*
  ocupa un 53% menos y no puede cargar código nativo; PGO del binario de release (`tools/pgo.sh`);
  `Makefile` con todos los comandos del proyecto.

### Rendimiento

Cada cifra está medida y contada en [`PERFORMANCE.md`](PERFORMANCE.md).

- **`time.sleep` preciso** (M119) — `sleep(33)` dormía ~37 ms (overshoot de 4–6 ms → pacing de ~25 fps
  en vez de 30) porque los tres motores acababan en `std::thread::sleep`, cuyo `nanosleep` se pasa en
  macOS por *timer coalescing*. Ahora se duerme con `poll(2)` de cero descriptores (la misma espera
  precisa del kernel que usa `read_timeout`): ~34 ms en VM, intérprete y nativo, ~1 ms de overshoot,
  sin coste de CPU. Para pacing sin deriva sobre muchos frames sigue conviniendo un reloj absoluto.

- **VM (arcos P0/A/D/V/MM/TA)**: `Map` sin alocar en el camino caliente (aHash, `get_or`, `add_to`),
  allocador `mimalloc`, superinstrucciones guiadas por histograma (−19 a −28% en todo el banco), PGO
  (−5 a −9%), opcode `ConcatN` (jsonserialize −27%), fusión del envoltorio `Option` (jsondeserialize
  −52/−59%), fast-paths ASCII, `s[i]` sin materializar los chars (~33× en bucles), fast-path flotante y
  fusión de indexado (matrixmul −35%), structs sin metadatos y `Slot` de 88→48 B (treealloc −15%),
  arreglos homogéneos de ints (−68% de RSS) y GC con umbral amortizado por trabajo trazado (17× en un
  `iter` perezoso de 1M).
- **`std/regex` en la VM sobre el crate `regex`** (R7): la VM despacha las `run_*` internas al mismo
  borde de `ray-runtime` que el binario nativo (feature `regex` de la toolchain, activa por defecto) —
  bench regex **18,05 s → 348 ms (52×)**, misma salida byte a byte. El intérprete (`--interp`), los
  builds slim y `RAYLANG_REGEX_PIKE=1` conservan la Pike VM escrita en raylang (oráculo del dialecto).
- **Revisión del intérprete** (V9): ronda 5 de superinstrucciones — guarda `i < n` con tope en
  variable y el cierre del bucle contado en UNA instrucción — y el camino de llamada sin consultar
  capturas slot a slot. En el banco (ray PGO): loopsum −22%, sortnums −16%, wordcount −13%,
  fibrec −8%, treealloc −11% respecto a lo publicado; la columna VM÷native baja en toda la tabla.
- **La llamada oculta por instrucción, eliminada** (V10): el cuerpo del despacho era una closure
  que LLVM no inlineaba — cada opcode pagaba una llamada de función real desde M12.3. Como método
  `#[inline(always)]` de un solo call-site: **−16% a −27% en TODO el banco** (con V9: loopsum
  16× del nativo, sortnums 6.8×, wordcount 4.8×, matrixmul 2.9×, regex 4.2×, jsonserialize 2.7×).
- **Camino de llamada e inlining selectivo** (V11): fast-paths por referencia en las guardas,
  la aritmética local-const y los incrementos fusionados (sin clonar valores ni materializar
  constantes), `new_locals`/`recycle` inlineados y la sonda del heap tras `RAY_HEAP_STATS`.
  En el banco (PGO): **fibrec 43× → 26×, treealloc 33× → 25×, loopsum 16× → 13×** y mejoras
  del 3-7% en el resto.
- **Register file por fibra** (V12): los locales de todos los marcos en una sola pila contigua
  (el marco guarda su base) — el camino de llamada no aloca, el GC rootea lineal y el pool de
  locales desaparece. fibrec −5%, y mejoras de 2-3% en la mitad del banco.
- **Arnés del banco poliglota**: el presupuesto por variante ya no corta las últimas rondas
  (sesgaba la mediana de las variantes lentas hasta un −17% ficticio y sin marcarlo) — ahora
  reparte las muestras que caben por TODA la sesión, y la columna **Corridas** (`n/runs ⚠`) más
  un aviso al pie hacen visible cualquier recorte. Cifras afectadas corregidas (PERFORMANCE.md
  Fase 73): fibrec 43×, treealloc 33×, loopsum 16× del nativo.
- **Kernel `DotRange` con deopt en la VM** (MM4): el bucle del producto punto
  (`for k in lo..hi { s = s + a[k]*b[k]; }`) se ejecuta como UN opcode cuando los arreglos son de
  floats (resultado bit a bit idéntico); cualquier otra forma cae al bytecode normal, que conserva la
  semántica de errores. Bench matrixmul **664 → 23,9 ms (28×)** — de 117× a 4,1× del nativo, empate
  estadístico con node (V8).
- **Binario nativo (arcos N/R/M96/SN/F)**: `mimalloc` y aHash en el transpilado (wordcount/logparse
  −40%, −8,5% extra), `join` y `concat` sin recopia, `for` sin clonar, `std/regex` sobre el crate
  `regex` (570 → 71 ms), pool de hilos shardeado y `print` sin lock global (18k → 58k req/s antes de
  las fibras), y el reactor de fibras a **cero asignaciones por ciclo**.
- **Framework web**: **~188k req/s** de techo — 93% de axum, p50/p99.9 empatadas (0,48/1,05 ms frente a
  0,47/1,04) y 1,5× Go+chi, sirviendo con 14 hilos y ~21 KB por conexión.
- **Banco poliglota** importado al repo (`benchmarks/poly/`) y banco de **carga web** con generador
  remoto, medianas y MAD; gate de regresión de **memoria** (pico de RSS) además del de tiempo.

### Cambiado

- **Los templates `.ray.html` se compilan en memoria** (M102): un `vistas/x.ray.html` es
  directamente el módulo `vistas/x` — el loader lo compila al resolver el import, sin generar
  ningún `.ray` en el proyecto (y si queda uno viejo, lo ignora). Desaparecen la regeneración por
  mtime y los generados commiteados; `ray build --templates-only` queda como materialización
  opcional para inspección. Y **todos los diagnósticos apuntan al template** (A2): un error de
  tipos, de runtime (con su traza) o de sintaxis en una expresión empalmada se reporta con la
  línea y el fuente del `.ray.html`, no con los del módulo generado invisible.
- **La función generada de un template se llama `render`** (M103): `import vistas/lista;` →
  `lista.render(…)` — el módulo ya namespacea, el sufijo `render_<stem>` de M55 era un artefacto
  de la era del generado commiteado. Sin alias de compatibilidad (criterio M99).
- **Todos los mensajes que el lenguaje entrega al usuario están en inglés** (compilador, runtime,
  tooling y stdlib), incluidos los espejos del compilador auto-alojado. Los comentarios del código
  siguen en español.
- **La CLI se agrupa en subcomandos** (M99): `ray registry publish/yank/keygen/verify` y
  `ray build --templates-only` sustituyen a los comandos sueltos anteriores (la interfaz legada por
  flags se conserva).
- El código del compilador se reorganizó en módulos-directorio (`vm/`, `transpile/`, `checker/`,
  `lsp/`), documentado en `docs/organizacion-codigo.md`.

### Cambiado

- **`ray fmt` reparte las listas delimitadas largas** (M106): argumentos de llamada, parámetros de
  `fn` y literales de arreglo, tupla, struct y Map que pasen de **100 columnas** bajan a un elemento
  por línea, con el delimitador de cierre **en su propia línea** y sin coma final. La regla que separa
  este cierre del de M104/M105: **con delimitador propio, cierra en línea propia**; sin él (el `;` de
  un import, el `)` que pertenece a la llamada que envuelve una cadena), pegado al último elemento.
- **`ray fmt` reparte las cadenas de métodos largas** (M105): una cadena de dos o más eslabones que
  pase de **100 columnas** —el patrón *builder*: `obj().field(…).field(…)`— baja cada `.metodo(…)` a
  su línea, un nivel por debajo de la sentencia. Mismo umbral y misma regla que los imports.
- **`ray fmt --write`** (`-w`, M105): reescribe **en el sitio** en vez de imprimir a stdout, y admite
  varios archivos (`ray fmt -w src/*.ray`). Solo reescribe lo que cambia, así que no toca el mtime de
  lo que ya es canónico.
- **`ray fmt` envuelve los `from … import` largos** (M104): si la línea renderizada pasa de **100
  columnas**, la lista se reparte a **un nombre por línea**; si cabe, se queda en una. El parser
  acepta además **coma final** en la lista (el formateador no la emite: sin llaves que cierren, el
  `;` quedaría colgando). La forma multilínea ya se parseaba — lo que faltaba era que el
  formateador la respetara. La **completion de imports del LSP** reconstruye ahora el contexto desde
  el inicio de la sentencia, no de la línea: con el import envuelto, el cursor en una línea de
  continuación sigue ofreciendo los `pub` del módulo.

### Corregido

- **`ray fmt archivo | head` (la salida del propio CLI) también reventaba con el ICE de "Broken
  pipe"** — el residuo del arreglo de `print`/`eprint`: fmt, doc, diagnósticos y REPL usan los
  macros de la libstd, que paniquean con el pipe cerrado. La red central de ICEs distingue ahora
  ese pánico (y solo ese) y aplica la misma convención: exit 141 en silencio, sin traza ni banner.
  Un pánico real sigue mostrando el ICE completo.

- **`programa | head` reventaba con un ICE de "Broken pipe".** Rust ignora SIGPIPE y `println!`
  paniquea al escribir en un stdout cerrado; el pánico salía disfrazado de error interno del
  compilador. Ahora `print`/`eprint` siguen la convención Unix: el proceso termina **en silencio
  con código 141** (128+SIGPIPE — el mismo destino observable que un programa C matado por la
  señal), en los tres motores (el nativo, vía su hilo escritor). `io.write`/`io.flush` no cambian:
  tienen `Result` y el programa decide qué hacer con el pipe cerrado.

- **`unit` escrito en posición de tipo no compilaba** — `extern "c" {{ fn free(p: ptr) -> unit; }}`
  se rechazaba con un mensaje que lo daba por válido (y REFERENCE §13 lo documenta), y
  `fn f() -> unit` tampoco pasaba: no hay token de tipo `unit`, así que llegaba como un nombre
  cualquiera y el checker no lo resolvía. Era un conflicto SPEC↔implementación (la SPEC decía «no
  es escribible») que se resuelve del lado útil: **la SPEC ahora lo declara escribible** y se
  resuelve como `Map`/`Channel`/`Task` (sombrea un tipo del usuario llamado así). Vale en firmas,
  tipos `fn` y externs, en los tres motores; `unit<int>` sigue siendo tipo desconocido y un
  parámetro extern `unit` sigue sin ser marshalable. En tándem: el espejo `selfhost/checker.ray`
  (que además tenía una divergencia latente de mensaje, `type unknown:` por `unknown type:`).

- **`ray fmt` movía comentarios de sitio.** Tres formas, todas destapadas al dejar el repo
  fmt-clean: el blanco que separa un banner de sección del ítem de abajo se comía (y el banner pasaba
  a leerse como su doc-comment); el comentario al final de la línea de un **campo de struct** se
  volcaba **tras el `}`**, donde queda como doc del ítem siguiente (los campos no llevaban línea en
  el AST — ahora sí, `StructDef.field_lines`); y el comentario al final de la línea de una **firma**
  bajaba al interior del cuerpo, lo que además mueve su línea (y hay marcas que dependen de ella,
  como el `// es-ok` de la política de nombres). Con esto, `src/prelude.ray` y
  `tools/registry_site.ray` —los dos últimos archivos que `ray fmt` no dejaba en paz— quedan
  formateados: **todo el `.ray` del repo es ahora un punto fijo del formateador**, y una guarda de CI
  (`tests/fmt_policy.rs`) lo asevera de aquí en adelante — distinguiendo un archivo sin formatear (lo
  arregla `ray fmt --write`) de un formateador que no converge (bug de `src/fmt.rs`).

- **`ray fmt` rompía el contenido de una cadena interpolada.** El repartir por ancho se colaba dentro
  de un `${…}`: en un template multilínea (un documento SVG/HTML, que rebasa el umbral por
  construcción) la primera llamada interpolada salía partida en varias líneas **dentro del literal**,
  cambiando el texto que el programa produce. Lo de dentro de una interpolación es contenido, no
  código: el envuelto ahora se apaga al entrar a un `${…}` y se restaura al salir.

- **Backend nativo: una `var` mutada por un closure DENTRO de `spawn`/`scope` no compilaba.** Ambos
  emiten el cuerpo del literal directamente, sin pasar por la emisión de función anónima, y con ello
  se saltaban el registro de "celdas" (B1): la variable salía como `let mut` dentro de un
  `Rc<closure>` y `rustc` la rechazaba (`E0596: cannot borrow data in an `Rc` as mutable`). La VM sí
  lo ejecutaba → era una divergencia nativo≠VM. Ahora el cuerpo de `spawn`/`scope` registra sus
  propias celdas.

- **VM: caída del GC (`index out of bounds` en `Heap::mark`) en servidores de larga vida.** Al hacer
  `spawn`, las celdas de los locales **capturados** de la fibra hija se alojaban en el heap del
  *spawner* en vez del suyo. Con heap-por-fibra ese handle cruzaba heaps: desde el arranque de la
  hija y hasta que su `InitLocal` estrenaba celda propia era una raíz del GC resuelta contra la tabla
  de slots equivocada — otro objeto si el índice cabía, pánico si no. Se disparaba cuando el spawner
  tenía un heap grande (p. ej. un `main` que siembra una base de datos y **luego** sirve): a la
  decimoquinta petición, `the len is 64 but the index is 828`.

- **Backend nativo: seis huecos de superficie** que la VM ejecutaba y el binario nativo rechazaba —
  arreglos `reverse`/`pop`/`position`, `math.atan2`/`float_bits`/`float_from_bits` y
  `fs.append_file` (existía `append_file_bytes`). El checklist `NATIVE_TRACKED_BUILTINS` los daba por
  soportados: comprobaba que estuvieran clasificados, no que tuvieran brazo. La prueba pasa a ser
  ejecutarlos (nativo ≡ VM en un test dedicado). `min`/`max` de iterador siguen fuera —bound
  `T: Ord`, la misma limitación que cualquier genérico de usuario acotado— pero ahora lo dicen.

- Binario nativo: `for i in 0..s.len()` no compilaba. En Rust, `for x in EXPR {` toma un bloque
  inicial de `EXPR` como **cuerpo** del loop, y varios builtins emiten un bloque (`len` de string,
  concatenación, `push`); ahora los extremos del rango se emiten entre paréntesis.
- Binario nativo: `for c in <string>.chars()` no compilaba (el transpilador decidía el modo de
  iteración por el tipo del ELEMENTO —`char` → `.chars()`— y lo aplicaba al `Vec<char>` ya
  materializado). Ahora lo decide el tipo del contenedor: string → `.chars()`, `[char]` → vía de
  arreglo. La VM ya lo ejecutaba bien.
- Playground web: el build wasm estaba **roto desde P0.1/M100** (aHash arrastra `getrandom`, que no
  compila a wasm32; el empaquetado `gen << 32` de handles de tareas/canales desborda el `usize` de
  32 bits; un builtin de procesos sin stub) — recompilado y verificado (núcleo + concurrencia M:1 +
  diagnósticos en inglés); el `.wasm` embarcado llevaba desde el 7 jul con mensajes en español.
- LSP: la completion tras `from std/M import `, las rutas `std/…` en posición de import y el
  **signature help** de funciones importadas de la stdlib ahora funcionan también para los módulos
  **embebidos** (antes la resolución iba solo a disco y devolvía vacío fuera del repo) — el sitio
  clave de la forma UFCS de los constructores de unidades, que exige el import sin calificar.
- Contención de fallos por tarea y `try_join` en el backend nativo; un fallo observado con `try_join`
  cuenta como manejado por el scope (M97.1).
- El almacén de tareas y el de canales liberan al consumirse/cerrarse (M98.1–M98.3): fugas de memoria en
  servicios de larga vida.
- Errores de ejecución del binario nativo con **exit 70** y mensajes idénticos a la VM (H6).
- Tope de salida anti-bomba al descomprimir (M64.2) y endurecimiento de parseo en HPACK, DNS, JWT,
  SCRAM y los clientes de BD.
- Diagnóstico dedicado para el gotcha del checker de DESIGN §55, en ambos checkers (M87).

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
- **Anotaciones** (`@test`, `@derive(Eq, Show, Hash)`).

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
