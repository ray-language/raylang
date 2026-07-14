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

/// ¿Es un **nombre de paquete** válido? (M51d) Solo letras/dígitos ASCII, `-` y `_`, empezando por
/// alfanumérico. La regla importa por **seguridad**, no por estética: el nombre viene del `ray.toml`
/// —incluido el de paquetes transitivos NO confiables— y se usa para construir rutas
/// (`.ray-deps/<nombre>`, `<índice>/<nombre>.toml`); sin esta valla, un nombre como `../../x`
/// escaparía de la caché (y el camino de re-descarga hace `remove_dir_all` sobre esa ruta).
pub fn valid_package_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// El error estándar para un nombre de paquete inválido, con el contexto de dónde apareció.
fn bad_name_err(name: &str, origin: &str) -> String {
    format!(
        "invalid package name '{name}' ({origin}): only letters, digits, '-' and '_', \
         starting with a letter or digit"
    )
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
        format!("unsupported dependency spec: '{spec}' (expected 'git+<URL>@<ref>')")
    })?;
    let (url, git_ref) = without_prefix.rsplit_once('@').ok_or_else(|| {
        format!(
            "the dependency '{spec}' does not fix a version (missing '@<tag>'); \
             a fixed version makes the build reproducible"
        )
    })?;
    if url.is_empty() || git_ref.is_empty() {
        return Err(format!("malformed dependency spec: '{spec}'"));
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
        .map_err(|e| format!("could not clone dependency '{name}' ({}): {e}", spec.url))?;
    // Checkout de la ref fijada. Sirve para tags, ramas y SHAs (a diferencia de `clone --branch`).
    if let Err(e) = git(&["checkout", "--quiet", &spec.git_ref], Some(dest)) {
        let _ = std::fs::remove_dir_all(dest); // deja la caché limpia si la ref no existe
        return Err(format!("could not check out '{}' in '{name}': {e}", spec.git_ref));
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
        .map_err(|e| format!("could not run 'git': {e} (is it installed?)"))?;
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
    let Some(raw) = index_raw(manifest) else {
        return Ok(None);
    };
    // Índice remoto por git (M51c): se clona/cachea en `.ray-deps/.index` y se usa como dir local.
    if raw.strip_prefix("git+").is_some() {
        let cache = manifest.root.join(".ray-deps").join(".index");
        ensure_index_clone(&raw, &cache)?;
        return Ok(Some(cache));
    }
    let p = Path::new(&raw);
    Ok(Some(if p.is_absolute() { p.to_path_buf() } else { manifest.root.join(&raw) }))
}

/// La spec cruda del índice configurado: `RAY_INDEX`, o `[registry] index` del `ray.toml`.
fn index_raw(manifest: &Manifest) -> Option<String> {
    std::env::var("RAY_INDEX")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| manifest.registry_index.clone())
}

/// El **mirror de paquetes** configurado (M90.1): `RAY_MIRROR`, o `[registry] mirror` del `ray.toml`.
/// Un mirror NO es otro índice (mismo índice, otra URL de descarga): la identidad del paquete —la URL
/// que ven el lock y el MVS— sigue siendo la original; el mirror es solo transporte.
fn mirror_raw(manifest: &Manifest) -> Option<String> {
    std::env::var("RAY_MIRROR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| manifest.registry_mirror.clone())
}

/// Reescribe la URL de un paquete al mirror: `prefijo/<url-sin-esquema>` (estilo proxy de Go: el
/// mirror sirve los mismos repos bajo su prefijo, direccionados por host+ruta originales).
pub(crate) fn mirror_url(url: &str, prefix: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    format!("{}/{}", prefix.trim_end_matches('/'), rest.trim_start_matches('/'))
}

/// Descarga `spec` intentando primero el mirror (si hay) y cayendo a la URL original si falla —
/// el hash del lock/índice verifica el contenido venga de donde venga (mirror *trustless*).
fn fetch_mirrored(
    name: &str,
    spec: &GitSpec,
    dest: &Path,
    mirror: Option<&str>,
) -> Result<String, String> {
    if let Some(prefix) = mirror {
        let mirrored =
            GitSpec { url: mirror_url(&spec.url, prefix), git_ref: spec.git_ref.clone() };
        match fetch(name, &mirrored, dest) {
            Ok(commit) => return Ok(commit),
            Err(e) => {
                let _ = std::fs::remove_dir_all(dest); // deja la caché limpia para el reintento
                eprintln!("  warning: the mirror did not serve '{name}' ({e}); the original URL is used");
            }
        }
    }
    fetch(name, spec, dest)
}

/// Parte una spec de índice remoto `git+URL[@ref]` en `(url, ref)`. La ref es opcional y no debe
/// confundirse con un `/` de la ruta (por eso el filtro `!r.contains('/')`).
fn parse_index_spec(raw: &str) -> (&str, Option<&str>) {
    let without = raw.strip_prefix("git+").unwrap_or(raw);
    match without.rsplit_once('@') {
        Some((u, r)) if !u.is_empty() && !r.contains('/') => (u, Some(r)),
        _ => (without, None),
    }
}

/// El archivo hermano de la caché del índice que registra **con qué spec** se clonó (M51d): si la
/// spec configurada cambia (otra URL u otra ref), la caché se descarta y se re-clona — antes se
/// quedaba obsoleta en silencio.
fn index_spec_file(cache: &Path) -> std::path::PathBuf {
    cache.with_extension("spec")
}

/// Clona el repo del índice en `cache` si aún no está (M51c). No re-clona en cada resolución (sería
/// lento y no determinista); `ray update` refresca (`refresh_index`). Con `@ref` en la spec, hace
/// checkout de él. M51d: registra la spec usada y **re-clona si cambió** desde el último clon.
fn ensure_index_clone(raw: &str, cache: &Path) -> Result<(), String> {
    let spec_file = index_spec_file(cache);
    if cache.exists() {
        if std::fs::read_to_string(&spec_file).is_ok_and(|s| s.trim() == raw) {
            return Ok(()); // cacheado con la misma spec; `ray update` lo refresca
        }
        // La spec cambió (URL o ref distinta) → descartar la caché y volver a clonar.
        let _ = std::fs::remove_dir_all(cache);
    }
    let (url, git_ref) = parse_index_spec(raw);
    if let Some(parent) = cache.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    git(&["clone", "--quiet", url, &cache.to_string_lossy()], None)
        .map_err(|e| format!("could not clone the package index ({url}): {e}"))?;
    if let Some(r) = git_ref
        && let Err(e) = git(&["checkout", "--quiet", r], Some(cache))
    {
        let _ = std::fs::remove_dir_all(cache);
        return Err(format!("could not check out '{r}' in the index: {e}"));
    }
    std::fs::write(&spec_file, raw)
        .map_err(|e| format!("could not register the index spec: {e}"))?;
    Ok(())
}

/// Refresca el índice remoto cacheado, si existe — para `ray update` (M51c). No-op para un índice
/// local (directorio) o si no hay caché. M51d: un índice **pinneado** (`@ref`) queda en checkout
/// *detached* donde `git pull` no funciona → se refresca con `fetch` + re-checkout de la ref
/// (`origin/<ref>` si es una rama; la ref a secas si es un tag/SHA).
pub fn refresh_index(manifest: &Manifest) -> Result<(), String> {
    let Some(raw) = index_raw(manifest).filter(|r| r.starts_with("git+")) else {
        return Ok(()); // índice local o sin índice → nada que refrescar
    };
    let cache = manifest.root.join(".ray-deps").join(".index");
    if !cache.exists() {
        return Ok(());
    }
    let (_url, git_ref) = parse_index_spec(&raw);
    match git_ref {
        None => git(&["pull", "--quiet"], Some(&cache))
            .map(|_| ())
            .map_err(|e| format!("could not refresh the package index: {e}")),
        Some(r) => {
            git(&["fetch", "--quiet", "--tags", "--force", "origin"], Some(&cache))
                .map_err(|e| format!("could not refresh the package index: {e}"))?;
            // Rama → seguir la punta remota; tag/SHA → la ref tal cual (posiblemente actualizada).
            let remote = format!("origin/{r}");
            if git(&["checkout", "--quiet", "--detach", &remote], Some(&cache)).is_err() {
                git(&["checkout", "--quiet", "--detach", r], Some(&cache)).map_err(|e| {
                    format!("could not re-check out '{r}' when refreshing the index: {e}")
                })?;
            }
            Ok(())
        }
    }
}

/// Resuelve la spec de una dependencia a una `GitSpec` descargable: `git+…` se parsea directo;
/// un **requisito de versión** (`1.2.0`/`^1.2`/…) se resuelve **por el índice** (M51a), respetando
/// el lock salvo `update` (M51c). Las `path:` se filtran antes de llegar aquí. Devuelve también el
/// **hash publicado** en el índice para la versión elegida (M51d; `None` para deps git directas o
/// sin hash publicado): `ensure` lo verifica contra el contenido descargado.
fn to_gitspec(
    name: &str,
    spec: &str,
    index: Option<&Path>,
    locked: Option<&GitSpec>,
    update: bool,
) -> Result<(GitSpec, Option<String>), String> {
    if crate::index::is_registry_spec(spec) {
        let dir = index.ok_or_else(|| {
            format!(
                "the dependency '{name} = \"{spec}\"' resolves by name, but no index is \
                 configured (declare '[registry] index = \"<dir>\"' in ray.toml or export RAY_INDEX)"
            )
        })?;
        crate::index::resolve_pinned(dir, name, spec, locked, update)
    } else {
        parse_spec(spec).map(|s| (s, None))
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
    let mirror = mirror_raw(manifest);
    // El `GitSpec` bloqueado por nombre (para `resolve_pinned`): reproducibilidad de los requisitos.
    let locked_spec = |name: &str| -> Option<GitSpec> {
        locked.get(name).map(|e| GitSpec { url: e.url.clone(), git_ref: e.git_ref.clone() })
    };

    // BFS del grafo. `chosen` = spec resuelto por nombre (tras MVS); `cached` = spec que esta
    // ejecución dejó en la caché (para re-descargar si un conflicto lo actualiza).
    let mut chosen: std::collections::HashMap<String, GitSpec> = std::collections::HashMap::new();
    let mut cached: std::collections::HashMap<String, GitSpec> = std::collections::HashMap::new();
    // M51d: hash **publicado en el índice** por paquete (con la spec a la que corresponde), para
    // verificar el contenido descargado contra lo que el índice avala (cierra el TOFU del lock).
    let mut index_hash: std::collections::HashMap<String, (GitSpec, String)> =
        std::collections::HashMap::new();
    let mut queue: std::collections::VecDeque<(String, GitSpec)> = std::collections::VecDeque::new();
    let mut downloaded = 0usize;
    let enqueue = |n: &str,
                   s: &str,
                   queue: &mut std::collections::VecDeque<(String, GitSpec)>,
                   index_hash: &mut std::collections::HashMap<String, (GitSpec, String)>|
     -> Result<(), String> {
        let (gs, h) = to_gitspec(n, s, index.as_deref(), locked_spec(n).as_ref(), update)?;
        if let Some(h) = h {
            index_hash.insert(n.to_string(), (gs.clone(), h));
        }
        queue.push_back((n.to_string(), gs));
        Ok(())
    };
    for (n, s) in &manifest.dependencies {
        if !valid_package_name(n) {
            return Err(bad_name_err(n, "declared in ray.toml"));
        }
        if path_of_path_dep(s).is_some() {
            continue; // M40.8a: las path-deps son locales; no se descargan (las registra el CLI)
        }
        enqueue(n, s, &mut queue, &mut index_hash)?;
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
                eprintln!("  downloading {name} ({}@{})", chosen_spec.url, chosen_spec.git_ref);
                fetch_mirrored(&name, &chosen_spec, &dest, mirror.as_deref())?;
                downloaded += 1;
            }
            cached.insert(name.clone(), chosen_spec.clone());
        }

        // Dependencias transitivas: leer el `ray.toml` del paquete y encolarlas (saltando path-deps).
        // Una transitiva también puede ser del índice (`foo = "^1.2"`) → se resuelve igual.
        let (pkg_deps, pkg_registry) = package_deps(&dest)?;
        // M51e (aviso de *dependency confusion*, DESIGN §54.7): las deps por nombre de una
        // transitiva se resuelven contra el índice de ESTE proyecto. Si el paquete declara su
        // PROPIO índice y difiere, el mismo nombre podría referirse a otro paquete allí → avisar
        // (solo si de verdad tiene deps por nombre; el lock + hash del índice mitigan después).
        if let Some(pr) = &pkg_registry
            && index_raw(manifest).as_deref() != Some(pr.as_str())
            && pkg_deps.iter().any(|(_, s)| crate::index::is_registry_spec(s) && path_of_path_dep(s).is_none())
        {
            eprintln!(
                "  warning: '{name}' declares its own index ('{pr}'); its dependencies by name \
                 are resolved against THIS project's index (risk of dependency confusion \
                 if the names differ between indexes)"
            );
        }
        for (dn, ds) in pkg_deps {
            // M51d: el `ray.toml` de una transitiva NO es confiable — validar su nombre ANTES de
            // usarlo en cualquier ruta (es la valla contra `../../x` → escape de la caché).
            if !valid_package_name(&dn) {
                return Err(bad_name_err(&dn, &format!("declared by dependency '{name}'")));
            }
            if path_of_path_dep(&ds).is_some() {
                continue;
            }
            enqueue(&dn, &ds, &mut queue, &mut index_hash)?;
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
                "the dependency '{name}' does not match 'ray.lock': its content changed from what \
                 was locked (possible tampering).\n  expected: {}\n  actual:   {}\n  If the change is \
                 legitimate, delete '.ray-deps/{name}' and 'ray.lock' and resolve again.",
                b.hash, hash
            ));
        }
        // M51d: verificar contra el hash **publicado en el índice** (si esa versión lo trae). A
        // diferencia del lock (TOFU: confía en la primera descarga), esto ancla la confianza en el
        // índice: lo descargado debe ser EXACTAMENTE lo que el autor publicó.
        if let Some((ispec, ihash)) = index_hash.get(name)
            && ispec == spec
            && *ihash != hash
        {
            return Err(format!(
                "the dependency '{name}' does not match the hash published in the index (possible \
                 tampering of the package repository).\n  published: {ihash}\n  downloaded: {hash}"
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
        "version conflict for dependency '{name}': '{}@{}' and '{}@{}' are requested, \
         irreconcilable (different URLs or non-semver refs). Pin a single version.",
        a.url, a.git_ref, b.url, b.git_ref
    ))
}

/// Parsea un ref semver `vX.Y.Z[-pre]` / `X.Y.Z[-pre]` a una [`crate::semver::Version`] completa
/// (M51e: la pre-release ya NO se recorta — `v2.0.0-rc1 < v2.0.0` en el orden de `mvs`, y el
/// lock-pinning de una rc casa solo contra un requisito que la mencione). `None` si no es semver
/// (un commit, una rama…).
pub(crate) fn semver(git_ref: &str) -> Option<crate::semver::Version> {
    let core = git_ref.strip_prefix('v').unwrap_or(git_ref);
    crate::semver::parse_version(core)
}

/// Las dependencias declaradas en el `ray.toml` de un paquete descargado (su `[dependencies]`),
/// para la resolución transitiva, más el índice `[registry] index` que ese paquete declare (M51e:
/// solo para AVISAR de una posible *dependency confusion*; las transitivas se resuelven contra el
/// índice del CONSUMIDOR). Vacío si el paquete no tiene `ray.toml` (paquete hoja). Lenient: no
/// exige `name`/`version` (a un paquete-dependencia solo le miramos sus dependencias).
type PackageMeta = (Vec<(String, String)>, Option<String>); // (dependencias, índice propio)

fn package_deps(pkg_dir: &Path) -> Result<PackageMeta, String> {
    let Ok(source) = std::fs::read_to_string(pkg_dir.join("ray.toml")) else {
        return Ok((Vec::new(), None));
    };
    let mut deps = Vec::new();
    let mut registry = None;
    let mut section = String::new();
    for line in source.lines() {
        let line = line.split_once('#').map_or(line, |(a, _)| a).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(s) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = s.trim().to_string();
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && let Some(val) = value.trim().strip_prefix('"').and_then(|v| v.strip_suffix('"'))
        {
            match (section.as_str(), key.trim()) {
                ("dependencies", k) => deps.push((k.to_string(), val.to_string())),
                ("registry", "index") => registry = Some(val.to_string()),
                _ => {}
            }
        }
    }
    Ok((deps, registry))
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
            .map_err(|e| format!("could not read '{}': {e}", abs.display()))?;
        summary.push_str(rel);
        summary.push(':');
        summary.push_str(&crate::sha256::sha256_hex(&content));
        summary.push('\n');
    }
    Ok(format!("sha256:{}", crate::sha256::sha256_hex(summary.as_bytes())))
}

/// Recolecta recursivamente los archivos bajo `dir` como `(ruta_relativa_a_base, ruta_absoluta)`,
/// saltando `.git`. Las rutas usan `/` (portable y determinista entre plataformas).
pub(crate) fn collect_files(
    base: &Path,
    dir: &Path,
    out: &mut Vec<(String, std::path::PathBuf)>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("could not list '{}': {e}", dir.display()))?;
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
                .ok_or_else(|| format!("ray.lock:{}: header without ']'", i + 1))?;
            current = Some(LockEntry {
                name: name.trim().to_string(),
                url: String::new(), git_ref: String::new(), commit: String::new(), hash: String::new(),
            });
            continue;
        }
        let (key, value) = line.split_once('=')
            .ok_or_else(|| format!("ray.lock:{}: expected 'key = value'", i + 1))?;
        let value = value.trim().strip_prefix('"').and_then(|v| v.strip_suffix('"'))
            .ok_or_else(|| format!("ray.lock:{}: the value must be in quotes", i + 1))?;
        let Some(e) = current.as_mut() else {
            return Err(format!("ray.lock:{}: key outside a [name] section", i + 1));
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

/// Los nombres presentes en `ray.lock` (M51f): tras `ray remove` + re-resolver, dice si la caché
/// `.ray-deps/<nombre>` sigue en uso (el paquete puede seguir siendo transitiva de otra dep).
pub fn locked_names(root: &Path) -> Vec<String> {
    read_lock(root).map(|m| m.keys().cloned().collect()).unwrap_or_default()
}

/// Las raíces de módulos de dependencias para el proyecto que contiene `dir`: el caché
/// `.ray-deps/` (git/registro, si existe) y el **padre** de cada dependencia por ruta
/// (`nombre = "path:<dir>"` — el loader busca `<raíz>/<nombre>/…`). No descarga nada: usa lo que
/// haya en disco (por eso sirve también para el LSP, que no debe tocar la red al diagnosticar).
/// Compartida por el CLI (`ray run/build/…`) y el LSP → un archivo diagnostica con las MISMAS
/// raíces con las que corre.
pub fn dependency_roots_for(dir: &Path) -> Vec<std::path::PathBuf> {
    let root = Manifest::find(dir)
        .and_then(|toml| toml.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| dir.to_path_buf());
    let cache = root.join(".ray-deps");
    let mut roots = Vec::new();
    if cache.is_dir() {
        roots.push(cache);
    }
    if let Ok(Some(m)) = Manifest::load(dir) {
        for (_name, spec) in &m.dependencies {
            if let Some(p) = path_of_path_dep(spec) {
                let pdir = m.root.join(p);
                if let Some(parent) = pdir.parent().map(Path::to_path_buf)
                    && pdir.exists()
                    && !roots.contains(&parent)
                {
                    roots.push(parent);
                }
            }
        }
    }
    roots
}

/// Escribe `ray.lock` en `root` con las entradas **ordenadas por nombre** (determinista → diffs
/// limpios en control de versiones). El lockfile SÍ se commitea (fija las versiones para el equipo).
fn write_lock(root: &Path, entries: &mut [LockEntry]) -> Result<(), String> {
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let mut s = String::from(
        "# ray.lock — locked versions and hashes of the dependencies (generated by 'ray').\n\
         # Committed to the repository. Do not edit by hand.\n",
    );
    for e in entries.iter() {
        s.push_str(&format!(
            "\n[{}]\nurl = \"{}\"\nref = \"{}\"\ncommit = \"{}\"\nhash = \"{}\"\n",
            e.name, e.url, e.git_ref, e.commit, e.hash
        ));
    }
    std::fs::write(root.join("ray.lock"), s)
        .map_err(|e| format!("could not write 'ray.lock': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_names_de_package() {
        // M51d: el nombre construye rutas (caché/índice) → charset estricto.
        assert!(valid_package_name("geo"));
        assert!(valid_package_name("mi-package_2"));
        assert!(valid_package_name("9lives"));
        assert!(!valid_package_name(""));
        assert!(!valid_package_name("../evil"));
        assert!(!valid_package_name("a/b"));
        assert!(!valid_package_name("a.b"));
        assert!(!valid_package_name("-a")); // no empieza por alfanumérico
        assert!(!valid_package_name("with spaces"));
    }

    #[test]
    fn reescribe_url_al_mirror() {
        // M90.1: `prefijo/<url-sin-esquema>`; el prefijo puede llevar `/` final y la URL puede no
        // tener esquema (se usa tal cual).
        assert_eq!(
            mirror_url("https://github.com/u/geo", "https://mirror.corp/git"),
            "https://mirror.corp/git/github.com/u/geo"
        );
        assert_eq!(
            mirror_url("ssh://git@host/geo", "https://mirror.corp/git/"),
            "https://mirror.corp/git/git@host/geo"
        );
        assert_eq!(
            mirror_url("file:///tmp/repos/geo", "file:///tmp/mirror"),
            "file:///tmp/mirror/tmp/repos/geo"
        );
    }

    #[test]
    fn parses_spec_git() {
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
    fn spec_errors() {
        assert!(parse_spec("https://x/geo@v1").unwrap_err().contains("git+"));
        assert!(parse_spec("git+https://x/geo").unwrap_err().contains("does not fix a version"));
        assert!(parse_spec("git+@v1").unwrap_err().contains("malformed"));
    }

    #[test]
    fn parses_semver() {
        use crate::semver::Version;
        assert_eq!(semver("v1.2.3"), Some(Version::new(1, 2, 3)));
        assert_eq!(semver("1.2"), Some(Version::new(1, 2, 0)));
        // M51e: la pre-release ya NO se recorta (v2.0.0-rc1 < v2.0.0 para mvs/lock-pinning).
        assert_eq!(
            semver("v2.0.0-rc1"),
            Some(Version { major: 2, minor: 0, patch: 0, pre: Some("rc1".to_string()) })
        );
        assert!(semver("v2.0.0-rc1") < semver("v2.0.0"));
        assert_eq!(semver("main"), None);
        assert_eq!(semver("abc123def"), None); // un commit no es semver
    }

    #[test]
    fn mvs_elige_o_fails() {
        let a = GitSpec { url: "u".into(), git_ref: "v1.0.0".into() };
        let b = GitSpec { url: "u".into(), git_ref: "v2.1.0".into() };
        // Misma URL, semver → gana el mayor.
        assert_eq!(mvs("x", &a, &b).unwrap(), b);
        assert_eq!(mvs("x", &b, &a).unwrap(), b);
        // URLs distintas → error (caché plana, un slot por nombre).
        let c = GitSpec { url: "other".into(), git_ref: "v3.0.0".into() };
        assert!(mvs("x", &a, &c).unwrap_err().contains("conflict"));
        // Ref no semver → error.
        let d = GitSpec { url: "u".into(), git_ref: "main".into() };
        assert!(mvs("x", &a, &d).unwrap_err().contains("conflict"));
    }
}
