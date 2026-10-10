# Ventanas a fondo

Español · [English](windows.en.md)

El capítulo [multiplataforma](cross-platform.md) abre una ventana y le pone menús. Este recorre
todo lo que `std/ui` ofrece a una app de escritorio: cómo se sirve la página, el puente con el
programa, menús, diálogos, tipos de ventana, el ciclo de un documento con cambios sin guardar, y
las diferencias entre sistemas.

El ejemplo es **Pad**, un editor de texto mínimo, en
[`examples/apps/pad-desktop`](../examples/apps/pad-desktop/). Los bloques de raylang están copiados
de él y el CI comprueba que sigan siéndolo.

## 1. Una ventana

Una ventana de raylang es una ventana nativa del sistema con el webview del sistema dentro. La
interfaz es HTML, CSS y JavaScript; el programa raylang vive en el mismo proceso.

<!-- check: project=examples/apps/pad-desktop -->
```rust
    // The embedded assets become ray://app/assets/…: no HTTP server, no port.
    match (ui.mount_embed("", "assets")) {
        Result.Ok(_) => { },
        Result.Err(e) => {
            eprint("pad: " + e);
            return 1;
        },
    }
    match (menus.install()) {
        Result.Ok(_) => { },
        Result.Err(e) => eprint("pad: menus: " + e),
    }
    // A minimum size, remembered geometry, and the title bar in the page's colour.
    var o = ui.options(900, 640);
    o.min_width = 420;
    o.min_height = 300;
    o.autosave = "pad-main";
    o.titlebar_color = DARK;
    o.background = DARK;
    let w = match (ui.open_with("Untitled — Pad", PAGE, o)) {
        Result.Ok(w) => w,
        Result.Err(e) => {
            eprint("pad: " + e);
            return 1;
        },
    };
    // Closing and quitting ask first: the attempt arrives as an event and the window stays.
    let _ = ui.intercept_close(w, true);
    let _ = ui.intercept_quit(true);
```

- `ui.options(ancho, alto)` da las opciones por defecto, y `open_with` las aplica. `ui.open(título,
  url, ancho, alto)` es el atajo sin opciones.
- `autosave` hace que macOS recuerde tamaño y posición entre ejecuciones. Linux y Windows lo
  ignoran.
- `titlebar_color` pinta la barra de título del color de la página: en macOS la barra se vuelve
  transparente, en Windows 11 cambia el color de la barra, y Linux lo ignora. Un valor que no sea
  `#rrggbb` es un error.
- `background` es el color que se ve hasta que la página termina de pintarse. Sin él, una app
  oscura muestra un destello blanco al abrir.

No hay una función `run()` que bloquee: el runtime toma el hilo principal al abrir la primera
ventana, y el programa sigue en sus fibras.

## 2. De dónde sale la página

**Sin servidor: `ray://app/`.** La página se sirve desde el propio proceso, sin puerto ni HTTP.
Ninguna otra app de la máquina ni ninguna página del navegador puede hablar con ella.

| Montaje | Sirve | En |
|---|---|---|
| `ui.mount_embed("", "assets")` | los archivos embebidos (`[native] embed`) | `ray://app/assets/…` |
| `ui.mount_embed_at("", "frontend/dist")` | un build de frontend, en la raíz | `ray://app/…` |
| `ui.mount_dir("files", carpeta)` | una carpeta del disco, con lecturas por tramos | `ray://app/files/…` |
| `ui.mount_bytes(ruta, datos)` | unos bytes en memoria | `ray://app/<ruta>` |
| (solo, al abrir o montar) | el paquete npm de una dependencia con `[web] package`, con un import map en cada página | `ray://app/node_modules/<nombre npm>/…` |

`mount_dir` nunca sirve nada de fuera de su carpeta, y admite peticiones `Range`: un vídeo o un
archivo grande se leen por trozos, sin cargarlos enteros.

**Con Vite: `app://`.** Con una sección `[frontend]` en el `ray.toml`, `app://index.html` apunta
al servidor de Vite bajo `ray dev` y al build embebido en producción. Lo usan los capítulos
[móvil](mobile.md) y [multiplataforma](cross-platform.md).

**Con un servidor local.** Si la interfaz necesita HTTP de verdad, el framework `web` puede
escuchar solo para la ventana: `web.listen_local(build_app, listener, token)` exige un token que
se pasa en la URL, y rechaza a cualquier otro proceso o página.

## 3. Los eventos

El programa recibe todo lo que pasa en un solo flujo: `ui.next_event()` espera el siguiente sin
consumir CPU.

| `kind` | Cuándo | `tag` |
|---|---|---|
| `"message"` | la página llamó a `window.ray.send` o `request` | el texto enviado |
| `"menu"` | se eligió un elemento de menú | la etiqueta del elemento |
| `"closed"` | una ventana se cerró | |
| `"close_requested"` | el usuario intentó cerrar (con `intercept_close`) | |
| `"quit_requested"` | el usuario intentó salir (con `intercept_quit`) | |
| `"open"` | el sistema pidió abrir algo: un archivo o carpeta soltado sobre el icono, «Abrir con», `open -a App ruta` (también con la app abierta; requiere `[app] opens`) | la ruta, un evento por elemento |

En Linux y Windows lo que el usuario abre llega como argumentos de un **proceso nuevo**. Para que
también acabe en la app ya abierta, llama a `ui.single_instance()` al empezar: la primera
instancia devuelve `true` y recibe sus `args()` y los de cada lanzamiento posterior como eventos
`"open"`; la posterior devuelve `false` y retorna de `main`. Así `miapp carpeta/` desde la
terminal y «Abrir con» se tratan igual que el arrastre al icono en macOS.
| `"focused"` | una ventana pasó al frente | |
| `"notification"` | el usuario pulsó una notificación de `ui.notify` | la etiqueta de la notificación |
| `"lifecycle"` | en móvil, la app pasó a segundo plano o volvió | `"background"` / `"foreground"` |

Cada evento lleva la ventana en `window`. Hay un único consumidor: usa `next_event()`, o
`ui.events()` si quieres el flujo como canal para combinarlo con otros en un `select`, o
`ui.split_events()` para separar los mensajes del resto en dos canales. No los mezcles.

## 4. El puente con la página

La página tiene `window.ray` en cualquier documento que cargue el webview:

- `window.ray.send(valor)` envía un mensaje sin esperar respuesta.
- `window.ray.request(valor)` devuelve una Promise que el programa resuelve.

<!-- check: project=examples/apps/pad-desktop -->
```rust
        if (e.kind == "message") {
            match (ui.as_request(e)) {
                Option.Some(request) => {
                    let (id, body) = request;
                    let req = json.parse(body).unwrap_or(Json.JNull);
                    let op = json.get_string(req, "op").unwrap_or("");
                    if (op == "changed") {
                        d.text = json.get_string(req, "text").unwrap_or("");
                        d.dirty = true;
                        refresh(w, d, on_top);
                        let _ = ui.reply_json(e.window, id, `{"ok": true}`);
                    }
                    if (op == "find") {
                        let n = doc.count_matches(d, json.get_string(req, "query").unwrap_or(""));
                        let _ = ui.reply_json(e.window, id, `{"count": ${n}}`);
                    }
                },
                // A plain window.ray.send("context"): show the context menu at the pointer.
                Option.None => {
                    if (e.tag == "context") {
                        let _ = menus.context(e.window, d.path != "");
                    }
                },
            }
        }
```

`ui.as_request` distingue los dos casos. `ui.reply_json` entrega a la página un objeto, y
`ui.reply` un texto. En sentido contrario, el programa ejecuta JavaScript en la página con
`ui.eval_js`:

<!-- check: project=examples/apps/pad-desktop -->
```rust
// Sends the document to the page and refreshes everything the window shows about it.
fn show(w: int, d: Doc, on_top: bool) {
    let _ = ui.eval_js(w, "setText(" + json.stringify(Json.JStr(d.text)) + ")");
    refresh(w, d, on_top);
}
```

`json.stringify` convierte el texto en un literal de JavaScript seguro, con sus comillas y saltos
de línea escapados. Nunca concatenes texto del usuario directamente en el JavaScript.

Lo que conviene saber del puente:

- Los valores que no son texto viajan como JSON.
- Responder con un megabyte cuesta unos 4 ms. Para archivos grandes o binarios, sírvelos por
  `ray://app/` con `mount_dir` en vez de meterlos en un mensaje.
- La cola de eventos tiene un límite de 65 536. Si la página envía sin parar y el programa no
  lee, se descartan los mensajes más viejos, nunca un `"closed"`.
- En macOS y iOS solo el documento principal alcanza el puente, no los iframes.

## 5. Menús

Los menús se declaran como datos, antes de abrir la ventana:

<!-- check: project=examples/apps/pad-desktop -->
```rust
/// The menu bar: the application menu, File, Edit and View.
pub fn install() -> Result<int, string> {
    // The application menu (macOS: the bold one). "role:about" is the native About panel.
    ui.app_menu("Pad", [ui.item("role:about", "", ""), ui.item("settings", "Settings…", "cmd+,")])?;
    ui.set_about("Pad", "Version 0.1", "A tiny editor written in raylang", "")?;
    ui.menu(
        "File",
        [
            ui.item("new", "New", "cmd+n"),
            ui.item("open", "Open…", "cmd+o"),
            ui.separator(),
            ui.item("save", "Save", "cmd+s"),
            ui.item("save_as", "Save As…", "cmd+shift+s"),
            ui.separator(),
            ui.item("reveal", "Show in Folder", ""),
            ui.item("role:close", "", "")
        ]
    )?;
    // Edit keeps the system roles (undo, clipboard, select all) and adds one item of ours.
    ui.edit_menu(
        [
            ui.item("role:undo", "", ""),
            ui.item("role:redo", "", ""),
            ui.separator(),
            ui.item("role:cut", "", ""),
            ui.item("role:copy", "", ""),
            ui.item("role:paste", "", ""),
            ui.item("role:select_all", "", ""),
            ui.separator(),
            ui.item("find", "Find…", "cmd+f")
        ]
    )?;
    ui.menu(
        "View",
        [
            ui.item("fullscreen", "Enter Full Screen", "cmd+ctrl+f"),
            ui.item("on_top", "Keep on Top", "")
        ]
    )
}
```

- **Roles.** Un elemento con etiqueta `role:undo`, `role:cut`, `role:copy`, `role:paste`,
  `role:select_all` o `role:close` hace lo que haría el del sistema y no genera evento. Con el
  título y el atajo vacíos toma los estándar. En macOS, sin el menú Edición no funcionan ni
  ⌘C ni ⌘V en los campos de la página: por eso se conservan los roles al añadir elementos propios.
- **Atajos.** `cmd+s`, `cmd+shift+s`, `ctrl+alt+p`, `f5`. `cmd` es Comando en macOS y Ctrl en
  Windows.
- **El menú de la aplicación.** `ui.app_menu` añade elementos al primer menú de macOS, y
  `role:about` muestra el panel «Acerca de» nativo, cuyo contenido fija `ui.set_about`.

Activar, desactivar o marcar un elemento se hace por su etiqueta:

<!-- check: project=examples/apps/pad-desktop -->
```rust
/// "Show in Folder" only makes sense once the document has a file.
pub fn sync(has_file: bool, on_top: bool) {
    let _ = ui.set_menu_item("reveal", has_file, false);
    let _ = ui.set_menu_item("on_top", true, on_top);
}
```

El **menú contextual** usa los mismos elementos. El clic derecho ocurre en la página, así que la
página avisa y el programa muestra el menú en el puntero:

<!-- check: project=examples/apps/pad-desktop -->
```rust
/// The context menu, at the pointer, over window `w`. The choice arrives as a "menu" event.
pub fn context(w: int, has_file: bool) -> Result<int, string> {
    var items = [
        ui.item("role:cut", "", ""),
        ui.item("role:copy", "", ""),
        ui.item("role:paste", "", "")
    ];
    if (has_file) {
        items.push(ui.separator());
        items.push(ui.item("copy_path", "Copy File Path", ""));
        items.push(ui.item("reveal", "Show in Folder", ""));
    }
    ui.popup_menu(w, items)
}
```

`ui.replace_menu(título, elementos)` rehace un menú entero, por ejemplo una lista de documentos
recientes.

## 6. Diálogos

Los diálogos son los del sistema, y la llamada espera hasta que el usuario responde.

<!-- check: project=examples/apps/pad-desktop -->
```rust
/// "Save changes?" with three buttons. Esc counts as the LAST button, so Cancel goes last.
pub fn ask_save(name: string) -> Choice {
    let answer = ui.message_styled(
        "Save changes to " + name + "?",
        "Your changes will be lost if you don't save them.",
        "warning",
        ["Save", "Don't Save", "Cancel"]
    );
    match (answer) {
        Result.Ok(0) => Choice.Save,
        Result.Ok(1) => Choice.Discard,
        _ => Choice.Cancel,
    }
}
```

- `ui.message` admite hasta tres botones y devuelve el índice del pulsado. Cerrar con Esc cuenta
  como el **último** botón: pon «Cancelar» al final.
- `ui.alert` es la versión de un botón y `ui.confirm` la de dos, que devuelve un `bool`.
- `ui.message_styled` añade el estilo `"warning"` o `"error"`.

Los de archivo aceptan título, carpeta inicial, nombre sugerido y filtros por extensión:

<!-- check: project=examples/apps/pad-desktop -->
```rust
fn text_files() -> ui.FileDialogOptions {
    var o = ui.file_options();
    o.filters = [ui.filter("Text", ["txt", "md"]), ui.filter("Source", ["ray", "toml", "json"])];
    o
}

/// The system "Open" panel. `None`: the user cancelled.
pub fn pick_document() -> Option<string> {
    var o = text_files();
    o.title = "Open a document";
    ui.pick_file_with(o).unwrap_or(Option.None)
}
```

`None` significa que el usuario canceló. `ui.pick_files` devuelve varios, `ui.pick_folder` una
carpeta y `ui.save_file_with` un destino para guardar.

## 7. Un documento con cambios sin guardar

Una app de documentos necesita tres cosas de la ventana: que el título siga al documento, que se
vea que hay cambios, y que cerrar con cambios pregunte antes.

<!-- check: project=examples/apps/pad-desktop -->
```rust
// The title, the "edited" dot of the close button (macOS) and the menu items that depend on it.
fn refresh(w: int, d: Doc, on_top: bool) {
    let _ = ui.set_title(w, doc.title(d));
    let _ = ui.set_edited(w, d.dirty);
    menus.sync(d.path != "", on_top);
}
```

`set_edited` pone el punto en el botón de cerrar de macOS. Con `intercept_close` e
`intercept_quit` (sección 1), cerrar la ventana o salir de la app ya no ocurre solo: llega un
evento, y la ventana sigue abierta hasta que el programa llama a `close`.

<!-- check: project=examples/apps/pad-desktop -->
```rust
        if (e.kind == "closed") {
            if (e.window == find) {
                find = 0;
            }
            if (e.window == w) {
                return 0;
            }
        }
        if (e.kind == "close_requested" && may_discard(d)) {
            let _ = close(e.window);
        }
        if (e.kind == "quit_requested" && may_discard(d)) {
            return 0;
        }
```

La pregunta es una sola función, que también usan «Nuevo» y «Abrir»:

<!-- check: project=examples/apps/pad-desktop -->
```rust
// Before losing the document (new, open, close, quit): true if it is safe to go on.
fn may_discard(d: Doc) -> bool {
    if (!d.dirty) {
        return true;
    }
    match (dialogs.ask_save(doc.name(d))) {
        Choice.Save => save(d, false),
        Choice.Discard => true,
        Choice.Cancel => false,
    }
}
```

## 8. Más ventanas

Hay cuatro tipos, que se eligen con `kind`:

| `kind` | Qué es |
|---|---|
| `"document"` | la ventana normal |
| `"panel"` | una paleta flotante sobre las ventanas de la app |
| `"borderless"` | sin marco ni título: un splash, un HUD |
| `"full_content"` | la página ocupa también la barra de título, como en un navegador |

El panel de búsqueda de Pad es un `panel` hijo de la ventana del documento:

<!-- check: project=examples/apps/pad-desktop -->
```rust
// The Find panel: a floating utility window that belongs to the document window.
fn open_find(parent: int) -> int {
    var o = ui.options(320, 96);
    o.kind = "panel";
    o.parent = parent;
    o.resizable = false;
    o.minimizable = false;
    o.background = "#2a3040";
    ui.open_with("Find", FIND, o).unwrap_or(0)
}
```

`parent` hace que la ventana quede encima de su dueña y la siga. Sobre una ventana abierta:
`set_fullscreen`, `set_always_on_top`, `set_size`, `set_position`, `center`, `minimize`,
`maximize` y `focus`, que la trae al frente.

Con `full_content`, en macOS, la página reserva la franja de la barra con
`window.ray.titlebar_height` y marca con el atributo `data-ray-drag` el elemento que arrastra la
ventana. En Linux y Windows se comporta como `document`.

## 9. El resto del sistema

- `ui.clipboard_write(texto)` y `ui.clipboard_read()` usan el portapapeles.
- `ui.open_path(ruta)` abre un archivo con su aplicación por defecto, y `ui.reveal(ruta)` lo
  muestra en el gestor de archivos.
- `std/keychain` guarda secretos en el llavero del sistema.
- **Notificaciones.** `ui.notify("Correo", "3 mensajes nuevos")` muestra una notificación del
  sistema con el icono de la app, sin botones; `ui.notify_with(título, cuerpo, etiqueta, sonido)`
  le pone una etiqueta, y pulsarla llega como evento `"notification"` con esa etiqueta. En macOS
  la versión real (icono propio, clic, permiso que el sistema pide la primera vez) es la de una
  app empaquetada con `ray bundle`; bajo `ray run` la notificación se ve igual, pero sin icono
  propio ni clic. En Linux usa `notify-send`; en Windows aún no está.
- **Badge y atención.** `ui.badge("3")` pone el contador en el icono del Dock (`""` lo quita; en
  Linux no existe y se ignora) y `ui.request_attention()` hace saltar el icono o resalta la
  ventana en la barra sin robar el foco.

Nada de eso pasa por una shell, salvo `notify-send` en Linux.

## 10. Desarrollar y probar

- **`ray dev`** reinicia el programa al guardar y recarga las ventanas. Cerrar la ventana termina
  el modo de desarrollo.
- **El inspector del webview** está disponible bajo `ray dev` y con `ray run --devtools`. Un
  binario nativo solo lo lleva si se compila con `--devtools`: en un release no existe.
- **Tests sin pantalla.** `ray test` usa un backend sin interfaz: abrir ventanas, instalar menús
  y llamar a diálogos funciona en CI. Un diálogo responde su primer botón, o el índice de
  `RAY_UI_ANSWER`; `RAY_UI_PICK` da la ruta de los diálogos de archivo, y `RAY_UI_MSG` inyecta un
  mensaje de la página. `RAY_UI_BACKEND=headless` activa ese backend en cualquier ejecución.

## 11. Diferencias entre sistemas

| | macOS | Linux | Windows | iOS y Android |
|---|---|---|---|---|
| Webview | WKWebView | WebKitGTK | WebView2 | el del sistema |
| Barra de menús | global | por ventana | por ventana | no hay |
| Atajos de menú | sí | no, solo clic | sí | |
| Menús declarados | en cualquier momento | antes de abrir la ventana | antes de abrir la ventana | |
| `autosave` | sí | no | no | |
| `titlebar_color` | sí | no | Windows 11 | |
| `full_content` | sí | como `document` | como `document` | |
| `window` en los eventos | el identificador | el identificador | el identificador | siempre 0 |

En Linux hacen falta GTK 3 y WebKitGTK instalados, y en Windows el WebView2 Runtime, que
Windows 11 ya trae. Si faltan, `ui.open` devuelve un error claro. `--without ui` deja el
subsistema fuera de un binario que no abre ventanas.

## Siguiente paso

Las notas salen del dispositivo: un [**sitio con plantillas**](ssr.md) renderizado en el servidor.
