//! El manifiesto de proyecto `ray.toml` (M39b).
//!
//! Un proyecto raylang es un directorio con un `ray.toml` en su raíz. El manifiesto declara
//! el paquete (`name`, `version`, `entry` opcional), sus dependencias y, opcionalmente, el estilo
//! de indentación del formateador (`[fmt] indent_style`/`indent_size`, que `ray fmt` respeta como
//! *fallback* de `.editorconfig`). Este módulo lo **encuentra** (subiendo desde el directorio actual,
//! como `cargo`/`git`) y lo **parsea**.
//!
//! **El parser es un lector TOML mínimo en Rust**, no la librería `toml.ray` de M32.2: el CLI
//! necesita leer la config *antes* de ejecutar nada, así que arrancar el intérprete solo para
//! parsear un archivo de configuración sería circular. El subconjunto soportado —secciones
//! `[tabla]`, `clave = "cadena"`, comentarios `#`— es todo lo que `ray.toml` usa; nada de
//! tablas anidadas, arrays ni tipos no-string (las specs de dependencia son cadenas).
//!
//! Las **dependencias** se parsean pero aún no se resuelven (eso es M39c); un manifiesto con
//! dependencias no vacías produce un aviso claro al construir/ejecutar.

use std::path::{Path, PathBuf};

/// El manifiesto parseado de un proyecto.
/// Un valor de `[app.plist]` (M209): cadena o booleano.
#[derive(Debug, Clone, PartialEq)]
pub enum PlistValue {
    Str(String),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    /// M309 (findings #65): `[package] raylang = "1.27.12"` — la versión MÍNIMA del lenguaje que el
    /// proyecto exige. Un toolchain más viejo se niega; uno de desarrollo (`+dev.<sha>`) avisa de
    /// que lo que compila aquí puede no compilar con esa release.
    pub raylang: Option<String>,
    /// El archivo de entrada del programa, relativo a la raíz. Por defecto `src/main.ray`.
    pub entry: String,
    /// M268: `[package] description` — una línea que dice qué es el paquete; va al índice
    /// (`<nombre>.meta.toml`) al publicar y la muestra `ray search`.
    pub description: Option<String>,
    /// M268: `[package] keywords` — palabras por las que `ray search` encuentra el paquete
    /// (`["http", "sse"]` o `"http, sse"`).
    pub keywords: Vec<String>,
    /// `(nombre, spec)` de cada dependencia declarada (spec = URL git + tag, resuelto en M39c).
    pub dependencies: Vec<(String, String)>,
    /// El directorio que contiene el `ray.toml` (la raíz del proyecto).
    pub root: PathBuf,
    /// `[fmt] indent_style` — `"space"` o `"tab"`. Lo usa `ray fmt` para la sangría. `None` = no
    /// declarado (cae a `.editorconfig` o al canónico de 4 espacios).
    pub indent_style: Option<String>,
    /// `[fmt] indent_size` — nº de espacios por nivel (si `indent_style = "space"`). `None` = no declarado.
    pub indent_size: Option<usize>,
    /// `[registry] index` — el **índice de paquetes** para resolver dependencias por nombre (M51). Un
    /// directorio local (M51a) o una URL git del repo del índice (M51c). Relativo a la raíz del
    /// proyecto si no es absoluto. `None` = no declarado → se usa el índice **oficial** por defecto
    /// (M136, `deps::OFFICIAL_INDEX`); `Some("")` (`index = ""`) = opt-out explícito (sin índice,
    /// solo deps git/`path:`). Lo puede sobrescribir la variable de entorno `RAY_INDEX`.
    pub registry_index: Option<String>,
    /// `[registry] mirror` — **mirror de paquetes** (M90.1): un prefijo de URL que reescribe la URL
    /// git de cada paquete al descargarlo (`prefijo/<url-sin-esquema>`). NO es otro índice (mismo
    /// índice, otra URL de descarga); el hash publicado verifica igual. Si el mirror falla, se cae a
    /// la URL original. Lo puede sobrescribir la variable de entorno `RAY_MIRROR`.
    pub registry_mirror: Option<String>,
    /// `[native] without` — subsistemas con-crate (crypto/tls/sqlite/mimalloc/ahash) a EXCLUIR del binario nativo
    /// (`ray build --native`), como política estable del proyecto. Equivale a `--without` pero versionado
    /// con el repo (builds herméticos/policy). El flag `--without` de CLI se UNE a esta lista. Vacío = sin
    /// exclusión. Ver docs/transpilador-nativo.md §3.3.
    pub native_without: Vec<String>,
    /// M147: `[native] embed` — directorios de assets embebidos en el binario nativo (y el
    /// espacio de nombres de `std/embed` en todos los motores). Vacío si no hay `[native]`.
    pub native_embed: Vec<String>,
    /// `[dev] listen` — dirección `host:port` que `ray dev` **pre-abre y retiene** entre reinicios
    /// (socket-activation, M92.3): el hijo la ADOPTA en vez de re-bind → cero conexiones rechazadas. El
    /// flag `--port`/`--listen` de la CLI la sobrescribe. `None` = sin socket retenido (bind por reinicio).
    pub dev_listen: Option<String>,
    /// M263 (IDEAS §91): `[frontend]` — el frontend web del proyecto construido con un bundler
    /// externo (Vite, Parcel, …). `None` = sin sección. Ver `Frontend`.
    pub frontend: Option<Frontend>,
    /// M156: `[android] application_id` — el identificador de la app Android que `ray bundle
    /// --android` escribe en el build.gradle generado. `None` = `org.raylang.<name>`.
    pub android_application_id: Option<String>,
    /// M155: `[app] copyright` — la línea de copyright del panel About (el bundle la escribe
    /// como `NSHumanReadableCopyright` en el Info.plist del .app). `None` = sin copyright.
    pub app_copyright: Option<String>,
    /// M208 (feedback 25 de ray-remote): `[app] name` / `icon` / `id` — lo que `ray bundle` tomaba
    /// solo de la línea de órdenes (`--name`, `--icon`, `--id`) se declara UNA vez aquí; los flags
    /// siguen mandando si se dan. `icon` es relativo a la raíz del proyecto.
    pub app_name: Option<String>,
    pub app_icon: Option<String>,
    pub app_id: Option<String>,
    /// M247 (IDEAS §89): `[app] public_key` — la clave pública Ed25519 (hex) con la que la app
    /// verifica el manifiesto de actualización (`std/update`); `ray bundle`/`build --native` la
    /// hornean en el binario y `ray run` la lee de aquí. `None` = la app no verifica firmas.
    pub app_public_key: Option<String>,
    /// M249 (IDEAS §89): `[app] sign` — la identidad de firma del bundle: en macOS el nombre del
    /// certificado (`"Developer ID Application: Nombre (TEAM)"`), en Windows el sujeto del
    /// certificado o la ruta de un `.pfx` (contraseña en `RAY_SIGN_PFX_PASSWORD`). Sin ella, el
    /// `.app` va con firma ad-hoc y el `.exe` sin firmar. `RAY_SIGN_IDENTITY` / `--sign` la pisan.
    pub app_sign: Option<String>,
    /// M249: `[app] notary` — el perfil de keychain de `notarytool` (`xcrun notarytool
    /// store-credentials <perfil>`) con el que `ray bundle` notariza y grapa el `.app` firmado.
    /// `RAY_NOTARY_PROFILE` / `--notary` lo pisan.
    pub app_notary: Option<String>,
    /// M249: `[app] entitlements` — ruta (relativa al proyecto) de un plist de entitlements para
    /// la firma con hardened runtime; sin ella, uno vacío (lo que una app raylang necesita).
    pub app_entitlements: Option<String>,
    /// M209 (feedback 26 de ray-remote): `[app.plist]` — claves extra que `ray bundle` vuelca tal
    /// cual al `Info.plist` del `.app` (macOS), en orden de declaración. `"texto"` → `<string>`,
    /// `true`/`false` sin comillas → `<true/>`/`<false/>`. Sin ella, `ray bundle` no daba forma de
    /// declarar `NSLocalNetworkUsageDescription` y el rodeo era `plutil` + `codesign`.
    pub app_plist: Vec<(String, PlistValue)>,
    /// M155: `[app] description` — la descripción corta de la app (hoy la usa `ui.set_about`
    /// como referencia documental; el panel la recibe por código). `None` = sin descripción.
    pub app_description: Option<String>,
    /// M151 (raydesk #9): `[ios] development_team` — el team de firma de Apple que `ray bundle
    /// --ios` escribe en el `App.xcconfig` generado (con `CODE_SIGN_STYLE = Automatic`). Sin él,
    /// cada regeneración borraba el team elegido en Xcode. `None` = sin firma declarada (el
    /// bundle además PRESERVA la firma de un xcconfig existente).
    pub ios_development_team: Option<String>,
}

impl Manifest {
    /// Busca la raíz del proyecto que contiene `dir`: sube por los ancestros hasta hallar un
    /// `ray.toml`. Devuelve la ruta del `ray.toml`, o `None` si no hay proyecto por encima.
    pub fn find(dir: &Path) -> Option<PathBuf> {
        let mut current = Some(dir);
        while let Some(d) = current {
            let candidate = d.join("ray.toml");
            if candidate.is_file() {
                return Some(candidate);
            }
            current = d.parent();
        }
        None
    }

    /// Carga el manifiesto del proyecto que contiene `dir` (subiendo). `Ok(None)` si no hay
    /// proyecto; `Err` si el `ray.toml` existe pero está mal formado o le falta algo.
    pub fn load(dir: &Path) -> Result<Option<Manifest>, String> {
        let Some(path) = Manifest::find(dir) else {
            return Ok(None);
        };
        let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let source = std::fs::read_to_string(&path)
            .map_err(|e| format!("could not read '{}': {e}", path.display()))?;
        parse(&source, root).map(Some)
    }

    /// La ruta absoluta del archivo de entrada (raíz + `entry`).
    pub fn entry_path(&self) -> PathBuf {
        self.root.join(&self.entry)
    }
}

/// M263: la sección `[frontend]` del manifiesto — el contrato genérico con un bundler externo
/// (Vite, Parcel, Astro, …; el mismo que Tauri: comando dev + URL, comando build + carpeta):
///
/// ```toml
/// [frontend]
/// dev   = "npm --prefix frontend run dev -- --strictPort --port 5173 --clearScreen false"
/// url   = "http://localhost:5173"   # opcional: por defecto la URL de Vite
/// build = "npm --prefix frontend run build"
/// dist  = "frontend/dist"           # se embebe como [native] embed en el binario/bundle
/// ```
///
/// `ray dev` lanza `dev`, espera a que `url` responda y exporta `RAY_FRONTEND_URL` al programa
/// (`ui.app_url` / `app://` resuelven ahí); `ray build --native` y `ray bundle` corren `build`
/// y embeben `dist`. Cada comando corre por el shell del sistema en la raíz del proyecto.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Frontend {
    /// `[frontend] dev` — el comando del servidor de desarrollo. `None` = `ray dev` no lanza nada.
    pub dev: Option<String>,
    /// `[frontend] url` — la URL que sirve `dev`. Por defecto `http://localhost:5173` (Vite).
    pub url: String,
    /// `[frontend] build` — el comando que produce `dist`. `None` = no se corre nada.
    pub build: Option<String>,
    /// `[frontend] dist` — la carpeta construida (relativa a la raíz), embebida en el nativo y
    /// vigilada como asset bajo `ray dev`. `None` = nada que embeber.
    pub dist: Option<String>,
}

/// La URL por defecto de `[frontend] url`: la de Vite.
pub const DEFAULT_FRONTEND_URL: &str = "http://localhost:5173";

/// Parsea el subconjunto de TOML que `ray.toml` usa. `root` es el directorio del manifiesto.
fn parse(src: &str, root: PathBuf) -> Result<Manifest, String> {
    let mut section = String::new();
    let mut name = None;
    let mut version = None;
    let mut entry = None;
    let mut description = None;
    let mut keywords: Vec<String> = Vec::new();
    let mut dependencies = Vec::new();
    let mut indent_style = None;
    let mut indent_size = None;
    let mut registry_index = None;
    let mut registry_mirror = None;
    let mut native_without = Vec::new();
    let mut native_embed = Vec::new();
    let mut dev_listen = None;
    let mut frontend: Option<Frontend> = None;
    let mut ios_development_team = None;
    let mut app_copyright = None;
    let mut app_name = None;
    let mut app_icon = None;
    let mut app_id = None;
    let mut app_public_key = None;
    let mut app_sign = None;
    let mut app_notary = None;
    let mut app_entitlements = None;
    let mut app_plist: Vec<(String, PlistValue)> = Vec::new();
    let mut raylang_min: Option<String> = None;
    let mut android_application_id = None;
    let mut app_description = None;

    for (i, raw_line) in src.lines().enumerate() {
        let num = i + 1;
        // Quitar comentario (`#`) y espacios. No hay `#` dentro de las cadenas de un manifiesto.
        let line = match raw_line.split_once('#') {
            Some((before, _)) => before,
            None => raw_line,
        }
        .trim();
        if line.is_empty() {
            continue;
        }
        // Cabecera de sección `[tabla]`.
        if let Some(rest) = line.strip_prefix('[') {
            let name = rest
                .strip_suffix(']')
                .ok_or_else(|| err(num, "section header without ']'"))?;
            section = name.trim().to_string();
            continue;
        }
        // Par `clave = valor`.
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| err(num, "expected 'key = value' or '[section]'"))?;
        let key = key.trim();
        let value_raw = value.trim();
        // La mayoría de valores son cadenas `"..."`; `[fmt] indent_size` admite un entero sin comillas.
        let as_string = || unquote_string(value_raw)
            .ok_or_else(|| err(num, "the value must be in double quotes"));
        match section.as_str() {
            "package" => match key {
                "name" => name = Some(as_string()?),
                "raylang" => raylang_min = Some(as_string()?),
                "version" => version = Some(as_string()?),
                "entry" => entry = Some(as_string()?),
                // M268: metadatos de búsqueda del índice.
                "description" => description = Some(as_string()?),
                "keywords" => {
                    keywords = match parse_string_array(value_raw) {
                        Some(list) => list,
                        None => as_string()?.split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect(),
                    };
                }
                _ => {} // claves desconocidas de [package] se ignoran (extensibilidad)
            },
            "dependencies" => dependencies.push((key.to_string(), as_string()?)),
            "fmt" => match key {
                "indent_style" => indent_style = Some(as_string()?),
                // `indent_size = 2` (entero) o `"2"` (cadena); ambos se aceptan.
                "indent_size" => {
                    let s = unquote_string(value_raw).unwrap_or_else(|| value_raw.to_string());
                    indent_size = s.parse::<usize>().ok();
                }
                _ => {}
            },
            "registry" => match key {
                "index" => registry_index = Some(as_string()?),
                "mirror" => registry_mirror = Some(as_string()?),
                _ => {} // otras claves del registro se ignoran por ahora (extensibilidad)
            },
            "native" => match key {
                // `without = ["tls", "sqlite"]` — array de subsistemas a excluir del binario nativo.
                "without" => {
                    native_without = parse_string_array(value_raw)
                        .ok_or_else(|| err(num, "the value must be an array of strings, e.g. [\"tls\", \"sqlite\"]"))?;
                }
                // M147: `embed = ["assets"]` — directorios (relativos a la raíz) cuyo contenido
                // viaja DENTRO del binario nativo y define el espacio de nombres de std/embed.
                "embed" => {
                    native_embed = parse_string_array(value_raw)
                        .ok_or_else(|| err(num, "the value must be an array of strings, e.g. [\"assets\"]"))?;
                }
                _ => {} // otras claves de [native] se ignoran por ahora (extensibilidad)
            },
            "dev" => match key {
                // `listen = "127.0.0.1:8080"` — socket que `ray dev` retiene entre reinicios (M92.3).
                "listen" => dev_listen = Some(as_string()?),
                _ => {} // otras claves de [dev] se ignoran por ahora (extensibilidad)
            },
            "frontend" => {
                // M263: el bundler externo del frontend (Vite y compañía). La sección existe en
                // cuanto aparece; las claves que falten quedan en su default.
                let f = frontend.get_or_insert_with(|| Frontend { url: DEFAULT_FRONTEND_URL.to_string(), ..Frontend::default() });
                match key {
                    "dev" => f.dev = Some(as_string()?),
                    "url" => f.url = as_string()?.trim_end_matches('/').to_string(),
                    "build" => f.build = Some(as_string()?),
                    "dist" => f.dist = Some(as_string()?),
                    _ => {} // otras claves de [frontend] se ignoran por ahora (extensibilidad)
                }
            }
            "app" => match key {
                // M155: metadatos de la app para el panel About / el bundle.
                "copyright" => app_copyright = Some(as_string()?),
                "description" => app_description = Some(as_string()?),
                // M208: nombre/icono/id de la app para `ray bundle`.
                "name" => app_name = Some(as_string()?),
                "icon" => app_icon = Some(as_string()?),
                "id" => app_id = Some(as_string()?),
                // M247: clave pública Ed25519 (hex) del manifiesto de actualización.
                "public_key" => app_public_key = Some(as_string()?),
                // M249: firma y notarización del bundle.
                "sign" => app_sign = Some(as_string()?),
                "notary" => app_notary = Some(as_string()?),
                "entitlements" => app_entitlements = Some(as_string()?),
                _ => {} // otras claves de [app] se ignoran por ahora (extensibilidad)
            },
            "android" => {
                // M156: `application_id = "com.tuorg.app"` — el id del APK generado; otras
                // claves de [android] se ignoran por ahora (extensibilidad).
                if key == "application_id" {
                    android_application_id = Some(as_string()?);
                }
            }
            "ios" => {
                // M151: `development_team = "ABCDE12345"` — el team de firma para `ray bundle
                // --ios`; otras claves de [ios] se ignoran por ahora (extensibilidad).
                if key == "development_team" {
                    ios_development_team = Some(as_string()?);
                }
            }
            // M209: `[app.plist]` — cada clave va al Info.plist tal cual (cadena o bool).
            "app.plist" => {
                let value = match value_raw {
                    "true" => PlistValue::Bool(true),
                    "false" => PlistValue::Bool(false),
                    _ => PlistValue::Str(as_string()?),
                };
                app_plist.push((key.to_string(), value));
            }
            "" => return Err(err(num, "key outside any section (missing '[package]')")),
            _ => {} // otras secciones se ignoran por ahora
        }
    }

    Ok(Manifest {
        name: name.ok_or("ray.toml: missing 'name' in [package]")?,
        version: version.ok_or("ray.toml: missing 'version' in [package]")?,
        entry: entry.unwrap_or_else(|| "src/main.ray".to_string()),
        description,
        keywords,
        dependencies,
        root,
        indent_style,
        indent_size,
        registry_index,
        registry_mirror,
        native_without,
        native_embed,
        dev_listen,
        frontend,
        ios_development_team,
        android_application_id,
        app_copyright,
        app_name,
        app_icon,
        app_id,
        app_plist,
        raylang: raylang_min,
        app_description,
        app_public_key,
        app_sign,
        app_notary,
        app_entitlements,
    })
}

/// Parsea un array TOML simple de cadenas en una línea: `["a", "b", "c"]` → `["a","b","c"]`; `[]` → vacío.
/// `None` si no tiene la forma `[ … ]` o algún elemento no está entre comillas. (No admite arrays
/// multilínea ni comas finales — suficiente para `[native] without`, que es una lista corta y plana.)
fn parse_string_array(s: &str) -> Option<Vec<String>> {
    let inner = s.strip_prefix('[')?.strip_suffix(']')?.trim();
    if inner.is_empty() {
        return Some(Vec::new());
    }
    inner.split(',').map(|part| unquote_string(part.trim())).collect()
}

/// Inserta o actualiza `nombre = "<req>"` en la sección `[dependencies]` del fuente de un `ray.toml`
/// (para `ray add`, M51a). Si el nombre ya está, reemplaza su requisito; si no, lo añade al final de
/// la sección. Si no hay sección `[dependencies]`, la crea al final del archivo. Preserva el resto
/// (comentarios, otras secciones) — es una edición mínima línea a línea, no un reserializado.
pub fn upsert_dependency(src: &str, name: &str, req: &str) -> String {
    let new_line = format!("{name} = \"{req}\"");
    let mut lines: Vec<String> = src.lines().map(str::to_string).collect();
    // ¿Existe ya la sección [dependencies]? Localiza su rango [inicio+1, fin_exclusivo).
    let dep_header = lines.iter().position(|l| l.trim() == "[dependencies]");
    if let Some(start) = dep_header {
        // Fin de la sección: la siguiente cabecera `[...]`, o el final del archivo.
        let end = lines[start + 1..]
            .iter()
            .position(|l| l.trim().starts_with('['))
            .map(|off| start + 1 + off)
            .unwrap_or(lines.len());
        // ¿Ya existe una entrada para `name`? (clave antes del `=`, ignorando espacios).
        let existing = lines[start + 1..end].iter().position(|l| {
            l.split_once('=').is_some_and(|(k, _)| k.trim() == name)
        });
        match existing {
            Some(off) => lines[start + 1 + off] = new_line, // reemplaza el requisito
            None => {
                // Inserta tras la última línea no vacía de la sección (antes de los blancos finales
                // y de los COMENTARIOS que encabezan la tabla siguiente — M309, findings #42: `ray add
                // web` se colaba entre el comentario de `[frontend]` y su cabecera).
                let mut insert_at = end;
                while insert_at > start + 1
                    && (lines[insert_at - 1].trim().is_empty() || lines[insert_at - 1].trim().starts_with('#'))
                {
                    insert_at -= 1;
                }
                lines.insert(insert_at, new_line);
            }
        }
    } else {
        if !lines.is_empty() && !lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("[dependencies]".to_string());
        lines.push(new_line);
    }
    let mut out = lines.join("\n");
    if src.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Elimina la entrada `nombre = "…"` de la sección `[dependencies]` (para `ray remove`, M51f).
/// Devuelve el fuente editado, o `None` si el nombre no estaba declarado. Edición mínima línea a
/// línea, como `upsert_dependency` (preserva comentarios y el resto de secciones).
pub fn remove_dependency(src: &str, name: &str) -> Option<String> {
    let mut lines: Vec<String> = src.lines().map(str::to_string).collect();
    let start = lines.iter().position(|l| l.trim() == "[dependencies]")?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim().starts_with('['))
        .map(|off| start + 1 + off)
        .unwrap_or(lines.len());
    let off = lines[start + 1..end]
        .iter()
        .position(|l| l.split_once('=').is_some_and(|(k, _)| k.trim() == name))?;
    lines.remove(start + 1 + off);
    let mut out = lines.join("\n");
    if src.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

/// Desenrolla una cadena TOML `"..."` a su contenido. `None` si no está entre comillas.
/// (Subconjunto: sin escapes; ni las URLs ni los nombres los necesitan.)
fn unquote_string(s: &str) -> Option<String> {
    s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).map(str::to_string)
}

fn err(line: usize, msg: &str) -> String {
    format!("ray.toml:{line}: {msg}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_src(src: &str) -> Result<Manifest, String> {
        parse(src, PathBuf::from("/proj"))
    }

    /// M309 (findings #42): `ray add` no se cuela entre el comentario que encabeza la tabla
    /// siguiente y su cabecera; y `[package] raylang = "…"` se lee.
    #[test]
    fn upsert_keeps_the_next_tables_comment_with_its_header_and_raylang_key_parses() {
        let base = "[package]\nname = \"x\"\nversion = \"0.1.0\"\nraylang = \"1.27.12\"\n\n[dependencies]\nnet = \"^0.3\"\n\n# El frontend (Vite)\n[frontend]\ndev = \"npm run dev\"\n";
        let out = upsert_dependency(base, "web", "^0.4");
        assert!(out.contains("net = \"^0.3\"\nweb = \"^0.4\"\n\n# El frontend (Vite)\n[frontend]\n"), "{out}");
        let m = parse_src(base).unwrap();
        assert_eq!(m.raylang.as_deref(), Some("1.27.12"));
    }

    #[test]
    fn minimal_manifest() {
        let m = parse_src("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\n").unwrap();
        assert_eq!(m.name, "demo");
        assert_eq!(m.version, "0.1.0");
        assert_eq!(m.entry, "src/main.ray"); // por defecto
        assert!(m.dependencies.is_empty());
        assert!(m.native_without.is_empty()); // sin [native] → sin exclusión
        assert_eq!(m.entry_path(), PathBuf::from("/proj/src/main.ray"));
    }

    #[test]
    fn frontend_section_is_parsed_with_defaults() {
        // M263: [frontend] — comandos y carpeta del bundler externo; la URL default es la de Vite.
        let m = parse_src(
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[frontend]\ndev = \"npm run dev\"\nbuild = \"npm run build\"\ndist = \"frontend/dist\"\n",
        )
        .unwrap();
        let f = m.frontend.expect("sección [frontend]");
        assert_eq!(f.dev.as_deref(), Some("npm run dev"));
        assert_eq!(f.build.as_deref(), Some("npm run build"));
        assert_eq!(f.dist.as_deref(), Some("frontend/dist"));
        assert_eq!(f.url, DEFAULT_FRONTEND_URL);
        // `url` explícita: se normaliza sin la barra final (se concatenan rutas detrás).
        let m = parse_src("[package]\nname = \"d\"\nversion = \"0.1.0\"\n[frontend]\nurl = \"http://localhost:3000/\"\n").unwrap();
        let f = m.frontend.unwrap();
        assert_eq!(f.url, "http://localhost:3000");
        assert!(f.dev.is_none() && f.build.is_none() && f.dist.is_none());
        // Sin la sección, `None`.
        let m = parse_src("[package]\nname = \"d\"\nversion = \"0.1.0\"\n").unwrap();
        assert!(m.frontend.is_none());
    }

    #[test]
    fn app_metadata_is_parsed() {
        // M155: [app] copyright/description — el panel About y el Info.plist del bundle.
        let m = parse_src(
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[app]\ncopyright = \"(c) 2026 Demo\"\ndescription = \"A demo\"\n",
        )
        .unwrap();
        assert_eq!(m.app_copyright.as_deref(), Some("(c) 2026 Demo"));
        assert_eq!(m.app_description.as_deref(), Some("A demo"));
        let bare = parse_src("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
        assert!(bare.app_copyright.is_none() && bare.app_description.is_none());
        // M208: [app] name/icon/id — lo que antes solo entraba por --name/--icon/--id.
        let m = parse_src(
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[app]\nname = \"Demo App\"\nicon = \"assets/icon.png\"\nid = \"org.example.demo\"\n",
        )
        .unwrap();
        assert_eq!(m.app_name.as_deref(), Some("Demo App"));
        assert_eq!(m.app_icon.as_deref(), Some("assets/icon.png"));
        assert_eq!(m.app_id.as_deref(), Some("org.example.demo"));
        assert!(bare.app_name.is_none() && bare.app_icon.is_none() && bare.app_id.is_none());
        // M209: [app.plist] — claves extra del Info.plist, cadena o bool, en orden.
        let m = parse_src(
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[app.plist]\nNSLocalNetworkUsageDescription = \"Talks to devices nearby\"\nLSUIElement = true\n",
        )
        .unwrap();
        assert_eq!(
            m.app_plist,
            vec![
                ("NSLocalNetworkUsageDescription".to_string(), PlistValue::Str("Talks to devices nearby".to_string())),
                ("LSUIElement".to_string(), PlistValue::Bool(true)),
            ]
        );
        assert!(bare.app_plist.is_empty());
    }

    #[test]
    fn ios_development_team_is_parsed() {
        // M151 (raydesk #9): [ios] development_team — la firma que `ray bundle --ios` persiste.
        let m = parse_src(
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[ios]\ndevelopment_team = \"ABCDE12345\"\n",
        )
        .unwrap();
        assert_eq!(m.ios_development_team.as_deref(), Some("ABCDE12345"));
        let without = parse_src("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
        assert!(without.ios_development_team.is_none());
    }

    #[test]
    fn native_without_is_an_array_of_subsystems() {
        // [native] without = ["tls", "sqlite"] → la política estable de exclusión del binario nativo.
        let src = "[package]\nname = \"svc\"\nversion = \"1.0.0\"\n\n[native]\nwithout = [\"tls\", \"sqlite\"]\n";
        let m = parse_src(src).unwrap();
        assert_eq!(m.native_without, vec!["tls".to_string(), "sqlite".to_string()]);
        // Array vacío → sin exclusión (equivalente a no declararlo).
        let empty = parse_src("[package]\nname=\"x\"\nversion=\"1\"\n[native]\nwithout = []\n").unwrap();
        assert!(empty.native_without.is_empty());
        // Un valor mal formado (no-array) es un error claro, no un ignorado silencioso.
        let bad = parse_src("[package]\nname=\"x\"\nversion=\"1\"\n[native]\nwithout = \"tls\"\n");
        assert!(bad.is_err(), "un `without` no-array debe fallar: {bad:?}");
    }

    #[test]
    fn entry_y_dependencies_y_comments() {
        let src = "\
# mi project
[package]
name = \"app\"          # el name
version = \"1.2.3\"
entry = \"src/app.ray\"

[dependencies]
geo = \"git+https://ejemplo/geo@v1.0\"
util = \"git+https://ejemplo/util@v2.1\"
";
        let m = parse_src(src).unwrap();
        assert_eq!(m.entry, "src/app.ray");
        assert_eq!(m.dependencies.len(), 2);
        assert_eq!(m.dependencies[0], ("geo".into(), "git+https://ejemplo/geo@v1.0".into()));
    }

    #[test]
    fn upsert_añade_reemplaza_y_crea_seccion() {
        // Añade a una sección [dependencies] existente (vacía).
        let base = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n";
        let a = upsert_dependency(base, "geo", "^1.2");
        assert!(a.contains("[dependencies]\ngeo = \"^1.2\""), "añade a la sección:\n{a}");
        // Reemplaza el requisito de una dep existente, sin duplicar.
        let b = upsert_dependency(&a, "geo", "2.0.0");
        assert!(b.contains("geo = \"2.0.0\""), "reemplaza:\n{b}");
        assert!(!b.contains("^1.2"), "sin duplicate:\n{b}");
        assert_eq!(b.matches("geo =").count(), 1);
        // Crea la sección si no existe.
        let c = upsert_dependency("[package]\nname = \"x\"\nversion = \"0.1.0\"\n", "util", "1.0.0");
        assert!(c.contains("[dependencies]\nutil = \"1.0.0\""), "crea la sección:\n{c}");
        // No mete la dep en otra sección posterior.
        let d = upsert_dependency("[package]\nname=\"x\"\nversion=\"1\"\n\n[dependencies]\na = \"1.0.0\"\n\n[fmt]\nindent_size = 2\n", "b", "2.0.0");
        let deps_idx = d.find("[dependencies]").unwrap();
        let fmt_idx = d.find("[fmt]").unwrap();
        let b_idx = d.find("b = ").unwrap();
        assert!(deps_idx < b_idx && b_idx < fmt_idx, "b va inside de [dependencies], antes de [fmt]:\n{d}");
    }

    #[test]
    fn remove_removes_the_dep_and_preserves_the_rest() {
        // Quita solo la línea de la dep pedida, sin tocar otras secciones (M51f).
        let src = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"^1.2\"\nutil = \"1.0.0\"\n\n[fmt]\nindent_size = 2\n";
        let out = remove_dependency(src, "geo").unwrap();
        assert!(!out.contains("geo ="), "geo eliminada:\n{out}");
        assert!(out.contains("util = \"1.0.0\"") && out.contains("[fmt]"), "el rest intacto:\n{out}");
        // Un nombre no declarado devuelve None (y una clave igual en OTRA sección no cuenta).
        assert!(remove_dependency(src, "nada").is_none());
        assert!(remove_dependency("[fmt]\ngeo = \"x\"\n", "geo").is_none());
    }

    #[test]
    fn errors_claros() {
        assert!(parse_src("name = \"x\"\n").unwrap_err().contains("outside any section"));
        assert!(parse_src("[package]\nname = x\n").unwrap_err().contains("quotes"));
        assert!(parse_src("[package]\nname = \"x\"\n").unwrap_err().contains("missing 'version'"));
        assert!(parse_src("[package\n").unwrap_err().contains("without ']'"));
    }
}
