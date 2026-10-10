# Windows in depth

[Español](windows.md) · English

The [cross-platform](cross-platform.en.md) chapter opens a window and gives it menus. This one
walks through everything `std/ui` offers a desktop app: how the page is served, the bridge to the
program, menus, dialogs, window kinds, the life of a document with unsaved changes, and the
differences between systems.

The example is **Pad**, a minimal text editor, in
[`examples/apps/pad-desktop`](../examples/apps/pad-desktop/). The raylang blocks are copied from it
and CI checks that they still are.

## 1. A window

A raylang window is a native system window with the system webview inside. The interface is HTML,
CSS and JavaScript; the raylang program lives in the same process.

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

- `ui.options(width, height)` gives the default options, and `open_with` applies them.
  `ui.open(title, url, width, height)` is the shortcut without options.
- `autosave` makes macOS remember size and position between runs. Linux and Windows ignore it.
- `titlebar_color` paints the title bar in the page's colour: on macOS the bar becomes
  transparent, on Windows 11 the bar colour changes, and Linux ignores it. A value that is not
  `#rrggbb` is an error.
- `background` is the colour shown until the page finishes painting. Without it, a dark app shows
  a white flash on opening.

There is no blocking `run()` function: the runtime takes the main thread when the first window
opens, and the program carries on in its fibers.

## 2. Where the page comes from

**No server: `ray://app/`.** The page is served from the process itself, with no port and no
HTTP. No other app on the machine and no page in the browser can talk to it.

| Mount | Serves | At |
|---|---|---|
| `ui.mount_embed("", "assets")` | the embedded files (`[native] embed`) | `ray://app/assets/…` |
| `ui.mount_embed_at("", "frontend/dist")` | a frontend build, at the root | `ray://app/…` |
| `ui.mount_dir("files", folder)` | a folder on disk, with range reads | `ray://app/files/…` |
| `ui.mount_bytes(path, data)` | bytes in memory | `ray://app/<path>` |
| (by itself, on open or mount) | the npm package of a dependency with `[web] package`, with an import map in every page | `ray://app/node_modules/<npm name>/…` |

`mount_dir` never serves anything outside its folder, and it supports `Range` requests: a video or
a large file is read in chunks, never loaded whole.

**With Vite: `app://`.** With a `[frontend]` section in `ray.toml`, `app://index.html` points at
the Vite server under `ray dev` and at the embedded build in production. The
[mobile](mobile.en.md) and [cross-platform](cross-platform.en.md) chapters use it.

**With a local server.** If the interface needs real HTTP, the `web` framework can listen for the
window only: `web.listen_local(build_app, listener, token)` requires a token passed in the URL,
and refuses any other process or page.

## 3. Events

The program receives everything that happens in a single stream: `ui.next_event()` waits for the
next one without using CPU.

| `kind` | When | `tag` |
|---|---|---|
| `"message"` | the page called `window.ray.send` or `request` | the text sent |
| `"menu"` | a menu item was chosen | the item's tag |
| `"closed"` | a window closed | |
| `"close_requested"` | the user tried to close (with `intercept_close`) | |
| `"quit_requested"` | the user tried to quit (with `intercept_quit`) | |
| `"focused"` | a window came to the front | |
| `"notification"` | the user clicked a notification from `ui.notify` | the notification's tag |
| `"lifecycle"` | on mobile, the app went to the background or came back | `"background"` / `"foreground"` |

Every event carries its window in `window`. There is a single consumer: use `next_event()`, or
`ui.events()` if you want the stream as a channel to combine with others in a `select`, or
`ui.split_events()` to separate the messages from the rest into two channels. Do not mix them.

## 4. The bridge to the page

The page has `window.ray` in every document the webview loads:

- `window.ray.send(value)` sends a message without waiting for an answer.
- `window.ray.request(value)` returns a Promise that the program resolves.

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

`ui.as_request` tells the two cases apart. `ui.reply_json` hands the page an object, and
`ui.reply` a text. In the other direction, the program runs JavaScript in the page with
`ui.eval_js`:

<!-- check: project=examples/apps/pad-desktop -->
```rust
// Sends the document to the page and refreshes everything the window shows about it.
fn show(w: int, d: Doc, on_top: bool) {
    let _ = ui.eval_js(w, "setText(" + json.stringify(Json.JStr(d.text)) + ")");
    refresh(w, d, on_top);
}
```

`json.stringify` turns the text into a safe JavaScript literal, with its quotes and line breaks
escaped. Never concatenate user text straight into the JavaScript.

What is worth knowing about the bridge:

- Values that are not text travel as JSON.
- Answering with a megabyte costs about 4 ms. For large or binary files, serve them through
  `ray://app/` with `mount_dir` instead of putting them in a message.
- The event queue has a limit of 65,536. If the page sends non-stop and the program does not
  read, the oldest messages are dropped, never a `"closed"`.
- On macOS and iOS only the main document reaches the bridge, not iframes.

## 5. Menus

Menus are declared as data, before opening the window:

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

- **Roles.** An item tagged `role:undo`, `role:cut`, `role:copy`, `role:paste`,
  `role:select_all` or `role:close` does what the system's own would and emits no event. With an
  empty title and shortcut it takes the standard ones. On macOS, without the Edit menu neither ⌘C
  nor ⌘V work in the page's fields: that is why the roles are kept when adding your own items.
- **Shortcuts.** `cmd+s`, `cmd+shift+s`, `ctrl+alt+p`, `f5`. `cmd` is Command on macOS and Ctrl on
  Windows.
- **The application menu.** `ui.app_menu` adds items to the first menu on macOS, and `role:about`
  shows the native About panel, whose content `ui.set_about` sets.

Enabling, disabling or checking an item goes by its tag:

<!-- check: project=examples/apps/pad-desktop -->
```rust
/// "Show in Folder" only makes sense once the document has a file.
pub fn sync(has_file: bool, on_top: bool) {
    let _ = ui.set_menu_item("reveal", has_file, false);
    let _ = ui.set_menu_item("on_top", true, on_top);
}
```

The **context menu** uses the same items. The right click happens in the page, so the page says so
and the program shows the menu at the pointer:

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

`ui.replace_menu(title, items)` rebuilds a whole menu, for example a list of recent documents.

## 6. Dialogs

Dialogs are the system's, and the call waits until the user answers.

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

- `ui.message` takes up to three buttons and returns the index of the one pressed. Closing with
  Esc counts as the **last** button: put "Cancel" last.
- `ui.alert` is the one-button version and `ui.confirm` the two-button one, which returns a
  `bool`.
- `ui.message_styled` adds the `"warning"` or `"error"` style.

The file dialogs take a title, a starting folder, a suggested name and extension filters:

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

`None` means the user cancelled. `ui.pick_files` returns several, `ui.pick_folder` a folder and
`ui.save_file_with` a target to save to.

## 7. A document with unsaved changes

A document app needs three things from the window: the title follows the document, unsaved changes
are visible, and closing with changes asks first.

<!-- check: project=examples/apps/pad-desktop -->
```rust
// The title, the "edited" dot of the close button (macOS) and the menu items that depend on it.
fn refresh(w: int, d: Doc, on_top: bool) {
    let _ = ui.set_title(w, doc.title(d));
    let _ = ui.set_edited(w, d.dirty);
    menus.sync(d.path != "", on_top);
}
```

`set_edited` puts the dot in the macOS close button. With `intercept_close` and `intercept_quit`
(section 1), closing the window or quitting the app no longer just happens: an event arrives, and
the window stays open until the program calls `close`.

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

The question is a single function, which "New" and "Open" use too:

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

## 8. More windows

There are four kinds, chosen with `kind`:

| `kind` | What it is |
|---|---|
| `"document"` | the normal window |
| `"panel"` | a palette floating above the app's windows |
| `"borderless"` | no frame and no title: a splash, a HUD |
| `"full_content"` | the page takes the title bar area too, as in a browser |

Pad's find panel is a `panel` that belongs to the document window:

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

`parent` keeps the window above its owner and makes it follow. On an open window:
`set_fullscreen`, `set_always_on_top`, `set_size`, `set_position`, `center`, `minimize`,
`maximize` and `focus`, which brings it to the front.

With `full_content`, on macOS, the page reserves the title bar strip with
`window.ray.titlebar_height` and marks the element that drags the window with the `data-ray-drag`
attribute. On Linux and Windows it behaves like `document`.

## 9. The rest of the system

- `ui.clipboard_write(text)` and `ui.clipboard_read()` use the clipboard.
- `ui.open_path(path)` opens a file with its default application, and `ui.reveal(path)` shows it
  in the file manager.
- `std/keychain` stores secrets in the system keychain.
- **Notifications.** `ui.notify("Mail", "3 new messages")` shows a system notification with the
  app's icon, no buttons; `ui.notify_with(title, body, tag, sound)` tags it, and a click arrives
  as a `"notification"` event with that tag. On macOS the real thing (own icon, click, the
  permission the system asks for the first time) needs an app packaged with `ray bundle`; under
  `ray run` the notification shows all the same, but with no own icon and no click. Linux uses
  `notify-send`; Windows is not there yet.
- **Badge and attention.** `ui.badge("3")` puts the count on the Dock icon (`""` clears it; Linux
  has none and ignores it) and `ui.request_attention()` bounces the icon or highlights the window
  in the taskbar without stealing focus.

None of that goes through a shell, except `notify-send` on Linux.

## 10. Developing and testing

- **`ray dev`** restarts the program on save and reloads the windows. Closing the window ends
  development mode.
- **The webview inspector** is available under `ray dev` and with `ray run --devtools`. A native
  binary only carries it when built with `--devtools`: in a release it does not exist.
- **Tests without a display.** `ray test` uses a headless backend: opening windows, installing
  menus and calling dialogs work in CI. A dialog answers its first button, or the index in
  `RAY_UI_ANSWER`; `RAY_UI_PICK` gives the path for file dialogs, and `RAY_UI_MSG` injects a
  message from the page. `RAY_UI_BACKEND=headless` turns that backend on for any run.

## 11. Differences between systems

| | macOS | Linux | Windows | iOS and Android |
|---|---|---|---|---|
| Webview | WKWebView | WebKitGTK | WebView2 | the system's |
| Menu bar | global | per window | per window | none |
| Menu shortcuts | yes | no, click only | yes | |
| Declaring menus | at any time | before opening the window | before opening the window | |
| `autosave` | yes | no | no | |
| `titlebar_color` | yes | no | Windows 11 | |
| `full_content` | yes | like `document` | like `document` | |
| `window` in events | the handle | the handle | the handle | always 0 |

Linux needs GTK 3 and WebKitGTK installed, and Windows the WebView2 Runtime, which Windows 11
already ships. If they are missing, `ui.open` returns a clear error. `--without ui` leaves the
subsystem out of a binary that opens no windows.

## Next step

The notes leave the device: a [**server-rendered site**](ssr.en.md).

<!-- sync: sha256:91c8df03114a -->
