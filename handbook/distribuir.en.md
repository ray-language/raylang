# Shipping

[Español](distribuir.md) · English

The previous chapters build the apps. This one covers how they reach their users: packaging for
each system, signing, publishing to the mobile stores and, on the desktop, updating themselves
safely. The example is the Notes app from the [cross-platform](multiplataforma.en.md) chapter,
which carries the update code.

| Target | What ships | How |
|---|---|---|
| macOS | `Notes.app`, signed and notarized | `ray bundle` with `[app] sign` and `[app] notary` |
| Windows | a signed `Notes.exe` | `ray bundle` with `[app] sign` |
| Linux | the binary and its `.desktop` | `ray bundle` |
| iPhone | the app on the App Store | `ray bundle --ios`, and Xcode to archive and upload |
| Android | the app on Google Play | `ray bundle --android` and `gradle bundleRelease` |
| Server | one binary | `ray build --native --release` |

## 1. Packaging the desktop

`ray bundle` compiles the native binary in release mode, with the embedded resources, and leaves
it in the format of the system it runs on. The identity and the icon come from `ray.toml`:

```toml
[app]
name = "Notes"
id = "dev.raylang.notes"
icon = "icon.png"          # optional: the app icon on every system
```

Since each system is packaged on its own machine, the usual setup is a CI matrix with one job per
operating system.

## 2. Signing

Unsigned, macOS asks the user to approve a downloaded app by hand, and Windows shows a SmartScreen
warning. Linux requires no signature.

**macOS** needs a "Developer ID Application" certificate from an Apple developer account, and a
notarization profile created once:

```sh
xcrun notarytool store-credentials notes-notary   # asks for the Apple ID and an app password
```

```toml
[app]
sign = "Developer ID Application: Your Name (TEAMID)"
notary = "notes-notary"
```

With that, `ray bundle` signs with the hardened runtime, verifies the signature, sends the app to
Apple, waits for approval and staples the ticket to the `.app`. If the signature does not verify,
it exits with code 74. The same options exist as `--sign`, `--notary` and `--entitlements`, and as
the `RAY_SIGN_IDENTITY` and `RAY_NOTARY_PROFILE` variables for CI.

**Windows** uses `signtool`: `[app] sign` is the subject of the installed certificate, or the path
of a `.pfx` file whose password goes in `RAY_SIGN_PFX_PASSWORD`.

## 3. Automatic updates on the desktop

The publisher puts three files at a fixed URL: `update.json` with the version and the artifacts,
its signature `update.json.sig`, and a `.zip` per platform. The app downloads the manifest, checks
the signature with the public key it carries, and if there is a newer version it downloads it,
verifies its SHA-256, replaces the installed bundle and restarts.

**Once**, create the app's signing key:

```sh
ray keygen          # the seed goes to ~/.ray/keys/<app-id>.key; the public key, to ray.toml
```

The private key never leaves your machine or the CI secrets (`RAY_SIGNING_KEY`). The public key
stays in `[app] public_key`, and `ray bundle` puts it inside the binary.

**For each version**, raise `version` in `ray.toml` and publish:

```sh
ray release -o dist                       # this platform's zip + a signed update.json
ray release -o dist --publish --tag v0.2.0   # also uploads it to GitHub Releases
```

`ray release` keeps the other platforms' artifacts of the same version in `update.json`, so each
CI job runs it over the same `dist/`. With GitHub Releases, the fixed manifest URL is
`https://github.com/<org>/<repo>/releases/latest/download/update.json`.

**In the app**, Notes checks on start, in a fiber so the window does not wait:

<!-- check: project=examples/apps/notes-everywhere -->
```rust
    // Updates are a desktop concern (the stores update phone apps). In a fiber, so the window
    // opens at once; a new instance replaces this one when the user accepts.
    if (places.is_desktop()) {
        let _ = spawn(fn() {
            if (updates.offer()) {
                exit(0);
            }
        });
    }
```

<!-- check: project=examples/apps/notes-everywhere -->
```rust
/// Offers a newer version, if there is one. Returns true when a new instance was launched and
/// this one must exit.
pub fn offer() -> bool {
    let _ = update.cleanup();
    let url = manifest_url();
    if (url == "") {
        return false;
    }
    let release = match (update.check(url)) {
        Result.Ok(Option.Some(r)) => r,
        Result.Ok(Option.None) => return false,
        Result.Err(e) => {
            eprint(e);
            return false;
        },
    };
    let question = "You have "
        + update.current()
        + ". Install "
        + release.version
        + " and restart?";
    if (!ui.confirm("Notes " + release.version + " is available", question, "Update", "Later")
        .unwrap_or(false)) {
        return false;
    }
    let installed = match (update.download(release)) {
        Result.Ok(package) => update.apply(package),
        Result.Err(e) => Result.Err(e),
    };
    match (installed) {
        Result.Ok(_) => update.relaunch().is_ok(),
        Result.Err(e) => {
            eprint(e);
            false
        },
    }
}
```

- `update.check` fails if the signature does not match the app's key: a tampered manifest, or one
  from another publisher, is refused.
- `update.download` verifies the size and the SHA-256 before returning the package.
- `update.apply` replaces the bundle atomically; `update.relaunch` starts the new version and this
  one exits. On start, `update.cleanup` removes what the previous version left behind.
- `min_version` (`ray release --min-version`) forces going through an intermediate version when a
  direct jump is not safe.

### Rehearsing without publishing anything

The whole chain can be rehearsed on your machine with a local server. With the app already
packaged at its current version:

```sh
# 1. a new version, published into a folder and served locally
sed -i '' 's/^version = "0.1.0"/version = "0.2.0"/' ray.toml
ray release -o dist --base-url http://127.0.0.1:8765/
ray serve dist --port 8765

# 2. the installed app (0.1.0) pointed at that server
NOTES_UPDATE_URL=http://127.0.0.1:8765/update.json ./Notes.app/Contents/MacOS/Notes
```

The app asks, updates itself to 0.2.0 and restarts. If you change `update.json` without signing it
again, it refuses with "the manifest signature does not verify".

## 4. The mobile stores

**iPhone.** `ray bundle --ios` generates the Xcode project with your development team
(`[ios] development_team`, see the [mobile chapter](movil.en.md)). To publish, open it in Xcode,
choose *Product → Archive* and upload the archive to App Store Connect from the *Organizer*. Xcode
handles the distribution signing with your developer account.

**Android.** The Gradle project comes with release signing prepared; only the keystore is missing.
Create it once at the root of the generated project:

```sh
keytool -genkeypair -v -keystore release.jks -alias app -keyalg RSA -keysize 2048 -validity 10000
```

and write `keystore.properties` next to it:

```properties
storeFile=release.jks
storePassword=…
keyAlias=app
keyPassword=…
```

```sh
gradle bundleRelease      # app/build/outputs/bundle/release/app-release.aab, for Google Play
gradle assembleRelease    # app/build/outputs/apk/release/app-release.apk, to install directly
```

Both files survive regenerating the project with `ray bundle --android`. Keep them out of version
control: every future version of the app is signed with that keystore.

## 5. Servers

A service ships as one binary: `ray build --native --release`. It carries the templates and the
embedded resources, and it is configured with environment variables, as in the [API](api.en.md)
and [React site](web-react.en.md) chapters.

- `--target <triple>` builds for another platform, for example `x86_64-unknown-linux-gnu` from a
  Mac, if the Rust toolchain has that target.
- `--without crypto,tls,sqlite,…` leaves out what the service does not use, for a smaller binary
  and container image.
- Behind a proxy such as nginx or Caddy, which handles HTTPS, the binary is ready for production.

<!-- sync: sha256:85e12d3b726d -->

## What now

That is the end of the tour. For any function, the [reference](../REFERENCE.en.md) has its
signature, and the [manual](../MANUAL.md) (Spanish) explains the language in depth. The
[overview](index.en.md) gathers every chapter.

## More examples: the ray-language apps

The apps in this handbook cover the most common cases. The [ray-language](https://github.com/ray-language) organization
publishes more apps written in raylang, with their full source, for other use cases:

| Use case | Apps |
|---|---|
| Desktop and mobile | [ray808](https://github.com/ray-language/ray808) (drum machine with React for desktop, iOS and Android), [raydesk](https://github.com/ray-language/raydesk) (task manager), [raynote](https://github.com/ray-language/raynote) (notepad with native menus and dialogs), [rayplay](https://github.com/ray-language/rayplay) (background audio player), [raystage](https://github.com/ray-language/raystage) (window kinds) |
| Servers and networking | [raygate](https://github.com/ray-language/raygate) (API gateway), [raywatch](https://github.com/ray-language/raywatch) (service monitor with an SSE dashboard), [raystream](https://github.com/ray-language/raystream) (media server), [rayq](https://github.com/ray-language/rayq) (message queue with a WAL), [raykv](https://github.com/ray-language/raykv) (Redis-compatible server), [raycall](https://github.com/ray-language/raycall) (microservices with RPC and tracing), [raybot](https://github.com/ray-language/raybot) (WebSocket bot), [raymail](https://github.com/ray-language/raymail) (SMTP client), [rayrelay](https://github.com/ray-language/rayrelay) (relay) |
| Security and P2P | [msg](https://github.com/ray-language/msg) (encrypted P2P chat), [takeit](https://github.com/ray-language/takeit) (encrypted file transfer), [raypass](https://github.com/ray-language/raypass) (secrets vault), [raysync](https://github.com/ray-language/raysync) (encrypted sync) |
| Web | [store](https://github.com/ray-language/store) (store with an Astro frontend and a raylang backend), [raysite](https://github.com/ray-language/raysite) (static site generator) |
| Terminal | [raytop](https://github.com/ray-language/raytop) (process viewer), [raylogs](https://github.com/ray-language/raylogs) (log analyzer), [raytetris](https://github.com/ray-language/raytetris), [rallyx](https://github.com/ray-language/rallyx) and [1942](https://github.com/ray-language/1942) (games at 30 fps) |

The packages they use are in the [package index](https://github.com/ray-language/ray-index), and `ray search` finds them.
