# Distribuir

Español · [English](shipping.en.md)

Los capítulos anteriores construyen las apps. Este cubre cómo llegan a sus usuarios: empaquetar
para cada sistema, firmar, publicar en las tiendas móviles y, en escritorio, actualizarse solas de
forma segura. El ejemplo es la Notes del capítulo [multiplataforma](cross-platform.md), que trae
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

Todo lo que `ray bundle` necesita saber de la app está en la sección `[app]`:

| Clave | Para qué |
|---|---|
| `name` | el nombre visible de la app |
| `id` | su identificador, en notación de dominio inverso |
| `icon` | un PNG; `ray bundle` genera el formato de cada sistema |
| `copyright` | el texto del panel «Acerca de» en macOS y de las propiedades del `.exe` en Windows |
| `sign` | la identidad de firma (sección 2) |
| `notary` | el perfil de notarización de macOS (sección 2) |
| `entitlements` | un archivo de permisos para la firma de macOS |
| `public_key` | la clave pública de las actualizaciones (sección 3); la escribe `ray keygen` |

Además, `[app.plist]` añade claves al `Info.plist` de macOS, y `[native] embed` lista las carpetas
que van dentro del binario. Una app empaquetada arranca con el directorio actual en `/`, así que
todo recurso que lea debe ir embebido.

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

Para comprobar el resultado en macOS antes de publicarlo:

```sh
codesign --verify --deep --strict Notes.app && echo firmada
spctl --assess --type execute Notes.app && echo aceptada por Gatekeeper
```

En CI, las identidades y las claves llegan como secretos, por variables de entorno:

| Variable | Equivale a | Qué es |
|---|---|---|
| `RAY_SIGN_IDENTITY` | `[app] sign`, `--sign` | la identidad de firma |
| `RAY_NOTARY_PROFILE` | `[app] notary`, `--notary` | el perfil de notarización |
| `RAY_SIGN_PFX_PASSWORD` | | la contraseña del `.pfx` en Windows |
| `RAY_SIGNING_KEY` | `--key` | la clave privada de las actualizaciones (sección 3) |

## 3. Actualizaciones automáticas en escritorio

El publicador deja tres archivos en una URL fija: `update.json` con la versión y los artefactos,
su firma `update.json.sig`, y un `.zip` por plataforma. La app descarga el manifiesto, comprueba la
firma con la clave pública que lleva dentro, y si hay una versión nueva la descarga, verifica su
SHA-256, reemplaza el bundle instalado y se reinicia.

Este es un `update.json` real, tal como lo escribe `ray release`:

```json
{
  "app": "dev.raylang.notes",
  "version": "0.2.0",
  "notes": "https://example.com/notes/0.2.0",
  "min_version": "",
  "artifacts": {
    "macos-aarch64": {"url": "Notes-0.2.0-macos-aarch64.zip", "sha256": "2d2f6fd5ae6ce1aa33efb89c746162d9f16e23cefad6443cb023623d63937fe9", "size": 271727}
  }
}
```

Cada plataforma tiene su entrada en `artifacts`, con la clave `<sistema>-<arquitectura>`:
`macos-aarch64`, `linux-x86_64`, `windows-x86_64`. La seguridad no depende del servidor que aloja
los archivos, sino de tres comprobaciones:

| Comprobación | Qué impide |
|---|---|
| la firma de `update.json` con la clave de la app | que alguien publique un manifiesto falso, aunque controle el servidor |
| el `sha256` y el `size` de cada artefacto | que el `.zip` descargado sea distinto del publicado |
| la clave pública va dentro del binario instalado | que el atacante cambie también la clave con la que se verifica |

**Una vez**, crea la clave de firma de la app:

```sh
ray keygen          # la semilla va a ~/.ray/keys/<app-id>.key; la pública, al ray.toml
```

La clave privada nunca sale de tu máquina o de los secretos de CI (`RAY_SIGNING_KEY`). La pública
queda en `[app] public_key`, y `ray bundle` la mete en el binario.

Guarda una copia de esa clave en un gestor de secretos. Si se pierde, las apps ya instaladas no
pueden aceptar ninguna actualización más, porque solo confían en esa clave. Por eso `ray keygen`
se niega a sobrescribir una clave existente si no se le pasa `--force`.

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
(`[ios] development_team`, ver el [capítulo móvil](mobile.md)). Para publicar, ábrelo en Xcode,
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

En un contenedor, la imagen solo necesita el binario, compilado para Linux:

```dockerfile
FROM debian:stable-slim
COPY notes-api /usr/local/bin/notes-api
ENV HOST=0.0.0.0 PORT=8080
EXPOSE 8080
CMD ["notes-api"]
```

## 6. Antes de publicar

| Comprueba | Cómo |
|---|---|
| La versión está subida | `version` en el `ray.toml`: es la que comparan las actualizaciones |
| Los tests pasan también en nativo | `ray test --native` |
| La firma verifica | `codesign --verify` y `spctl --assess` en macOS |
| La actualización funciona | el ensayo en local de la sección 3, desde la versión anterior |
| Las claves tienen copia | la clave de actualizaciones y el keystore de Android: sin ellos no hay más versiones |
| Los secretos no están en el repositorio | `keystore.properties`, el `.pfx` y `~/.ray/keys/` quedan fuera |

## Siguiente paso

Con esto termina el recorrido por los capítulos. Para otros casos de uso,
[**Más ejemplos**](examples.md) reúne las apps de la organización ray-language, con su código
completo.
