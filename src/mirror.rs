//! `ray registry mirror` (M368): publicar un paquete que vive DENTRO de otro repo (el `packages/web`
//! de un monorepo, el `ray_ds/` de un design system) como ESPEJO de solo lectura — un repo propio
//! con el paquete en la raíz, un tag por versión — y, si se pide, su entrada en el índice.
//!
//! Es la forma oficial de publicar desde un monorepo (hallazgo #140 de ray-ds): una dependencia
//! `git+URL@ref` es la raíz de un repo y el índice registra repos, así que el paquete necesita un
//! repo donde ser la raíz. Antes lo hacía `tools/publish-packages.sh` (M135) a mano para los
//! paquetes de raylang; ray-ds había reescrito lo mismo en un workflow. Pasos:
//!
//! 1. clonar el espejo (o iniciarlo si el repo remoto está vacío) y comprobar que `v<versión>` no
//!    existe (las versiones son inmutables: para publicar de nuevo se sube la versión);
//! 2. volcar el paquete — todo menos `.git`, `.ray-deps/` y `ray.lock` — sobre el clon vacío;
//! 3. reescribir en `ray.toml` las dependencias hermanas por ruta (`x = "path:../x"`) a su URL git
//!    pública al tag de la versión ACTUAL del hermano (el espejo es autocontenido: el consumidor
//!    no tiene el monorepo al lado); lo mismo en el README, que además recibe el bloque de
//!    instalación (`ray add <pkg>` / dependencia git directa) y el aviso de espejo;
//! 4. commit `"<pkg> <versión> (from <repo fuente>@<sha>)"`, tag `v<versión>`, push de `main` y
//!    del tag;
//! 5. con `--index URL`: clonar ese índice, correr el `publish` normal contra el clon del espejo
//!    (valida, chequea y hashea el contenido del tag), commit `"publish: <pkg>@<versión>"` y push.
//!
//! El trabajo ocurre en un temporal que se borra siempre; el working tree del paquete no se toca.

use std::path::{Path, PathBuf};
use std::process::{self, Command};

use crate::manifest::Manifest;

const USAGE: &str = "\
usage: ray registry mirror <repo> [--public URL] [--index GIT_URL] [--sign] [--readme-only]

  <repo>           the mirror repository to push to (ssh or https with credentials), e.g.
                   git@github.com:ray-language/web.git
  --public URL     the anonymous https URL of the mirror written in the index and the README
                   (default: derived from <repo>: git@host:org/x.git -> https://host/org/x)
  --index GIT_URL  also publish the version in that index (clone, publish, commit, push)
  --sign           sign the index entry (see `ray registry publish --sign`)
  --readme-only    only refresh README and LICENSE on the mirror's main (no tag, no index)

Run it inside the package directory (the one with its ray.toml).";

struct Opts {
    repo: String,
    public: String,
    index: Option<String>,
    sign: bool,
    readme_only: bool,
}

pub fn run(args: &[String]) {
    let opts = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            process::exit(64);
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let m = match Manifest::load(&cwd) {
        Ok(Some(m)) => m,
        Ok(None) => {
            eprintln!("no package here: missing 'ray.toml' (run it inside the package directory)");
            process::exit(64);
        }
        Err(e) => {
            eprintln!("{e}");
            process::exit(65);
        }
    };
    if !crate::deps::valid_package_name(&m.name) {
        eprintln!("invalid package name '{}': only letters, digits, '-' and '_'", m.name);
        process::exit(65);
    }
    if crate::semver::parse_version(&m.version).is_none() {
        eprintln!("the package version '{}' is not valid semver: '{}'", m.name, m.version);
        process::exit(65);
    }
    let work = std::env::temp_dir().join(format!("ray-mirror-{}-{}", m.name, process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let result = if opts.readme_only { refresh_readme(&m, &opts, &work) } else { mirror(&m, &opts, &work) };
    let _ = std::fs::remove_dir_all(&work);
    if let Err(e) = result {
        eprintln!("{e}");
        process::exit(65);
    }
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut repo: Option<String> = None;
    let mut public: Option<String> = None;
    let mut index: Option<String> = None;
    let mut sign = false;
    let mut readme_only = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--public" => public = Some(it.next().ok_or("--public requires a URL")?.clone()),
            "--index" => index = Some(it.next().ok_or("--index requires a git URL")?.clone()),
            "--sign" => sign = true,
            "--readme-only" => readme_only = true,
            other if other.starts_with('-') => return Err(format!("unrecognized argument: '{other}'")),
            other if repo.is_none() => repo = Some(other.to_string()),
            other => return Err(format!("unexpected argument: '{other}'")),
        }
    }
    let repo = repo.ok_or("the mirror repository is required")?;
    let public = match public {
        Some(p) => p.trim_end_matches('/').to_string(),
        None => public_url_of(&repo).ok_or_else(|| {
            format!("cannot derive the public https URL of '{repo}'; pass --public URL")
        })?,
    };
    Ok(Opts { repo, public, index, sign, readme_only })
}

/// La URL https anónima de un repo dado por ssh (`git@host:org/x.git`, `ssh://git@host/org/x`)
/// o https (`https://host/org/x.git`): sin `.git`. Pura (testeable).
fn public_url_of(repo: &str) -> Option<String> {
    let strip = |s: &str| s.trim_end_matches('/').trim_end_matches(".git").to_string();
    if let Some(rest) = repo.strip_prefix("https://") {
        return Some(format!("https://{}", strip(rest)));
    }
    if let Some(rest) = repo.strip_prefix("ssh://") {
        let rest = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
        return Some(format!("https://{}", strip(rest)));
    }
    if let Some((user_host, path)) = repo.split_once(':')
        && !user_host.contains('/')
    {
        let host = user_host.split_once('@').map(|(_, h)| h).unwrap_or(user_host);
        return Some(format!("https://{host}/{}", strip(path)));
    }
    None
}

/// La URL pública de un paquete HERMANO en la misma organización que el espejo
/// (`https://host/org/x` → `https://host/org/<name>`; se conserva el sufijo `.git` si lo hay).
fn sibling_public_url(public: &str, name: &str) -> String {
    let suffix = if public.ends_with(".git") { ".git" } else { "" };
    match public.rsplit_once('/') {
        Some((org, _)) => format!("{org}/{name}{suffix}"),
        None => format!("{public}/{name}{suffix}"),
    }
}

fn git(args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut cmd = Command::new("git");
    if let Some(d) = cwd {
        cmd.arg("-C").arg(d);
    }
    cmd.args(args);
    let output = cmd.output().map_err(|e| format!("could not run 'git': {e} (is it installed?)"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Clona el espejo en `dest`, o lo inicia con `origin` apuntando al repo si este está vacío o
/// aún no existe (el push lo creará si el remoto lo permite).
fn clone_or_init(repo: &str, dest: &Path) -> Result<(), String> {
    if git(&["clone", "--quiet", repo, &dest.to_string_lossy()], None).is_ok() {
        return Ok(());
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("could not create '{}': {e}", dest.display()))?;
    git(&["init", "--quiet"], Some(dest))?;
    git(&["remote", "add", "origin", repo], Some(dest))?;
    Ok(())
}

/// ¿Existe el tag en el remoto? (`git ls-remote --tags`).
fn remote_has_tag(repo: &str, tag: &str) -> Result<bool, String> {
    let out = git(&["ls-remote", "--tags", repo, &format!("refs/tags/{tag}")], None)?;
    Ok(!out.trim().is_empty())
}

fn configure_identity(dir: &Path) -> Result<(), String> {
    // Un runner sin identidad git no puede commitear; la del espejo es la del comando.
    if git(&["config", "user.email"], Some(dir)).map(|s| s.trim().is_empty()).unwrap_or(true) {
        git(&["config", "user.email", "ray-mirror@users.noreply.github.com"], Some(dir))?;
        git(&["config", "user.name", "ray registry mirror"], Some(dir))?;
    }
    Ok(())
}

/// El `<repo>@<sha>` del repo que contiene el paquete, para el mensaje de commit del espejo.
fn source_stamp(root: &Path) -> String {
    let sha = git(&["rev-parse", "--short", "HEAD"], Some(root)).map(|s| s.trim().to_string()).unwrap_or_default();
    if sha.is_empty() {
        return String::new();
    }
    let top = git(&["rev-parse", "--show-toplevel"], Some(root)).map(|s| s.trim().to_string()).unwrap_or_default();
    let name = Path::new(&top).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    format!(" (from {name}@{sha})")
}

/// Vuelca el paquete sobre el clon: borra todo lo que no sea `.git` y copia todo menos `.git`,
/// `.ray-deps/` y `ray.lock` (artefactos del publicador, no del paquete).
fn snapshot(src: &Path, dest: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(dest).map_err(|e| format!("could not read '{}': {e}", dest.display()))?.flatten() {
        if entry.file_name() == ".git" {
            continue;
        }
        let p = entry.path();
        if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) }
            .map_err(|e| format!("could not clear '{}': {e}", p.display()))?;
    }
    copy_tree(src, dest, src)
}

fn copy_tree(dir: &Path, dest_root: &Path, src_root: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("could not read '{}': {e}", dir.display()))?.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = p.strip_prefix(src_root).unwrap_or(&p);
        if dir == src_root && (name == ".git" || name == ".ray-deps" || name == "ray.lock") {
            continue;
        }
        let target = dest_root.join(rel);
        if p.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| format!("could not create '{}': {e}", target.display()))?;
            copy_tree(&p, dest_root, src_root)?;
        } else {
            std::fs::copy(&p, &target).map_err(|e| format!("could not copy '{}': {e}", p.display()))?;
        }
    }
    Ok(())
}

/// Las dependencias hermanas por ruta del paquete, con la versión que declara cada una AHORA en
/// el repo fuente: `(nombre, versión)`. Un hermano sin versión es un error (el espejo quedaría
/// apuntando a un tag inexistente).
fn sibling_versions(m: &Manifest) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (name, spec) in &m.dependencies {
        let Some(p) = crate::deps::path_of_path_dep(spec) else { continue };
        let dir = m.root.join(p);
        let version = Manifest::load(&dir)
            .ok()
            .flatten()
            .filter(|dm| dm.root == dir.canonicalize().unwrap_or(dir.clone()) || dm.root == dir)
            .map(|dm| dm.version)
            .ok_or_else(|| format!("'{}' depends on '{name}' by path ('{p}') but it has no ray.toml with a version", m.name))?;
        out.push((name.clone(), version));
    }
    Ok(out)
}

/// Reescribe en un texto (ray.toml o README) cada `name = "path:…"` a su dependencia git pública
/// pinneada: `name = "git+https://host/org/name@vX"`. Línea a línea; pura.
fn rewrite_path_deps(text: &str, public: &str, siblings: &[(String, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let mut replaced = None;
        if let Some((lhs, _)) = trimmed.split_once('=') {
            let key = lhs.trim();
            if let Some((_, v)) = siblings.iter().find(|(n, _)| n == key)
                && trimmed[lhs.len() + 1..].trim_start().starts_with("\"path:")
            {
                let indent = &line[..line.len() - trimmed.len()];
                let eol = if line.ends_with('\n') { "\n" } else { "" };
                replaced = Some(format!("{indent}{key} = \"git+{}@v{v}\"{eol}", sibling_public_url(public, key)));
            }
        }
        out.push_str(replaced.as_deref().unwrap_or(line));
    }
    out
}

const MIRROR_MARKERS: [&str; 2] = ["Read-only mirror", "Espejo de solo lectura"];

/// El README público del espejo: bloques ```raylang etiquetados ```rust (GitHub aún no colorea
/// raylang), las dependencias por ruta reescritas y, tras el título, el aviso de espejo con las
/// instrucciones de instalación. Idempotente (el aviso se añade una vez). Pura.
fn public_readme(text: &str, name: &str, version: &str, public: &str, source: Option<&str>, siblings: &[(String, String)]) -> String {
    let body = rewrite_path_deps(text, public, siblings).replace("\n```raylang\n", "\n```rust\n");
    let body = if body.starts_with("```raylang\n") { body.replacen("```raylang\n", "```rust\n", 1) } else { body };
    if MIRROR_MARKERS.iter().any(|m| body.contains(m)) {
        return body;
    }
    let from = match source {
        Some(s) => format!(" — published from [`{s}`]({s}); development and pull requests go there"),
        None => String::new(),
    };
    let header = format!(
        "> **Read-only mirror**{from}.\n\
         >\n\
         > **Install** — `ray add {name}` in your project (the official index is the default), or by\n\
         > hand in `ray.toml`:\n\
         >\n\
         > ```toml\n\
         > [dependencies]\n\
         > {name} = \"^{version}\"\n\
         > ```\n\
         >\n\
         > Without an index, the direct git dependency:\n\
         > `{name} = \"git+{public}@v{version}\"`.\n\n"
    );
    match body.split_once('\n') {
        Some((title, rest)) if title.starts_with("# ") => format!("{title}\n\n{header}{rest}"),
        _ => format!("{header}{body}"),
    }
}

/// La URL pública del paquete DENTRO de su repo fuente (`https://host/org/raylang/tree/main/packages/web`)
/// si el repo fuente tiene un `origin` en GitHub/GitLab; si no, nada.
fn source_url(root: &Path) -> Option<String> {
    let top = git(&["rev-parse", "--show-toplevel"], Some(root)).ok()?.trim().to_string();
    let origin = git(&["remote", "get-url", "origin"], Some(root)).ok()?.trim().to_string();
    let public = public_url_of(&origin)?;
    let rel = root.canonicalize().ok()?.strip_prefix(Path::new(&top).canonicalize().ok()?).ok()?.to_string_lossy().replace('\\', "/");
    Some(if rel.is_empty() { public } else { format!("{public}/tree/main/{rel}") })
}

fn write_readme(m: &Manifest, opts: &Opts, mirror: &Path, siblings: &[(String, String)]) -> Result<(), String> {
    let readme = mirror.join("README.md");
    if !readme.is_file() {
        return Ok(());
    }
    let text = std::fs::read_to_string(&readme).map_err(|e| format!("could not read README.md: {e}"))?;
    let out = public_readme(&text, &m.name, &m.version, &opts.public, source_url(&m.root).as_deref(), siblings);
    std::fs::write(&readme, out).map_err(|e| format!("could not write README.md: {e}"))
}

fn mirror(m: &Manifest, opts: &Opts, work: &Path) -> Result<(), String> {
    let tag = format!("v{}", m.version);
    let dir = work.join(&m.name);
    let already = remote_has_tag(&opts.repo, &tag)?;
    if already {
        // Un run anterior a medias (el tag subió, el índice no): solo falta la entrada.
        if opts.index.is_none() {
            return Err(format!(
                "{} {tag} already exists in the mirror (versions are immutable; bump the version to publish again)",
                m.name
            ));
        }
        println!("{} {tag} already exists in the mirror; publishing only its index entry", m.name);
        git(&["clone", "--quiet", &opts.repo, &dir.to_string_lossy()], None)?;
        git(&["checkout", "--quiet", &tag], Some(&dir))?;
    } else {
        clone_or_init(&opts.repo, &dir)?;
        configure_identity(&dir)?;
        snapshot(&m.root, &dir)?;
        let siblings = sibling_versions(m)?;
        if !siblings.is_empty() {
            let toml = dir.join("ray.toml");
            let text = std::fs::read_to_string(&toml).map_err(|e| format!("could not read ray.toml: {e}"))?;
            std::fs::write(&toml, rewrite_path_deps(&text, &opts.public, &siblings)).map_err(|e| format!("could not write ray.toml: {e}"))?;
            for (name, v) in &siblings {
                println!("{}: dependency {name} -> git+{}@v{v}", m.name, sibling_public_url(&opts.public, name));
            }
        }
        write_readme(m, opts, &dir, &siblings)?;
        git(&["add", "-A"], Some(&dir))?;
        let changed = git(&["diff", "--cached", "--quiet"], Some(&dir)).is_err();
        if changed {
            let msg = format!("{} {}{}", m.name, m.version, source_stamp(&m.root));
            git(&["commit", "--quiet", "-m", &msg], Some(&dir))?;
        } else {
            println!("{}: no content changes vs the mirror; tagging the current content as {tag}", m.name);
        }
        git(&["branch", "-M", "main"], Some(&dir))?;
        git(&["tag", &tag], Some(&dir))?;
        git(&["push", "--quiet", "-u", "origin", "main", &tag], Some(&dir))?;
        println!("{}: mirrored as {tag} at {}", m.name, opts.public);
    }
    let Some(index_url) = &opts.index else {
        println!(
            "note: to list it in an index, run `ray registry mirror {} --index <index git URL>` (or `ray registry publish --repo \"git+{}@{tag}\"` from a checkout of the mirror).",
            opts.repo, opts.public
        );
        return Ok(());
    };
    publish_entry(m, opts, work, &dir, index_url, &tag)
}

/// La entrada del índice: clon fresco del índice, el `publish` normal desde el clon del espejo
/// (valida y hashea el contenido del tag; la URL de la entrada es la pública), commit y push.
fn publish_entry(m: &Manifest, opts: &Opts, work: &Path, mirror_dir: &Path, index_url: &str, tag: &str) -> Result<(), String> {
    let index = work.join("index");
    git(&["clone", "--quiet", index_url, &index.to_string_lossy()], None)
        .map_err(|e| format!("could not clone the index '{index_url}': {e}"))?;
    configure_identity(&index)?;
    let entry_file = index.join(format!("{}.toml", m.name));
    if std::fs::read_to_string(&entry_file).map(|t| t.contains(&format!("[{}]", m.version))).unwrap_or(false) {
        println!("{} {} is already in the index (versions are immutable)", m.name, m.version);
        return Ok(());
    }
    let mm = Manifest::load(mirror_dir)?.ok_or("the mirror has no ray.toml")?;
    let spec = format!("git+{}@{tag}", opts.public);
    crate::cli::publish_in_index(&mm, Some(&spec), &index, opts.sign)?;
    git(&["add", "-A"], Some(&index))?;
    let msg = format!("publish: {}@{}", m.name, m.version);
    git(&["commit", "--quiet", "-m", &msg], Some(&index))?;
    git(&["push", "--quiet"], Some(&index))?;
    println!("index updated: {}@{}", m.name, m.version);
    Ok(())
}

/// `--readme-only`: solo el README público y el LICENSE en el `main` del espejo, sin tocar tags
/// (el hash del índice es el del contenido del TAG, así que es seguro). Para contenido nuevo el
/// camino es subir la versión y publicar normal.
fn refresh_readme(m: &Manifest, opts: &Opts, work: &Path) -> Result<(), String> {
    let dir = work.join(&m.name);
    git(&["clone", "--quiet", &opts.repo, &dir.to_string_lossy()], None).map_err(|e| format!("no mirror for '{}': {e}", m.name))?;
    configure_identity(&dir)?;
    for f in ["README.md", "LICENSE"] {
        if m.root.join(f).is_file() {
            std::fs::copy(m.root.join(f), dir.join(f)).map_err(|e| format!("could not copy {f}: {e}"))?;
        }
    }
    write_readme(m, opts, &dir, &sibling_versions(m)?)?;
    git(&["add", "-A"], Some(&dir))?;
    if git(&["diff", "--cached", "--quiet"], Some(&dir)).is_ok() {
        println!("{}: README/LICENSE already up to date", m.name);
        return Ok(());
    }
    git(&["commit", "--quiet", "-m", "docs: public-mirror README and LICENSE"], Some(&dir))?;
    git(&["push", "--quiet"], Some(&dir))?;
    println!("{}: README/LICENSE refreshed on main", m.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_urls_are_derived_from_ssh_and_https_remotes() {
        assert_eq!(public_url_of("git@github.com:ray-language/web.git").as_deref(), Some("https://github.com/ray-language/web"));
        assert_eq!(public_url_of("ssh://git@github.com/ray-language/web").as_deref(), Some("https://github.com/ray-language/web"));
        assert_eq!(public_url_of("https://github.com/ray-language/web.git").as_deref(), Some("https://github.com/ray-language/web"));
        assert_eq!(public_url_of("/tmp/local/repo"), None);
        assert_eq!(sibling_public_url("https://github.com/ray-language/web", "net"), "https://github.com/ray-language/net");
        assert_eq!(sibling_public_url("file:///tmp/org/web.git", "net"), "file:///tmp/org/net.git");
    }

    #[test]
    fn path_dependencies_become_pinned_git_dependencies() {
        let toml = "[dependencies]\nnet = \"path:../net\"\nother = \"^1.0\"\n  db = \"path:../db\"\n";
        let out = rewrite_path_deps(toml, "https://github.com/ray-language/web", &[("net".into(), "0.10.1".into()), ("db".into(), "0.5.1".into())]);
        assert_eq!(
            out,
            "[dependencies]\nnet = \"git+https://github.com/ray-language/net@v0.10.1\"\nother = \"^1.0\"\n  db = \"git+https://github.com/ray-language/db@v0.5.1\"\n"
        );
    }

    #[test]
    fn the_public_readme_gets_the_install_block_once_after_the_title() {
        let readme = "# `web` — the framework\n\nIntro.\n\n```raylang\nimport web/framework;\n```\n\nnet = \"path:../net\"\n";
        let out = public_readme(readme, "web", "0.6.0", "https://github.com/ray-language/web", Some("https://github.com/ray-language/raylang/tree/main/packages/web"), &[("net".into(), "0.10.1".into())]);
        assert!(out.starts_with("# `web` — the framework\n\n> **Read-only mirror** — published from [`https://github.com/ray-language/raylang/tree/main/packages/web`]"), "{out}");
        assert!(out.contains("> web = \"^0.6.0\"\n"), "{out}");
        assert!(out.contains("`web = \"git+https://github.com/ray-language/web@v0.6.0\"`"), "{out}");
        assert!(out.contains("\n```rust\nimport web/framework;\n```\n"), "{out}");
        assert!(out.contains("net = \"git+https://github.com/ray-language/net@v0.10.1\"\n"), "{out}");
        // Idempotente: una segunda pasada no añade otro aviso.
        let again = public_readme(&out, "web", "0.6.0", "https://github.com/ray-language/web", None, &[]);
        assert_eq!(again, out);
        // Sin título H1: el bloque va al principio.
        let no_title = public_readme("Just text.\n", "x", "1.0.0", "https://h/o/x", None, &[]);
        assert!(no_title.starts_with("> **Read-only mirror**.\n"), "{no_title}");
    }
}
