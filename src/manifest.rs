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
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    /// El archivo de entrada del programa, relativo a la raíz. Por defecto `src/main.ray`.
    pub entry: String,
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

/// Parsea el subconjunto de TOML que `ray.toml` usa. `root` es el directorio del manifiesto.
fn parse(src: &str, root: PathBuf) -> Result<Manifest, String> {
    let mut section = String::new();
    let mut name = None;
    let mut version = None;
    let mut entry = None;
    let mut dependencies = Vec::new();
    let mut indent_style = None;
    let mut indent_size = None;
    let mut registry_index = None;
    let mut registry_mirror = None;
    let mut native_without = Vec::new();
    let mut native_embed = Vec::new();
    let mut dev_listen = None;

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
                "version" => version = Some(as_string()?),
                "entry" => entry = Some(as_string()?),
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
            "" => return Err(err(num, "key outside any section (missing '[package]')")),
            _ => {} // otras secciones se ignoran por ahora
        }
    }

    Ok(Manifest {
        name: name.ok_or("ray.toml: missing 'name' in [package]")?,
        version: version.ok_or("ray.toml: missing 'version' in [package]")?,
        entry: entry.unwrap_or_else(|| "src/main.ray".to_string()),
        dependencies,
        root,
        indent_style,
        indent_size,
        registry_index,
        registry_mirror,
        native_without,
        native_embed,
        dev_listen,
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
                // Inserta tras la última línea no vacía de la sección (antes de los blancos finales).
                let mut insert_at = end;
                while insert_at > start + 1 && lines[insert_at - 1].trim().is_empty() {
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
