//! M435 (rayblob #285, raydns #287): el lado SERVIDOR de `net/sigv4` (verificar firmas de
//! cabecera y de URL prefirmada, piezas públicas para los chunks firmados) y el códec
//! `net/dns_message` (leer consultas, escribir respuestas con compresión/TC/EDNS, leer respuestas
//! completas con rcode y SOA). Las suites raylang viven en `tests/fixtures/net_server/` — son las
//! de rayblob (los siete ejemplos de la doc de S3) y raydns (contra `net/dns`) traídas al paquete —
//! y corren con `ray test` sobre un proyecto que depende de `packages/net` por ruta: VM y nativo.

use std::path::PathBuf;
use std::process::Command;

fn project(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let d = std::env::temp_dir().join(format!("raylang_test_net_server_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::create_dir_all(d.join("tests")).unwrap();
    std::fs::write(
        d.join("ray.toml"),
        format!(
            "[package]\nname = \"netserver\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{}\"\n",
            root.join("packages/net").display()
        ),
    )
    .unwrap();
    std::fs::write(d.join("src/main.ray"), "fn main() {\n    print(\"ok\");\n}\n").unwrap();
    for f in ["sigv4_test.ray", "dns_message_test.ray"] {
        std::fs::copy(root.join("tests/fixtures/net_server").join(f), d.join("tests").join(f)).unwrap();
    }
    d
}

fn ray_test(dir: &PathBuf, args: &[&str]) -> String {
    let mut all = vec!["test"];
    all.extend_from_slice(args);
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(&all).current_dir(dir).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "ray test {args:?} falló\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn sigv4_server_side_and_dns_message_pass_on_the_vm() {
    let d = project("vm");
    let out = ray_test(&d, &[]);
    assert!(out.contains("result: 12 test(s), all passed ✓"), "{out}");
    assert!(out.contains("ok    presigned_url_round_trip"), "{out}");
    assert!(out.contains("ok    nxdomain_with_a_soa_in_authority"), "{out}");
}

#[test]
fn sigv4_server_side_and_dns_message_pass_natively() {
    if !has_rustc() {
        eprintln!("sin rustc: se omite el nativo");
        return;
    }
    let d = project("native");
    let out = ray_test(&d, &["--native"]);
    assert!(out.contains("running 12 test(s) — native binaries"), "{out}");
    assert!(out.contains("result: 12 test(s), all passed ✓"), "{out}");
}
