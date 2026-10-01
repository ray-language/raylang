# Mobile app for iOS and Android

[Español](movil.md) · English

In this chapter you build **Notes**, a notes app that runs on the iPhone and on Android with a
React + TypeScript interface and its data stored on the phone itself with `std/kv`. It is a single
raylang program: the same code opens a desktop window while you develop and is packaged as a
native app for each phone.

The complete project is in
[`examples/apps/notes-mobile`](../examples/apps/notes-mobile/), with its tests. Every raylang
block on this page is copied from that project, and CI checks that it still is.

## What you need

- **raylang** and **Node.js** with npm, for the frontend.
- For **iOS**: a Mac with Xcode and the iPhone Rust targets:
  `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`.
- For **Android**: the Android SDK and NDK (Android Studio installs them), Gradle and the target
  `rustup target add aarch64-linux-android` (add `x86_64-linux-android` if your emulator is x86).

## 1. How a raylang mobile app is built

A raylang app on the phone has two parts that live **in the same process**:

- **The shell** is a minimal native app that `ray bundle` generates: an Xcode project on iOS and
  a Gradle project on Android. It shows a full-screen webview and starts your program.
- **Your raylang program**, compiled to machine code as a library. It opens the interface with
  `ui.open`, answers the page through a message bridge and stores its data wherever it wants.

The page does not talk HTTP to the program: it calls `window.ray.request(...)`, the message
reaches the program as an event, and the answer resolves the page's Promise. There is no server
and no open port, so no other app on the phone can talk to yours.

The interface is served from inside the binary through the `ray://app/` scheme. During
development, the Vite server loads it instead, with hot reload.

## 2. Create the project

`ray new` has a template with a Vite frontend. Any `npm create vite` template works; here, React
with TypeScript:

```sh
ray new notes-mobile --frontend react-ts
cd notes-mobile
npm create vite@latest frontend -- --template react-ts
npm --prefix frontend install
```

The resulting `ray.toml` declares the frontend, and we add the app's identity:

```toml
[package]
name = "notes-mobile"
version = "0.1.0"

[app]
name = "Notes"               # the name under the icon
id = "dev.raylang.notes"     # the iOS bundle id and the Android application id

[dependencies]

[frontend]
dev = "npm --prefix frontend run dev -- --strictPort --port 5173 --clearScreen false"
url = "http://localhost:5173"
build = "npm --prefix frontend run build"
dist = "frontend/dist"
```

With `[frontend]`, `ray dev` starts Vite for you, and `ray build --native` and `ray bundle` build
the frontend and put `frontend/dist` inside the binary.

## 3. The model and the store

Each note is a struct. `@derive(ToJson)` generates its `to_json()`, which serves both to store it
and to send it to the page:

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

The data goes into `std/kv`, a key-value store in a single file. Each note is one key (its id, a
uuid v7, which sorts by creation time) and its JSON is the value. `save()` writes the file
**atomically**: first a temporary file, then a rename. If the system kills the app halfway through
a write, the previous file stays intact. On a phone that is not a detail: iOS and Android close
background apps without warning.

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

Errors are values: a note without a title returns `Err`, and `?` propagates a disk failure from
`store.save()` to the caller.

### Where the data lives

On the phone there is no useful working directory: the program starts with the current directory
at `/`. The right place depends on the system, and `$HOME` resolves it on both:

- on **iOS**, `$HOME` is the app container. Its root is read-only, but `Documents/` is writable;
- on **Android**, the shell points `HOME` at the app's private folder (`files/`);
- on the **desktop**, `$HOME` is the user's home folder.

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

`NOTES_DIR` lets tests or CI change the folder without touching the code.

## 4. The protocol with the page

Every request goes through one function: it receives the JSON the page sent and returns the JSON
of the answer. Since it does not depend on any window, `ray test` tests it like any other function.

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

Every operation answers with the full list. For a notes app it is the simplest option: the
interface repaints from a single source of truth and there is no state to reconcile.

## 5. The program

`main` opens the store, mounts the embedded frontend, opens the window and handles events. The loop
receives three kinds of event:

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

- `"message"` is a request from the page. `ui.as_request` decodes it and `ui.reply_json` answers:
  the page's Promise resolves with an object, not with text.
- `"lifecycle"` tells you when the app goes to the background (`tag == "background"`) or comes back
  (`"foreground"`). Notes has nothing to do, because every change is already saved.
- `"closed"` only arrives on the desktop. On the phone, the system suspends or kills the process
  without closing any window.

On the phone, `ui.open` does not create a new window: it hands the URL to the shell's webview. That
is why the same `main` works on the desktop and on mobile.

## 6. The interface in React

The frontend is a normal React app. Everything it knows about raylang is in a small module that
wraps `window.ray.request` with types:

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

The webview injects `window.ray` into every page it loads, both the Vite server during development
and the build embedded in the app. That is why the same code works in both cases. The main
component calls `listNotes()`, `saveNote()` and `deleteNote()` and repaints with the list they
return. It is in `frontend/src/App.tsx`.

Two CSS details matter on a phone:

- `viewport-fit=cover` in the `<meta name="viewport">` of `index.html`, together with
  `env(safe-area-inset-top)` and `env(safe-area-inset-bottom)` in the CSS, keeps the content away
  from the iPhone's dynamic island and home indicator.
- Text fields use `font-size: 17px` or more: below that, iOS zooms in when they get focus.

## 7. Try it on the desktop

Before touching a phone, the whole app runs on the Mac, Linux or Windows:

```sh
ray test      # the model and the protocol, without a window
ray dev       # Vite + the program in a native window
```

`ray dev` starts the Vite server, waits until it answers and opens the window on it. If you edit a
React component, the page updates without reloading. If you edit a `.ray` file, only the program
restarts. The Notes tests create a store in a temporary folder and set the clock, so they are
deterministic:

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

`ray bundle --ios` builds the frontend, compiles the program as a static library for the iPhone
and the simulator, and generates an Xcode project in `Notes-ios/`. The **simulator** needs no
signing:

```sh
cd Notes-ios
xcodebuild -project Notes.xcodeproj -target Notes -sdk iphonesimulator \
  -configuration Debug build CODE_SIGNING_ALLOWED=NO
xcrun simctl boot "iPhone 15 Pro"
xcrun simctl install booted build/Debug-iphonesimulator/Notes.app
xcrun simctl launch --console-pty booted dev.raylang.notes
```

With `--console-pty`, what the program prints shows up in your terminal: on start you will see
`notes: app moved to the foreground`.

For a **real iPhone**, declare your development team once in `ray.toml` and open the project in
Xcode:

```toml
[ios]
development_team = "ABCDE12345"
```

Every `ray bundle --ios` writes it into the project configuration. Picking the team only in Xcode
is not enough, because regenerating the bundle rewrites the project. If you only change the
program or the frontend there is no need to regenerate: the command that `ray bundle` prints
rebuilds the library and leaves the Xcode project as it was.

## 9. Android

```sh
ray bundle --android
```

It generates a Gradle project in `Notes-android/` with the program compiled as a `.so`. With an
emulator or a phone connected:

```sh
cd Notes-android
gradle assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n dev.raylang.notes/org.raylang.shell.MainActivity
adb logcat -s ray            # what the program prints
```

`--android-abi arm64|x86_64|all` picks the architecture: the emulator on an Apple Silicon Mac is
arm64. To **publish**, create a `release.jks` with `keytool` and a `keystore.properties` at the root
of the generated project; `gradle assembleRelease` produces the signed APK. Both files survive
regenerating the bundle and the passwords never go through `ray.toml`. The README the bundle
generates has the step-by-step.

On Android the data stays in the app's private folder. You can see it with
`adb shell run-as dev.raylang.notes ls files/Documents/Notes`.

## 10. Iterate on the phone without reinstalling

Rebuilding and installing the app for every change takes minutes. `ray dev --device` brings it down
to seconds: you install a **development app** once and, from then on, every time you save a `.ray`
file the new program is sent to the phone and restarts there.

```sh
ray bundle --ios --dev        # or --android --dev: the development app, once
ray dev --device              # prints a QR code: scan it with the phone's camera
```

The development app is called `Notes-dev` and lives next to the real one. The code runs **on the
phone**, on its VM: files, network and permissions are the device's, and what the program prints
reaches your terminal. A change that does not compile is not sent: the diagnostic shows up in the
terminal and the phone keeps the previous version.

To iterate on the **interface** with hot reload too, the phone can load the Vite server on your
Mac:

1. start Vite listening on the network: `npm --prefix frontend run dev -- --host 0.0.0.0`;
2. build the app with `--devtools` (`ray bundle --ios --devtools`);
3. launch it with `RAY_DEV_FRONTEND_URL=http://<your-mac-ip>:5173` in the environment. In Xcode it
   is a scheme variable; on Android, `adb reverse tcp:5173 tcp:5173` and `http://127.0.0.1:5173`.

A release build always ignores that variable.

> [!NOTE]
> With 1.27.26, the development app cannot find the embedded frontend and shows "not found". It is
> fixed in the next version: update raylang and regenerate the development app with
> `ray bundle --ios --dev` or `--android --dev`.

## 11. What changes compared to the desktop

- **There is no `closed`.** The system suspends or kills the app without warning: save every change
  as soon as it happens, as Notes does.
- **The current directory is `/`.** Use `$HOME` (section 3) for data and `std/embed` or the
  embedded frontend for resources.
- **iOS does not allow launching processes:** `std/process` is not available there.
- **Background:** JavaScript timers freeze when the app is not visible. Whatever must keep running,
  like an audio player, lives in the raylang program; `[ios] background_audio = true` and
  `[android] background_audio = true` keep it playing.
- **Inspecting the page:** with `--devtools`, Safari (Develop menu) inspects the iPhone's webview,
  and `chrome://inspect` the Android one.
- **Android edge to edge:** since Android 15 the system draws the app under the bars. The shell from
  `ray bundle --android` reserves the system bars and the keyboard from the version after 1.27.26.
  If your app shows up under the status bar or the keyboard covers the fields, update raylang and
  regenerate the bundle.

## Next step

The same Notes, now on the **desktop**: macOS, Linux and Windows with native menus, dialogs and
SQLite. It is the next handbook chapter. Until it is published, the details are in the manual
(Spanish): [windows with `std/ui`](../MANUAL.md#ventanas-stdui) and
[packaging with `ray bundle`](../MANUAL.md#empaquetar-la-app-ray-bundle).

<!-- sync: sha256:dd01548155b6 -->
