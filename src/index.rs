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

/// Una versión semver `(mayor, menor, parche)`.
pub type Version = (u64, u64, u64);

/// Un requisito de versión declarado en `ray.toml` (`foo = "<req>"`). Semántica (DESIGN §54.3):
/// - **`1.2.0`** (pelado) o **`=1.2.0`** → exacta (rellena con 0 lo omitido: `1.2` = `1.2.0`).
/// - **`^1.2.0`** → *caret*: compatible sin cambiar el componente distinto de cero más a la izquierda
///   (`^1.2.3` = `[1.2.3, 2.0.0)`; `^0.2.3` = `[0.2.3, 0.3.0)`).
/// - **`~1.2.3`** → *tilde*: solo parche (`[1.2.3, 1.3.0)`); `~1.2` = `[1.2.0, 1.3.0)`; `~1` = `[1.0.0, 2.0.0)`.
/// - **`*`** o vacío → cualquiera (la más alta publicada).
#[derive(Debug, Clone, PartialEq)]
enum VersionReq {
    /// Igualdad exacta con la versión dada.
    Exact(Version),
    /// Rango `[low, high)` (low inclusivo, high exclusivo). Caret y tilde se compilan a esto.
    Range(Version, Version),
    /// Cualquier versión.
    Any,
}

impl VersionReq {
    /// Parsea el texto del requisito. Nunca falla: un texto no reconocible se trata como exacto de
    /// `(0,0,0)` no casaría nada útil, así que devolvemos error explícito para avisar al usuario.
    fn parse(s: &str) -> Result<VersionReq, String> {
        let s = s.trim();
        if s.is_empty() || s == "*" {
            return Ok(VersionReq::Any);
        }
        if let Some(rest) = s.strip_prefix('^') {
            let (v, _prec) = parse_partial(rest)?;
            return Ok(VersionReq::Range(v, caret_upper(v)));
        }
        if let Some(rest) = s.strip_prefix('~') {
            let (v, prec) = parse_partial(rest)?;
            return Ok(VersionReq::Range(v, tilde_upper(v, prec)));
        }
        let core = s.strip_prefix('=').unwrap_or(s);
        let (v, _prec) = parse_partial(core)?;
        Ok(VersionReq::Exact(v))
    }

    /// ¿La versión `v` satisface el requisito?
    fn matches(&self, v: Version) -> bool {
        match self {
            VersionReq::Exact(e) => v == *e,
            VersionReq::Range(lo, hi) => *lo <= v && v < *hi,
            VersionReq::Any => true,
        }
    }
}

/// El límite superior (exclusivo) de un requisito *caret* `^X.Y.Z`: sube el componente distinto de
/// cero más a la izquierda (regla de cargo, que preserva la compatibilidad semver con `0.x`).
fn caret_upper((major, minor, patch): Version) -> Version {
    if major > 0 {
        (major + 1, 0, 0)
    } else if minor > 0 {
        (major, minor + 1, 0)
    } else {
        (major, minor, patch + 1)
    }
}

/// El límite superior (exclusivo) de un requisito *tilde*: con menor especificado (`~1.2`/`~1.2.3`)
/// sube el menor; con solo el mayor (`~1`) sube el mayor. `prec` = nº de componentes escritos.
fn tilde_upper((major, minor, _patch): Version, prec: u8) -> Version {
    if prec >= 2 {
        (major, minor + 1, 0)
    } else {
        (major + 1, 0, 0)
    }
}

/// Parsea `X[.Y[.Z]]` a `(versión, precisión)`, rellenando con 0 lo omitido (`1.2` → `(1,2,0)`,
/// precisión 2). Rechaza un componente no numérico o vacío.
fn parse_partial(s: &str) -> Result<(Version, u8), String> {
    let mut it = s.split('.');
    let mut nums = [0u64; 3];
    let mut prec = 0u8;
    for slot in nums.iter_mut() {
        match it.next() {
            Some(part) => {
                *slot = part.trim().parse::<u64>().map_err(|_| {
                    format!("requisito de versión inválido: '{s}' (se esperaba X.Y.Z)")
                })?;
                prec += 1;
            }
            None => break,
        }
    }
    if prec == 0 {
        return Err(format!("requisito de versión vacío: '{s}'"));
    }
    if it.next().is_some() {
        return Err(format!("requisito de versión con demasiados componentes: '{s}'"));
    }
    Ok(((nums[0], nums[1], nums[2]), prec))
}

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

/// La versión **más alta no retirada** publicada de `name` (para `ray add` sin versión). Error si
/// no hay ninguna instalable.
pub fn latest(index_dir: &Path, name: &str) -> Result<String, String> {
    let mut versions = read_package(index_dir, name)?;
    versions.retain(|e| !e.yanked);
    versions.sort_by(|a, b| a.version.cmp(&b.version));
    versions
        .last()
        .map(|e| e.num.clone())
        .ok_or_else(|| format!("el paquete '{name}' no tiene versiones publicadas (todas retiradas)"))
}

/// Resuelve `name` con el requisito `req` contra el índice → la `GitSpec` de la versión **más alta
/// no retirada** que lo satisface. Error si el paquete no existe o ninguna versión casa.
pub fn resolve(index_dir: &Path, name: &str, req: &str) -> Result<GitSpec, String> {
    let req_parsed = VersionReq::parse(req)?;
    let mut versions = read_package(index_dir, name)?;
    versions.retain(|e| !e.yanked && req_parsed.matches(e.version));
    versions.sort_by(|a, b| a.version.cmp(&b.version));
    let chosen = versions.last().ok_or_else(|| {
        format!("ninguna versión de '{name}' satisface '{req}' en el índice")
    })?;
    deps::parse_spec(&chosen.git).map_err(|e| {
        format!("la versión '{}' de '{name}' tiene una spec git inválida en el índice: {e}", chosen.num)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: u64, b: u64, c: u64) -> Version {
        (a, b, c)
    }

    #[test]
    fn parsea_requisitos() {
        assert_eq!(VersionReq::parse("1.2.0").unwrap(), VersionReq::Exact(v(1, 2, 0)));
        assert_eq!(VersionReq::parse("=1.2.0").unwrap(), VersionReq::Exact(v(1, 2, 0)));
        assert_eq!(VersionReq::parse("1.2").unwrap(), VersionReq::Exact(v(1, 2, 0)));
        assert_eq!(VersionReq::parse("*").unwrap(), VersionReq::Any);
        assert_eq!(VersionReq::parse("").unwrap(), VersionReq::Any);
        assert_eq!(VersionReq::parse("^1.2.3").unwrap(), VersionReq::Range(v(1, 2, 3), v(2, 0, 0)));
        assert_eq!(VersionReq::parse("^0.2.3").unwrap(), VersionReq::Range(v(0, 2, 3), v(0, 3, 0)));
        assert_eq!(VersionReq::parse("^0.0.3").unwrap(), VersionReq::Range(v(0, 0, 3), v(0, 0, 4)));
        assert_eq!(VersionReq::parse("~1.2.3").unwrap(), VersionReq::Range(v(1, 2, 3), v(1, 3, 0)));
        assert_eq!(VersionReq::parse("~1.2").unwrap(), VersionReq::Range(v(1, 2, 0), v(1, 3, 0)));
        assert_eq!(VersionReq::parse("~1").unwrap(), VersionReq::Range(v(1, 0, 0), v(2, 0, 0)));
        assert!(VersionReq::parse("abc").is_err());
        assert!(VersionReq::parse("1.2.3.4").is_err());
    }

    #[test]
    fn casa_requisitos() {
        assert!(VersionReq::parse("^1.2.0").unwrap().matches(v(1, 5, 0)));
        assert!(!VersionReq::parse("^1.2.0").unwrap().matches(v(2, 0, 0)));
        assert!(!VersionReq::parse("^1.2.0").unwrap().matches(v(1, 1, 0)));
        assert!(VersionReq::parse("~1.2.0").unwrap().matches(v(1, 2, 9)));
        assert!(!VersionReq::parse("~1.2.0").unwrap().matches(v(1, 3, 0)));
        assert!(VersionReq::parse("1.2.0").unwrap().matches(v(1, 2, 0)));
        assert!(!VersionReq::parse("1.2.0").unwrap().matches(v(1, 2, 1)));
    }

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
}
