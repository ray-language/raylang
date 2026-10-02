# App móvil para iOS y Android

Español · [English](mobile.en.md)

En este capítulo construyes **Notes**, una app de notas que corre en el iPhone y en Android con
una interfaz en React + TypeScript y los datos guardados en el propio teléfono con `std/kv`. Es
un solo programa raylang: el mismo código abre una ventana en el escritorio mientras desarrollas
y se empaqueta como app nativa para cada teléfono.

El proyecto completo está en
[`examples/apps/notes-mobile`](../examples/apps/notes-mobile/), con sus tests. Cada bloque de
raylang de esta página está copiado de ese proyecto, y el CI comprueba que siga siéndolo.

## Qué necesitas

| Para | Hace falta |
|---|---|
| Cualquier destino | raylang y Node.js con npm, para el frontend |
| Compilar a nativo | una toolchain de Rust (`ray toolchain install` instala una privada) |
| iOS | un Mac con Xcode, y los targets `rustup target add aarch64-apple-ios aarch64-apple-ios-sim` |
| Android | el SDK y el NDK de Android (Android Studio los instala), Gradle 9 con JDK 17 o posterior, y `rustup target add aarch64-linux-android` (añade `x86_64-linux-android` si tu emulador es x86) |

Nada de eso hace falta para empezar: hasta la sección 7 la app se desarrolla y se prueba en el
escritorio, solo con raylang y Node.js.

## 1. Cómo está hecha una app móvil en raylang

Una app de raylang en el teléfono tiene dos partes que viven **en el mismo proceso**:

- **El shell** es una app nativa mínima que genera `ray bundle`: un proyecto Xcode en iOS y un
  proyecto Gradle en Android. Muestra un webview a pantalla completa y arranca tu programa.
- **Tu programa raylang**, compilado a código máquina como librería. Abre la interfaz con
  `ui.open`, responde a la página por un puente de mensajes y guarda los datos donde quiera.

La página no habla HTTP con el programa: llama a `window.ray.request(...)`, el mensaje llega al
programa como un evento, y la respuesta resuelve la Promise de la página. No hay servidor ni
puerto abierto, así que ninguna otra app del teléfono puede hablar con el tuyo.

```text
                         la app: un solo proceso
┌────────────────────┐   window.ray.request(…)   ┌────────────────────┐
│ webview del shell  │ ────────────────────────▶ │  programa raylang  │
│ React + TypeScript │ ◀──────────────────────── │  (código máquina)  │
└────────────────────┘     ui.reply_json(…)      └────────────────────┘
  página servida desde                             datos: std/kv,
  ray://app/ (embebida)                            archivos, red
```

El reparto de trabajo sale de ahí: la página dibuja y recoge lo que hace el usuario; el programa
guarda los datos, habla con la red y con el sistema. La página nunca toca el disco.

La interfaz se sirve desde dentro del binario con el esquema `ray://app/`. En desarrollo, en
cambio, la carga el servidor de Vite, con recarga en caliente.

## 2. Crear el proyecto

`ray new` trae una plantilla con frontend de Vite. Cualquier plantilla de `npm create vite` sirve;
aquí, React con TypeScript:

```sh
ray new notes-mobile --frontend react-ts
cd notes-mobile
npm create vite@latest frontend -- --template react-ts
npm --prefix frontend install
```

El `ray.toml` resultante declara el frontend, y le añadimos la identidad de la app:

```toml
[package]
name = "notes-mobile"
version = "0.1.0"

[app]
name = "Notes"               # el nombre bajo el icono
id = "dev.raylang.notes"     # el bundle id de iOS y el application id de Android

[dependencies]

[frontend]
dev = "npm --prefix frontend run dev -- --strictPort --port 5173 --clearScreen false"
url = "http://localhost:5173"
build = "npm --prefix frontend run build"
dist = "frontend/dist"
```

Con `[frontend]`, `ray dev` arranca Vite por ti, y `ray build --native` y `ray bundle` construyen
el frontend y meten `frontend/dist` dentro del binario.

## 3. El modelo y el almacén

Cada nota es un struct. `@derive(ToJson)` genera su `to_json()`, que sirve tanto para guardarla
como para enviarla a la página:

<!-- check: project=examples/apps/notes-mobile -->
```rust
/// A note as it is stored and as the page receives it.
@derive(ToJson)
pub struct Note {
    id: string,
    title: string,
    body: string,
    updated_ms: int,
}
```

Los datos van en `std/kv`, un almacén clave-valor en un solo archivo. Cada nota es una clave (su
id, un uuid v7, que se ordena por fecha de creación) y su JSON es el valor. `save()` escribe el
archivo de forma **atómica**: primero un temporal y luego un renombrado. Si el sistema mata la app
a mitad de escritura, el archivo anterior sigue intacto. En un teléfono eso no es un detalle: iOS
y Android cierran apps en segundo plano sin avisar.

<!-- check: project=examples/apps/notes-mobile -->
```rust
/// Creates a note (empty `id`) or replaces an existing one, and persists the store.
pub fn save(
    store: kv.Store,
    id: string,
    title: string,
    body: string,
    now_ms: int
) -> Result<Note, string> {
    let clean = title.trim();
    if (clean.len() == 0) {
        return Result.Err("a note needs a title");
    }
    let key = if (id == "") { uuid.uuid_v7_at(now_ms) } else { id };
    if (id != "" && store.get(key).is_none()) {
        return Result.Err("no note with id " + id);
    }
    let note = Note { id: key, title: clean, body: body, updated_ms: now_ms };
    store.set_string(key, note.to_json());
    store.save()?;
    Result.Ok(note)
}
```

Los errores son valores: una nota sin título devuelve `Err`, y `?` propaga el fallo de disco de
`store.save()` a quien llamó.

### Dónde se guardan los datos

En el teléfono no hay una carpeta de trabajo útil: el programa arranca con el directorio actual en
`/`. El sitio correcto depende del sistema, y `$HOME` lo resuelve en los dos:

| Sistema | `$HOME` es | Las notas quedan en | Para verlas |
|---|---|---|---|
| iOS | el contenedor de la app; su raíz es de solo lectura, `Documents/` se puede escribir | `<contenedor>/Documents/Notes` | `xcrun simctl get_app_container booted dev.raylang.notes data` da la ruta en el simulador |
| Android | la carpeta privada de la app, `files/` | `files/Documents/Notes` | `adb shell run-as dev.raylang.notes ls files/Documents/Notes` |
| Escritorio | la carpeta del usuario | `~/Documents/Notes` | el gestor de archivos |

Los datos de la carpeta privada se borran al desinstalar la app, y ninguna otra app puede leerlos.

<!-- check: project=examples/apps/notes-mobile -->
```rust
fn data_dir() -> string {
    match (env("NOTES_DIR")) {
        Option.Some(d) => d,
        Option.None => match (env("HOME")) {
            Option.Some(home) => home + "/Documents/Notes",
            Option.None => "notes-data",
        },
    }
}
```

`NOTES_DIR` permite cambiar la carpeta en tests o en CI sin tocar el código.

## 4. El protocolo con la página

Todas las peticiones pasan por una función: recibe el JSON que envió la página y devuelve el JSON
de la respuesta. Como no depende de ninguna ventana, se prueba con `ray test` como cualquier otra
función.

<!-- check: project=examples/apps/notes-mobile -->
```rust
/// Answers one request from the page. `now_ms` comes from the caller so tests are deterministic.
pub fn handle(store: kv.Store, body: string, now_ms: int) -> string {
    let req = match (json.parse(body)) {
        Result.Ok(j) => j,
        Result.Err(e) => return fail("bad request: " + e),
    };
    let op = json.get_string(req, "op").unwrap_or("");
    if (op == "list") {
        return ok(notes.list(store));
    }
    if (op == "save") {
        let id = json.get_string(req, "id").unwrap_or("");
        let title = json.get_string(req, "title").unwrap_or("");
        let text = json.get_string(req, "body").unwrap_or("");
        return match (notes.save(store, id, title, text, now_ms)) {
            Result.Ok(_) => ok(notes.list(store)),
            Result.Err(e) => fail(e),
        };
    }
    if (op == "delete") {
        let id = json.get_string(req, "id").unwrap_or("");
        return match (notes.remove(store, id)) {
            Result.Ok(_) => ok(notes.list(store)),
            Result.Err(e) => fail(e),
        };
    }
    fail("unknown op '" + op + "'")
}
```

Cada operación responde con la lista completa. Para una app de notas es la opción más simple: la
interfaz se repinta desde una sola fuente de verdad y no hay estado que reconciliar.

## 5. El programa

`main` abre el almacén, monta el frontend embebido, abre la ventana y atiende eventos. El bucle
recibe tres clases de evento:

<!-- check: project=examples/apps/notes-mobile -->
```rust
    while (true) {
        let e = match (ui.next_event()) {
            Result.Err(err) => {
                eprint("ui: " + err);
                return 1;
            },
            Result.Ok(e) => e,
        };
        // On the desktop, closing the window ends the app. On a phone it never arrives: the
        // system suspends or kills the process, which is why every change is already saved.
        if (e.kind == "closed" && e.window == window) {
            return 0;
        }
        if (e.kind == "lifecycle") {
            print("notes: app moved to the " + e.tag);
        }
        if (e.kind == "message") {
            match (ui.as_request(e)) {
                Option.Some(request) => {
                    let (id, body) = request;
                    let answer = api.handle(store, body, time.now());
                    let _ = ui.reply_json(e.window, id, answer);
                },
                Option.None => { },
            }
        }
    }
```

- `"message"` es una petición de la página. `ui.as_request` la decodifica y `ui.reply_json`
  responde: la Promise de la página resuelve con un objeto, no con texto.
- `"lifecycle"` avisa cuando la app pasa a segundo plano (`tag == "background"`) o vuelve
  (`"foreground"`). Notes no necesita hacer nada, porque cada cambio ya está guardado.
- `"closed"` solo llega en el escritorio. En el teléfono, el sistema suspende o mata el proceso
  sin cerrar ninguna ventana.

En el teléfono, `ui.open` no crea una ventana nueva: entrega la URL al webview del shell. Por eso
el mismo `main` sirve para el escritorio y para el móvil.

## 6. La interfaz en React

El frontend es una app de React normal. Todo lo que sabe de raylang está en un módulo pequeño que
envuelve `window.ray.request` con tipos:

```ts
async function call(request: Record<string, string>): Promise<Note[]> {
  if (!window.ray) {
    throw new Error('No raylang program behind this page: open it with `ray dev`.')
  }
  const answer = (await window.ray.request(request)) as Answer
  if (!answer.ok) {
    throw new Error(answer.error)
  }
  return answer.notes
}

export const listNotes = () => call({ op: 'list' })

export const saveNote = (id: string, title: string, body: string) =>
  call({ op: 'save', id, title, body })

export const deleteNote = (id: string) => call({ op: 'delete', id })
```

El webview inyecta `window.ray` en cualquier página que cargue, tanto el servidor de Vite en
desarrollo como el build embebido en la app. Por eso el mismo código funciona en los dos casos.
El componente principal llama a `listNotes()`, `saveNote()` y `deleteNote()` y repinta con la
lista que devuelven. Está en `frontend/src/App.tsx`.

Dos detalles de CSS importan en un teléfono:

- `viewport-fit=cover` en el `<meta name="viewport">` de `index.html`, junto con
  `env(safe-area-inset-top)` y `env(safe-area-inset-bottom)` en el CSS, mantiene el contenido
  lejos de la isla dinámica y del indicador de inicio del iPhone.
- Los campos de texto usan `font-size: 16px` o más (Notes usa 17): con menos, iOS hace zoom
  sobre el campo al enfocarlo y la página queda desplazada.

## 7. Probar en el escritorio

Antes de tocar un teléfono, la app entera corre en el Mac, Linux o Windows:

```sh
ray test      # el modelo y el protocolo, sin ventana
ray dev       # Vite + el programa en una ventana nativa
```

`ray dev` arranca el servidor de Vite, espera a que responda y abre la ventana sobre él. Si editas
un componente de React, la página se actualiza sin recargar. Si editas un `.ray`, se reinicia solo
el programa. Los tests de Notes crean un almacén en una carpeta temporal y fijan la hora, así que
son deterministas:

<!-- check: project=examples/apps/notes-mobile -->
```rust
@test
fn notes_survive_reopening_the_store() {
    let dir = fs.make_temp_dir("notes-reopen").unwrap();
    let s = kv.open(dir + "/notes.kv").unwrap();
    let _ = notes.save(s, "", "Persisted", "", 1000).unwrap();
    let again = kv.open(dir + "/notes.kv").unwrap();
    assert_eq(notes.list(again)[0].title, "Persisted");
}
```

## 8. iOS

```sh
ray bundle --ios
```

`ray bundle --ios` construye el frontend, compila el programa como librería estática para el
iPhone y el simulador, y genera un proyecto Xcode en `Notes-ios/`. Para el **simulador** no hace
falta firma:

```sh
cd Notes-ios
xcodebuild -project Notes.xcodeproj -target Notes -sdk iphonesimulator \
  -configuration Debug build CODE_SIGNING_ALLOWED=NO
xcrun simctl boot "iPhone 15 Pro"
xcrun simctl install booted build/Debug-iphonesimulator/Notes.app
xcrun simctl launch --console-pty booted dev.raylang.notes
```

Con `--console-pty`, lo que el programa imprime aparece en tu terminal: al arrancar verás
`notes: app moved to the foreground`. `--ios-target sim` compila solo la librería del simulador,
que es más rápido mientras no pruebes en un teléfono. El icono de la app sale de `[app] icon` en el
`ray.toml`, un PNG.

Para un **iPhone real**, declara tu equipo de desarrollo una vez en el `ray.toml` y abre el
proyecto en Xcode:

```toml
[ios]
development_team = "ABCDE12345"
```

Cada `ray bundle --ios` lo escribe en la configuración del proyecto. Elegir el equipo solo en
Xcode no basta, porque regenerar el bundle reescribe el proyecto. Si solo cambias el programa o el
frontend no hace falta regenerar: el comando que imprime `ray bundle` recompila la librería y deja
el proyecto Xcode como estaba.

## 9. Android

```sh
ray bundle --android
```

Genera un proyecto Gradle en `Notes-android/` con el programa compilado como `.so`. Con un
emulador o un teléfono conectado:

```sh
cd Notes-android
gradle assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n dev.raylang.notes/org.raylang.shell.MainActivity
adb logcat -s ray            # lo que imprime el programa
```

`--android-abi arm64|x86_64|all` elige la arquitectura: el emulador de un Mac con Apple Silicon es
arm64. Para **publicar**, crea un `release.jks` con `keytool` y un `keystore.properties` en la raíz
del proyecto generado; `gradle assembleRelease` produce el APK firmado. Los dos archivos
sobreviven a regenerar el bundle y las contraseñas nunca pasan por el `ray.toml`. El README que
genera el bundle trae el paso a paso.

En Android los datos quedan en la carpeta privada de la app. Puedes verlos con
`adb shell run-as dev.raylang.notes ls files/Documents/Notes`.

## 10. Iterar en el teléfono sin reinstalar

Recompilar e instalar la app por cada cambio es un ciclo de minutos. `ray dev --device` lo reduce
a segundos: instalas una vez una **app de desarrollo** y, desde entonces, cada vez que guardas un
`.ray` el programa nuevo se envía al teléfono y se reinicia allí.

```sh
ray bundle --ios --dev        # o --android --dev: la app de desarrollo, una sola vez
ray dev --device              # imprime un QR: escanéalo con la cámara del teléfono
```

La app de desarrollo se llama `Notes-dev` y convive con la app real. El código corre **en el
teléfono**, en su VM: archivos, red y permisos son los del dispositivo, y lo que imprime el
programa llega a tu terminal. Un cambio que no compila no se envía: el diagnóstico sale en la
terminal y el teléfono sigue con la versión anterior.

El enlace con el teléfono se guarda en `.ray-dev`, dentro del proyecto, para emparejar una sola
vez; ese archivo no va al control de versiones. En el escritorio, `ray dev-client <url> <carpeta>`
hace el papel del teléfono, útil para probar el flujo en CI.

Para iterar también la **interfaz** con recarga en caliente, el teléfono puede cargar el servidor
de Vite de tu Mac:

1. arranca Vite escuchando en la red: `npm --prefix frontend run dev -- --host 0.0.0.0`;
2. construye la app con `--devtools` (`ray bundle --ios --devtools`);
3. lánzala con `RAY_DEV_FRONTEND_URL=http://<ip-de-tu-mac>:5173` en el entorno. En Xcode es una
   variable del esquema; en Android, `adb reverse tcp:5173 tcp:5173` y `http://127.0.0.1:5173`.

Un build de release ignora esa variable siempre.

## 11. Lo que cambia respecto al escritorio

El mismo `main` corre en los tres sitios, pero el sistema que hay debajo no se comporta igual:

| | Escritorio | iOS | Android |
|---|---|---|---|
| La interfaz | una ventana nativa, con menús | el webview del shell, a pantalla completa | el webview del shell, a pantalla completa |
| Evento `closed` | al cerrar la ventana | nunca llega | nunca llega |
| Evento `lifecycle` | no | `background` y `foreground` | `background` y `foreground` |
| Directorio actual | el del lanzamiento | `/` | `/` |
| `std/process` | sí | no existe | no existe |
| La página se carga desde | `ray://app/…` | `ray://app/…` | `https://app.ray.invalid/…` |
| Lo que imprime el programa | la terminal | `simctl launch --console-pty`, o la consola de Xcode | `adb logcat -s ray` |
| Inspeccionar la página | `ray dev`, o `ray run --devtools` | Safari, menú Develop, con `--devtools` | `chrome://inspect`, con `--devtools` |

Lo que eso obliga a hacer:

- **Guarda cada cambio en cuanto ocurre.** El sistema suspende o mata la app sin avisar, así que no
  hay un momento «al salir» en el que guardar. Es lo que hace Notes.
- **No dependas del directorio actual.** Usa `$HOME` (sección 3) para los datos, y el frontend
  embebido o `std/embed` para los recursos.
- **Lo que deba seguir en segundo plano vive en el programa.** Los temporizadores de JavaScript se
  congelan cuando la app no está a la vista. Un reproductor de audio suena desde raylang, y
  `[ios] background_audio = true` y `[android] background_audio = true` lo mantienen sonando.
- **No escribas URLs absolutas a mano.** En Android la página se carga por
  `https://app.ray.invalid/…`, un alias de `ray://app/` que nunca sale a la red. Las rutas
  relativas y `fetch("/api/x")` funcionan igual en los tres sitios.

## 12. Opciones y configuración

Las opciones de `ray bundle` para el teléfono:

| Opción | Qué hace |
|---|---|
| `--ios` | genera el proyecto Xcode en `<Nombre>-ios/` |
| `--ios-target device\|sim\|both` | compila solo la librería del teléfono o la del simulador; por defecto, las dos |
| `--android` | genera el proyecto Gradle en `<Nombre>-android/` |
| `--android-abi arm64\|x86_64\|all` | la arquitectura del `.so`; por defecto, arm64 |
| `--dev` | la app de desarrollo, para `ray dev --device` |
| `--devtools` | permite inspeccionar el webview desde Safari o Chrome |
| `--without lista` | deja subsistemas fuera del binario, por ejemplo `--without audio,sqlite` |

Y lo que se declara en el `ray.toml`:

| Clave | Efecto |
|---|---|
| `[app] name` | el nombre bajo el icono |
| `[app] id` | el bundle id de iOS y, si no se indica otro, el application id de Android |
| `[app] icon` | un PNG: el icono de la app en los dos sistemas |
| `[app.plist]` | claves que van tal cual al `Info.plist` de iOS, por ejemplo el texto de un permiso |
| `[ios] development_team` | el equipo de desarrollo, para instalar en un iPhone real |
| `[ios] background_audio` | el audio sigue sonando con la app en segundo plano |
| `[android] application_id` | un application id distinto del de `[app] id` |
| `[android] background_audio` | lo mismo en Android, con un servicio en primer plano |
| `[frontend]` | los comandos y la carpeta del frontend (sección 2) |

## 13. Problemas frecuentes

| Síntoma | Causa y arreglo |
|---|---|
| `ray dev` termina con el código 73 | el puerto de Vite está ocupado por otro proceso, normalmente una sesión anterior; el mensaje dice cuál |
| Xcode pide elegir equipo después de cada `ray bundle --ios` | declara `[ios] development_team` en el `ray.toml`: regenerar el bundle reescribe el proyecto |
| El teléfono no conecta con `ray dev --device` | el enlace usa la red local: el teléfono y el ordenador deben estar en la misma red |
| La app de desarrollo muestra «not found» | era un fallo hasta la 1.27.26; actualiza raylang y regenera la app con `ray bundle --ios --dev` o `--android --dev` |
| En Android la app queda bajo la barra de estado, o el teclado tapa los campos | el shell reserva las barras y el teclado desde la 1.27.27; actualiza raylang y regenera el bundle |
| iOS hace zoom al tocar un campo de texto | el campo tiene menos de 16px de letra |
| El contenido queda bajo la isla dinámica | falta `viewport-fit=cover` o los `env(safe-area-inset-*)` (sección 6) |
| Un cambio en `[app]` no aparece en el teléfono | la identidad y el icono se escriben al generar el proyecto: vuelve a ejecutar `ray bundle` |

## Siguiente paso

La misma Notes, ahora también en **escritorio**: macOS, Linux y Windows con menús nativos,
diálogos y SQLite, en la [app multiplataforma](cross-platform.md).
