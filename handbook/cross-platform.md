# App multiplataforma

Español · [English](cross-platform.en.md)

Este capítulo continúa el de la [app móvil](mobile.md). La misma **Notes** corre ahora también como
app de escritorio en macOS, Linux y Windows, con un solo `src/` y una sola interfaz para las cinco
plataformas. En el camino cambia el almacén: las notas pasan de `std/kv` a **SQLite**, con
búsqueda, y se guardan en la carpeta que cada sistema espera.

El proyecto completo está en
[`examples/apps/notes-everywhere`](../examples/apps/notes-everywhere/), con sus tests. Como en el
capítulo anterior, cada bloque de raylang está copiado de ese proyecto y el CI comprueba que siga
siéndolo.

## Qué cambia respecto a la app móvil

| | App móvil | Multiplataforma |
|---|---|---|
| Plataformas | iOS y Android | macOS, Linux, Windows, iOS y Android |
| Datos | `std/kv`, un archivo clave-valor | SQLite con el paquete `db` |
| Carpeta de datos | `$HOME/Documents/Notes` | la de cada sistema (sección 2) |
| Escritorio | ventana sin más | menús nativos, atajos, diálogos, tamaño mínimo |
| Interfaz | una columna | una columna en el teléfono; lista y editor lado a lado en pantallas anchas |

El modelo de la app es el mismo: el programa raylang y el webview viven en un proceso, y la página
habla con el programa por `window.ray.request`.

## 1. SQLite con el paquete `db`

SQLite no viene en la biblioteca estándar: es parte del paquete `db`, publicado en el índice de
paquetes de raylang. Se añade con un comando:

```sh
ray add db
```

`ray add` busca la última versión, la descarga a `.ray-deps/` y la declara en el `ray.toml`:

```toml
[dependencies]
db = "^0.2.1"
```

La versión exacta y su hash quedan fijados en `ray.lock`, que va al control de versiones. El
paquete compila SQLite dentro del binario, así que no hace falta ninguna librería del sistema en
ninguna de las cinco plataformas.

Al abrir la base se crea el esquema si no existe. El modo WAL hace que una lectura nunca espere a
una escritura, y que un corte a mitad de escritura no corrompa el archivo:

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

Las consultas usan parámetros `?1`, `?2`, que se enlazan aparte del SQL. Por eso buscar con una
comilla no rompe nada: es dato, no código. La búsqueda es un `LIKE` sobre el título y el cuerpo:

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

`db` devuelve cada celda como texto, de modo que la fecha se convierte con `parse_int()`. El resto
del módulo (`save`, `remove`) sigue la misma forma que en la versión móvil, con `INSERT`, `UPDATE`
y `DELETE`.

## 2. La carpeta de datos de cada sistema

Una app de escritorio no debe escribir en la carpeta Documentos del usuario. Cada sistema tiene la
suya para datos de aplicaciones, y `platform()` dice en cuál estás:

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

| Sistema | Carpeta |
|---|---|
| macOS | `~/Library/Application Support/Notes` |
| Linux | `$XDG_DATA_HOME/Notes`, o `~/.local/share/Notes` |
| Windows | `%APPDATA%\Notes` |
| iOS y Android | dentro del sandbox de la app, como en el capítulo móvil |

`fs.mkdir` crea las carpetas intermedias que falten.

## 3. Menús nativos

En el escritorio, una app sin menús se siente extraña: sin el menú Edición, en macOS ni siquiera
funcionan copiar y pegar en los campos de texto. Los menús se declaran como datos, y solo en el
escritorio:

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

- Los elementos con etiqueta `role:` hacen lo que hace el sistema (deshacer, cortar, copiar, pegar,
  seleccionar todo) y no generan eventos. `role:about` es el «Acerca de» nativo de macOS.
- Los demás generan un evento `"menu"` con su etiqueta. Los atajos se escriben como `cmd+n`: en
  Windows `cmd` es Ctrl. En Linux los menús de la barra responden al clic pero todavía no a los
  atajos de teclado.

Cuando llega un evento de menú, el programa se lo pasa a la página como un evento del DOM, y React
lo escucha:

<!-- check: project=examples/apps/notes-everywhere -->
```rust
// A menu command goes to the page as a DOM event; the React app listens for `ray-menu`.
fn tell_page(window: int, command: string) {
    let js = "window.dispatchEvent(new CustomEvent('ray-menu', {detail: '" + command + "'}))";
    let _ = ui.eval_js(window, js);
}
```

En la página, un `useEffect` hace `window.addEventListener('ray-menu', …)` y abre una nota nueva,
pone el foco en la búsqueda o exporta, según el comando.

## 4. Diálogos nativos

Dos operaciones necesitan una ventana del sistema, así que el programa las atiende antes de pasar
la petición a `api.handle`.

**Confirmar antes de borrar.** En el escritorio, un diálogo nativo con botones propios:

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

En el teléfono, la página pide un segundo toque («Tap again to delete»). El webview de iOS no
muestra `window.confirm()` por su cuenta, así que es más fiable resolverlo en la propia página.

**Exportar.** El comando «Export…» abre el diálogo de guardado del sistema y escribe todas las
notas en un archivo Markdown:

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

Un `Option.None` significa que el usuario canceló el diálogo: no es un error.

## 5. La ventana

En el escritorio la ventana tiene un tamaño mínimo, y `autosave` hace que el sistema recuerde su
tamaño y su posición entre ejecuciones. En el teléfono esas opciones no hacen nada: la página ocupa
la pantalla entera.

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

## 6. Una interfaz para todas las pantallas

La página pregunta al programa en qué plataforma corre (`op: "hello"`), y con eso decide qué
mostrar: el botón «Export…» solo existe en el escritorio. El diseño lo decide el CSS:

- en el **teléfono**, un panel cada vez: la lista, o el editor mientras editas;
- en **pantallas anchas** (escritorio o tablet), la lista a la izquierda y el editor a la derecha.

Basta una media query sobre el ancho; la lógica de React es la misma en todos los casos. El código
está en `frontend/src/App.tsx` y `frontend/src/index.css`.

## 7. Tests

Los tests abren una base en una carpeta temporal, así que no tocan tus datos. Uno de ellos
comprueba que una búsqueda con comillas no se interpreta como SQL:

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

## 8. Empaquetar para cada sistema

```sh
ray bundle                 # en macOS: Notes.app
ray bundle                 # en Linux: el binario y su lanzador Notes.desktop
ray bundle                 # en Windows: Notes.exe, sin consola, con icono y acceso directo
ray bundle --ios           # el proyecto Xcode, como en el capítulo móvil
ray bundle --android       # el proyecto Gradle
```

`ray bundle` empaqueta para el sistema en el que se ejecuta: el `.exe` se construye en Windows y el
`.desktop` en Linux. Lo habitual es una matriz de CI con un job por sistema. Con SQLite dentro, el
`.app` de macOS de Notes ocupa unos 3 MB.

Para distribuir la app fuera de tu máquina, macOS pide firmarla y notarizarla, y Windows muestra un
aviso de SmartScreen si no está firmada. Lo explica el capítulo [Distribuir](shipping.md), junto
con las actualizaciones automáticas.

## Siguiente paso

[**Ventanas a fondo**](windows.md): todo lo demás que `std/ui` ofrece a una app de escritorio,
con un editor de texto de ejemplo.
