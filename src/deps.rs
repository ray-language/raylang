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

/// Asegura que todas las dependencias del manifiesto estén en la caché `.ray-deps/` y **verifica su
/// integridad** contra el lockfile `ray.lock` (M39c-2b): descarga las que falten, y para cada una
/// **recomputa su hash de contenido** y lo compara con el bloqueado — un desajuste (una dependencia
/// cacheada modificada) es error de *supply-chain*. Actualiza `ray.lock`. Devuelve cuántas se
/// descargaron en esta llamada (0 si estaban todas y verifican).
pub fn asegurar(manifest: &Manifest) -> Result<usize, String> {
    let cache = manifest.root.join(".ray-deps");
    let bloqueadas = leer_lock(&manifest.root)?; // nombre → entrada bloqueada
    let mut nuevo_lock: Vec<LockEntry> = Vec::new();
    let mut nuevas = 0;

    for (nombre, spec_raw) in &manifest.dependencies {
        let spec = parse_spec(spec_raw)?;
        let dest = cache.join(nombre);

        // Descargar si falta. Para una dependencia ya cacheada, se lee su commit (si es un repo
        // git; una colocada a mano —M39c-1— no lo es → commit vacío, el hash igual la verifica).
        let commit = if dest.exists() {
            rev_parse(&dest).unwrap_or_default()
        } else {
            eprintln!("  descargando {nombre} ({}@{})", spec.url, spec.git_ref);
            nuevas += 1;
            fetch(nombre, &spec, &dest)?
        };

        // Hash de contenido de lo que hay ahora en la caché.
        let hash = hash_package(&dest)?;

        // Verificación: si estaba bloqueada con el MISMO spec, el hash debe coincidir.
        if let Some(b) = bloqueadas.get(nombre)
            && b.url == spec.url
            && b.git_ref == spec.git_ref
            && b.hash != hash
        {
            return Err(format!(
                "la dependencia '{nombre}' no coincide con 'ray.lock': su contenido cambió desde que \
                 se bloqueó (posible manipulación).\n  esperado: {}\n  actual:   {}\n  Si el cambio es \
                 legítimo, borra '.ray-deps/{nombre}' y 'ray.lock' y vuelve a resolver.",
                b.hash, hash
            ));
        }

        nuevo_lock.push(LockEntry {
            name: nombre.clone(),
            url: spec.url,
            git_ref: spec.git_ref,
            commit,
            hash,
        });
    }

    escribir_lock(&manifest.root, &mut nuevo_lock)?;
    Ok(nuevas)
}

// ── Hash de contenido de un paquete ──────────────────────────────────────────────────

/// El **hash de contenido** de un paquete descargado en `dir`: un SHA-256 sobre el resumen de
/// `ruta_relativa:sha256(contenido)` de cada archivo (ordenados por ruta) — un árbol de hashes tipo
/// Merkle. Detecta cualquier cambio de contenido o de rutas; ignora `.git` (el historial no es parte
/// del paquete). Devuelve `sha256:<hex>`. Memoria acotada (no concatena los contenidos).
pub fn hash_package(dir: &Path) -> Result<String, String> {
    let mut archivos: Vec<(String, std::path::PathBuf)> = Vec::new();
    recolectar_archivos(dir, dir, &mut archivos)?;
    archivos.sort();
    let mut resumen = String::new();
    for (rel, abs) in &archivos {
        let contenido = std::fs::read(abs)
            .map_err(|e| format!("no se pudo leer '{}': {e}", abs.display()))?;
        resumen.push_str(rel);
        resumen.push(':');
        resumen.push_str(&crate::sha256::sha256_hex(&contenido));
        resumen.push('\n');
    }
    Ok(format!("sha256:{}", crate::sha256::sha256_hex(resumen.as_bytes())))
}

/// Recolecta recursivamente los archivos bajo `dir` como `(ruta_relativa_a_base, ruta_absoluta)`,
/// saltando `.git`. Las rutas usan `/` (portable y determinista entre plataformas).
fn recolectar_archivos(base: &Path, dir: &Path, out: &mut Vec<(String, std::path::PathBuf)>) -> Result<(), String> {
    let entradas = std::fs::read_dir(dir)
        .map_err(|e| format!("no se pudo listar '{}': {e}", dir.display()))?;
    for entrada in entradas {
        let entrada = entrada.map_err(|e| format!("error listando '{}': {e}", dir.display()))?;
        if entrada.file_name() == *".git" {
            continue; // el historial de git no es parte del contenido del paquete
        }
        let path = entrada.path();
        if path.is_dir() {
            recolectar_archivos(base, &path, out)?;
        } else {
            let rel = path.strip_prefix(base).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}

// ── Lockfile `ray.lock` ───────────────────────────────────────────────────────────────

/// Una entrada de `ray.lock`: qué se descargó (url + ref), a qué commit resolvió, y el hash de su
/// contenido (para verificar la integridad en cada build, M39c-2b).
#[derive(Debug, Clone, PartialEq)]
pub struct LockEntry {
    pub name: String,
    pub url: String,
    pub git_ref: String,
    pub commit: String,
    pub hash: String,
}

/// Lee `ray.lock` de `root` a un mapa `nombre → entrada`. Vacío si no existe. Formato: secciones
/// `[nombre]` con `clave = "valor"` (el mismo subconjunto de TOML que `ray.toml`).
fn leer_lock(root: &Path) -> Result<std::collections::HashMap<String, LockEntry>, String> {
    let ruta = root.join("ray.lock");
    let Ok(fuente) = std::fs::read_to_string(&ruta) else {
        return Ok(std::collections::HashMap::new()); // sin lock aún → mapa vacío
    };
    let mut mapa = std::collections::HashMap::new();
    let mut actual: Option<LockEntry> = None;
    let cerrar = |actual: &mut Option<LockEntry>, mapa: &mut std::collections::HashMap<String, LockEntry>| {
        if let Some(e) = actual.take() {
            mapa.insert(e.name.clone(), e);
        }
    };
    for (i, linea) in fuente.lines().enumerate() {
        let linea = linea.split_once('#').map_or(linea, |(a, _)| a).trim();
        if linea.is_empty() {
            continue;
        }
        if let Some(resto) = linea.strip_prefix('[') {
            cerrar(&mut actual, &mut mapa);
            let nombre = resto.strip_suffix(']')
                .ok_or_else(|| format!("ray.lock:{}: cabecera sin ']'", i + 1))?;
            actual = Some(LockEntry {
                name: nombre.trim().to_string(),
                url: String::new(), git_ref: String::new(), commit: String::new(), hash: String::new(),
            });
            continue;
        }
        let (clave, valor) = linea.split_once('=')
            .ok_or_else(|| format!("ray.lock:{}: se esperaba 'clave = valor'", i + 1))?;
        let valor = valor.trim().strip_prefix('"').and_then(|v| v.strip_suffix('"'))
            .ok_or_else(|| format!("ray.lock:{}: el valor debe ir entre comillas", i + 1))?;
        let Some(e) = actual.as_mut() else {
            return Err(format!("ray.lock:{}: clave fuera de una sección [nombre]", i + 1));
        };
        match clave.trim() {
            "url" => e.url = valor.to_string(),
            "ref" => e.git_ref = valor.to_string(),
            "commit" => e.commit = valor.to_string(),
            "hash" => e.hash = valor.to_string(),
            _ => {} // claves desconocidas se ignoran (extensibilidad)
        }
    }
    cerrar(&mut actual, &mut mapa);
    Ok(mapa)
}

/// Escribe `ray.lock` en `root` con las entradas **ordenadas por nombre** (determinista → diffs
/// limpios en control de versiones). El lockfile SÍ se commitea (fija las versiones para el equipo).
fn escribir_lock(root: &Path, entradas: &mut [LockEntry]) -> Result<(), String> {
    entradas.sort_by(|a, b| a.name.cmp(&b.name));
    let mut s = String::from(
        "# ray.lock — versiones y hashes bloqueados de las dependencias (generado por 'ray').\n\
         # Se commitea al repositorio. No editar a mano.\n",
    );
    for e in entradas.iter() {
        s.push_str(&format!(
            "\n[{}]\nurl = \"{}\"\nref = \"{}\"\ncommit = \"{}\"\nhash = \"{}\"\n",
            e.name, e.url, e.git_ref, e.commit, e.hash
        ));
    }
    std::fs::write(root.join("ray.lock"), s)
        .map_err(|e| format!("no se pudo escribir 'ray.lock': {e}"))
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
