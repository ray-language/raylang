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

/// Si el spec es una **dependencia por ruta local** (`path:<dir>`), devuelve el `<dir>` (relativo a la
/// raíz del proyecto, o absoluto). A diferencia de las git, no se descargan ni se bloquean/hashean (son
/// locales y mutables, pensadas para desarrollo o un paquete que vive en el mismo repo): el CLI registra
/// su carpeta como raíz de módulos. `None` si no es una path-dep. (M40.8a)
pub fn ruta_de_path_dep(spec: &str) -> Option<&str> {
    spec.strip_prefix("path:").map(str::trim)
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

/// Asegura que **todo el grafo** de dependencias (directas y **transitivas**, M39c-3) esté en la
/// caché `.ray-deps/` y **verifica su integridad** contra `ray.lock` (M39c-2b). Es un BFS sobre el
/// grafo: por cada paquete descargado se lee su propio `ray.toml` y se encolan SUS dependencias.
/// Ciclos seguros (mapa de elegidos). Conflictos (mismo nombre, distinto spec): **MVS ligero** —el
/// mayor tag semver de la misma URL, o error si no son comparables (caché plana: un slot por nombre)—.
/// Para cada paquete recomputa el hash y lo compara con el bloqueado; un desajuste = *supply-chain*.
pub fn asegurar(manifest: &Manifest) -> Result<usize, String> {
    let cache = manifest.root.join(".ray-deps");
    let bloqueadas = leer_lock(&manifest.root)?;

    // BFS del grafo. `elegido` = spec resuelto por nombre (tras MVS); `cacheado` = spec que esta
    // ejecución dejó en la caché (para re-descargar si un conflicto lo actualiza).
    let mut elegido: std::collections::HashMap<String, GitSpec> = std::collections::HashMap::new();
    let mut cacheado: std::collections::HashMap<String, GitSpec> = std::collections::HashMap::new();
    let mut cola: std::collections::VecDeque<(String, GitSpec)> = std::collections::VecDeque::new();
    let mut nuevas = 0usize;
    for (n, s) in &manifest.dependencies {
        if ruta_de_path_dep(s).is_some() {
            continue; // M40.8a: las path-deps son locales; no se descargan (las registra el CLI)
        }
        cola.push_back((n.clone(), parse_spec(s)?));
    }

    while let Some((nombre, spec)) = cola.pop_front() {
        // Elegir el spec: nuevo, o MVS con el ya elegido si difieren (conflicto).
        let elegido_spec = match elegido.get(&nombre) {
            None => spec,
            Some(prev) if *prev == spec => prev.clone(),
            Some(prev) => mvs(&nombre, prev, &spec)?,
        };
        let sin_cambio = elegido.get(&nombre) == Some(&elegido_spec);
        elegido.insert(nombre.clone(), elegido_spec.clone());
        if sin_cambio && cacheado.get(&nombre) == Some(&elegido_spec) {
            continue; // ya procesado con este spec (dedup / ciclo)
        }

        // Descargar (o re-descargar si esta ejecución tenía otra versión por un conflicto).
        let dest = cache.join(&nombre);
        if cacheado.get(&nombre) != Some(&elegido_spec) {
            if cacheado.contains_key(&nombre) && dest.exists() {
                let _ = std::fs::remove_dir_all(&dest); // upgrade dentro de esta resolución
            }
            if !dest.exists() {
                eprintln!("  descargando {nombre} ({}@{})", elegido_spec.url, elegido_spec.git_ref);
                fetch(&nombre, &elegido_spec, &dest)?;
                nuevas += 1;
            }
            cacheado.insert(nombre.clone(), elegido_spec.clone());
        }

        // Dependencias transitivas: leer el `ray.toml` del paquete y encolarlas (saltando path-deps).
        for (dn, ds) in deps_del_paquete(&dest)? {
            if ruta_de_path_dep(&ds).is_some() {
                continue;
            }
            cola.push_back((dn, parse_spec(&ds)?));
        }
    }

    // Verificar el hash de cada paquete elegido contra el lock y reescribir `ray.lock`.
    let mut nuevo_lock: Vec<LockEntry> = Vec::new();
    for (nombre, spec) in &elegido {
        let dest = cache.join(nombre);
        let commit = rev_parse(&dest).unwrap_or_default();
        let hash = hash_package(&dest)?;
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
            url: spec.url.clone(),
            git_ref: spec.git_ref.clone(),
            commit,
            hash,
        });
    }
    escribir_lock(&manifest.root, &mut nuevo_lock)?;
    Ok(nuevas)
}

/// Selección de versión ante un conflicto (mismo nombre, distinto spec): con la **misma URL** y
/// refs **semver** (`vX.Y.Z`/`X.Y.Z`), gana el mayor (la mínima versión que satisface a ambos, estilo
/// Go-MVS reinterpretando `@vX` como "al menos vX"). Si las URLs difieren o los refs no son semver
/// comparables, es error: la caché es plana (un solo slot por nombre) y no se puede reconciliar.
fn mvs(nombre: &str, a: &GitSpec, b: &GitSpec) -> Result<GitSpec, String> {
    if a.url == b.url
        && let (Some(va), Some(vb)) = (semver(&a.git_ref), semver(&b.git_ref))
    {
        return Ok(if vb > va { b.clone() } else { a.clone() });
    }
    Err(format!(
        "conflicto de versiones para la dependencia '{nombre}': se pide '{}@{}' y '{}@{}', \
         irreconciliables (URLs distintas o refs no semver). Fija una sola versión.",
        a.url, a.git_ref, b.url, b.git_ref
    ))
}

/// Parsea un ref semver `vX.Y.Z` / `X.Y.Z` (ignora un sufijo de pre-release tras `-`) a `(mayor,
/// menor, parche)`. `None` si no es semver (un commit, una rama…). Para ordenar en `mvs`.
fn semver(git_ref: &str) -> Option<(u64, u64, u64)> {
    let nucleo = git_ref.strip_prefix('v').unwrap_or(git_ref);
    let nucleo = nucleo.split('-').next().unwrap_or(nucleo); // corta pre-release
    let mut it = nucleo.split('.');
    let mayor = it.next()?.parse().ok()?;
    let menor = it.next()?.parse().ok()?;
    let parche = it.next().unwrap_or("0").parse().ok()?;
    Some((mayor, menor, parche))
}

/// Las dependencias declaradas en el `ray.toml` de un paquete descargado (su `[dependencies]`), para
/// la resolución transitiva. Vacío si el paquete no tiene `ray.toml` (paquete hoja). Lenient: no
/// exige `name`/`version` (a un paquete-dependencia solo le miramos sus dependencias).
fn deps_del_paquete(pkg_dir: &Path) -> Result<Vec<(String, String)>, String> {
    let Ok(fuente) = std::fs::read_to_string(pkg_dir.join("ray.toml")) else {
        return Ok(Vec::new());
    };
    let mut deps = Vec::new();
    let mut en_deps = false;
    for linea in fuente.lines() {
        let linea = linea.split_once('#').map_or(linea, |(a, _)| a).trim();
        if linea.is_empty() {
            continue;
        }
        if let Some(sec) = linea.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            en_deps = sec.trim() == "dependencies";
            continue;
        }
        if en_deps
            && let Some((clave, valor)) = linea.split_once('=')
            && let Some(val) = valor.trim().strip_prefix('"').and_then(|v| v.strip_suffix('"'))
        {
            deps.push((clave.trim().to_string(), val.to_string()));
        }
    }
    Ok(deps)
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
    fn distingue_path_dep_de_git() {
        // M40.8a: una path-dep se reconoce por el prefijo `path:` y NO es una git spec.
        assert_eq!(ruta_de_path_dep("path:../pkgs/net"), Some("../pkgs/net"));
        assert_eq!(ruta_de_path_dep("path:  packages/web  "), Some("packages/web"));
        assert_eq!(ruta_de_path_dep("git+https://x/geo@v1"), None);
        assert!(parse_spec("path:../pkgs/net").is_err()); // no es git → parse_spec la rechaza
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

    #[test]
    fn parsea_semver() {
        assert_eq!(semver("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(semver("1.2"), Some((1, 2, 0)));
        assert_eq!(semver("v2.0.0-rc1"), Some((2, 0, 0))); // corta el pre-release
        assert_eq!(semver("main"), None);
        assert_eq!(semver("abc123def"), None); // un commit no es semver
    }

    #[test]
    fn mvs_elige_o_falla() {
        let a = GitSpec { url: "u".into(), git_ref: "v1.0.0".into() };
        let b = GitSpec { url: "u".into(), git_ref: "v2.1.0".into() };
        // Misma URL, semver → gana el mayor.
        assert_eq!(mvs("x", &a, &b).unwrap(), b);
        assert_eq!(mvs("x", &b, &a).unwrap(), b);
        // URLs distintas → error (caché plana, un slot por nombre).
        let c = GitSpec { url: "otra".into(), git_ref: "v3.0.0".into() };
        assert!(mvs("x", &a, &c).unwrap_err().contains("conflicto"));
        // Ref no semver → error.
        let d = GitSpec { url: "u".into(), git_ref: "main".into() };
        assert!(mvs("x", &a, &d).unwrap_err().contains("conflicto"));
    }
}
