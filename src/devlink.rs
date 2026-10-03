//! M330 D2 (hot reload móvil, `docs/diseno-hot-reload-movil.md`): el **enlace de desarrollo**
//! entre `ray dev --device` (el anfitrión, en el Mac) y la **librería de desarrollo** que corre
//! en el teléfono (o el cliente de prueba `ray dev-client` en escritorio). El anfitrión vigila el
//! proyecto, comprueba que compila y, en cada cambio, envía a los dispositivos conectados un
//! **snapshot del fuente** (`.ray`, `ray.toml`, `.ray.html`, assets embebidos y `.ray-deps`
//! ya resueltos); el dispositivo lo escribe en su sandbox, para el programa en curso (la
//! parada cooperativa de D1), deja el runtime limpio y carga → chequea → ejecuta el nuevo en su
//! VM. Fuente, no bytecode: el dispositivo lleva la toolchain entera.
//!
//! **Protocolo**: TCP en la LAN, marcos `[u32 BE len][u8 kind][payload]`. Del dispositivo al
//! anfitrión: `HELLO` (token, nombre del dispositivo, versión), `HASHES` (D4: lo que ya tiene —
//! `[u32 len][ruta][u64 hash]`*), `STATUS` (un octeto de estado + texto) y `LOG` (D4: un octeto
//! de flujo + una línea de `print`/`eprint`, la consola remota). Del anfitrión al dispositivo:
//! `SNAPSHOT` (entradas `[u32 len][ruta][u64 len][bytes]`, rutas relativas con `/`; reemplaza
//! todo) y `DELTA` (D4: `[u32 n]{[u32 len][ruta]}*` rutas borradas + las entradas cambiadas o
//! nuevas — el anfitrión lleva por dispositivo lo que ya tiene). Ambos reinician el programa.
//! Sin dependencias: todo con `std::net`. El token viaja en la URL `ray-dev://host:puerto/token`
//! que imprime `ray dev --device`; una conexión con otro token se cierra sin más.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Tipos de marco.
const KIND_HELLO: u8 = 1;
const KIND_STATUS: u8 = 2;
const KIND_LOG: u8 = 3;
const KIND_HASHES: u8 = 4;
const KIND_SNAPSHOT: u8 = 10;
const KIND_DELTA: u8 = 11;

/// Los archivos de un snapshot: (ruta relativa con `/`, bytes).
pub type Files = Vec<(String, Vec<u8>)>;
/// Un delta (D4): rutas borradas + entradas cambiadas o nuevas.
type Delta = (Vec<String>, Files);

/// D4: hash de contenido (FNV-1a 64) — solo para decidir qué viaja; nunca es de seguridad.
fn content_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Estados que reporta el dispositivo en `STATUS`.
pub const STATE_RUNNING: u8 = 1;
pub const STATE_FINISHED: u8 = 2;
pub const STATE_STOPPED: u8 = 3;
pub const STATE_COMPILE_ERROR: u8 = 4;
pub const STATE_RUNTIME_ERROR: u8 = 5;
pub const STATE_SNAPSHOT: u8 = 6;

/// Tope del snapshot (un proyecto raylang con sus dependencias cabe de sobra; por encima es que
/// se coló un artefacto — se avisa y no se manda).
const MAX_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;

/// Cuánto espera el dispositivo a que el programa viejo termine tras pedirle la parada antes
/// de rendirse (la decisión de diseño: salir del proceso; el shell vuelve a arrancar limpio).
const STOP_GRACE: Duration = Duration::from_secs(5);

// ── Marcos ──────────────────────────────────────────────────────────────────

fn write_frame(s: &mut TcpStream, kind: u8, payload: &[u8]) -> std::io::Result<()> {
    let len = (payload.len() + 1) as u32;
    s.write_all(&len.to_be_bytes())?;
    s.write_all(&[kind])?;
    s.write_all(payload)?;
    s.flush()
}

fn read_frame(s: &mut TcpStream) -> std::io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 4];
    s.read_exact(&mut head)?;
    let len = u32::from_be_bytes(head) as usize;
    if len == 0 || len as u64 > MAX_SNAPSHOT_BYTES + 1024 {
        return Err(std::io::Error::other(format!("bad frame length {len}")));
    }
    let mut body = vec![0u8; len];
    s.read_exact(&mut body)?;
    let kind = body[0];
    body.remove(0);
    Ok((kind, body))
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn get_str(buf: &[u8], pos: &mut usize) -> Option<String> {
    let n = u32::from_be_bytes(buf.get(*pos..*pos + 4)?.try_into().ok()?) as usize;
    *pos += 4;
    let s = std::str::from_utf8(buf.get(*pos..*pos + n)?).ok()?.to_string();
    *pos += n;
    Some(s)
}

// ── La URL del enlace ───────────────────────────────────────────────────────

/// El esquema genérico del enlace (cliente de escritorio, tecleo manual).
pub const GENERIC_SCHEME: &str = "ray-dev";

/// `<esquema>://host:port/token` → (host:port, token). El esquema es `ray-dev` o, D5, el id
/// del shell de desarrollo (`org.raylang.app.dev`): así un QR abre exactamente esa app.
pub fn parse_url(url: &str) -> Result<(String, String), String> {
    let (scheme, rest) = url.split_once("://").ok_or_else(|| format!("not a device link URL: {url}"))?;
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')) {
        return Err(format!("not a device link URL: {url}"));
    }
    let (addr, token) = rest
        .split_once('/')
        .ok_or_else(|| format!("the URL has no token: {url}"))?;
    if addr.is_empty() || token.is_empty() {
        return Err(format!("malformed device link URL: {url}"));
    }
    Ok((addr.to_string(), token.to_string()))
}

/// El esquema de una URL de enlace (`ray-dev`, o el id del shell).
pub fn url_scheme(url: &str) -> &str {
    url.split_once("://").map(|(s, _)| s).unwrap_or("")
}

/// D5: el enlace como código QR para la terminal (bloques Unicode, dos módulos por línea).
/// `None` si no cupo (no ocurre con una URL de enlace; el tope del QR son ~2 KB).
pub fn qr_text(url: &str) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use qrcode::render::unicode;
        let code = qrcode::QrCode::with_error_correction_level(url.as_bytes(), qrcode::EcLevel::L).ok()?;
        Some(code.render::<unicode::Dense1x2>().dark_color(unicode::Dense1x2::Light).light_color(unicode::Dense1x2::Dark).quiet_zone(true).build())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = url; // el playground no tiene terminal ni enlace de dispositivos
        None
    }
}

/// D5: el enlace que el shell recibe por su esquema URL (un QR escaneado con la cámara del
/// sistema). Lo deja aquí y el bucle del dispositivo lo recoge: en la página de emparejamiento
/// al instante, y con un enlace vivo al siguiente tick (cierra la conexión y vuelve a enlazar).
fn pending_link() -> &'static Mutex<Option<String>> {
    static P: std::sync::OnceLock<Mutex<Option<String>>> = std::sync::OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

/// Ofrece un enlace nuevo (D5). Se ignora si no parece una URL de enlace.
pub fn offer_link(url: &str) {
    if parse_url(url).is_ok() {
        *pending_link().lock().unwrap_or_else(|e| e.into_inner()) = Some(url.to_string());
    }
}

fn take_pending_link() -> Option<String> {
    pending_link().lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// D5: la entrada C del shell de desarrollo para un enlace recibido por su esquema URL.
///
/// # Safety
/// `url` debe ser un C-string NUL-terminated válido durante la llamada (NULL se ignora).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ray_dev_link(url: *const std::ffi::c_char) {
    if url.is_null() {
        return;
    }
    // SAFETY: contrato del shell — C-string válido durante la llamada; se copia antes de volver.
    let url = unsafe { std::ffi::CStr::from_ptr(url) }.to_string_lossy().into_owned();
    offer_link(&url);
}

/// La IP de LAN del anfitrión: la ruta por defecto, sin enviar nada (un UDP `connect` no manda
/// paquetes). Sin ruta (sin red), `127.0.0.1`.
fn lan_ip() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|_| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}

fn random_token() -> String {
    // Suficiente para un enlace de desarrollo en la LAN: 16 octetos del generador del runtime.
    let mut out = String::new();
    for _ in 0..4 {
        out.push_str(&format!("{:08x}", crate::builtins::random_int(i64::MAX) as u64 as u32));
    }
    out
}

// ── El snapshot ─────────────────────────────────────────────────────────────

/// Los archivos que viajan al dispositivo: `ray.toml`/`ray.lock`, todo `.ray`/`.ray.html` fuera
/// de artefactos y ocultos, los assets de `[native] embed` y `[frontend] dist`, y la caché
/// `.ray-deps/` entera (sin su `.index` ni los `.git`). Rutas relativas a la raíz con `/`.
pub fn collect_snapshot(root: &Path) -> Result<Files, String> {
    let manifest = crate::manifest::Manifest::load(root).ok().flatten();
    let mut extra_dirs: Vec<PathBuf> = Vec::new();
    // D4: las dependencias `path = …` FUERA de la raíz viajan bajo `.ray-path-deps/<dir>/` (solo
    // fuentes y manifiestos); el dispositivo añade ese directorio como raíz de dependencias, y el
    // loader las resuelve por el nombre del directorio, como hace con el padre de cada ruta.
    let mut path_deps: Vec<(String, PathBuf)> = Vec::new();
    if let Some(m) = &manifest {
        for d in crate::cli::embed_dirs_of(m) {
            extra_dirs.push(root.join(d));
        }
        for (_name, spec) in &m.dependencies {
            if let Some(p) = crate::deps::path_of_path_dep(spec) {
                let pdir = m.root.join(p);
                let pdir = pdir.canonicalize().unwrap_or(pdir);
                let inside = root.canonicalize().map(|r| pdir.starts_with(&r)).unwrap_or(false);
                if pdir.is_dir()
                    && !inside
                    && let Some(base) = pdir.file_name().map(|b| b.to_string_lossy().into_owned())
                {
                    path_deps.push((base, pdir));
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut total = 0u64;
    let mut push = |rel: String, path: &Path, out: &mut Vec<(String, Vec<u8>)>| -> Result<(), String> {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        total += data.len() as u64;
        if total > MAX_SNAPSHOT_BYTES {
            return Err(format!("the project snapshot exceeds {} MB (an artifact slipped in?)", MAX_SNAPSHOT_BYTES >> 20));
        }
        out.push((rel, data));
        Ok(())
    };
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            let in_deps = rel.starts_with(".ray-deps/") || rel == ".ray-deps";
            let in_extra = extra_dirs.iter().any(|d| path.starts_with(d));
            if path.is_dir() {
                if in_deps {
                    // Dentro de la caché: todo salvo los repos git y el índice.
                    if name == ".git" || name == ".index" {
                        continue;
                    }
                } else if name == ".ray-deps" || in_extra {
                    // entra
                } else if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                pending.push(path);
                continue;
            }
            let is_source = name.ends_with(".ray") || name.ends_with(".ray.html");
            let is_manifest = dir == root && (name == "ray.toml" || name == "ray.lock");
            if in_deps || in_extra || is_source || is_manifest {
                // Un `.ray` generado de un `.ray.html` hermano es derivado: viaja el fuente.
                if name.ends_with(".ray") && !name.ends_with(".ray.html") && path.with_extension("ray.html").exists() {
                    continue;
                }
                push(rel, &path, &mut out)?;
            }
        }
    }
    for (base, pdir) in path_deps {
        let mut pending = vec![pdir.clone()];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            let mut entries: Vec<_> = entries.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if path.is_dir() {
                    if !(name.starts_with('.') || name == "target" || name == "node_modules") {
                        pending.push(path);
                    }
                    continue;
                }
                if name.ends_with(".ray") || name.ends_with(".ray.html") || name == "ray.toml" {
                    if name.ends_with(".ray") && !name.ends_with(".ray.html") && path.with_extension("ray.html").exists() {
                        continue;
                    }
                    let inner = path.strip_prefix(&pdir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                    push(format!(".ray-path-deps/{base}/{inner}"), &path, &mut out)?;
                }
            }
        }
    }
    Ok(out)
}

/// D4: el delta entre lo que el dispositivo tiene (`have`: ruta → hash) y el snapshot actual:
/// (rutas a borrar, entradas a escribir). `None` si no hay nada que mandar.
fn delta(have: &std::collections::HashMap<String, u64>, files: &[(String, Vec<u8>)]) -> Option<Delta> {
    let mut present = std::collections::HashSet::new();
    let mut changed = Vec::new();
    for (rel, data) in files {
        present.insert(rel.as_str());
        if have.get(rel) != Some(&content_hash(data)) {
            changed.push((rel.clone(), data.clone()));
        }
    }
    let removed: Vec<String> = have.keys().filter(|k| !present.contains(k.as_str())).cloned().collect();
    if changed.is_empty() && removed.is_empty() {
        None
    } else {
        Some((removed, changed))
    }
}

fn encode_delta(removed: &[String], changed: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(removed.len() as u32).to_be_bytes());
    for r in removed {
        put_str(&mut out, r);
    }
    out.extend_from_slice(&encode_snapshot(changed));
    out
}

fn decode_delta(buf: &[u8]) -> Option<Delta> {
    let mut pos = 0;
    let n = u32::from_be_bytes(buf.get(pos..pos + 4)?.try_into().ok()?) as usize;
    pos += 4;
    let mut removed = Vec::with_capacity(n);
    for _ in 0..n {
        removed.push(get_str(buf, &mut pos)?);
    }
    let changed = decode_snapshot(buf.get(pos..)?)?;
    Some((removed, changed))
}

fn encode_hashes(have: &std::collections::HashMap<String, u64>) -> Vec<u8> {
    let mut out = Vec::new();
    for (rel, h) in have {
        put_str(&mut out, rel);
        out.extend_from_slice(&h.to_be_bytes());
    }
    out
}

fn decode_hashes(buf: &[u8]) -> Option<std::collections::HashMap<String, u64>> {
    let mut pos = 0;
    let mut out = std::collections::HashMap::new();
    while pos < buf.len() {
        let rel = get_str(buf, &mut pos)?;
        let h = u64::from_be_bytes(buf.get(pos..pos + 8)?.try_into().ok()?);
        pos += 8;
        out.insert(rel, h);
    }
    Some(out)
}

/// D4: lo que el dispositivo tiene en disco bajo `dir` (ruta relativa → hash), para pedir solo
/// el delta al reconectar.
fn hashes_on_disk(dir: &Path) -> std::collections::HashMap<String, u64> {
    let mut out = std::collections::HashMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                pending.push(p);
            } else if let Ok(data) = std::fs::read(&p) {
                let rel = p.strip_prefix(dir).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                out.insert(rel, content_hash(&data));
            }
        }
    }
    out
}

/// D4: aplica un delta (borra y escribe; sin barrido — el anfitrión sabe qué había).
fn apply_delta(dir: &Path, removed: &[String], changed: &[(String, Vec<u8>)]) -> Result<(), String> {
    for rel in removed {
        if rel.starts_with('/') || rel.split('/').any(|c| c == "..") {
            return Err(format!("refusing a delta path outside the project: {rel}"));
        }
        let _ = std::fs::remove_file(dir.join(rel));
    }
    for (rel, data) in changed {
        if rel.starts_with('/') || rel.split('/').any(|c| c == "..") {
            return Err(format!("refusing a delta path outside the project: {rel}"));
        }
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

fn encode_snapshot(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (rel, data) in files {
        put_str(&mut out, rel);
        out.extend_from_slice(&(data.len() as u64).to_be_bytes());
        out.extend_from_slice(data);
    }
    out
}

fn decode_snapshot(buf: &[u8]) -> Option<Files> {
    let mut pos = 0;
    let mut out = Vec::new();
    while pos < buf.len() {
        let rel = get_str(buf, &mut pos)?;
        let n = u64::from_be_bytes(buf.get(pos..pos + 8)?.try_into().ok()?) as usize;
        pos += 8;
        let data = buf.get(pos..pos + n)?.to_vec();
        pos += n;
        out.push((rel, data));
    }
    Some(out)
}

/// Escribe el snapshot en `dir` y borra lo que ya no viene (un módulo eliminado no debe seguir
/// resolviendo). Rechaza rutas que salgan del directorio.
fn apply_snapshot(dir: &Path, files: &[(String, Vec<u8>)]) -> Result<(), String> {
    let mut keep = std::collections::HashSet::new();
    for (rel, data) in files {
        if rel.starts_with('/') || rel.split('/').any(|c| c == "..") {
            return Err(format!("refusing a snapshot path outside the project: {rel}"));
        }
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))?;
        keep.insert(path);
    }
    // Barrido de sobrantes (archivos; los directorios vacíos no molestan).
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                pending.push(p);
            } else if !keep.contains(&p) {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    Ok(())
}

// ── El anfitrión: `ray dev --device` ────────────────────────────────────────

/// Un dispositivo conectado (su socket de escritura), cómo se presentó y lo que ya tiene (D4:
/// ruta → hash, para mandarle solo el delta).
struct Device {
    name: String,
    stream: TcpStream,
    have: std::collections::HashMap<String, u64>,
}

/// El anfitrión del enlace: acepta dispositivos, les manda el snapshot actual al conectar y
/// difunde cada snapshot nuevo. Los `STATUS` de cada dispositivo se imprimen en la terminal.
pub struct Host {
    devices: Arc<Mutex<Vec<Device>>>,
    latest: Arc<Mutex<Option<Files>>>,
    /// La URL que el dispositivo necesita (`<esquema>://ip:puerto/token`; el esquema es el id
    /// del shell de desarrollo si se conoce, o `ray-dev`).
    pub url: String,
    /// El esquema del enlace (ver `url`).
    pub scheme: String,
    /// El puerto local (para el cliente de prueba en la misma máquina).
    pub port: u16,
}

impl Host {
    /// Escucha en un puerto alto aleatorio de todas las interfaces (el teléfono llega por la
    /// LAN) y arranca el hilo de aceptación.
    pub fn start() -> Result<Host, String> {
        Host::start_with_state(None, GENERIC_SCHEME)
    }

    /// Como [`Host::start`], pero **recuerda puerto y token** en `state` (D3): el teléfono se
    /// empareja UNA vez por proyecto y `ray dev --device` vuelve a escuchar en el mismo sitio
    /// la próxima sesión (si el puerto está ocupado, toma otro y lo dice; el token se conserva).
    pub fn start_with_state(state: Option<&Path>, scheme: &str) -> Result<Host, String> {
        let saved = state.and_then(|p| std::fs::read_to_string(p).ok()).map(|t| {
            let grab = |k: &str| t.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string()));
            (grab("port=").and_then(|p| p.parse::<u16>().ok()), grab("token="))
        });
        let (saved_port, saved_token) = saved.unwrap_or((None, None));
        let listener = match saved_port.and_then(|p| TcpListener::bind(("0.0.0.0", p)).ok()) {
            Some(l) => l,
            None => {
                if let Some(p) = saved_port {
                    eprintln!("[dev] port {p} is busy; the device link moves to a new port (pair the phone again)");
                }
                TcpListener::bind("0.0.0.0:0").map_err(|e| format!("could not open the device link: {e}"))?
            }
        };
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let token = saved_token.filter(|t| !t.is_empty()).unwrap_or_else(random_token);
        if let Some(p) = state {
            let _ = std::fs::write(p, format!("# ray dev --device: this project's link (do not share)\nport={port}\ntoken={token}\n"));
        }
        let url = format!("{scheme}://{}:{port}/{token}", lan_ip());
        let host = Host {
            devices: Arc::new(Mutex::new(Vec::new())),
            latest: Arc::new(Mutex::new(None)),
            url,
            scheme: scheme.to_string(),
            port,
        };
        let own_scheme = scheme.to_string();
        let devices = host.devices.clone();
        let latest = host.latest.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let devices = devices.clone();
                let latest = latest.clone();
                let token = token.clone();
                let own_scheme = own_scheme.clone();
                std::thread::spawn(move || serve_device(stream, &token, &own_scheme, devices, latest));
            }
        });
        Ok(host)
    }

    /// Fija el snapshot vigente y lo manda a todos los dispositivos conectados — a cada uno
    /// solo su delta (D4). Devuelve (dispositivos que lo recibieron, archivos enviados en total).
    pub fn publish(&self, files: &[(String, Vec<u8>)]) -> (usize, usize) {
        *self.latest.lock().unwrap() = Some(files.to_vec());
        let mut devices = self.devices.lock().unwrap();
        let before = devices.len();
        let mut sent_files = 0usize;
        devices.retain_mut(|d| match send_update(d, files) {
            Ok(n) => {
                sent_files += n;
                true
            }
            Err(_) => false,
        });
        let dropped = before - devices.len();
        if dropped > 0 {
            eprintln!("[dev] {dropped} device(s) disconnected");
        }
        (devices.len(), sent_files)
    }

    /// La misma URL con el esquema genérico (para `ray dev-client` y el tecleo manual).
    pub fn generic_url(&self) -> String {
        format!("{GENERIC_SCHEME}://{}", self.url.split_once("://").map(|(_, r)| r).unwrap_or(""))
    }

    /// Nº de dispositivos conectados ahora mismo.
    pub fn connected(&self) -> usize {
        self.devices.lock().unwrap().len()
    }
}

/// Manda a `d` lo que le falta del snapshot `files`: el snapshot entero si aún no tiene nada,
/// el delta si sí; nada si ya está al día. Actualiza lo que sabemos que tiene. Devuelve
/// cuántos archivos viajaron.
fn send_update(d: &mut Device, files: &[(String, Vec<u8>)]) -> std::io::Result<usize> {
    let n = if d.have.is_empty() {
        write_frame(&mut d.stream, KIND_SNAPSHOT, &encode_snapshot(files))?;
        files.len()
    } else if let Some((removed, changed)) = delta(&d.have, files) {
        write_frame(&mut d.stream, KIND_DELTA, &encode_delta(&removed, &changed))?;
        changed.len()
    } else {
        // Al día: reinicia igual (un snapshot vacío como delta) — el usuario guardó a propósito.
        write_frame(&mut d.stream, KIND_DELTA, &encode_delta(&[], &[]))?;
        0
    };
    d.have = files.iter().map(|(rel, data)| (rel.clone(), content_hash(data))).collect();
    Ok(n)
}

fn serve_device(mut stream: TcpStream, token: &str, own_scheme: &str, devices: Arc<Mutex<Vec<Device>>>, latest: Arc<Mutex<Option<Files>>>) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok((KIND_HELLO, body)) = read_frame(&mut stream) else { return };
    let mut pos = 0;
    let (Some(got), Some(name), Some(version)) = (get_str(&body, &mut pos), get_str(&body, &mut pos), get_str(&body, &mut pos)) else { return };
    // D5: el esquema con el que enlazó (cuarto campo; un dispositivo viejo no lo manda).
    let used_scheme = get_str(&body, &mut pos).unwrap_or_default();
    if got != token {
        eprintln!("[dev] a device presented a wrong token; ignored");
        return;
    }
    if !used_scheme.is_empty() && used_scheme != GENERIC_SCHEME && used_scheme != own_scheme {
        eprintln!("[dev] {}", paint("33", &format!("warning: {name} linked with the scheme '{used_scheme}' but this project's development shell is '{own_scheme}' — is it the right app?")));
    }
    let peer = stream.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
    eprintln!("[dev] {}", paint("32", &format!("device connected: {name} ({peer}, raylang {version})")));
    if version != env!("CARGO_PKG_VERSION") {
        eprintln!("[dev] {}", paint("33", &format!("warning: the device runs raylang {version} and this host {}; the program is compiled by the device's toolchain", env!("CARGO_PKG_VERSION"))));
    }
    // D4: lo que el dispositivo ya tiene, para mandarle solo el delta.
    let have = match read_frame(&mut stream) {
        Ok((KIND_HASHES, body)) => decode_hashes(&body).unwrap_or_default(),
        _ => std::collections::HashMap::new(),
    };
    let _ = stream.set_read_timeout(None);
    let Ok(writer) = stream.try_clone() else { return };
    let mut device = Device { name: name.clone(), stream: writer, have };
    if let Some(files) = latest.lock().unwrap().as_ref() {
        match send_update(&mut device, files) {
            Ok(n) => eprintln!("[dev] {name}: sent {n} file(s) to catch up"),
            Err(_) => return,
        }
    }
    devices.lock().unwrap().push(device);
    // Lector: los STATUS del dispositivo, hasta que cierre.
    loop {
        match read_frame(&mut stream) {
            Ok((KIND_STATUS, body)) if !body.is_empty() => {
                let text = String::from_utf8_lossy(&body[1..]);
                let label = match body[0] {
                    STATE_RUNNING => "running",
                    STATE_FINISHED => "finished",
                    STATE_STOPPED => "stopped",
                    STATE_COMPILE_ERROR => "does not compile",
                    STATE_RUNTIME_ERROR => "runtime error",
                    STATE_SNAPSHOT => "snapshot",
                    _ => "status",
                };
                if text.is_empty() {
                    eprintln!("[dev] {name}: {label}");
                } else {
                    eprintln!("[dev] {name}: {label} — {text}");
                }
            }
            // D4: la consola remota — la línea tal cual, en el mismo flujo que en el dispositivo.
            Ok((KIND_LOG, body)) if !body.is_empty() => {
                use std::io::Write;
                let line = String::from_utf8_lossy(&body[1..]);
                if body[0] == 1 {
                    let mut e = std::io::stderr().lock();
                    let _ = writeln!(e, "{line}");
                } else {
                    let mut o = std::io::stdout().lock();
                    let _ = writeln!(o, "{line}");
                    let _ = o.flush();
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    eprintln!("[dev] device disconnected: {name}");
    devices.lock().unwrap().retain(|d| d.name != name || d.stream.peer_addr().ok() != stream.peer_addr().ok());
}

// ── El dispositivo: la librería de desarrollo ────────────────────────────────

/// Corre el bucle del dispositivo: conecta con el anfitrión (reintentando cada segundo hasta
/// conseguirlo y al perder el enlace), recibe snapshots en `dir/project` y ejecuta el programa
/// en la VM, reiniciándolo con cada snapshot. No retorna salvo error irrecuperable.
pub fn run_device(url: &str, dir: &Path, device_name: &str) -> Result<(), String> {
    run_device_until(url, dir, device_name, None).map(|_| ())
}

/// Como [`run_device`], pero se rinde (`Err`) si el anfitrión lleva `unreachable_after` sin
/// responder (D3: la librería de desarrollo vuelve a la página de emparejamiento — la URL
/// guardada puede haber caducado). `None` = reintentar para siempre (el cliente de escritorio).
pub fn run_device_until(url: &str, dir: &Path, device_name: &str, unreachable_after: Option<Duration>) -> Result<Option<String>, String> {
    let (addr, token) = parse_url(url)?;
    let scheme = url_scheme(url).to_string();
    let project = dir.join("project");
    std::fs::create_dir_all(&project).map_err(|e| format!("{}: {e}", project.display()))?;
    let mut runner = Runner::default();
    let mut unreachable_since: Option<std::time::Instant> = None;
    loop {
        let mut stream = match TcpStream::connect_timeout(
            &addr.parse().map_err(|_| format!("bad address in the link URL: {addr}"))?,
            Duration::from_secs(3),
        ) {
            Ok(s) => s,
            Err(e) => {
                if let Some(new_link) = take_pending_link() {
                    return Ok(Some(new_link));
                }
                let since = *unreachable_since.get_or_insert_with(std::time::Instant::now);
                if let Some(limit) = unreachable_after
                    && since.elapsed() > limit
                {
                    runner.stop();
                    return Err(format!("the host at {addr} did not answer for {}s", limit.as_secs()));
                }
                eprintln!("[dev-client] cannot reach {addr}: {e}; retrying");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        unreachable_since = None;
        let _ = stream.set_nodelay(true);
        let mut hello = Vec::new();
        put_str(&mut hello, &token);
        put_str(&mut hello, device_name);
        put_str(&mut hello, env!("CARGO_PKG_VERSION"));
        put_str(&mut hello, &scheme);
        if write_frame(&mut stream, KIND_HELLO, &hello).is_err() {
            continue;
        }
        // D4: lo que ya hay en disco — el anfitrión manda solo el delta.
        if write_frame(&mut stream, KIND_HASHES, &encode_hashes(&hashes_on_disk(&project))).is_err() {
            continue;
        }
        eprintln!("[dev-client] linked to {addr}");
        // D5: lectura con plazo para atender un enlace nuevo (QR escaneado con la app ya enlazada).
        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
        let status_stream = Arc::new(Mutex::new(stream.try_clone().map_err(|e| e.to_string())?));
        runner.status = Some(status_stream.clone());
        // D4: la consola remota — cada print/eprint del programa viaja también al anfitrión.
        crate::set_output_mirror(Some(Box::new(move |stderr, line| {
            let mut payload = vec![if stderr { 1 } else { 0 }];
            payload.extend_from_slice(line.as_bytes());
            let _ = write_frame(&mut status_stream.lock().unwrap_or_else(|e| e.into_inner()), KIND_LOG, &payload);
        })));
        loop {
            match read_frame(&mut stream) {
                Ok((KIND_SNAPSHOT, body)) => {
                    let Some(files) = decode_snapshot(&body) else {
                        eprintln!("[dev-client] malformed snapshot; ignored");
                        continue;
                    };
                    let n = files.len();
                    runner.stop();
                    if let Err(e) = apply_snapshot(&project, &files) {
                        runner.report(STATE_COMPILE_ERROR, &e);
                        continue;
                    }
                    runner.report(STATE_SNAPSHOT, &format!("{n} files"));
                    runner.start(&project);
                }
                Ok((KIND_DELTA, body)) => {
                    let Some((removed, changed)) = decode_delta(&body) else {
                        eprintln!("[dev-client] malformed delta; ignored");
                        continue;
                    };
                    runner.stop();
                    if let Err(e) = apply_delta(&project, &removed, &changed) {
                        runner.report(STATE_COMPILE_ERROR, &e);
                        continue;
                    }
                    runner.report(STATE_SNAPSHOT, &format!("{} changed, {} removed", changed.len(), removed.len()));
                    runner.start(&project);
                }
                Ok(_) => {}
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    if let Some(new_link) = take_pending_link() {
                        crate::set_output_mirror(None);
                        runner.status = None;
                        eprintln!("[dev-client] new link received; relinking");
                        return Ok(Some(new_link));
                    }
                }
                Err(_) => break,
            }
        }
        crate::set_output_mirror(None);
        eprintln!("[dev-client] link lost; the program keeps running until the host is back");
        runner.status = None;
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// El programa en curso en el dispositivo y el canal de estado hacia el anfitrión.
#[derive(Default)]
struct Runner {
    thread: Option<std::thread::JoinHandle<()>>,
    status: Option<Arc<Mutex<TcpStream>>>,
}

impl Runner {
    fn report(&self, state: u8, text: &str) {
        if let Some(s) = &self.status {
            let mut payload = vec![state];
            payload.extend_from_slice(text.as_bytes());
            let _ = write_frame(&mut s.lock().unwrap(), KIND_STATUS, &payload);
        }
    }

    /// Para el programa en curso (D1) y espera a que su hilo termine; si no lo hace en
    /// `STOP_GRACE`, sale del proceso (decisión de diseño: el shell arranca limpio y se
    /// reconecta; mejor que un reinicio a medias).
    fn stop(&mut self) {
        let Some(handle) = self.thread.take() else { return };
        if handle.is_finished() {
            let _ = handle.join();
            crate::builtins::runtime_reset();
            return;
        }
        crate::vm::stop::request_stop();
        let deadline = std::time::Instant::now() + STOP_GRACE;
        while !handle.is_finished() {
            if std::time::Instant::now() > deadline {
                self.report(STATE_STOPPED, "the program did not stop in time; exiting so the shell restarts clean");
                eprintln!("[dev-client] the program did not stop within {STOP_GRACE:?}; exiting");
                std::process::exit(0);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = handle.join();
        crate::builtins::runtime_reset();
    }

    /// Carga, chequea y ejecuta el proyecto de `project` en un hilo propio; reporta el estado.
    fn start(&mut self, project: &Path) {
        let status = self.status.clone();
        let project = project.to_path_buf();
        let report = move |state: u8, text: &str| {
            if let Some(s) = &status {
                let mut payload = vec![state];
                payload.extend_from_slice(text.as_bytes());
                let _ = write_frame(&mut s.lock().unwrap(), KIND_STATUS, &payload);
            }
        };
        let handle = std::thread::Builder::new()
            .name("ray-dev-program".into())
            .stack_size(256 * 1024 * 1024)
            .spawn(move || {
                // El cwd del proceso es del programa (rutas relativas de `fs`, `ray.toml`…).
                let _ = std::env::set_current_dir(&project);
                // M335: `std/embed` (los `[native] embed` y el `[frontend] dist`) y la identidad
                // de la app, como en `ray run`. Sin esto `ui.mount_embed_at("", "frontend/dist")`
                // fallaba en el teléfono y la página salía «not found». La config se fija una vez
                // por proceso: la raíz del snapshot no cambia entre recargas, y los archivos se
                // leen en vivo de ella.
                if let Ok(Some(m)) = crate::manifest::Manifest::load(&project) {
                    crate::cli::configure_embed(&m.entry_path().to_string_lossy());
                }
                match load_and_compile(&project) {
                    Err(e) => {
                        eprintln!("{e}");
                        report(STATE_COMPILE_ERROR, &e);
                    }
                    Ok(compiled) => {
                        report(STATE_RUNNING, "");
                        match crate::vm::run_program(&compiled) {
                            Ok(crate::runtime::Value::Int(code)) => report(STATE_FINISHED, &format!("exit code {code}")),
                            Ok(_) => report(STATE_FINISHED, ""),
                            Err(e) if e.is_stop() => report(STATE_STOPPED, ""),
                            Err(e) => {
                                eprintln!("{e}");
                                report(STATE_RUNTIME_ERROR, &e.to_string());
                            }
                        }
                    }
                }
            });
        match handle {
            Ok(h) => self.thread = Some(h),
            Err(e) => self.report(STATE_RUNTIME_ERROR, &format!("could not start the program thread: {e}")),
        }
    }
}

/// Carga (con las raíces de `.ray-deps` del snapshot; nunca git ni índice), chequea y compila
/// el proyecto. Los diagnósticos vuelven como texto (el anfitrión ya los mostró: aquí solo
/// pueden diferir si las toolchains no coinciden).
fn load_and_compile(project: &Path) -> Result<crate::bytecode::CompiledProgram, String> {
    let manifest = crate::manifest::Manifest::load(project)?;
    let entry = match &manifest {
        Some(m) => m.entry_path(),
        None => project.join("main.ray"),
    };
    if !entry.is_file() {
        return Err(format!("entry not found in the snapshot: {}", entry.display()));
    }
    let mut roots = crate::deps::dependency_roots_for(project);
    // D4: las dependencias `path = …` que el anfitrión empaquetó fuera de la raíz.
    let path_deps = project.join(".ray-path-deps");
    if path_deps.is_dir() {
        roots.push(path_deps);
    }
    let mut loaded = crate::loader::load_with_deps(&entry, &roots).map_err(|e| e.message)?;
    crate::checker::check(&mut loaded.program).map_err(|e| format!("type error at {}:{}: {}", e.line, e.col, e.msg))?;
    crate::compiler::compile_program(&loaded.program).map_err(|e| e.to_string())
}

/// M330 D2/D3: la entrada C-llamable de la **librería de desarrollo**: `url` es la
/// `ray-dev://…` que imprime `ray dev --device` (vacía = usar la guardada o pedirla en la página
/// de emparejamiento), `dir` un directorio escribible del sandbox de la app (vacío = el de
/// [`default_dir`]). Retorna 0 con el enlace corriendo en su hilo (1 = punteros nulos o no se
/// pudo crear el hilo). Contrato de strings como el del shell: NUL-terminated, copiados durante
/// la llamada. El `ray_start` de la librería de desarrollo generada por `ray bundle --dev` es
/// exactamente `ray_dev_start("", "")`: el shell no cambia.
///
/// # Safety
/// `url` y `dir` deben ser punteros válidos a C-strings NUL-terminated (o NULL, que da 1).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ray_dev_start(url: *const std::ffi::c_char, dir: *const std::ffi::c_char) -> i32 {
    if url.is_null() || dir.is_null() {
        return 1;
    }
    // SAFETY: el contrato exige C-strings válidos durante la llamada; se copian antes de volver.
    let (url, dir) = unsafe {
        (
            std::ffi::CStr::from_ptr(url).to_string_lossy().into_owned(),
            std::ffi::CStr::from_ptr(dir).to_string_lossy().into_owned(),
        )
    };
    start_from_shell(if url.is_empty() { None } else { Some(url) }, if dir.is_empty() { None } else { Some(PathBuf::from(dir)) })
}

/// El directorio de trabajo de la librería de desarrollo cuando el shell no da uno: el
/// sandbox de la app (`$HOME` en iOS/Android es el contenedor de la app) bajo un nombre propio.
pub fn default_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    if cfg!(target_os = "ios") {
        home.join("Library").join("Application Support").join("ray-dev")
    } else {
        home.join(".ray-dev")
    }
}

/// D3: arranca el enlace desde un shell (retorna 0 con el hilo lanzado, como `ray_start`):
/// ignora `SIGPIPE`, y en su hilo enlaza con `url`, o con la URL guardada en `dir/link.url`, o
/// pide una en la página de emparejamiento del webview; si el anfitrión deja de responder
/// (una sesión nueva de `ray dev --device` en otro puerto), vuelve a la página con la URL
/// anterior rellenada.
pub fn start_from_shell(url: Option<String>, dir: Option<PathBuf>) -> i32 {
    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn signal(s: i32, h: usize) -> usize;
        }
        signal(13, 1); // SIGPIPE → SIG_IGN, como el ray_start emitido (un cdylib no pasa por el shim de main)
    }
    let dir = dir.unwrap_or_else(default_dir);
    let name = if cfg!(target_os = "ios") { "iPhone" } else if cfg!(target_os = "android") { "Android" } else { "device" };
    match std::thread::Builder::new().name("ray-dev-link".into()).spawn(move || shell_loop(url, &dir, name)) {
        Ok(_) => 0,
        Err(_) => 1,
    }
}

fn shell_loop(explicit: Option<String>, dir: &Path, name: &str) {
    let _ = std::fs::create_dir_all(dir);
    let saved = dir.join("link.url");
    // `RAY_DEV_URL` (D3): el enlace por entorno — `xcrun simctl launch` lo pasa con
    // `SIMCTL_CHILD_RAY_DEV_URL` y `adb shell setprop`/`am start` en Android: automatiza el
    // emparejamiento en simuladores y CI sin tocar la página.
    let mut url = explicit
        .or_else(|| std::env::var("RAY_DEV_URL").ok().filter(|s| !s.is_empty()))
        .or_else(|| std::fs::read_to_string(&saved).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
    loop {
        let link = match url.take() {
            Some(u) => u,
            None => pair(None, name),
        };
        let _ = std::fs::write(&saved, &link);
        match run_device_until(&link, dir, name, Some(Duration::from_secs(20))) {
            Ok(None) => return,
            Ok(Some(new_link)) => url = Some(new_link),
            Err(e) => {
                eprintln!("[dev-client] {e}");
                url = Some(pair(Some(&link), name));
            }
        }
    }
}

/// M343: `text` con el color ANSI `code` (verde 32 para conexiones, ámbar 33 para avisos) solo
/// cuando stderr es una terminal y no hay `NO_COLOR`; en una tubería, el texto tal cual.
pub(crate) fn paint(code: &str, text: &str) -> String {
    let tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    if tty && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()) {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// El nombre con el que el usuario ve la app de desarrollo, sin el sufijo `-dev` que `ray bundle
/// --dev` añade. La librería es la misma para todos los proyectos (un asset por release), así que
/// el nombre lo pone el shell en `RAY_DEV_APP_NAME` (la etiqueta de la app en Android,
/// `CFBundleDisplayName`/`CFBundleName` en iOS); como respaldo en iOS, el nombre del ejecutable
/// del bundle (`Notes-dev.app/Notes-dev`). `None` = no se sabe.
fn shell_app_name() -> Option<String> {
    let raw = std::env::var("RAY_DEV_APP_NAME").ok().filter(|s| !s.trim().is_empty()).or_else(|| {
        if !cfg!(target_os = "ios") {
            return None;
        }
        let exe = std::env::current_exe().ok()?;
        Some(exe.file_stem()?.to_string_lossy().into_owned())
    })?;
    Some(strip_dev_suffix(raw.trim()))
}

/// `Notes-dev` → `Notes` (también `Notes Dev`/`Notes dev`, por si el shell capitaliza).
pub fn strip_dev_suffix(label: &str) -> String {
    for suffix in ["-dev", " Dev", " dev"] {
        if let Some(base) = label.strip_suffix(suffix)
            && !base.is_empty()
        {
            return base.to_string();
        }
    }
    label.to_string()
}

/// La página de emparejamiento, servida por `ray://app` con el mismo puente que usan los
/// programas: la sirve la librería de desarrollo, así que el shell no necesita nada nuevo.
const PAIR_HTML: &str = r##"<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<title>raylang dev</title>
<style>
:root{--bg:#0b1f33;--soft:#0e2947;--line:#1e3a57;--ink:#eaf4fb;--muted:#8fb0cc;--sky:#4ba3f5;--aqua:#34d2c6;--amber:#f2c66d;--green:#5ad1a0;--red:#f08080}
*{box-sizing:border-box}
body{margin:0;min-height:100vh;padding:max(28px,env(safe-area-inset-top)) 24px max(28px,env(safe-area-inset-bottom));font:16px/1.45 -apple-system,system-ui,"Segoe UI",Roboto,sans-serif;background:var(--bg);color:var(--ink);display:flex;flex-direction:column;gap:20px}
.brand{display:flex;align-items:center;gap:12px}.brand svg{width:44px;height:40px;flex:none}
.eyebrow{font-size:12px;letter-spacing:.08em;text-transform:uppercase;color:var(--sky);font-weight:600}
h1{margin:2px 0 0;font-size:24px;line-height:1.1}
.pill{display:inline-flex;align-items:center;gap:8px;font-size:13px;color:var(--muted);background:var(--soft);border:1px solid var(--line);padding:6px 12px;border-radius:999px;width:fit-content}
.pill i{width:8px;height:8px;border-radius:50%;background:var(--amber);box-shadow:0 0 0 4px rgba(242,198,109,.18)}
.pill.ok i{background:var(--green);box-shadow:0 0 0 4px rgba(90,209,160,.18)}.pill.err i{background:var(--red);box-shadow:0 0 0 4px rgba(240,128,128,.18)}
.steps{display:grid;gap:14px}.step{display:grid;grid-template-columns:30px 1fr;gap:12px;align-items:start}
.n{width:30px;height:30px;border-radius:50%;display:grid;place-items:center;font:600 14px ui-monospace,monospace;color:var(--bg);background:var(--sky)}
.done .n{background:var(--line);color:var(--muted)}
h2{margin:4px 0 2px;font-size:16px}.step p{margin:0;color:var(--muted);font-size:14px}
code{font:13px ui-monospace,monospace;color:var(--ink);background:var(--soft);border:1px solid var(--line);border-radius:6px;padding:2px 6px}
.focus{background:linear-gradient(160deg,rgba(75,163,245,.16),rgba(52,210,198,.08));border:1px solid rgba(75,163,245,.35);border-radius:18px;padding:16px;display:grid;gap:8px}
.cam{display:flex;gap:12px;align-items:center}.cam svg{width:34px;height:34px;flex:none}.focus h2{margin:0;font-size:17px}.focus p{margin:0;color:var(--muted);font-size:14px}
details{border-top:1px solid var(--line);padding-top:14px}summary{cursor:pointer;color:var(--muted);font-size:14px;list-style:none;display:flex;justify-content:space-between}summary::-webkit-details-marker{display:none}summary::after{content:"\25BE"}
.manual{display:grid;gap:10px;margin-top:12px}
input{width:100%;font:15px ui-monospace,monospace;padding:12px 14px;border-radius:12px;border:1px solid var(--line);background:var(--soft);color:var(--ink)}
button{padding:13px;font:600 16px -apple-system,system-ui,sans-serif;border:0;border-radius:12px;background:var(--sky);color:var(--bg)}
#msg{color:var(--red);font-size:14px;min-height:1.4em}
.foot{margin-top:auto;color:var(--muted);font-size:12px;display:flex;justify-content:space-between;gap:12px}
</style></head><body>
<div class="brand">
<svg viewBox="0 20 200 184" aria-hidden="true"><path d="M100 50 C108 49 113 53 115 60 C150 66 175 84 191 116 C166 127 141 137 119 150 C111 154 105 159 100 165 C95 159 89 154 81 150 C59 137 34 127 9 116 C25 84 50 66 85 60 C87 53 92 49 100 50 Z" fill="#4ba3f5"/><path d="M93 55 C90 45 88 38 87 30" stroke="#4ba3f5" stroke-width="7" stroke-linecap="round" fill="none"/><path d="M107 55 C110 45 112 38 113 30" stroke="#4ba3f5" stroke-width="7" stroke-linecap="round" fill="none"/><path d="M100 160 C100 175 100 185 99 196" stroke="#4ba3f5" stroke-width="6" stroke-linecap="round" fill="none"/></svg>
<div><div class="eyebrow">raylang &middot; development app</div><h1>/*NAME*/</h1></div>
</div>
<span class="pill /*STATE*/" id="state"><i></i><span id="statetext">/*STATUS*/</span></span>
<div class="steps">
<div class="step done"><span class="n">1</span><div><h2>Install this app</h2><p>Done. You install it once; the program you are writing arrives over Wi-Fi.</p></div></div>
<div class="step"><span class="n">2</span><div><h2>Pair with your computer</h2><p>In the project folder run <code>ray dev --device</code>. It prints a QR code.</p></div></div>
</div>
<div class="focus">
<div class="cam"><svg viewBox="0 0 24 24" fill="none" stroke="#4ba3f5" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 8V6a2 2 0 0 1 2-2h2M16 4h2a2 2 0 0 1 2 2v2M20 16v2a2 2 0 0 1-2 2h-2M8 20H6a2 2 0 0 1-2-2v-2"/><rect x="7" y="7" width="4" height="4"/><rect x="13" y="7" width="4" height="4"/><rect x="7" y="13" width="4" height="4"/><path d="M13 13h4v4"/></svg><h2>Scan the QR with your camera</h2></div>
<p>Point the phone's camera at the terminal and tap the link that appears. This app opens already paired, and from then on every file you save reloads here. The pairing is remembered: next time just open the app.</p>
</div>
<div class="steps"><div class="step"><span class="n">3</span><div><h2>Save a file</h2><p>The phone restarts the program in about a second. What it prints shows up in your terminal.</p></div></div></div>
<details /*OPEN*/><summary>Can't scan? Paste the link instead</summary>
<div class="manual">
<input id="u" placeholder="ray-dev://192.168.1.20:52731/token" autocapitalize="none" autocorrect="off" spellcheck="false" value="/*PREFILL*/">
<button id="go">Pair</button>
<div id="msg">/*MSG*/</div>
</div></details>
<div class="foot"><span>raylang /*VERSION*/</span><span>paired once &middot; remembered by this app</span></div>
<script>
const u=document.getElementById('u'),msg=document.getElementById('msg'),st=document.getElementById('state'),stt=document.getElementById('statetext');
document.getElementById('go').onclick=()=>{const v=u.value.trim();if(!v.includes('://')){msg.textContent='Paste the link that ray dev --device prints under the QR code.';return;}msg.textContent='';st.className='pill ok';stt.textContent='Pairing…';window.ray.send(v);};
u.addEventListener('keydown',e=>{if(e.key==='Enter')document.getElementById('go').click();});
</script></body></html>"##;

/// Muestra la página de emparejamiento y espera la URL (un `window.ray.send` que empiece por
/// `ray-dev://`). Deja la UI limpia al volver (la ventana del programa la abre el programa).
/// Pública para su test (headless + `RAY_UI_MSG`); el shell entra por [`start_from_shell`].
/// El HTML de la página para `name` (el nombre de la app). Con un enlace anterior que dejó de
/// responder, abre directamente el campo del enlace (prefijado) con el indicador en rojo; sin
/// enlace, el indicador en ámbar y el campo plegado: el camino principal es la cámara. Pura.
pub fn pair_html(prefill: Option<&str>, name: &str) -> String {
    let (state, status, msg, open) = match prefill {
        Some(_) => ("err", "The computer stopped answering", "Is `ray dev --device` still running there? Scan its new QR, or check the link.", "open"),
        None => ("", "Not paired yet", "", ""),
    };
    PAIR_HTML
        .replace("/*NAME*/", name)
        .replace("/*STATE*/", state)
        .replace("/*STATUS*/", status)
        .replace("/*OPEN*/", open)
        .replace("/*PREFILL*/", prefill.unwrap_or(""))
        .replace("/*MSG*/", msg)
        .replace("/*VERSION*/", env!("CARGO_PKG_VERSION"))
}

pub fn pair(prefill: Option<&str>, name: &str) -> String {
    let html = pair_html(prefill, &shell_app_name().unwrap_or_else(|| name.to_string()));
    #[cfg(all(feature = "ui", any(unix, windows), not(target_arch = "wasm32")))]
    {
        use ray_runtime::ui;
        let _ = ui::scheme::mount_bytes("__raydev/pair.html", html.into_bytes());
        let opts = ui::WindowOptions {
            width: 420,
            height: 640,
            min_width: 0,
            min_height: 0,
            resizable: true,
            center: true,
            autosave: String::new(),
            titlebar_color: String::new(),
            background: "#0b1f33".to_string(),
            minimizable: true,
            kind: "document".to_string(),
            always_on_top: false,
            parent: 0,
        };
        if let Err(e) = ui::open_window_with(1, "raylang dev", "ray://app/__raydev/pair.html", &opts) {
            eprintln!("[dev-client] cannot show the pairing page: {e}");
        }
        loop {
            // D5: un enlace por el esquema URL (QR escaneado) gana a la página.
            if let Some(link) = take_pending_link() {
                ui::reset_for_restart();
                return link;
            }
            match ui::next_event_blocking(100) {
                Some((kind, _, tag)) if kind == "message" && parse_url(&tag).is_ok() => {
                    ui::reset_for_restart();
                    return tag;
                }
                _ => {}
            }
        }
    }
    #[cfg(not(all(feature = "ui", any(unix, windows), not(target_arch = "wasm32"))))]
    {
        let _ = html;
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_round_trips_its_address_and_token() {
        let (addr, token) = parse_url("ray-dev://192.168.1.20:41234/abcd").unwrap();
        assert_eq!(addr, "192.168.1.20:41234");
        assert_eq!(token, "abcd");
        // D5: el esquema puede ser el id del shell de desarrollo.
        let (addr2, token2) = parse_url("org.raylang.app.dev://192.168.1.20:41234/abcd").unwrap();
        assert_eq!((addr2, token2), (addr, token));
        assert_eq!(url_scheme("org.raylang.app.dev://h:1/t"), "org.raylang.app.dev");
        assert!(parse_url("ray-dev://host:1/").is_err());
        assert!(parse_url("no scheme").is_err());
        assert!(parse_url("bad scheme://h:1/t").is_err());
    }

    #[test]
    fn the_dev_suffix_is_stripped_from_the_app_label() {
        assert_eq!(strip_dev_suffix("Notes-dev"), "Notes");
        assert_eq!(strip_dev_suffix("Notes Dev"), "Notes");
        assert_eq!(strip_dev_suffix("Notes"), "Notes");
        assert_eq!(strip_dev_suffix("-dev"), "-dev");
    }

    #[test]
    fn the_pairing_page_leads_with_the_camera_and_opens_the_link_field_on_failure() {
        let fresh = pair_html(None, "Notes");
        assert!(fresh.contains("<h1>Notes</h1>") && fresh.contains("Scan the QR with your camera"), "{fresh}");
        assert!(fresh.contains("Not paired yet") && fresh.contains("<details >"), "campo plegado:\n{fresh}");
        assert!(!fresh.contains("/*"), "sin marcadores sin rellenar");
        let again = pair_html(Some("ray-dev://10.0.0.2:5000/tok"), "Notes");
        assert!(again.contains("<details open>") && again.contains("value=\"ray-dev://10.0.0.2:5000/tok\""), "{again}");
        assert!(again.contains("pill err") && again.contains("The computer stopped answering"), "{again}");
    }

    #[test]
    fn the_qr_renders_and_a_scanned_link_is_offered_once() {
        let qr = qr_text("org.raylang.app.dev://192.168.1.20:41234/0123456789abcdef0123456789abcdef").unwrap();
        assert!(qr.lines().count() > 10, "a QR is several lines tall");
        assert!(qr.lines().all(|l| l.chars().count() == qr.lines().next().unwrap().chars().count()), "square");
        offer_link("not a link");
        assert!(take_pending_link().is_none());
        offer_link("org.raylang.app.dev://h:1/t");
        assert_eq!(take_pending_link().as_deref(), Some("org.raylang.app.dev://h:1/t"));
        assert!(take_pending_link().is_none(), "consumed");
    }

    #[test]
    fn a_snapshot_survives_encoding_and_decoding() {
        let files = vec![("src/main.ray".to_string(), b"fn main() {}".to_vec()), ("ray.toml".to_string(), vec![]), ("a/b.bin".to_string(), vec![0, 255, 7])];
        let back = decode_snapshot(&encode_snapshot(&files)).unwrap();
        assert_eq!(back, files);
        assert!(decode_snapshot(&[0, 0, 0, 9]).is_none());
    }

    #[test]
    fn a_delta_carries_only_what_changed_and_round_trips() {
        let v1 = vec![("a.ray".to_string(), b"1".to_vec()), ("b.ray".to_string(), b"2".to_vec()), ("c.ray".to_string(), b"3".to_vec())];
        let have: std::collections::HashMap<String, u64> = v1.iter().map(|(r, d)| (r.clone(), content_hash(d))).collect();
        assert!(delta(&have, &v1).is_none(), "nothing changed → nothing to send");
        let v2 = vec![("a.ray".to_string(), b"1".to_vec()), ("b.ray".to_string(), b"22".to_vec()), ("d.ray".to_string(), b"4".to_vec())];
        let (removed, changed) = delta(&have, &v2).unwrap();
        assert_eq!(removed, vec!["c.ray".to_string()]);
        assert_eq!(changed, vec![("b.ray".to_string(), b"22".to_vec()), ("d.ray".to_string(), b"4".to_vec())]);
        let back = decode_delta(&encode_delta(&removed, &changed)).unwrap();
        assert_eq!(back, (removed, changed));
        let hashes_back = decode_hashes(&encode_hashes(&have)).unwrap();
        assert_eq!(hashes_back, have);
    }

    #[test]
    fn applying_a_delta_removes_and_writes_without_pruning() {
        let dir = std::env::temp_dir().join(format!("ray-devdelta-{}-{}", std::process::id(), crate::builtins::random_int(1 << 30)));
        std::fs::create_dir_all(&dir).unwrap();
        apply_snapshot(&dir, &[("src/a.ray".into(), b"1".to_vec()), ("src/b.ray".into(), b"2".to_vec())]).unwrap();
        apply_delta(&dir, &["src/b.ray".to_string()], &[("src/c.ray".into(), b"3".to_vec())]).unwrap();
        assert!(dir.join("src/a.ray").is_file(), "untouched files stay");
        assert!(!dir.join("src/b.ray").exists());
        assert_eq!(std::fs::read(dir.join("src/c.ray")).unwrap(), b"3");
        let on_disk = hashes_on_disk(&dir);
        assert_eq!(on_disk.len(), 2);
        assert_eq!(on_disk["src/c.ray"], content_hash(b"3"));
        assert!(apply_delta(&dir, &["../x".to_string()], &[]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_dependency_outside_the_root_travels_under_ray_path_deps() {
        let base = std::env::temp_dir().join(format!("ray-devpath-{}-{}", std::process::id(), crate::builtins::random_int(1 << 30)));
        let root = base.join("app");
        let dep = base.join("geo");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(dep.join("target")).unwrap();
        std::fs::write(root.join("ray.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"path:../geo\"\n").unwrap();
        std::fs::write(root.join("src/main.ray"), "fn main() {}").unwrap();
        std::fs::write(dep.join("mod.ray"), "pub fn area() -> int { 1 }").unwrap();
        std::fs::write(dep.join("ray.toml"), "[package]\nname = \"geo\"\nversion = \"0.1.0\"\n").unwrap();
        std::fs::write(dep.join("target/x.ray"), "").unwrap();
        std::fs::write(dep.join("notes.txt"), "").unwrap();
        let mut names: Vec<String> = collect_snapshot(&root).unwrap().into_iter().map(|(n, _)| n).collect();
        names.sort();
        assert_eq!(names, vec![".ray-path-deps/geo/mod.ray", ".ray-path-deps/geo/ray.toml", "ray.toml", "src/main.ray"]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn applying_a_snapshot_writes_and_prunes() {
        let dir = std::env::temp_dir().join(format!("ray-devlink-{}-{}", std::process::id(), crate::builtins::random_int(1 << 30)));
        std::fs::create_dir_all(&dir).unwrap();
        apply_snapshot(&dir, &[("src/main.ray".into(), b"1".to_vec()), ("src/old.ray".into(), b"2".to_vec())]).unwrap();
        apply_snapshot(&dir, &[("src/main.ray".into(), b"3".to_vec())]).unwrap();
        assert_eq!(std::fs::read(dir.join("src/main.ray")).unwrap(), b"3");
        assert!(!dir.join("src/old.ray").exists());
        assert!(apply_snapshot(&dir, &[("../x".into(), vec![])]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_snapshot_collects_sources_deps_and_manifest_but_not_artifacts() {
        let dir = std::env::temp_dir().join(format!("ray-devsnap-{}-{}", std::process::id(), crate::builtins::random_int(1 << 30)));
        for d in ["src", ".ray-deps/geo", ".ray-deps/geo/.git", ".ray-deps/.index", "target", "node_modules", ".git"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("ray.toml"), "[package]\nname = \"p\"\nversion = \"0.1.0\"\n").unwrap();
        std::fs::write(dir.join("ray.lock"), "").unwrap();
        std::fs::write(dir.join("src/main.ray"), "fn main() {}").unwrap();
        std::fs::write(dir.join("src/page.ray.html"), "<p>").unwrap();
        std::fs::write(dir.join("src/page.ray"), "generated").unwrap();
        std::fs::write(dir.join(".ray-deps/geo/mod.ray"), "").unwrap();
        std::fs::write(dir.join(".ray-deps/geo/.git/HEAD"), "").unwrap();
        std::fs::write(dir.join(".ray-deps/.index/geo.toml"), "").unwrap();
        std::fs::write(dir.join("target/x.ray"), "").unwrap();
        std::fs::write(dir.join("node_modules/y.ray"), "").unwrap();
        std::fs::write(dir.join(".git/z.ray"), "").unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        let mut names: Vec<String> = collect_snapshot(&dir).unwrap().into_iter().map(|(n, _)| n).collect();
        names.sort();
        assert_eq!(names, vec![".ray-deps/geo/mod.ray", "ray.lock", "ray.toml", "src/main.ray", "src/page.ray.html"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
