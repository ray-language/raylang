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

/// Una versión semver: `(mayor, menor, parche)` + **pre-release** opcional (M51e: `1.0.0-rc1`).
/// El orden es el de semver §11: se compara el triple y, a triple igual, una pre-release es
/// **menor** que la final (`1.0.0-rc1 < 1.0.0`); dos pre-releases comparan identificador a
/// identificador (numérico < alfanumérico; numéricos por valor, alfanuméricos ASCII).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// La parte pre-release (`rc1`, `beta.2`, …), sin el `-`. `None` = versión final.
    pub pre: Option<String>,
}

impl Version {
    pub fn new(major: u64, minor: u64, patch: u64) -> Version {
        Version { major, minor, patch, pre: None }
    }
    fn triple(&self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Version) -> std::cmp::Ordering {
        self.triple().cmp(&other.triple()).then_with(|| match (&self.pre, &other.pre) {
            (None, None) => std::cmp::Ordering::Equal,
            (Some(_), None) => std::cmp::Ordering::Less, // pre-release < final
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => cmp_pre(a, b),
        })
    }
}

/// Compara dos partes pre-release por identificadores separados por `.` (semver §11.4): un
/// identificador numérico compara por valor y es menor que uno alfanumérico; los alfanuméricos
/// comparan ASCII; con prefijo igual, la lista más corta es menor (`rc < rc.1`).
fn cmp_pre(a: &str, b: &str) -> std::cmp::Ordering {
    let ids = |s: &str| s.split('.').map(str::to_string).collect::<Vec<_>>();
    for (x, y) in ids(a).iter().zip(ids(b).iter()) {
        let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(nx), Ok(ny)) => nx.cmp(&ny),
            (Ok(_), Err(_)) => std::cmp::Ordering::Less, // numérico < alfanumérico
            (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
            (Err(_), Err(_)) => x.cmp(y),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    a.split('.').count().cmp(&b.split('.').count())
}

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
            let hi = caret_upper(&v);
            return Ok(VersionReq::Range(v, hi));
        }
        if let Some(rest) = s.strip_prefix('~') {
            let (v, prec) = parse_partial(rest)?;
            let hi = tilde_upper(&v, prec);
            return Ok(VersionReq::Range(v, hi));
        }
        let core = s.strip_prefix('=').unwrap_or(s);
        let (v, _prec) = parse_partial(core)?;
        Ok(VersionReq::Exact(v))
    }

    /// ¿La versión `v` satisface el requisito? **Pre-releases** (M51e, regla de cargo): una versión
    /// pre-release solo casa si el requisito menciona **explícitamente** una pre-release con el
    /// mismo triple `X.Y.Z` (así `^1.0` jamás elige `1.1.0-rc1` por sorpresa; para probar una rc
    /// hay que pedirla: `1.1.0-rc1` o `^1.1.0-rc1`).
    fn matches(&self, v: &Version) -> bool {
        match self {
            VersionReq::Exact(e) => v == e,
            VersionReq::Range(lo, hi) => {
                if v.pre.is_some() && !(lo.pre.is_some() && v.triple() == lo.triple()) {
                    return false;
                }
                lo <= v && v < hi
            }
            VersionReq::Any => v.pre.is_none(),
        }
    }
}

/// El límite superior (exclusivo) de un requisito *caret* `^X.Y.Z`: sube el componente distinto de
/// cero más a la izquierda (regla de cargo, que preserva la compatibilidad semver con `0.x`).
fn caret_upper(v: &Version) -> Version {
    if v.major > 0 {
        Version::new(v.major + 1, 0, 0)
    } else if v.minor > 0 {
        Version::new(v.major, v.minor + 1, 0)
    } else {
        Version::new(v.major, v.minor, v.patch + 1)
    }
}

/// El límite superior (exclusivo) de un requisito *tilde*: con menor especificado (`~1.2`/`~1.2.3`)
/// sube el menor; con solo el mayor (`~1`) sube el mayor. `prec` = nº de componentes escritos.
fn tilde_upper(v: &Version, prec: u8) -> Version {
    if prec >= 2 {
        Version::new(v.major, v.minor + 1, 0)
    } else {
        Version::new(v.major + 1, 0, 0)
    }
}

/// Parsea `X[.Y[.Z]][-pre]` a `(versión, precisión)`, rellenando con 0 lo omitido (`1.2` → `(1,2,0)`,
/// precisión 2). Rechaza un componente no numérico o vacío. Una **pre-release** (M51e) exige el
/// triple completo (`1.0.0-rc1` sí; `1.0-rc1` no: sería ambiguo qué componente rellena el 0).
fn parse_partial(s: &str) -> Result<(Version, u8), String> {
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) if !p.trim().is_empty() => (c, Some(p.trim().to_string())),
        Some(_) => return Err(format!("requisito de versión inválido: '{s}' (pre-release vacía)")),
        None => (s, None),
    };
    let mut it = core.split('.');
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
    if pre.is_some() && prec != 3 {
        return Err(format!(
            "requisito de versión inválido: '{s}' (una pre-release exige el triple completo X.Y.Z-pre)"
        ));
    }
    Ok((Version { major: nums[0], minor: nums[1], patch: nums[2], pre }, prec))
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

/// Parsea una versión `X[.Y[.Z]]` (rellenando con 0). `None` si no es semver. Para validar la
/// versión de un paquete a publicar (M51b).
pub fn parse_version(s: &str) -> Option<Version> {
    parse_partial(s).ok().map(|(v, _)| v)
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

    fn v(a: u64, b: u64, c: u64) -> Version {
        Version::new(a, b, c)
    }

    /// Una versión con pre-release (M51e): `vpre(1, 0, 0, "rc1")` = `1.0.0-rc1`.
    fn vpre(a: u64, b: u64, c: u64, pre: &str) -> Version {
        Version { major: a, minor: b, patch: c, pre: Some(pre.to_string()) }
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
        // M51e: pre-releases — exigen el triple completo.
        assert_eq!(VersionReq::parse("1.0.0-rc1").unwrap(), VersionReq::Exact(vpre(1, 0, 0, "rc1")));
        assert!(VersionReq::parse("1.0-rc1").is_err());
        assert!(VersionReq::parse("1.0.0-").is_err());
    }

    #[test]
    fn casa_requisitos() {
        assert!(VersionReq::parse("^1.2.0").unwrap().matches(&v(1, 5, 0)));
        assert!(!VersionReq::parse("^1.2.0").unwrap().matches(&v(2, 0, 0)));
        assert!(!VersionReq::parse("^1.2.0").unwrap().matches(&v(1, 1, 0)));
        assert!(VersionReq::parse("~1.2.0").unwrap().matches(&v(1, 2, 9)));
        assert!(!VersionReq::parse("~1.2.0").unwrap().matches(&v(1, 3, 0)));
        assert!(VersionReq::parse("1.2.0").unwrap().matches(&v(1, 2, 0)));
        assert!(!VersionReq::parse("1.2.0").unwrap().matches(&v(1, 2, 1)));
    }

    #[test]
    fn ordena_y_casa_pre_releases() {
        // Orden semver §11: pre-release < final; identificadores numéricos por valor.
        assert!(vpre(1, 0, 0, "rc1") < v(1, 0, 0));
        assert!(v(0, 9, 9) < vpre(1, 0, 0, "rc1"));
        assert!(vpre(1, 0, 0, "alpha") < vpre(1, 0, 0, "beta"));
        assert!(vpre(1, 0, 0, "rc.2") < vpre(1, 0, 0, "rc.10")); // numérico: 2 < 10
        assert!(vpre(1, 0, 0, "rc") < vpre(1, 0, 0, "rc.1")); // prefijo igual: más corta es menor
        assert!(vpre(1, 0, 0, "1") < vpre(1, 0, 0, "alpha")); // numérico < alfanumérico
        // Matching (regla de cargo): una pre solo casa si el requisito la menciona (mismo triple).
        assert!(VersionReq::parse("1.0.0-rc1").unwrap().matches(&vpre(1, 0, 0, "rc1")));
        assert!(!VersionReq::parse("1.0.0").unwrap().matches(&vpre(1, 0, 0, "rc1")));
        assert!(!VersionReq::parse("^1.0.0").unwrap().matches(&vpre(1, 1, 0, "rc1")));
        assert!(!VersionReq::parse("*").unwrap().matches(&vpre(1, 0, 0, "rc1")));
        // ^1.0.0-rc1: admite esa pre (y posteriores del MISMO triple) y las finales del rango.
        let caret_pre = VersionReq::parse("^1.0.0-rc1").unwrap();
        assert!(caret_pre.matches(&vpre(1, 0, 0, "rc1")));
        assert!(caret_pre.matches(&vpre(1, 0, 0, "rc2")));
        assert!(!caret_pre.matches(&vpre(1, 1, 0, "rc1"))); // pre de OTRO triple: no
        assert!(caret_pre.matches(&v(1, 0, 0)));
        assert!(caret_pre.matches(&v(1, 5, 0)));
        assert!(!caret_pre.matches(&vpre(1, 0, 0, "rc0"))); // menor que la pedida
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
