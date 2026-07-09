//! El índice de paquetes (M51a) — resolución de dependencias **por nombre**.
//!
//! El gestor de paquetes (M39c) resuelve dependencias por **git** (`git+URL@ref`) y por **ruta**
//! (`path:dir`): para instalar hay que conocer la URL exacta. El **índice** añade la última milla:
//! declarar `foo = "1.2.0"` (o `"^1.2"`, `"~1.2.3"`, `"*"`) en `ray.toml` y que el CLI **descubra**
//! de dónde sale ese paquete. (DESIGN §54.)
//!
//! **El índice es un repositorio git** (o, en M51a, un **directorio local** para tests deterministas)
//! con **un archivo por paquete**, `<índice>/<nombre>.toml`, que lista sus versiones publicadas:
//!
//! ```toml
//! # índice de `foo`
//! [1.2.0]
//! git = "git+https://ejemplo/foo@v1.2.0"
//! hash = "sha256:…"          # opcional (verificación previa; el lock ya lo re-verifica)
//!
//! [1.3.0]
//! git = "git+https://ejemplo/foo@v1.3.0"
//! yanked = "true"            # M51c: retirada sin borrar; la resolución la salta
//! ```
//!
//! Reusa el subconjunto de TOML de `manifest.rs` (secciones `[<versión>]` + `clave = "valor"`). La
//! resolución de una versión → `GitSpec` **delega en la maquinaria git+lock existente** (`deps.rs`):
//! el índice solo aporta el `nombre → (git URL, hash)`; la descarga, la cápsula, el lockfile y las
//! transitivas son las de M39c.

use std::path::Path;

use crate::deps::{self, GitSpec};
use crate::semver::{parse_partial, Version, VersionReq};

/// Una versión publicada de un paquete, leída del índice.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexEntry {
    pub version: Version,
    /// El texto original de la versión (`num` de la sección), para mensajes.
    pub num: String,
    /// La spec git de esa versión (`git+URL@ref`), tal cual aparece en el índice.
    pub git: String,
    pub hash: Option<String>,
    pub yanked: bool,
}

/// ¿La spec de una dependencia se resuelve por el **índice** (por nombre)? Lo es si NO es una dep
/// git (`git+…`) ni por ruta (`path:…`) — es decir, un requisito de versión pelado (`1.2.0`, `^1.2`, …).
pub fn is_registry_spec(spec: &str) -> bool {
    !spec.starts_with("git+") && deps::path_of_path_dep(spec).is_none()
}

/// Lee el archivo de índice de `name` (`<index_dir>/<name>.toml`) y devuelve sus versiones. Error
/// si el paquete no está en el índice. Ignora entradas sin `git` (malformadas) con un error claro.
pub fn read_package(index_dir: &Path, name: &str) -> Result<Vec<IndexEntry>, String> {
    // M51d: el nombre construye una ruta dentro del índice — validarlo (defensa en profundidad;
    // `deps::ensure` ya validó las deps, pero esta API también la llaman los comandos directamente).
    if !deps::valid_package_name(name) {
        return Err(format!("nombre de paquete inválido '{name}' (solo letras, dígitos, '-' y '_')"));
    }
    let path = index_dir.join(format!("{name}.toml"));
    let source = std::fs::read_to_string(&path).map_err(|_| {
        format!("el paquete '{name}' no está en el índice ({})", index_dir.display())
    })?;
    let mut entries: Vec<IndexEntry> = Vec::new();
    // Sección actual: (num, versión, git, hash, yanked).
    let mut cur: Option<(String, Version, Option<String>, Option<String>, bool)> = None;
    let close = |cur: &mut Option<(String, Version, Option<String>, Option<String>, bool)>,
                 out: &mut Vec<IndexEntry>|
     -> Result<(), String> {
        if let Some((num, version, git, hash, yanked)) = cur.take() {
            let git = git.ok_or_else(|| format!("versión '{num}' de '{name}' sin 'git' en el índice"))?;
            out.push(IndexEntry { version, num, git, hash, yanked });
        }
        Ok(())
    };
    for (i, line) in source.lines().enumerate() {
        let line = line.split_once('#').map_or(line, |(a, _)| a).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            close(&mut cur, &mut entries)?;
            let num = rest
                .strip_suffix(']')
                .ok_or_else(|| format!("{}:{}: cabecera de versión sin ']'", path.display(), i + 1))?
                .trim();
            let (version, _) = parse_partial(num)
                .map_err(|e| format!("{}:{}: {e}", path.display(), i + 1))?;
            cur = Some((num.to_string(), version, None, None, false));
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("{}:{}: se esperaba 'clave = valor'", path.display(), i + 1))?;
        let value = value
            .trim()
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .ok_or_else(|| format!("{}:{}: el valor debe ir entre comillas", path.display(), i + 1))?;
        let Some((_, _, git, hash, yanked)) = cur.as_mut() else {
            return Err(format!("{}:{}: clave fuera de una sección [versión]", path.display(), i + 1));
        };
        match key.trim() {
            "git" => *git = Some(value.to_string()),
            "hash" => *hash = Some(value.to_string()),
            "yanked" => *yanked = value == "true",
            _ => {} // claves desconocidas se ignoran (extensibilidad)
        }
    }
    close(&mut cur, &mut entries)?;
    if entries.is_empty() {
        return Err(format!("el paquete '{name}' no tiene versiones en el índice"));
    }
    Ok(entries)
}

/// La versión **más alta no retirada y FINAL** publicada de `name` (para `ray add` sin versión).
/// Las pre-releases no cuentan (M51e: instalarlas es opt-in explícito, `ray add foo@1.0.0-rc1`).
/// Error si no hay ninguna instalable.
pub fn latest(index_dir: &Path, name: &str) -> Result<String, String> {
    let mut versions = read_package(index_dir, name)?;
    let had_pre = versions.iter().any(|e| e.version.pre.is_some() && !e.yanked);
    versions.retain(|e| !e.yanked && e.version.pre.is_none());
    versions.sort_by(|a, b| a.version.cmp(&b.version));
    versions.last().map(|e| e.num.clone()).ok_or_else(|| {
        if had_pre {
            format!(
                "el paquete '{name}' solo tiene pre-releases publicadas; pide una explícita \
                 (p. ej. 'ray add {name}@<X.Y.Z-pre>')"
            )
        } else {
            format!("el paquete '{name}' no tiene versiones publicadas (todas retiradas)")
        }
    })
}

/// Resuelve `name` con el requisito `req` contra el índice → la `GitSpec` de la versión **más alta
/// no retirada** que lo satisface. Error si el paquete no existe o ninguna versión casa.
pub fn resolve(index_dir: &Path, name: &str, req: &str) -> Result<GitSpec, String> {
    resolve_pinned(index_dir, name, req, None, true).map(|(spec, _)| spec)
}

/// Como `resolve`, pero **respeta el lock** (M51c): si `locked` (la versión ya bloqueada de `name`)
/// sigue satisfaciendo `req` y no se pide `update`, la reusa —build reproducible: un caret no sube
/// solo porque el índice gane una versión—. Con `update = true` o sin lock válido, re-resuelve la
/// más alta del índice. Devuelve también el **hash publicado** de la versión elegida (M51d): el
/// llamador lo verifica contra el contenido descargado — el índice pasa de "descubrimiento" a raíz
/// de confianza (cierra el TOFU del lock, que confía en la primera descarga). `None` en el atajo del
/// lock (ya verificado en su primera descarga) o si el índice no publicó hash.
pub fn resolve_pinned(
    index_dir: &Path,
    name: &str,
    req: &str,
    locked: Option<&GitSpec>,
    update: bool,
) -> Result<(GitSpec, Option<String>), String> {
    let req_parsed = VersionReq::parse(req)?;
    if !update
        && let Some(l) = locked
        && let Some(v) = deps::semver(&l.git_ref)
        && req_parsed.matches(&v)
    {
        return Ok((l.clone(), None)); // el lock sigue satisfaciendo el requisito → reproducible
    }
    let mut versions = read_package(index_dir, name)?;
    versions.retain(|e| !e.yanked && req_parsed.matches(&e.version));
    versions.sort_by(|a, b| a.version.cmp(&b.version));
    let chosen = versions.last().ok_or_else(|| {
        format!("ninguna versión de '{name}' satisface '{req}' en el índice")
    })?;
    let spec = deps::parse_spec(&chosen.git).map_err(|e| {
        format!("la versión '{}' de '{name}' tiene una spec git inválida en el índice: {e}", chosen.num)
    })?;
    Ok((spec, chosen.hash.clone()))
}

/// Marca (o desmarca) como **retirada** (`yanked`) la versión `num` de `name` en el índice (M51c,
/// `ray yank`). Una versión retirada no se elige en nuevas resoluciones, pero un lock que ya la fijó
/// la sigue usando (no rompe builds existentes). Edición mínima línea a línea (preserva el resto).
pub fn set_yanked(index_dir: &Path, name: &str, num: &str, yanked: bool) -> Result<(), String> {
    if !deps::valid_package_name(name) {
        return Err(format!("nombre de paquete inválido '{name}' (solo letras, dígitos, '-' y '_')"));
    }
    let (target, _) = parse_partial(num).map_err(|e| format!("versión inválida: {e}"))?;
    let path = index_dir.join(format!("{name}.toml"));
    let source = std::fs::read_to_string(&path)
        .map_err(|_| format!("el paquete '{name}' no está en el índice ({})", index_dir.display()))?;
    let mut lines: Vec<String> = source.lines().map(str::to_string).collect();
    // Localiza la sección `[<versión>]` cuyo número parsea a `target`.
    let sec = lines.iter().position(|l| {
        l.trim()
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .and_then(|n| parse_partial(n.trim()).ok())
            .is_some_and(|(v, _)| v == target)
    });
    let Some(start) = sec else {
        return Err(format!("la versión '{num}' de '{name}' no está en el índice"));
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim().starts_with('['))
        .map(|off| start + 1 + off)
        .unwrap_or(lines.len());
    let value = format!("yanked = \"{}\"", if yanked { "true" } else { "false" });
    match lines[start + 1..end].iter().position(|l| l.trim_start().starts_with("yanked")) {
        Some(off) => lines[start + 1 + off] = value,
        None => lines.insert(end, value),
    }
    let mut out = lines.join("\n");
    if source.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    std::fs::write(&path, out).map_err(|e| format!("no se pudo escribir el índice de '{name}': {e}"))
}

/// Añade una versión al archivo de índice de `name` (`<index_dir>/<name>.toml`), creándolo si no
/// existe (M51b, `ray publish`). **Inmutabilidad**: rechaza sobrescribir una versión ya publicada
/// (build reproducible). Escribe `git` y, si se da, `hash`. No hace commit/push del repo del índice
/// —eso es acción del autor (o M51c)—: solo edita el archivo local.
pub fn append_version(
    index_dir: &Path,
    name: &str,
    num: &str,
    git: &str,
    hash: Option<&str>,
) -> Result<(), String> {
    if !deps::valid_package_name(name) {
        return Err(format!("nombre de paquete inválido '{name}' (solo letras, dígitos, '-' y '_')"));
    }
    let (target, _) = parse_partial(num).map_err(|e| format!("versión a publicar inválida: {e}"))?;
    let path = index_dir.join(format!("{name}.toml"));
    if path.exists() {
        // Inmutabilidad: comparar por versión parseada (`1.2` y `1.2.0` son la misma).
        if read_package(index_dir, name)?.iter().any(|e| e.version == target) {
            return Err(format!(
                "la versión '{num}' de '{name}' ya está publicada (las versiones son inmutables); \
                 sube el número de versión"
            ));
        }
    } else {
        std::fs::create_dir_all(index_dir)
            .map_err(|e| format!("no se pudo crear el índice '{}': {e}", index_dir.display()))?;
    }
    let mut s = if path.exists() {
        std::fs::read_to_string(&path)
            .map_err(|e| format!("no se pudo leer el índice de '{name}': {e}"))?
    } else {
        format!("# índice de {name}\n")
    };
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&format!("\n[{num}]\ngit = \"{git}\"\n"));
    if let Some(h) = hash {
        s.push_str(&format!("hash = \"{h}\"\n"));
    }
    std::fs::write(&path, s).map_err(|e| format!("no se pudo escribir el índice de '{name}': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distingue_specs_de_registro() {
        assert!(is_registry_spec("1.2.0"));
        assert!(is_registry_spec("^1.2"));
        assert!(is_registry_spec("*"));
        assert!(!is_registry_spec("git+https://x/foo@v1"));
        assert!(!is_registry_spec("path:../foo"));
    }

    #[test]
    fn lee_y_resuelve_del_indice() {
        let dir = std::env::temp_dir().join("ray_index_test_unit");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("foo.toml"),
            "# índice de foo\n\
             [1.2.0]\ngit = \"git+https://ej/foo@v1.2.0\"\n\n\
             [1.3.0]\ngit = \"git+https://ej/foo@v1.3.0\"\n\n\
             [2.0.0]\ngit = \"git+https://ej/foo@v2.0.0\"\nyanked = \"true\"\n",
        )
        .unwrap();
        // caret 1.x: la más alta no retirada que casa es 1.3.0 (2.0.0 está yanked y fuera de rango).
        assert_eq!(resolve(&dir, "foo", "^1.2.0").unwrap().git_ref, "v1.3.0");
        // exacta.
        assert_eq!(resolve(&dir, "foo", "1.2.0").unwrap().git_ref, "v1.2.0");
        // latest ignora la yanked 2.0.0.
        assert_eq!(latest(&dir, "foo").unwrap(), "1.3.0");
        // paquete inexistente.
        assert!(resolve(&dir, "bar", "*").unwrap_err().contains("no está en el índice"));
        // ninguna casa.
        assert!(resolve(&dir, "foo", "^3.0").unwrap_err().contains("ninguna versión"));
    }

    #[test]
    fn append_version_es_inmutable() {
        let dir = std::env::temp_dir().join("ray_index_test_append");
        let _ = std::fs::remove_dir_all(&dir);
        // Publica 1.0.0 en un índice nuevo (se crea el archivo).
        append_version(&dir, "foo", "1.0.0", "git+https://ej/foo@v1.0.0", Some("sha256:aa")).unwrap();
        assert_eq!(resolve(&dir, "foo", "1.0.0").unwrap().git_ref, "v1.0.0");
        // Otra versión se añade sin problema.
        append_version(&dir, "foo", "1.1.0", "git+https://ej/foo@v1.1.0", None).unwrap();
        assert_eq!(latest(&dir, "foo").unwrap(), "1.1.0");
        // Re-publicar la misma versión (incluso escrita distinto) → error de inmutabilidad.
        let e = append_version(&dir, "foo", "1.0.0", "git+https://ej/foo@otro", None).unwrap_err();
        assert!(e.contains("ya está publicada"), "{e}");
    }
}
