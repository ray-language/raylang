//! El manifiesto de proyecto `ray.toml` (M39b).
//!
//! Un proyecto raylang es un directorio con un `ray.toml` en su raíz. El manifiesto declara
//! el paquete (`name`, `version`, `entry` opcional) y sus dependencias. Este módulo lo
//! **encuentra** (subiendo desde el directorio actual, como `cargo`/`git`) y lo **parsea**.
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
}

impl Manifest {
    /// Busca la raíz del proyecto que contiene `dir`: sube por los ancestros hasta hallar un
    /// `ray.toml`. Devuelve la ruta del `ray.toml`, o `None` si no hay proyecto por encima.
    pub fn find(dir: &Path) -> Option<PathBuf> {
        let mut actual = Some(dir);
        while let Some(d) = actual {
            let candidato = d.join("ray.toml");
            if candidato.is_file() {
                return Some(candidato);
            }
            actual = d.parent();
        }
        None
    }

    /// Carga el manifiesto del proyecto que contiene `dir` (subiendo). `Ok(None)` si no hay
    /// proyecto; `Err` si el `ray.toml` existe pero está mal formado o le falta algo.
    pub fn load(dir: &Path) -> Result<Option<Manifest>, String> {
        let Some(ruta) = Manifest::find(dir) else {
            return Ok(None);
        };
        let raiz = ruta.parent().unwrap_or(Path::new(".")).to_path_buf();
        let fuente = std::fs::read_to_string(&ruta)
            .map_err(|e| format!("no se pudo leer '{}': {e}", ruta.display()))?;
        parse(&fuente, raiz).map(Some)
    }

    /// La ruta absoluta del archivo de entrada (raíz + `entry`).
    pub fn entry_path(&self) -> PathBuf {
        self.root.join(&self.entry)
    }
}

/// Parsea el subconjunto de TOML que `ray.toml` usa. `root` es el directorio del manifiesto.
fn parse(src: &str, root: PathBuf) -> Result<Manifest, String> {
    let mut seccion = String::new();
    let mut name = None;
    let mut version = None;
    let mut entry = None;
    let mut dependencies = Vec::new();

    for (i, linea_cruda) in src.lines().enumerate() {
        let num = i + 1;
        // Quitar comentario (`#`) y espacios. No hay `#` dentro de las cadenas de un manifiesto.
        let linea = match linea_cruda.split_once('#') {
            Some((antes, _)) => antes,
            None => linea_cruda,
        }
        .trim();
        if linea.is_empty() {
            continue;
        }
        // Cabecera de sección `[tabla]`.
        if let Some(resto) = linea.strip_prefix('[') {
            let nombre = resto
                .strip_suffix(']')
                .ok_or_else(|| err(num, "cabecera de sección sin ']'"))?;
            seccion = nombre.trim().to_string();
            continue;
        }
        // Par `clave = valor`.
        let (clave, valor) = linea
            .split_once('=')
            .ok_or_else(|| err(num, "se esperaba 'clave = valor' o '[seccion]'"))?;
        let clave = clave.trim();
        let valor = desenrollar_cadena(valor.trim())
            .ok_or_else(|| err(num, "el valor debe ir entre comillas dobles"))?;
        match seccion.as_str() {
            "package" => match clave {
                "name" => name = Some(valor),
                "version" => version = Some(valor),
                "entry" => entry = Some(valor),
                _ => {} // claves desconocidas de [package] se ignoran (extensibilidad)
            },
            "dependencies" => dependencies.push((clave.to_string(), valor)),
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
    })
}

/// Desenrolla una cadena TOML `"..."` a su contenido. `None` si no está entre comillas.
/// (Subconjunto: sin escapes; ni las URLs ni los nombres los necesitan.)
fn desenrollar_cadena(s: &str) -> Option<String> {
    s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).map(str::to_string)
}

fn err(linea: usize, msg: &str) -> String {
    format!("ray.toml:{linea}: {msg}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsear(src: &str) -> Result<Manifest, String> {
        parse(src, PathBuf::from("/proj"))
    }

    #[test]
    fn manifiesto_minimo() {
        let m = parsear("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\n").unwrap();
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
        let m = parsear(src).unwrap();
        assert_eq!(m.entry, "src/app.ray");
        assert_eq!(m.dependencies.len(), 2);
        assert_eq!(m.dependencies[0], ("geo".into(), "git+https://ejemplo/geo@v1.0".into()));
    }

    #[test]
    fn errores_claros() {
        assert!(parsear("name = \"x\"\n").unwrap_err().contains("fuera de toda sección"));
        assert!(parsear("[package]\nname = x\n").unwrap_err().contains("comillas"));
        assert!(parsear("[package]\nname = \"x\"\n").unwrap_err().contains("falta 'version'"));
        assert!(parsear("[package\n").unwrap_err().contains("sin ']'"));
    }
}
