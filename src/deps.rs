//! Descarga de dependencias (M39c-2a).
//!
//! Una dependencia se declara en `ray.toml` como `nombre = "git+<URL>@<ref>"`. Este módulo la
//! **descarga** clonando el repositorio git en la caché `.ray-deps/<nombre>/`, donde el loader la
//! encuentra como cápsula (M39c-1). Un paquete publicable tiene su `mod.ray` en la **raíz** del
//! repositorio (su cara pública como cápsula).
//!
//! **Cómo se clona: se delega en el binario `git` del sistema** (`std::process::Command`). Es la
//! única forma sensata de hablar el protocolo de git (packfiles, smart-HTTP, resolución de refs…)
//! sin reimplementarlo a mano, y **no rompe la invariante cero-dependencias de Cargo**: `git` es
//! una dependencia del *entorno* de desarrollo, no una librería enlazada en el binario. (Igual que
//! *shelling out* nunca añade crates al árbol de compilación.)
//!
//! La verificación de integridad (lockfile + hashes de contenido, *supply-chain*) llega en M39c-2b.

use std::path::Path;
use std::process::Command;

use crate::manifest::Manifest;

/// Una especificación de dependencia git ya parseada: de dónde clonar y qué ref sacar.
#[derive(Debug, Clone, PartialEq)]
pub struct GitSpec {
    pub url: String,
    pub git_ref: String,
}

/// Parsea `git+<URL>@<ref>`. El prefijo `git+` marca el esquema (por ahora el único), y el
/// `@<ref>` es **obligatorio**: fijar una versión concreta (un tag o commit) es lo que hace el
/// build reproducible. Se parte por el **último** `@` para no romper URLs con `usuario@host`.
pub fn parse_spec(spec: &str) -> Result<GitSpec, String> {
    let sin_prefijo = spec.strip_prefix("git+").ok_or_else(|| {
        format!("spec de dependencia no soportada: '{spec}' (se esperaba 'git+<URL>@<ref>')")
    })?;
    let (url, git_ref) = sin_prefijo.rsplit_once('@').ok_or_else(|| {
        format!(
            "la dependencia '{spec}' no fija una versión (falta '@<tag>'); \
             una versión fija hace el build reproducible"
        )
    })?;
    if url.is_empty() || git_ref.is_empty() {
        return Err(format!("spec de dependencia mal formada: '{spec}'"));
    }
    Ok(GitSpec { url: url.to_string(), git_ref: git_ref.to_string() })
}

/// Clona `spec` en `dest` y hace checkout de su ref. Devuelve el **commit resuelto** (SHA, para el
/// lockfile de M39c-2b). Precondición: `dest` no existe (el llamador salta las ya descargadas).
pub fn fetch(name: &str, spec: &GitSpec, dest: &Path) -> Result<String, String> {
    if let Some(padre) = dest.parent() {
        let _ = std::fs::create_dir_all(padre); // asegura `.ray-deps/`
    }
    git(&["clone", "--quiet", &spec.url, &dest.to_string_lossy()], None)
        .map_err(|e| format!("no se pudo clonar la dependencia '{name}' ({}): {e}", spec.url))?;
    // Checkout de la ref fijada. Sirve para tags, ramas y SHAs (a diferencia de `clone --branch`).
    if let Err(e) = git(&["checkout", "--quiet", &spec.git_ref], Some(dest)) {
        let _ = std::fs::remove_dir_all(dest); // deja la caché limpia si la ref no existe
        return Err(format!("no se pudo hacer checkout de '{}' en '{name}': {e}", spec.git_ref));
    }
    rev_parse(dest)
}

/// El commit al que apunta HEAD en un clon (`git rev-parse HEAD`).
fn rev_parse(dest: &Path) -> Result<String, String> {
    git(&["rev-parse", "HEAD"], Some(dest)).map(|s| s.trim().to_string())
}

/// Corre `git [-C <cwd>] <args>`; devuelve su stdout, o el stderr como error si el estado no es 0.
fn git(args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut cmd = Command::new("git");
    if let Some(d) = cwd {
        cmd.arg("-C").arg(d);
    }
    cmd.args(args);
    let salida = cmd
        .output()
        .map_err(|e| format!("no se pudo ejecutar 'git': {e} (¿está instalado?)"))?;
    if salida.status.success() {
        Ok(String::from_utf8_lossy(&salida.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&salida.stderr).trim().to_string())
    }
}

/// Asegura que todas las dependencias del manifiesto estén en la caché `.ray-deps/`: descarga las
/// que **falten** (las presentes se dejan tal cual; su verificación por hash llega en M39c-2b).
/// Devuelve cuántas se descargaron en esta llamada (0 si estaban todas). Solo mira el disco → sin
/// red ni `git` cuando ya están cacheadas.
pub fn asegurar(manifest: &Manifest) -> Result<usize, String> {
    let cache = manifest.root.join(".ray-deps");
    let mut nuevas = 0;
    for (nombre, spec_raw) in &manifest.dependencies {
        let dest = cache.join(nombre);
        if dest.exists() {
            continue; // ya descargada
        }
        let spec = parse_spec(spec_raw)?;
        eprintln!("  descargando {nombre} ({}@{})", spec.url, spec.git_ref);
        fetch(nombre, &spec, &dest)?;
        nuevas += 1;
    }
    Ok(nuevas)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_spec_git() {
        let s = parse_spec("git+https://ejemplo/geo@v1.0").unwrap();
        assert_eq!(s.url, "https://ejemplo/geo");
        assert_eq!(s.git_ref, "v1.0");
    }

    #[test]
    fn spec_con_arroba_en_la_url() {
        // Se parte por el ÚLTIMO `@`: `usuario@host` no confunde a la ref.
        let s = parse_spec("git+ssh://git@host/geo@v2").unwrap();
        assert_eq!(s.url, "ssh://git@host/geo");
        assert_eq!(s.git_ref, "v2");
    }

    #[test]
    fn spec_errores() {
        assert!(parse_spec("https://x/geo@v1").unwrap_err().contains("git+"));
        assert!(parse_spec("git+https://x/geo").unwrap_err().contains("no fija una versión"));
        assert!(parse_spec("git+@v1").unwrap_err().contains("mal formada"));
    }
}
