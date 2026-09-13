//! M247 (IDEAS §89): `std/update` de punta a punta contra un servidor HTTP de juguete: manifiesto
//! firmado (Ed25519, firma en `update.json.sig` sobre los bytes crudos), artefacto `.zip` verificado
//! por tamaño y sha256, instalación por `apply_at` (swap con `<root>.old`), y los rechazos: firma con
//! otra clave, sha256 alterado. VM, intérprete y nativo; en el nativo `current()` es la versión
//! horneada del `ray.toml` (bajo `ray run` es "dev").

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Un `.zip` sin compresión (método 0) con las entradas dadas: lo que std/zip lee.
fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let n = data.len() as u32;
        let mut local = vec![0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&n.to_le_bytes());
        local.extend_from_slice(&n.to_le_bytes());
        local.extend_from_slice(&(name.len() as u16).to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&local);
        out.extend_from_slice(data);
        let mut c = vec![0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        c.extend_from_slice(&crc.to_le_bytes());
        c.extend_from_slice(&n.to_le_bytes());
        c.extend_from_slice(&n.to_le_bytes());
        c.extend_from_slice(&(name.len() as u16).to_le_bytes());
        c.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        c.extend_from_slice(&offset.to_le_bytes());
        c.extend_from_slice(name.as_bytes());
        central.extend_from_slice(&c);
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    let mut eocd = vec![0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0];
    eocd.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    eocd.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    eocd.extend_from_slice(&(central.len() as u32).to_le_bytes());
    eocd.extend_from_slice(&cd_offset.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&eocd);
    out
}

/// Servidor estático de juguete: sirve los archivos de `dir` por ruta; `/redir/x` → 302 a `/x`.
fn toy_server(dir: PathBuf) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            let resp = if let Some(rest) = path.strip_prefix("/redir") {
                format!("HTTP/1.1 302 Found\r\nLocation: {rest}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
            } else {
                match std::fs::read(dir.join(path.trim_start_matches('/'))) {
                    Ok(body) => {
                        let mut r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                        r.extend_from_slice(&body);
                        r
                    }
                    Err(_) => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                }
            };
            let _ = s.write_all(&resp);
        }
    });
    port
}

fn ray(dir: &Path, args: &[&str]) -> (String, String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string(), out.status.success())
}

/// Firma `msg` con la semilla dada usando el propio raylang (std/crypto); devuelve (pubkey_hex, sig_hex).
fn sign_with_ray(work: &Path, seed_hex: &str, msg: &[u8]) -> (String, String) {
    std::fs::write(work.join("msg.bin"), msg).unwrap();
    std::fs::write(
        work.join("sign.ray"),
        r#"import std/crypto;
import std/fs;
import std/update;
fn main() -> int {
    let seed = update.from_hex(args()[0]).unwrap();
    let msg = fs.read_file_bytes(args()[1]).unwrap();
    print(update.to_hex(crypto.ed25519_public_key(seed).unwrap()));
    print(update.to_hex(crypto.ed25519_sign(seed, msg).unwrap()));
    0
}
"#,
    )
    .unwrap();
    let (out, err, ok) = ray(work, &["run", "sign.ray", seed_hex, "msg.bin"]);
    assert!(ok, "sign.ray: {err}");
    let mut lines = out.lines();
    (lines.next().unwrap().to_string(), lines.next().unwrap().to_string())
}

const SEED: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const OTHER_SEED: &str = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb";

/// Monta el escenario: un artefacto con la app "2.0.0" para esta plataforma, el manifiesto y su
/// firma en `served/`, el proyecto de la app (ray.toml con [app] id/public_key, version 1.0.0) y
/// una "instalación" vieja en `install/MyApp` para `apply_at`.
fn scenario() -> (PathBuf, u16, String) {
    let base = std::env::temp_dir().join(format!("raylang_test_update_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let served = base.join("served");
    std::fs::create_dir_all(&served).unwrap();
    let work = base.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let key = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let zip = stored_zip(&[("MyApp/", b""), ("MyApp/VERSION", b"2.0.0\n"), ("MyApp/data/hello.txt", b"hola")]);
    std::fs::write(served.join("MyApp-2.0.0.zip"), &zip).unwrap();
    let mut hasher = sha256_simple::Sha256::new();
    hasher.update(&zip);
    let sha = hasher.hex();
    let manifest = format!(
        r#"{{"app": "com.example.myapp", "version": "2.0.0", "notes": "https://example.test/notes", "min_version": "0.5.0",
 "artifacts": {{ "{key}": {{ "url": "/redir/MyApp-2.0.0.zip", "sha256": "{sha}", "size": {} }} }} }}"#,
        zip.len()
    );
    // Las URLs del manifiesto son relativas al servidor de juguete: el programa las absolutiza.
    std::fs::write(served.join("update.json"), &manifest).unwrap();
    let (pubkey, sig) = sign_with_ray(&work, SEED, manifest.as_bytes());
    std::fs::write(served.join("update.json.sig"), &sig).unwrap();
    let (_, bad_sig) = sign_with_ray(&work, OTHER_SEED, manifest.as_bytes());
    std::fs::write(served.join("bad.json"), &manifest).unwrap();
    std::fs::write(served.join("bad.json.sig"), &bad_sig).unwrap();
    let tampered = manifest.replace(&sha, &format!("{}00", &sha[..62]));
    let (_, tsig) = sign_with_ray(&work, SEED, tampered.as_bytes());
    std::fs::write(served.join("tampered.json"), &tampered).unwrap();
    std::fs::write(served.join("tampered.json.sig"), &tsig).unwrap();
    // El proyecto de la app.
    let app = base.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!("[package]\nname = \"myapp\"\nversion = \"1.0.0\"\nentry = \"src/main.ray\"\n\n[app]\nid = \"com.example.myapp\"\npublic_key = \"{pubkey}\"\n"),
    )
    .unwrap();
    std::fs::write(
        app.join("src/main.ray"),
        r#"import std/update;
import std/fs;
fn main() -> int {
    let base = args()[0];
    let root = args()[1];
    print(update.app_id());
    print(update.current());
    let checked = match (update.check(base + "/update.json")) {
        Result.Ok(o) => o,
        Result.Err(e) => { print(e); return 1; },
    };
    let r = match (checked) {
        Option.Some(r) => r,
        Option.None => { print("up to date?"); return 1; },
    };
    print(r.version + " " + r.notes + " " + r.min_version + " " + to_string(r.size));
    // Las URLs del artefacto son relativas en este escenario.
    let rel = update.Release { version: r.version, notes: r.notes, min_version: r.min_version, url: base + r.url, sha256: r.sha256, size: r.size };
    let pkg = match (update.download(rel)) {
        Result.Ok(p) => p,
        Result.Err(e) => { print(e); return 1; },
    };
    print(fs.exists(pkg.path));
    match (update.apply_at(pkg, root)) {
        Result.Ok(p) => print("installed " + (if (p == root) { "ok" } else { p })),
        Result.Err(e) => { print(e); return 1; },
    }
    print(fs.read_file(root + "/VERSION").unwrap_or("?").trim());
    print(fs.read_file(root + "/data/hello.txt").unwrap_or("?"));
    print(fs.exists(root + ".old/VERSION") || platform() == "windows");
    match (update.check(base + "/bad.json")) {
        Result.Ok(_) => print("bad accepted"),
        Result.Err(e) => print(e.contains("signature")),
    }
    match (update.check(base + "/tampered.json")) {
        Result.Ok(o) => {
            match (o) {
                Option.Some(t) => {
                    let trel = update.Release { version: t.version, notes: t.notes, min_version: t.min_version, url: base + t.url, sha256: t.sha256, size: t.size };
                    match (update.download(trel)) { Result.Ok(_) => print("tampered accepted"), Result.Err(e) => print(e.contains("sha256")) }
                },
                Option.None => print("tampered none"),
            }
        },
        Result.Err(e) => print(e),
    }
    // Bajo ray run no hay instalación que reemplazar; en el binario, install_root() es su directorio.
    match (update.apply(pkg)) {
        Result.Ok(_) => print(update.current() != "dev"),
        Result.Err(e) => print(e.contains("not a bundled app")),
    }
    0
}
"#,
    )
    .unwrap();
    let port = toy_server(served);
    (base, port, pubkey)
}

fn fresh_install(base: &Path, name: &str) -> PathBuf {
    let root = base.join(name).join("MyApp");
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("VERSION"), "1.0.0\n").unwrap();
    root
}

fn want(current: &str) -> String {
    format!("com.example.myapp\n{current}\n2.0.0 https://example.test/notes 0.5.0 {{size}}\ntrue\ninstalled ok\n2.0.0\nhola\ntrue\ntrue\ntrue\ntrue\n")
}

fn normalize(s: &str) -> String {
    // El tamaño del zip varía con el nombre de la plataforma: se neutraliza.
    let mut out = String::new();
    for line in s.lines() {
        if line.starts_with("2.0.0 https://") {
            let mut parts: Vec<&str> = line.split(' ').collect();
            parts.pop();
            out.push_str(&parts.join(" "));
            out.push_str(" {size}\n");
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[test]
fn update_flow_on_vm_and_interpreter() {
    let (base, port, _) = scenario();
    let app = base.join("app");
    let url = format!("http://127.0.0.1:{port}");
    for (label, engine) in [("vm", &["run", "src/main.ray"][..]), ("interp", &["run", "--interp", "src/main.ray"][..])] {
        let root = fresh_install(&base, label);
        let mut args: Vec<&str> = engine.to_vec();
        let root_s = root.to_string_lossy().to_string();
        args.push(&url);
        args.push(&root_s);
        let (out, err, ok) = ray(&app, &args);
        assert!(ok, "{label}: {out}{err}");
        assert_eq!(normalize(&out), want("dev"), "{label}: {err}");
    }
}

#[test]
fn update_flow_natively_with_the_baked_identity() {
    let (base, port, _) = scenario();
    let app = base.join("app");
    let bin = app.join(if cfg!(windows) { "myapp.exe" } else { "myapp" });
    let (_, err, ok) = ray(&app, &["build", "--native", "src/main.ray", "-o", bin.to_str().unwrap(), "--without", "mimalloc,ahash,fibers"]);
    assert!(ok, "build --native: {err}");
    let root = fresh_install(&base, "native");
    let url = format!("http://127.0.0.1:{port}");
    let out = Command::new(&bin).arg(&url).arg(root.to_string_lossy().to_string()).current_dir(&app).output().unwrap();
    assert!(out.status.success(), "native: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    // Horneado: current() = 1.0.0 (el [package] version del ray.toml), y apply() ya no dice "dev".
    assert_eq!(normalize(&String::from_utf8_lossy(&out.stdout)), want("1.0.0"));
}

/// SHA-256 mínimo para el test (sin dependencias): la implementación de referencia de FIPS 180-4.
mod sha256_simple {
    pub struct Sha256 { data: Vec<u8> }
    impl Sha256 {
        pub fn new() -> Self { Sha256 { data: Vec::new() } }
        pub fn update(&mut self, d: &[u8]) { self.data.extend_from_slice(d); }
        pub fn hex(&self) -> String {
            const K: [u32; 64] = [
                0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
                0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
                0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
                0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
            ];
            let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
            let mut msg = self.data.clone();
            let bitlen = (msg.len() as u64) * 8;
            msg.push(0x80);
            while msg.len() % 64 != 56 { msg.push(0); }
            msg.extend_from_slice(&bitlen.to_be_bytes());
            for chunk in msg.chunks(64) {
                let mut w = [0u32; 64];
                for i in 0..16 { w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]); }
                for i in 16..64 {
                    let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                    let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                    w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
                }
                let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) = (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
                for i in 0..64 {
                    let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                    let ch = (e & f) ^ (!e & g);
                    let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
                    let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                    let maj = (a & b) ^ (a & c) ^ (b & c);
                    let t2 = s0.wrapping_add(maj);
                    hh = g; g = f; f = e; e = d.wrapping_add(t1); d = c; c = b; b = a; a = t1.wrapping_add(t2);
                }
                h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b); h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
                h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f); h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
            }
            h.iter().map(|x| format!("{x:08x}")).collect()
        }
    }
}
