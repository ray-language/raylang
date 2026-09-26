// build.rs — M309 (findings #65): un binario compilado de la HEAD se llamaba igual que la release
// (`1.27.11`) y aceptaba código distinto. La versión completa lleva `+dev.<sha>` salvo cuando HEAD
// es exactamente el tag `v<versión>` (lo que construye el CI de release) o no hay git (tarball).
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");
    println!("cargo:rerun-if-env-changed=RAYLANG_RELEASE_BUILD");
    let suffix = if std::env::var_os("RAYLANG_RELEASE_BUILD").is_some() {
        String::new()
    } else {
        match (git(&["rev-parse", "--short=8", "HEAD"]), git(&["describe", "--tags", "--exact-match", "HEAD"])) {
            (Some(_), Some(tag)) if tag == format!("v{version}") => String::new(),
            (Some(sha), _) => {
                let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
                format!("+dev.{sha}{}", if dirty { ".dirty" } else { "" })
            }
            (None, _) => String::new(),
        }
    };
    println!("cargo:rustc-env=RAYLANG_VERSION_FULL={version}{suffix}");
    println!("cargo:rustc-env=RAYLANG_BUILD_SUFFIX={suffix}");
}
