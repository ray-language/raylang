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
pub fn path_of_path_dep(spec: &str) -> Option<&str> {
    spec.strip_prefix("path:").map(str::trim)
}

/// Parsea `git+<URL>@<ref>`. El prefijo `git+` marca el esquema (por ahora el único), y el
/// `@<ref>` es **obligatorio**: fijar una versión concreta (un tag o commit) es lo que hace el
/// build reproducible. Se parte por el **último** `@` para no romper URLs con `usuario@host`.
pub fn parse_spec(spec: &str) -> Result<GitSpec, String> {
    let without_prefix = spec.strip_prefix("git+").ok_or_else(|| {
        format!("spec de dependencia no soportada: '{spec}' (se esperaba 'git+<URL>@<ref>')")
    })?;
    let (url, git_ref) = without_prefix.rsplit_once('@').ok_or_else(|| {
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
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent); // asegura `.ray-deps/`
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
    let output = cmd
        .output()
        .map_err(|e| format!("no se pudo ejecutar 'git': {e} (¿está instalado?)"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Asegura que **todo el grafo** de dependencias (directas y **transitivas**, M39c-3) esté en la
/// caché `.ray-deps/` y **verifica su integridad** contra `ray.lock` (M39c-2b). Es un BFS sobre el
/// grafo: por cada paquete descargado se lee su propio `ray.toml` y se encolan SUS dependencias.
/// Ciclos seguros (mapa de elegidos). Conflictos (mismo nombre, distinto spec): **MVS ligero** —el
/// mayor tag semver de la misma URL, o error si no son comparables (caché plana: un slot por nombre)—.
/// Para cada paquete recomputa el hash y lo compara con el bloqueado; un desajuste = *supply-chain*.
/// El directorio del **índice de paquetes** (M51a), para resolver deps por nombre. Precedencia:
/// la variable de entorno `RAY_INDEX`, luego `[registry] index` del `ray.toml` (relativo a la raíz
/// si no es absoluto). `Ok(None)` = sin índice (solo deps git/`path:`). Un índice remoto por git → M51c.
pub(crate) fn index_dir(manifest: &Manifest) -> Result<Option<std::path::PathBuf>, String> {
    let raw = std::env::var("RAY_INDEX")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| manifest.registry_index.clone());
    let Some(raw) = raw else {
        return Ok(None);
    };
    // Índice remoto por git (M51c): se clona/cachea en `.ray-deps/.index` y se usa como dir local.
    if let Some(without) = raw.strip_prefix("git+") {
        let cache = manifest.root.join(".ray-deps").join(".index");
        // `git+URL` o `git+URL@ref` (ref opcional; no confundir un '/' de la ruta con un ref).
        let (url, git_ref) = match without.rsplit_once('@') {
            Some((u, r)) if !u.is_empty() && !r.contains('/') => (u, Some(r)),
            _ => (without, None),
        };
        ensure_index_clone(url, git_ref, &cache)?;
        return Ok(Some(cache));
    }
    let p = Path::new(&raw);
    Ok(Some(if p.is_absolute() { p.to_path_buf() } else { manifest.root.join(&raw) }))
}

/// Clona el repo del índice en `cache` si aún no está (M51c). No re-clona en cada resolución (sería
/// lento y no determinista); `ray update` refresca (`git pull`). Con `git_ref`, hace checkout de él.
fn ensure_index_clone(url: &str, git_ref: Option<&str>, cache: &Path) -> Result<(), String> {
    if cache.exists() {
        return Ok(()); // ya cacheado; `ray update` lo refresca
    }
    if let Some(parent) = cache.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    git(&["clone", "--quiet", url, &cache.to_string_lossy()], None)
        .map_err(|e| format!("no se pudo clonar el índice de paquetes ({url}): {e}"))?;
    if let Some(r) = git_ref
        && let Err(e) = git(&["checkout", "--quiet", r], Some(cache))
    {
        let _ = std::fs::remove_dir_all(cache);
        return Err(format!("no se pudo hacer checkout de '{r}' en el índice: {e}"));
    }
    Ok(())
}

/// Refresca el índice remoto cacheado (`git pull`), si existe. Para `ray update` (M51c). No-op para
/// un índice local (directorio) o si no hay caché.
pub fn refresh_index(manifest: &Manifest) -> Result<(), String> {
    let raw = std::env::var("RAY_INDEX")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| manifest.registry_index.clone());
    if !raw.is_some_and(|r| r.starts_with("git+")) {
        return Ok(()); // índice local o sin índice → nada que refrescar
    }
    let cache = manifest.root.join(".ray-deps").join(".index");
    if cache.exists() {
        git(&["pull", "--quiet"], Some(&cache))
            .map(|_| ())
            .map_err(|e| format!("no se pudo refrescar el índice de paquetes: {e}"))?;
    }
    Ok(())
}

/// Resuelve la spec de una dependencia a una `GitSpec` descargable: `git+…` se parsea directo;
/// un **requisito de versión** (`1.2.0`/`^1.2`/…) se resuelve **por el índice** (M51a), respetando
/// el lock salvo `update` (M51c). Las `path:` se filtran antes de llegar aquí.
fn to_gitspec(
    name: &str,
    spec: &str,
    index: Option<&Path>,
    locked: Option<&GitSpec>,
    update: bool,
) -> Result<GitSpec, String> {
    if crate::index::is_registry_spec(spec) {
        let dir = index.ok_or_else(|| {
            format!(
                "la dependencia '{name} = \"{spec}\"' se resuelve por nombre, pero no hay índice \
                 configurado (declara '[registry] index = \"<dir>\"' en ray.toml o exporta RAY_INDEX)"
            )
        })?;
        crate::index::resolve_pinned(dir, name, spec, locked, update)
    } else {
        parse_spec(spec)
    }
}

/// Asegura las dependencias respetando el lock (build reproducible). Ver `ensure` / `update`.
pub fn ensure(manifest: &Manifest) -> Result<usize, String> {
    ensure_impl(manifest, false)
}

/// Como `ensure`, pero **re-resuelve** las deps del índice a la versión más alta que satisface su
/// requisito (ignora el lock previo) y refresca el índice remoto — `ray update` (M51c).
pub fn update(manifest: &Manifest) -> Result<usize, String> {
    refresh_index(manifest)?;
    ensure_impl(manifest, true)
}

fn ensure_impl(manifest: &Manifest, update: bool) -> Result<usize, String> {
    let cache = manifest.root.join(".ray-deps");
    let locked = read_lock(&manifest.root)?;
    let index = index_dir(manifest)?;
    // El `GitSpec` bloqueado por nombre (para `resolve_pinned`): reproducibilidad de los requisitos.
    let locked_spec = |name: &str| -> Option<GitSpec> {
        locked.get(name).map(|e| GitSpec { url: e.url.clone(), git_ref: e.git_ref.clone() })
    };

    // BFS del grafo. `chosen` = spec resuelto por nombre (tras MVS); `cached` = spec que esta
    // ejecución dejó en la caché (para re-descargar si un conflicto lo actualiza).
    let mut chosen: std::collections::HashMap<String, GitSpec> = std::collections::HashMap::new();
    let mut cached: std::collections::HashMap<String, GitSpec> = std::collections::HashMap::new();
    let mut queue: std::collections::VecDeque<(String, GitSpec)> = std::collections::VecDeque::new();
    let mut downloaded = 0usize;
    for (n, s) in &manifest.dependencies {
        if path_of_path_dep(s).is_some() {
            continue; // M40.8a: las path-deps son locales; no se descargan (las registra el CLI)
        }
        queue.push_back((n.clone(), to_gitspec(n, s, index.as_deref(), locked_spec(n).as_ref(), update)?));
    }

    while let Some((name, spec)) = queue.pop_front() {
        // Elegir el spec: nuevo, o MVS con el ya elegido si difieren (conflicto).
        let chosen_spec = match chosen.get(&name) {
            None => spec,
            Some(prev) if *prev == spec => prev.clone(),
            Some(prev) => mvs(&name, prev, &spec)?,
        };
        let unchanged = chosen.get(&name) == Some(&chosen_spec);
        chosen.insert(name.clone(), chosen_spec.clone());
        if unchanged && cached.get(&name) == Some(&chosen_spec) {
            continue; // ya procesado con este spec (dedup / ciclo)
        }

        // Descargar (o re-descargar si esta ejecución tenía otra versión por un conflicto, o si lo
        // que hay EN DISCO —según el lock previo— no es la versión ya elegida: pasa al resolver por
        // el índice cambiando de versión entre ejecuciones, p. ej. tras `ray update` o un yank).
        let dest = cache.join(&name);
        let on_disk_stale = dest.exists()
            && locked.get(&name).is_some_and(|e| {
                e.git_ref != chosen_spec.git_ref || e.url != chosen_spec.url
            });
        if cached.get(&name) != Some(&chosen_spec) {
            if (cached.contains_key(&name) || on_disk_stale) && dest.exists() {
                let _ = std::fs::remove_dir_all(&dest); // upgrade dentro de esta resolución o vs. disco
            }
            if !dest.exists() {
                eprintln!("  descargando {name} ({}@{})", chosen_spec.url, chosen_spec.git_ref);
                fetch(&name, &chosen_spec, &dest)?;
                downloaded += 1;
            }
            cached.insert(name.clone(), chosen_spec.clone());
        }

        // Dependencias transitivas: leer el `ray.toml` del paquete y encolarlas (saltando path-deps).
        // Una transitiva también puede ser del índice (`foo = "^1.2"`) → se resuelve igual.
        for (dn, ds) in package_deps(&dest)? {
            if path_of_path_dep(&ds).is_some() {
                continue;
            }
            let gs = to_gitspec(&dn, &ds, index.as_deref(), locked_spec(&dn).as_ref(), update)?;
            queue.push_back((dn, gs));
        }
    }

    // Verificar el hash de cada paquete elegido contra el lock y reescribir `ray.lock`.
    let mut new_lock: Vec<LockEntry> = Vec::new();
    for (name, spec) in &chosen {
        let dest = cache.join(name);
        let commit = rev_parse(&dest).unwrap_or_default();
        let hash = hash_package(&dest)?;
        if let Some(b) = locked.get(name)
            && b.url == spec.url
            && b.git_ref == spec.git_ref
            && b.hash != hash
        {
            return Err(format!(
                "la dependencia '{name}' no coincide con 'ray.lock': su contenido cambió desde que \
                 se bloqueó (posible manipulación).\n  esperado: {}\n  actual:   {}\n  Si el cambio es \
                 legítimo, borra '.ray-deps/{name}' y 'ray.lock' y vuelve a resolver.",
                b.hash, hash
            ));
        }
        new_lock.push(LockEntry {
            name: name.clone(),
            url: spec.url.clone(),
            git_ref: spec.git_ref.clone(),
            commit,
            hash,
        });
    }
    write_lock(&manifest.root, &mut new_lock)?;
    Ok(downloaded)
}

/// Selección de versión ante un conflicto (mismo nombre, distinto spec): con la **misma URL** y
/// refs **semver** (`vX.Y.Z`/`X.Y.Z`), gana el mayor (la mínima versión que satisface a ambos, estilo
/// Go-MVS reinterpretando `@vX` como "al menos vX"). Si las URLs difieren o los refs no son semver
/// comparables, es error: la caché es plana (un solo slot por nombre) y no se puede reconciliar.
fn mvs(name: &str, a: &GitSpec, b: &GitSpec) -> Result<GitSpec, String> {
    if a.url == b.url
        && let (Some(va), Some(vb)) = (semver(&a.git_ref), semver(&b.git_ref))
    {
        return Ok(if vb > va { b.clone() } else { a.clone() });
    }
    Err(format!(
        "conflicto de versiones para la dependencia '{name}': se pide '{}@{}' y '{}@{}', \
         irreconciliables (URLs distintas o refs no semver). Fija una sola versión.",
        a.url, a.git_ref, b.url, b.git_ref
    ))
}

/// Parsea un ref semver `vX.Y.Z` / `X.Y.Z` (ignora un sufijo de pre-release tras `-`) a `(mayor,
/// menor, parche)`. `None` si no es semver (un commit, una rama…). Para ordenar en `mvs`.
pub(crate) fn semver(git_ref: &str) -> Option<(u64, u64, u64)> {
    let core = git_ref.strip_prefix('v').unwrap_or(git_ref);
    let core = core.split('-').next().unwrap_or(core); // corta pre-release
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// Las dependencias declaradas en el `ray.toml` de un paquete descargado (su `[dependencies]`), para
/// la resolución transitiva. Vacío si el paquete no tiene `ray.toml` (paquete hoja). Lenient: no
/// exige `name`/`version` (a un paquete-dependencia solo le miramos sus dependencias).
fn package_deps(pkg_dir: &Path) -> Result<Vec<(String, String)>, String> {
    let Ok(source) = std::fs::read_to_string(pkg_dir.join("ray.toml")) else {
        return Ok(Vec::new());
    };
    let mut deps = Vec::new();
    let mut in_deps = false;
    for line in source.lines() {
        let line = line.split_once('#').map_or(line, |(a, _)| a).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_deps = section.trim() == "dependencies";
            continue;
        }
        if in_deps
            && let Some((key, value)) = line.split_once('=')
            && let Some(val) = value.trim().strip_prefix('"').and_then(|v| v.strip_suffix('"'))
        {
            deps.push((key.trim().to_string(), val.to_string()));
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
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    collect_files(dir, dir, &mut files)?;
    files.sort();
    let mut summary = String::new();
    for (rel, abs) in &files {
        let content = std::fs::read(abs)
            .map_err(|e| format!("no se pudo leer '{}': {e}", abs.display()))?;
        summary.push_str(rel);
        summary.push(':');
        summary.push_str(&crate::sha256::sha256_hex(&content));
        summary.push('\n');
    }
    Ok(format!("sha256:{}", crate::sha256::sha256_hex(summary.as_bytes())))
}

/// Recolecta recursivamente los archivos bajo `dir` como `(ruta_relativa_a_base, ruta_absoluta)`,
/// saltando `.git`. Las rutas usan `/` (portable y determinista entre plataformas).
fn collect_files(base: &Path, dir: &Path, out: &mut Vec<(String, std::path::PathBuf)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("no se pudo listar '{}': {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("error listando '{}': {e}", dir.display()))?;
        if entry.file_name() == *".git" {
            continue; // el historial de git no es parte del contenido del paquete
        }
        let path = entry.path();
        if path.is_dir() {
            collect_files(base, &path, out)?;
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
fn read_lock(root: &Path) -> Result<std::collections::HashMap<String, LockEntry>, String> {
    let path = root.join("ray.lock");
    let Ok(source) = std::fs::read_to_string(&path) else {
        return Ok(std::collections::HashMap::new()); // sin lock aún → mapa vacío
    };
    let mut map = std::collections::HashMap::new();
    let mut current: Option<LockEntry> = None;
    let close = |current: &mut Option<LockEntry>, map: &mut std::collections::HashMap<String, LockEntry>| {
        if let Some(e) = current.take() {
            map.insert(e.name.clone(), e);
        }
    };
    for (i, line) in source.lines().enumerate() {
        let line = line.split_once('#').map_or(line, |(a, _)| a).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            close(&mut current, &mut map);
            let name = rest.strip_suffix(']')
                .ok_or_else(|| format!("ray.lock:{}: cabecera sin ']'", i + 1))?;
            current = Some(LockEntry {
                name: name.trim().to_string(),
                url: String::new(), git_ref: String::new(), commit: String::new(), hash: String::new(),
            });
            continue;
        }
        let (key, value) = line.split_once('=')
            .ok_or_else(|| format!("ray.lock:{}: se esperaba 'clave = valor'", i + 1))?;
        let value = value.trim().strip_prefix('"').and_then(|v| v.strip_suffix('"'))
            .ok_or_else(|| format!("ray.lock:{}: el valor debe ir entre comillas", i + 1))?;
        let Some(e) = current.as_mut() else {
            return Err(format!("ray.lock:{}: clave fuera de una sección [nombre]", i + 1));
        };
        match key.trim() {
            "url" => e.url = value.to_string(),
            "ref" => e.git_ref = value.to_string(),
            "commit" => e.commit = value.to_string(),
            "hash" => e.hash = value.to_string(),
            _ => {} // claves desconocidas se ignoran (extensibilidad)
        }
    }
    close(&mut current, &mut map);
    Ok(map)
}

/// Escribe `ray.lock` en `root` con las entradas **ordenadas por nombre** (determinista → diffs
/// limpios en control de versiones). El lockfile SÍ se commitea (fija las versiones para el equipo).
fn write_lock(root: &Path, entries: &mut [LockEntry]) -> Result<(), String> {
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let mut s = String::from(
        "# ray.lock — versiones y hashes bloqueados de las dependencias (generado por 'ray').\n\
         # Se commitea al repositorio. No editar a mano.\n",
    );
    for e in entries.iter() {
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
        assert_eq!(path_of_path_dep("path:../pkgs/net"), Some("../pkgs/net"));
        assert_eq!(path_of_path_dep("path:  packages/web  "), Some("packages/web"));
        assert_eq!(path_of_path_dep("git+https://x/geo@v1"), None);
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
