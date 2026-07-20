//! Versionado **semver** (M51a/M51e; extraído de `index.rs` en la limpieza post-M51).
//!
//! Dos piezas: [`Version`] (el triple `X.Y.Z` + pre-release opcional, con el **orden** de semver
//! §11) y [`VersionReq`] (un **requisito** de `ray.toml` — exacto `1.2.0`/`=`, caret `^`, tilde
//! `~`, `*` — con las reglas de *matching*, incluida la de cargo para pre-releases). Lo consumen
//! el **índice** (`index.rs`: elegir la versión publicada más alta que satisface un requisito),
//! el **resolutor** (`deps.rs`: ordenar refs git en `mvs` y validar el *lock-pinning*) y el CLI
//! (`ray publish` valida que la versión del paquete sea semver).

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
pub(crate) enum VersionReq {
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
    pub(crate) fn parse(s: &str) -> Result<VersionReq, String> {
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
    pub(crate) fn matches(&self, v: &Version) -> bool {
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
pub(crate) fn parse_partial(s: &str) -> Result<(Version, u8), String> {
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) if !p.trim().is_empty() => (c, Some(p.trim().to_string())),
        Some(_) => return Err(format!("invalid version requirement: '{s}' (empty pre-release)")),
        None => (s, None),
    };
    let mut it = core.split('.');
    let mut nums = [0u64; 3];
    let mut prec = 0u8;
    for slot in nums.iter_mut() {
        match it.next() {
            Some(part) => {
                *slot = part.trim().parse::<u64>().map_err(|_| {
                    format!("invalid version requirement: '{s}' (expected X.Y.Z)")
                })?;
                prec += 1;
            }
            None => break,
        }
    }
    if prec == 0 {
        return Err(format!("empty version requirement: '{s}'"));
    }
    if it.next().is_some() {
        return Err(format!("version requirement with too many components: '{s}'"));
    }
    if pre.is_some() && prec != 3 {
        return Err(format!(
            "invalid version requirement: '{s}' (a pre-release requires the full triple X.Y.Z-pre)"
        ));
    }
    Ok((Version { major: nums[0], minor: nums[1], patch: nums[2], pre }, prec))
}

/// Parsea una versión `X[.Y[.Z]][-pre]` (rellenando con 0). `None` si no es semver. Para validar la
/// versión de un paquete a publicar (M51b) y los refs git semver (`deps::semver`).
pub fn parse_version(s: &str) -> Option<Version> {
    parse_partial(s).ok().map(|(v, _)| v)
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
    fn parses_requirements() {
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
    fn matches_requirements() {
        assert!(VersionReq::parse("^1.2.0").unwrap().matches(&v(1, 5, 0)));
        assert!(!VersionReq::parse("^1.2.0").unwrap().matches(&v(2, 0, 0)));
        assert!(!VersionReq::parse("^1.2.0").unwrap().matches(&v(1, 1, 0)));
        assert!(VersionReq::parse("~1.2.0").unwrap().matches(&v(1, 2, 9)));
        assert!(!VersionReq::parse("~1.2.0").unwrap().matches(&v(1, 3, 0)));
        assert!(VersionReq::parse("1.2.0").unwrap().matches(&v(1, 2, 0)));
        assert!(!VersionReq::parse("1.2.0").unwrap().matches(&v(1, 2, 1)));
    }

    #[test]
    fn orders_and_matches_pre_releases() {
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
}
