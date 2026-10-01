# Distribuir

Español · [English](distribuir.en.md)

Los capítulos anteriores construyen las apps. Este cubre cómo llegan a sus usuarios: empaquetar
para cada sistema, firmar, publicar en las tiendas móviles y, en escritorio, actualizarse solas de
forma segura. El ejemplo es la Notes del capítulo [multiplataforma](multiplataforma.md), que trae
el código de actualización.

| Destino | Qué se entrega | Cómo |
|---|---|---|
| macOS | `Notes.app` firmada y notarizada | `ray bundle` con `[app] sign` y `[app] notary` |
| Windows | `Notes.exe` firmado | `ray bundle` con `[app] sign` |
| Linux | el binario y su `.desktop` | `ray bundle` |
| iPhone | la app en App Store | `ray bundle --ios`, y Xcode para archivar y subir |
| Android | la app en Google Play | `ray bundle --android` y `gradle bundleRelease` |
| Servidor | un binario | `ray build --native --release` |

## 1. Empaquetar el escritorio

`ray bundle` compila el binario nativo en modo release, con los recursos embebidos, y lo deja en el
formato del sistema en el que se ejecuta. La identidad y el icono salen del `ray.toml`:

```toml
[app]
name = "Notes"
id = "dev.raylang.notes"
icon = "icon.png"          # opcional: el icono de la app en cada sistema
```

Como cada sistema se empaqueta en su propia máquina, lo habitual es una matriz de CI con un job por
sistema operativo.

## 2. Firmar

Sin firma, macOS pide al usuario que apruebe a mano una app descargada, y Windows muestra un aviso
de SmartScreen. Linux no exige firma.

**macOS** necesita un certificado «Developer ID Application» de una cuenta de desarrollador de
Apple, y un perfil de notarización creado una vez:

```sh
xcrun notarytool store-credentials notes-notary   # pide el Apple ID y una contraseña de app
```

```toml
[app]
sign = "Developer ID Application: Tu Nombre (TEAMID)"
notary = "notes-notary"
```

Con eso, `ray bundle` firma con *hardened runtime*, verifica la firma, envía la app a Apple, espera
la aprobación y grapa el ticket a la `.app`. Si la firma no verifica, sale con el código 74. Las
mismas opciones existen como `--sign`, `--notary` y `--entitlements`, y como las variables
`RAY_SIGN_IDENTITY` y `RAY_NOTARY_PROFILE` para CI.

**Windows** usa `signtool`: `[app] sign` es el sujeto del certificado instalado, o la ruta de un
archivo `.pfx` cuya contraseña va en `RAY_SIGN_PFX_PASSWORD`.

## 3. Actualizaciones automáticas en escritorio

El publicador deja tres archivos en una URL fija: `update.json` con la versión y los artefactos,
su firma `update.json.sig`, y un `.zip` por plataforma. La app descarga el manifiesto, comprueba la
firma con la clave pública que lleva dentro, y si hay una versión nueva la descarga, verifica su
SHA-256, reemplaza el bundle instalado y se reinicia.

**Una vez**, crea la clave de firma de la app:

```sh
ray keygen          # la semilla va a ~/.ray/keys/<app-id>.key; la pública, al ray.toml
```

La clave privada nunca sale de tu máquina o de los secretos de CI (`RAY_SIGNING_KEY`). La pública
queda en `[app] public_key`, y `ray bundle` la mete en el binario.

**En cada versión**, sube `version` en el `ray.toml` y publica:

```sh
ray release -o dist                       # el zip de esta plataforma + update.json firmado
ray release -o dist --publish --tag v0.2.0   # además lo sube a GitHub Releases
```

`ray release` conserva en `update.json` los artefactos de otras plataformas de la misma versión,
así que cada job de CI lo ejecuta sobre el mismo `dist/`. Con GitHub Releases, la URL fija del
manifiesto es `https://github.com/<org>/<repo>/releases/latest/download/update.json`.

**En la app**, Notes comprueba al arrancar, en una fibra para no retrasar la ventana:

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

- `update.check` falla si la firma no corresponde a la clave de la app: un manifiesto alterado o de
  otro publicador se rechaza.
- `update.download` verifica el tamaño y el SHA-256 antes de devolver el paquete.
- `update.apply` reemplaza el bundle de forma atómica; `update.relaunch` arranca la versión nueva y
  esta termina. Al arrancar, `update.cleanup` borra los restos de la versión anterior.
- `min_version` (`ray release --min-version`) obliga a pasar por una versión intermedia cuando un
  salto directo no es seguro.

### Ensayar sin publicar nada

Toda la cadena se puede ensayar en tu máquina con un servidor local. Con la app ya empaquetada en
su versión actual:

```sh
# 1. una versión nueva, publicada en una carpeta y servida en local
sed -i '' 's/^version = "0.1.0"/version = "0.2.0"/' ray.toml
ray release -o dist --base-url http://127.0.0.1:8765/
ray serve dist --port 8765

# 2. la app instalada (0.1.0) apuntando a ese servidor
NOTES_UPDATE_URL=http://127.0.0.1:8765/update.json ./Notes.app/Contents/MacOS/Notes
```

La app pregunta, se actualiza a 0.2.0 y se reinicia. Si alteras `update.json` sin volver a firmarlo,
la rechaza con «the manifest signature does not verify».

## 4. Las tiendas móviles

**iPhone.** `ray bundle --ios` genera el proyecto Xcode con tu equipo de desarrollo
(`[ios] development_team`, ver el [capítulo móvil](movil.md)). Para publicar, ábrelo en Xcode,
elige *Product → Archive* y sube el archivo a App Store Connect desde el *Organizer*. La firma de
distribución la gestiona Xcode con tu cuenta de desarrollador.

**Android.** El proyecto Gradle trae la firma de release preparada; solo falta el keystore. Créalo
una vez en la raíz del proyecto generado:

```sh
keytool -genkeypair -v -keystore release.jks -alias app -keyalg RSA -keysize 2048 -validity 10000
```

y escribe al lado `keystore.properties`:

```properties
storeFile=release.jks
storePassword=…
keyAlias=app
keyPassword=…
```

```sh
gradle bundleRelease      # app/build/outputs/bundle/release/app-release.aab, para Google Play
gradle assembleRelease    # app/build/outputs/apk/release/app-release.apk, para instalar directo
```

Los dos archivos sobreviven a regenerar el proyecto con `ray bundle --android`. No los subas al
control de versiones: con ese keystore se firman todas las versiones futuras de la app.

## 5. Servidores

Un servicio se entrega como un binario: `ray build --native --release`. Lleva dentro las plantillas
y los recursos embebidos, y se configura con variables de entorno, como en los capítulos de la
[API](api.md) y del [sitio con React](web-react.md).

- `--target <triple>` compila para otra plataforma, por ejemplo `x86_64-unknown-linux-gnu` desde
  un Mac, si la toolchain de Rust tiene ese target.
- `--without crypto,tls,sqlite,…` deja fuera lo que el servicio no usa, para un binario y una
  imagen de contenedor más pequeños.
- Detrás de un proxy como nginx o Caddy, que pone el HTTPS, el binario está listo para producción.

## Y ahora

Con esto termina el recorrido. Para cualquier función, la [referencia](../REFERENCE.md) tiene su
firma, y el [manual](../MANUAL.md) explica el lenguaje en detalle. La [portada](index.md) reúne todos
los capítulos.

## Más ejemplos: las apps de ray-language

Las apps de este handbook cubren los casos más comunes. La organización
[ray-language](https://github.com/ray-language) publica más apps escritas en raylang, con su código completo, para
otros casos de uso:

| Caso de uso | Apps |
|---|---|
| Escritorio y móvil | [ray808](https://github.com/ray-language/ray808) (caja de ritmos con React para escritorio, iOS y Android), [raydesk](https://github.com/ray-language/raydesk) (gestor de tareas), [raynote](https://github.com/ray-language/raynote) (bloc de notas con menús y diálogos nativos), [rayplay](https://github.com/ray-language/rayplay) (reproductor de audio en segundo plano), [raystage](https://github.com/ray-language/raystage) (tipos de ventana) |
| Servidores y redes | [raygate](https://github.com/ray-language/raygate) (API gateway), [raywatch](https://github.com/ray-language/raywatch) (monitor de servicios con panel SSE), [raystream](https://github.com/ray-language/raystream) (servidor de medios), [rayq](https://github.com/ray-language/rayq) (cola de mensajes con WAL), [raykv](https://github.com/ray-language/raykv) (servidor compatible con Redis), [raycall](https://github.com/ray-language/raycall) (microservicios con RPC y trazas), [raybot](https://github.com/ray-language/raybot) (bot por WebSocket), [raymail](https://github.com/ray-language/raymail) (cliente SMTP), [rayrelay](https://github.com/ray-language/rayrelay) (relay) |
| Seguridad y P2P | [msg](https://github.com/ray-language/msg) (chat P2P cifrado), [takeit](https://github.com/ray-language/takeit) (transferencia de archivos cifrada), [raypass](https://github.com/ray-language/raypass) (bóveda de secretos), [raysync](https://github.com/ray-language/raysync) (sincronización cifrada) |
| Web | [store](https://github.com/ray-language/store) (tienda con frontend Astro y backend en raylang), [raysite](https://github.com/ray-language/raysite) (generador de sitios estáticos) |
| Terminal | [raytop](https://github.com/ray-language/raytop) (visor de procesos), [raylogs](https://github.com/ray-language/raylogs) (analizador de logs), [raytetris](https://github.com/ray-language/raytetris), [rallyx](https://github.com/ray-language/rallyx) y [1942](https://github.com/ray-language/1942) (juegos a 30 fps) |

Los paquetes que usan están en el [índice de paquetes](https://github.com/ray-language/ray-index), y se buscan con
`ray search`.
