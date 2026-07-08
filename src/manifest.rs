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
            .map_err(|e| format!("no se pudo leer '{}': {e}", path.display()))?;
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
                .ok_or_else(|| err(num, "cabecera de sección sin ']'"))?;
            section = name.trim().to_string();
            continue;
        }
        // Par `clave = valor`.
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| err(num, "se esperaba 'clave = valor' o '[seccion]'"))?;
        let key = key.trim();
        let value_raw = value.trim();
        // La mayoría de valores son cadenas `"..."`; `[fmt] indent_size` admite un entero sin comillas.
        let as_string = || unquote_string(value_raw)
            .ok_or_else(|| err(num, "el valor debe ir entre comillas dobles"));
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
            "" => return Err(err(num, "clave fuera de toda sección (falta '[package]')")),
            _ => {} // otras secciones se ignoran por ahora
        }
    }

    Ok(Manifest {
        name: name.ok_or("ray.toml: falta 'name' en [package]")?,
        version: version.ok_or("ray.toml: falta 'version' en [package]")?,
        entry: entry.unwrap_or_else(|| "src/main.ray".to_string()),
        dependencies,
        root,
        indent_style,
        indent_size,
    })
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
    fn manifiesto_minimo() {
        let m = parse_src("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\n").unwrap();
        assert_eq!(m.name, "demo");
        assert_eq!(m.version, "0.1.0");
        assert_eq!(m.entry, "src/main.ray"); // por defecto
        assert!(m.dependencies.is_empty());
        assert_eq!(m.entry_path(), PathBuf::from("/proj/src/main.ray"));
    }

    #[test]
    fn entry_y_dependencias_y_comentarios() {
        let src = "\
# mi proyecto
[package]
name = \"app\"          # el nombre
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
    fn errores_claros() {
        assert!(parse_src("name = \"x\"\n").unwrap_err().contains("fuera de toda sección"));
        assert!(parse_src("[package]\nname = x\n").unwrap_err().contains("comillas"));
        assert!(parse_src("[package]\nname = \"x\"\n").unwrap_err().contains("falta 'version'"));
        assert!(parse_src("[package\n").unwrap_err().contains("sin ']'"));
    }
}
