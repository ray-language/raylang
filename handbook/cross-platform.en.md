# Cross-platform app

[Español](cross-platform.md) · English

This chapter continues the [mobile app](mobile.en.md) one. The same **Notes** now also runs as a
desktop app on macOS, Linux and Windows, with a single `src/` and a single interface for all five
platforms. Along the way the store changes: the notes move from `std/kv` to **SQLite**, with
search, and they live in the folder each system expects.

The complete project is in
[`examples/apps/notes-everywhere`](../examples/apps/notes-everywhere/), with its tests. As in the
previous chapter, every raylang block is copied from that project and CI checks that it still is.

## What changes compared to the mobile app

| | Mobile app | Cross-platform |
|---|---|---|
| Platforms | iOS and Android | macOS, Linux, Windows, iOS and Android |
| Data | `std/kv`, a key-value file | SQLite with the `db` package |
| Data folder | `$HOME/Documents/Notes` | each system's own (section 2) |
| Desktop | a plain window | native menus, shortcuts, dialogs, minimum size |
| Interface | one column | one column on the phone; list and editor side by side on wide screens |

The app model is the same: the raylang program and the webview live in one process, and the page
talks to the program through `window.ray.request`. What is new is added around the same event
loop, and nearly all of it is on the program's side:

| The program decides | The page decides |
|---|---|
| where and how the data is stored | the layout and the navigation |
| the menus, their shortcuts and the system dialogs | what each menu command does |
| which platform it runs on | what to show on each one |
| reading and writing files | the state of what is being edited |

That boundary is what allows a single interface: the page does not know whether there are menus
or where the database is, and the program does not know how a note is drawn.

## 1. SQLite with the `db` package

SQLite is not in the standard library: it is part of the `db` package, published in the raylang
package index. One command adds it:

```sh
ray add db
```

`ray add` finds the latest version, downloads it into `.ray-deps/` and declares it in `ray.toml`:

```toml
[dependencies]
db = "^0.5.1"
```

The exact version and its hash are pinned in `ray.lock`, which goes into version control. The
package compiles SQLite into the binary, so no system library is needed on any of the five
platforms.

Opening the database creates the schema if it does not exist. WAL mode means a read never waits
for a write, and a crash in the middle of a write cannot corrupt the file:

<!-- check: project=examples/apps/notes-everywhere -->
```rust
const SCHEMA: string = `CREATE TABLE IF NOT EXISTS notes (
    id         TEXT PRIMARY KEY,
    title      TEXT NOT NULL,
    body       TEXT NOT NULL,
    updated_ms INTEGER NOT NULL
)`;

/// Opens (or creates) the database at `path` and makes sure the schema exists.
pub fn open(path: string) -> Result<Conn, string> {
    let conn = sqlite.connect(path)?;
    // WAL: readers never wait for the writer, and a crash mid-write cannot corrupt the file.
    let _ = sqlite.query(conn, "PRAGMA journal_mode=WAL", [])?;
    let _ = sqlite.exec(conn, SCHEMA, [])?;
    let _ = sqlite.exec(
        conn,
        "CREATE INDEX IF NOT EXISTS notes_by_date ON notes (updated_ms DESC)",
        []
    )?;
    Result.Ok(conn)
}
```

Queries use `?1`, `?2` parameters, bound separately from the SQL. That is why searching with a quote
breaks nothing: it is data, not code. The search is a `LIKE` over the title and the body:

<!-- check: project=examples/apps/notes-everywhere -->
```rust
/// Notes whose title or body contains `text` (all of them if it is empty), newest first.
pub fn search(conn: Conn, text: string) -> Result<[Note], string> {
    let rows = if (text.trim() == "") {
        sqlite.query(
            conn,
            "SELECT id, title, body, updated_ms FROM notes ORDER BY updated_ms DESC",
            []
        )?
    } else {
        let pattern = "%" + text.trim() + "%";
        sqlite.query(conn, `SELECT id, title, body, updated_ms FROM notes
             WHERE title LIKE ?1 OR body LIKE ?1 ORDER BY updated_ms DESC`, [pattern])?
    };
    Result.Ok(rows.map(from_row))
}
```

`db` returns every cell as text, so the date is converted with `parse_int()`. The rest of the
module (`save`, `remove`) has the same shape as in the mobile version, with `INSERT`, `UPDATE` and
`DELETE`.

## 2. Each system's data folder

A desktop app should not write into the user's Documents folder. Each system has its own place for
application data, and `platform()` tells you which one you are on:

<!-- check: project=examples/apps/notes-everywhere -->
```rust
/// The folder for the app's data on this platform. `NOTES_DIR` overrides it (tests, CI).
pub fn data_dir(app: string) -> string {
    match (env("NOTES_DIR")) {
        Option.Some(d) => return d,
        Option.None => { },
    }
    let home = env("HOME").unwrap_or(".");
    let os = platform();
    if (os == "macos") {
        return home + "/Library/Application Support/" + app;
    }
    if (os == "windows") {
        return env("APPDATA").unwrap_or(home) + "\\" + app;
    }
    if (os == "linux") {
        return match (env("XDG_DATA_HOME")) {
            Option.Some(d) => d + "/" + app,
            Option.None => home + "/.local/share/" + app,
        };
    }
    // iOS and Android
    home + "/Documents/" + app
}
```

| System | Folder |
|---|---|
| macOS | `~/Library/Application Support/Notes` |
| Linux | `$XDG_DATA_HOME/Notes`, or `~/.local/share/Notes` |
| Windows | `%APPDATA%\Notes` |
| iOS and Android | inside the app sandbox, as in the mobile chapter |

`fs.mkdir` creates any missing parent folders.

## 3. Native menus

On the desktop, an app without menus feels wrong: without the Edit menu, on macOS even copy and
paste stop working in text fields. Menus are declared as data, and only on the desktop:

<!-- check: project=examples/apps/notes-everywhere -->
```rust
// The menus exist only on the desktop; on a phone the page has its own buttons.
fn install_menus() -> Result<int, string> {
    ui.app_menu(APP, [ui.item("role:about", "", "")])?;
    ui.menu(
        "File",
        [
            ui.item("new", "New Note", "cmd+n"),
            ui.item("find", "Search", "cmd+f"),
            ui.separator(),
            ui.item("export", "Export…", "cmd+e")
        ]
    )?;
    ui.edit_menu(
        [
            ui.item("role:undo", "", ""),
            ui.item("role:redo", "", ""),
            ui.separator(),
            ui.item("role:cut", "", ""),
            ui.item("role:copy", "", ""),
            ui.item("role:paste", "", ""),
            ui.item("role:select_all", "", "")
        ]
    )
}
```

- Items tagged `role:` do what the system does (undo, cut, copy, paste, select all) and emit no
  events. `role:about` is the native macOS "About" item.
- The others emit a `"menu"` event with their tag. Shortcuts are written as `cmd+n`: on Windows
  `cmd` is Ctrl. On Linux the menu bar items respond to clicks but not yet to keyboard shortcuts.

When a menu event arrives, the program passes it to the page as a DOM event, and React listens:

<!-- check: project=examples/apps/notes-everywhere -->
```rust
// A menu command goes to the page as a DOM event; the React app listens for `ray-menu`.
fn tell_page(window: int, command: string) {
    let js = "window.dispatchEvent(new CustomEvent('ray-menu', {detail: '" + command + "'}))";
    let _ = ui.eval_js(window, js);
}
```

In the page, the command ends in the same functions the buttons call:

```ts
  // Commands from the native menu (desktop): the program dispatches a `ray-menu` event.
  useEffect(() => {
    const onMenu = (e: Event) => {
      const command = (e as CustomEvent<string>).detail
      if (command === 'new') open(empty)
      if (command === 'find') search.current?.focus()
      if (command === 'export') void doExport()
    }
    window.addEventListener('ray-menu', onMenu)
    return () => window.removeEventListener('ray-menu', onMenu)
  }, [doExport])
```

So the menus do not add a second path through the interface: "New note" does the same from the
menu, from the shortcut and from the button, and on the phone, where there are no menus, nothing
is missing.

`eval_js` runs the text it receives. Here the command is one of the tags the program itself
declared, so concatenating it is safe. Text that comes from the user is never concatenated into
JavaScript: it is passed as JSON, or returned as the answer to a request.

## 4. Native dialogs

Two operations need a system window, so the program handles them before passing the request to
`api.handle`. One function looks at what the page asks for and decides who answers:

<!-- check: project=examples/apps/notes-everywhere -->
```rust
// One request from the page. Most go to `api.handle`; the ones that need a window live here.
fn answer(conn: Conn, body: string) -> string {
    let op = match (json.parse(body)) {
        Result.Ok(j) => json.get_string(j, "op").unwrap_or(""),
        Result.Err(_) => "",
    };
    if (op == "hello") {
        return json.render(json.obj()
            .field("ok", true)
            .field("platform", platform())
            .field("desktop", places.is_desktop()));
    }
    if (op == "export") {
        return export(conn);
    }
    if (op == "delete" && !confirmed_delete(body)) {
        return json.render(json.obj().field("ok", false).field("error", ""));
    }
    api.handle(conn, body, time.now())
}
```

`api.handle` still knows nothing about windows, so its tests do not change. What depends on the
platform stays in `main.ray`, in a short function that reads top to bottom.

**Confirm before deleting.** On the desktop, a native dialog with its own buttons:

<!-- check: project=examples/apps/notes-everywhere -->
```rust
// The desktop asks before deleting with a native dialog; the phone page asks on its own.
fn confirmed_delete(body: string) -> bool {
    if (!places.is_desktop()) {
        return true;
    }
    let title = match (json.parse(body)) {
        Result.Ok(j) => json.get_string(j, "title").unwrap_or("this note"),
        Result.Err(_) => "this note",
    };
    ui.confirm("Delete “" + title + "”?", "This cannot be undone.", "Delete", "Cancel")
        .unwrap_or(false)
}
```

On the phone, the page asks for a second tap ("Tap again to delete"). The iOS webview does not show
`window.confirm()` on its own, so it is more reliable to solve it in the page itself.

**Export.** The "Export…" command opens the system save dialog and writes every note into one
Markdown file:

<!-- check: project=examples/apps/notes-everywhere -->
```rust
    match (ui.save_file_with(o)) {
        Result.Ok(Option.Some(path)) => match (fs.write_file(path, notes.to_markdown(list))) {
            Result.Ok(_) => json.render(json.obj().field("ok", true).field("exported", path)),
            Result.Err(e) => api.fail(e),
        },
        Result.Ok(Option.None) => json.render(json.obj().field("ok", true).field("exported", "")),
        Result.Err(e) => api.fail(e),
    }
}
```

An `Option.None` means the user cancelled the dialog: it is not an error.

## 5. The window

On the desktop the window has a minimum size, and `autosave` makes the system remember its size
and position between runs. On the phone those options do nothing: the page fills the screen.

<!-- check: project=examples/apps/notes-everywhere -->
```rust
    if (places.is_desktop()) {
        match (install_menus()) {
            Result.Err(e) => eprint("menus: " + e),
            Result.Ok(_) => { },
        }
    }
    // On the desktop: a minimum size, and the system remembers size and position ("main").
    var o = ui.options(900, 640);
    o.min_width = 560;
    o.min_height = 420;
    o.autosave = "main";
    let window = match (ui.open_with(APP, "app://index.html", o)) {
        Result.Err(e) => {
            eprint("ui: " + e);
            return 1;
        },
        Result.Ok(w) => w,
    };
```

## 6. One interface for every screen

The page asks the program which platform it runs on (`op: "hello"`) and uses that to decide what
to show: the "Export…" button only exists on the desktop. The layout is up to CSS:

- on the **phone**, one pane at a time: the list, or the editor while you edit;
- on **wide screens** (desktop or tablet), the list on the left and the editor on the right.

A media query on the width is enough; the React logic is the same in every case:

```css
@media (min-width: 720px) {
  .list-pane {
    flex: 0 0 300px;
    overflow-y: auto;
  }
  .phone.editing .list-pane {
    display: flex;
  }
}
```

The rule looks at the width, not the platform: a tablet, or a narrow desktop window, get the
layout that fits them. The code is in `frontend/src/App.tsx` and `frontend/src/index.css`.

## 7. Tests

The tests open a database in a temporary folder, so they never touch your data. One of them checks
that a search with quotes is not interpreted as SQL:

<!-- check: project=examples/apps/notes-everywhere -->
```rust
@test
fn quotes_in_a_search_are_data_not_sql() {
    let c = fresh("quotes");
    let _ = notes.save(c, "", "It's fine", "", 1000).unwrap();
    assert_eq(notes.search(c, "' OR 1=1 --").unwrap().len(), 0);
    assert_eq(notes.search(c, "It's").unwrap().len(), 1);
}
```

```sh
ray test
```

## 8. Packaging for each system

```sh
ray bundle                 # on macOS: Notes.app
ray bundle                 # on Linux: the binary and its Notes.desktop launcher
ray bundle                 # on Windows: Notes.exe, no console, with icon and shortcut
ray bundle --ios           # the Xcode project, as in the mobile chapter
ray bundle --android       # the Gradle project
```

`ray bundle` packages for the system it runs on: the `.exe` is built on Windows and the `.desktop`
on Linux. The usual setup is a CI matrix with one job per system.

| System | What it produces | Needed on the user's machine |
|---|---|---|
| macOS | `Notes.app` | nothing |
| Linux | a folder with the binary and `Notes.desktop` | GTK 3 and WebKitGTK |
| Windows | a folder with `Notes.exe`, no console, with its icon, and the `Notes.lnk` shortcut | the WebView2 Runtime, which Windows 11 already ships |
| iOS | the Xcode project `Notes-ios/` | |
| Android | the Gradle project `Notes-android/` | |

The program, SQLite and the frontend go inside the executable: there is no separate runtime to
install. The Notes macOS `.app` takes about 3 MB. A packaged app starts with the current directory
at `/`, which is why the data goes to the folder from section 2 and the resources are embedded.

To distribute the app beyond your machine, macOS asks you to sign and notarize it, and Windows
shows a SmartScreen warning if it is unsigned. The [Shipping](shipping.en.md) chapter covers it,
together with automatic updates.

## Next step

[**Windows in depth**](windows.en.md): everything else `std/ui` offers a desktop app, with a text
editor as the example.

<!-- sync: sha256:b5a37280a835 -->
