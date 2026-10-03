# Mobile app for iOS and Android

[Español](mobile.md) · English

In this chapter you build **Notes**, a notes app that runs on the iPhone and on Android with a
React + TypeScript interface and its data stored on the phone itself with `std/kv`. It is a single
raylang program: the same code opens a desktop window while you develop and is packaged as a
native app for each phone.

The complete project is in
[`examples/apps/notes-mobile`](../examples/apps/notes-mobile/), with its tests. Every raylang
block on this page is copied from that project, and CI checks that it still is.

## What you need

| For | You need |
|---|---|
| Any target | raylang and Node.js with npm, for the frontend |
| Compiling to native | a Rust toolchain (`ray toolchain install` installs a private one) |
| iOS | a Mac with Xcode, and the iOS targets: `ray toolchain add-target ios` |
| Android | the Android SDK and NDK (Android Studio installs them), Gradle 9 with JDK 17 or later, and the Android targets: `ray toolchain add-target android` |

`ray toolchain add-target` installs the Rust standard library for those targets into the toolchain
`ray` uses, whether your own or the private one `ray toolchain install` sets up (which is not on the
PATH, so a manual `rustup target add` does not reach it). Installing the private one from scratch is
one step: `ray toolchain install --targets ios,android`.

None of that is needed to start: up to section 7 the app is developed and tested on the desktop,
with just raylang and Node.js.

## 1. How a raylang mobile app is built

A raylang app on the phone has two parts that live **in the same process**:

- **The shell** is a minimal native app that `ray bundle` generates: an Xcode project on iOS and
  a Gradle project on Android. It shows a full-screen webview and starts your program.
- **Your raylang program**, compiled to machine code as a library. It opens the interface with
  `ui.open`, answers the page through a message bridge and stores its data wherever it wants.

The page does not talk HTTP to the program: it calls `window.ray.request(...)`, the message
reaches the program as an event, and the answer resolves the page's Promise. There is no server
and no open port, so no other app on the phone can talk to yours.

```text
                          the app: one process
┌────────────────────┐   window.ray.request(…)   ┌────────────────────┐
│ the shell's webview│ ────────────────────────▶ │  raylang program   │
│ React + TypeScript │ ◀──────────────────────── │  (machine code)    │
└────────────────────┘     ui.reply_json(…)      └────────────────────┘
  page served from                                 data: std/kv,
  ray://app/ (embedded)                            files, network
```

The split of work follows from that: the page draws and collects what the user does; the program
stores the data and talks to the network and the system. The page never touches the disk.

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

| System | `$HOME` is | The notes end up in | To look at them |
|---|---|---|---|
| iOS | the app container; its root is read-only, `Documents/` is writable | `<container>/Documents/Notes` | `xcrun simctl get_app_container booted dev.raylang.notes data` prints the path on the simulator |
| Android | the app's private folder, `files/` | `files/Documents/Notes` | `adb shell run-as dev.raylang.notes ls files/Documents/Notes` |
| Desktop | the user's home folder | `~/Documents/Notes` | the file manager |

Data in the private folder is deleted when the app is uninstalled, and no other app can read it.

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
- Text fields use `font-size: 16px` or more (Notes uses 17): below that, iOS zooms into the
  field when it gets focus and the page is left shifted.

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
`notes: app moved to the foreground`. `--ios-target sim` builds only the simulator library, which
is quicker while you are not testing on a phone. The app icon comes from `[app] icon` in
`ray.toml`, a PNG.

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

| Step | How often |
|---|---|
| `ray bundle --ios --dev` (or `--android --dev`) and install the resulting app | once per phone |
| `ray dev --device` and scan the QR code | once per project |
| Save a file | every change |

### Install the development app

```sh
ray bundle --ios --dev        # or --android --dev
```

The app is called `Notes-dev` and lives next to the real one. It does not carry your program: it
carries the raylang VM and a pairing screen. Install it like the normal app (Xcode or `adb
install`), and from then on you never touch it again, however much the code changes.

### Pair the phone with the QR code

```sh
ray dev --device
```

The terminal prints the three steps and a **QR code**. Open the phone's camera, point it at the
code and tap the link that appears: `Notes-dev` opens already paired and receives the program. You
do not open the app first or type anything; the QR encodes your Mac's address on the local network,
the port and a token.

If you open `Notes-dev` by hand without scanning, it shows a screen with the same three steps and
asks you to use the camera. Below, folded, there is a field to **paste the link** the terminal
prints under the QR, for when the camera cannot read it (a low-contrast terminal, an emulator
without a camera).

The pairing is remembered on both sides: the app keeps the link, and the project keeps the port and
the token in `.ray-dev` (a hidden file that `ray new` already leaves out of git). Next time, run
`ray dev --device` and open `Notes-dev`; the terminal says "Already paired". The QR is still
printed, to pair a second phone.

### Save and see the change

The code runs **on the phone**, on its VM: files, network and permissions are the device's, and what
the program prints reaches your terminal. Every file you save is sent and the program restarts in
about a second. A change that does not compile is not sent: the diagnostic shows up in the terminal,
in amber, and the phone keeps the previous version.

On the desktop, `ray dev-client <url> <folder>` plays the phone's part, useful for testing the flow
in CI; the terminal prints the exact command at the foot of the banner.

### If something does not connect

- **The phone opens nothing when scanning.** Phone and Mac must be on the same Wi-Fi network, and
  the link is for *this* project's development app: `Notes-dev`, not another one.
- **"The computer stopped answering".** The app kept a link from an earlier session and `ray dev
  --device` is no longer running, or runs on another port (if the one in `.ray-dev` was busy, the
  terminal warns and asks you to pair again). Scan the new QR.
- **Version warning.** If the development app comes from a different raylang version than your
  Mac's, the terminal says so; the phone's toolchain compiles the program. Reinstall the app after
  updating raylang.

### The interface with hot reload

To iterate on the **interface** with hot reload too, the phone can load the Vite server on your
Mac:

1. start Vite listening on the network: `npm --prefix frontend run dev -- --host 0.0.0.0`;
2. build the app with `--devtools` (`ray bundle --ios --devtools`);
3. launch it with `RAY_DEV_FRONTEND_URL=http://<your-mac-ip>:5173` in the environment. In Xcode it
   is a scheme variable; on Android, `adb reverse tcp:5173 tcp:5173` and `http://127.0.0.1:5173`.

A release build always ignores that variable.

## 11. What changes compared to the desktop

The same `main` runs in all three places, but the system underneath does not behave the same:

| | Desktop | iOS | Android |
|---|---|---|---|
| The interface | a native window, with menus | the shell's webview, full screen | the shell's webview, full screen |
| `closed` event | when the window closes | never arrives | never arrives |
| `lifecycle` event | no | `background` and `foreground` | `background` and `foreground` |
| Current directory | where it was launched | `/` | `/` |
| `std/process` | yes | does not exist | does not exist |
| The page loads from | `ray://app/…` | `ray://app/…` | `https://app.ray.invalid/…` |
| What the program prints | the terminal | `simctl launch --console-pty`, or the Xcode console | `adb logcat -s ray` |
| Inspecting the page | `ray dev`, or `ray run --devtools` | Safari, Develop menu, with `--devtools` | `chrome://inspect`, with `--devtools` |

What that forces you to do:

- **Save every change as soon as it happens.** The system suspends or kills the app without
  warning, so there is no "on exit" moment to save in. That is what Notes does.
- **Do not depend on the current directory.** Use `$HOME` (section 3) for data, and the embedded
  frontend or `std/embed` for resources.
- **Whatever must keep running in the background lives in the program.** JavaScript timers freeze
  when the app is not visible. An audio player plays from raylang, and
  `[ios] background_audio = true` and `[android] background_audio = true` keep it playing.
- **Do not write absolute URLs by hand.** On Android the page loads through
  `https://app.ray.invalid/…`, an alias of `ray://app/` that never reaches the network. Relative
  URLs and `fetch("/api/x")` work the same in all three places.

## 12. Options and configuration

The `ray bundle` options for the phone:

| Option | What it does |
|---|---|
| `--ios` | generates the Xcode project in `<Name>-ios/` |
| `--ios-target device\|sim\|both` | builds only the phone library or the simulator one; both by default |
| `--android` | generates the Gradle project in `<Name>-android/` |
| `--android-abi arm64\|x86_64\|all` | the architecture of the `.so`; arm64 by default |
| `--dev` | the development app, for `ray dev --device` |
| `--devtools` | allows inspecting the webview from Safari or Chrome |
| `--without list` | leaves subsystems out of the binary, for example `--without audio,sqlite` |

And what is declared in `ray.toml`:

| Key | Effect |
|---|---|
| `[app] name` | the name under the icon |
| `[app] id` | the iOS bundle id and, unless another is given, the Android application id |
| `[app] icon` | a PNG: the app icon on both systems |
| `[app.plist]` | keys that go as they are into the iOS `Info.plist`, for example the text of a permission |
| `[ios] development_team` | the development team, to install on a real iPhone |
| `[ios] background_audio` | audio keeps playing with the app in the background |
| `[android] application_id` | an application id different from `[app] id` |
| `[android] background_audio` | the same on Android, with a foreground service |
| `[frontend]` | the frontend's commands and folder (section 2) |

## 13. Common problems

| Symptom | Cause and fix |
|---|---|
| `ray dev` exits with code 73 | the Vite port is taken by another process, usually an earlier session; the message says which |
| Xcode asks for a team after every `ray bundle --ios` | declare `[ios] development_team` in `ray.toml`: regenerating the bundle rewrites the project |
| The phone does not connect to `ray dev --device` | the link uses the local network: the phone and the computer must be on the same network |
| The development app shows "not found" | it was a bug up to 1.27.26; update raylang and regenerate the app with `ray bundle --ios --dev` or `--android --dev` |
| On Android the app sits under the status bar, or the keyboard covers the fields | the shell reserves the bars and the keyboard since 1.27.27; update raylang and regenerate the bundle |
| iOS zooms in when a text field is tapped | the field's font is smaller than 16px |
| The content sits under the dynamic island | `viewport-fit=cover` or the `env(safe-area-inset-*)` values are missing (section 6) |
| A change in `[app]` does not show up on the phone | the identity and the icon are written when the project is generated: run `ray bundle` again |

## Next step

The same Notes, now also on the **desktop**: macOS, Linux and Windows with native menus, dialogs
and SQLite, in the [cross-platform app](cross-platform.en.md).

<!-- sync: sha256:f78382ac02b9 -->
