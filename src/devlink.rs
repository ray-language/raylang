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
//! anfitrión: `HELLO` (token, nombre del dispositivo, versión), `STATUS` (un octeto de estado +
//! texto). Del anfitrión al dispositivo: `SNAPSHOT` (entradas `[u32 len][ruta][u64 len][bytes]`,
//! rutas relativas con `/`). Un `SNAPSHOT` siempre reinicia el programa. Sin dependencias: todo
//! con `std::net`. El token viaja en la URL `ray-dev://host:puerto/token` que imprime `ray dev
//! --device`; una conexión con otro token se cierra sin más.
//!
//! Lo que NO cubre esta fase: diferencial de snapshot (se manda entero), consola remota (los
//! `print` del dispositivo van a su stdout/logcat), QR y emparejamiento en pantalla (D3).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Tipos de marco.
const KIND_HELLO: u8 = 1;
const KIND_STATUS: u8 = 2;
const KIND_SNAPSHOT: u8 = 10;

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

/// `ray-dev://host:port/token` → (host:port, token).
pub fn parse_url(url: &str) -> Result<(String, String), String> {
    let rest = url
        .strip_prefix("ray-dev://")
        .ok_or_else(|| format!("not a ray-dev:// URL: {url}"))?;
    let (addr, token) = rest
        .split_once('/')
        .ok_or_else(|| format!("the URL has no token: {url}"))?;
    if addr.is_empty() || token.is_empty() {
        return Err(format!("malformed ray-dev URL: {url}"));
    }
    Ok((addr.to_string(), token.to_string()))
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
pub fn collect_snapshot(root: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let manifest = crate::manifest::Manifest::load(root).ok().flatten();
    let mut extra_dirs: Vec<PathBuf> = Vec::new();
    if let Some(m) = &manifest {
        for d in crate::cli::embed_dirs_of(m) {
            extra_dirs.push(root.join(d));
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
    Ok(out)
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

fn decode_snapshot(buf: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
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

/// Un dispositivo conectado (su socket de escritura) y cómo se presentó.
struct Device {
    name: String,
    stream: TcpStream,
}

/// El anfitrión del enlace: acepta dispositivos, les manda el snapshot actual al conectar y
/// difunde cada snapshot nuevo. Los `STATUS` de cada dispositivo se imprimen en la terminal.
pub struct Host {
    devices: Arc<Mutex<Vec<Device>>>,
    latest: Arc<Mutex<Option<Vec<u8>>>>,
    /// La URL que el dispositivo necesita (`ray-dev://ip:puerto/token`).
    pub url: String,
    /// El puerto local (para el cliente de prueba en la misma máquina).
    pub port: u16,
}

impl Host {
    /// Escucha en un puerto alto aleatorio de todas las interfaces (el teléfono llega por la
    /// LAN) y arranca el hilo de aceptación.
    pub fn start() -> Result<Host, String> {
        let listener = TcpListener::bind("0.0.0.0:0").map_err(|e| format!("could not open the device link: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let token = random_token();
        let url = format!("ray-dev://{}:{port}/{token}", lan_ip());
        let host = Host {
            devices: Arc::new(Mutex::new(Vec::new())),
            latest: Arc::new(Mutex::new(None)),
            url,
            port,
        };
        let devices = host.devices.clone();
        let latest = host.latest.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let devices = devices.clone();
                let latest = latest.clone();
                let token = token.clone();
                std::thread::spawn(move || serve_device(stream, &token, devices, latest));
            }
        });
        Ok(host)
    }

    /// Fija el snapshot vigente y lo manda a todos los dispositivos conectados. Devuelve
    /// cuántos lo recibieron.
    pub fn publish(&self, files: &[(String, Vec<u8>)]) -> usize {
        let payload = encode_snapshot(files);
        *self.latest.lock().unwrap() = Some(payload.clone());
        let mut devices = self.devices.lock().unwrap();
        let before = devices.len();
        devices.retain_mut(|d| write_frame(&mut d.stream, KIND_SNAPSHOT, &payload).is_ok());
        let dropped = before - devices.len();
        if dropped > 0 {
            eprintln!("[dev] {dropped} device(s) disconnected");
        }
        devices.len()
    }

    /// Nº de dispositivos conectados ahora mismo.
    pub fn connected(&self) -> usize {
        self.devices.lock().unwrap().len()
    }
}

fn serve_device(mut stream: TcpStream, token: &str, devices: Arc<Mutex<Vec<Device>>>, latest: Arc<Mutex<Option<Vec<u8>>>>) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok((KIND_HELLO, body)) = read_frame(&mut stream) else { return };
    let mut pos = 0;
    let (Some(got), Some(name), Some(version)) = (get_str(&body, &mut pos), get_str(&body, &mut pos), get_str(&body, &mut pos)) else { return };
    if got != token {
        eprintln!("[dev] a device presented a wrong token; ignored");
        return;
    }
    let _ = stream.set_read_timeout(None);
    let peer = stream.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
    eprintln!("[dev] device connected: {name} ({peer}, raylang {version})");
    let Ok(writer) = stream.try_clone() else { return };
    let mut writer = writer;
    if let Some(snapshot) = latest.lock().unwrap().as_ref()
        && write_frame(&mut writer, KIND_SNAPSHOT, snapshot).is_err()
    {
        return;
    }
    devices.lock().unwrap().push(Device { name: name.clone(), stream: writer });
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
    let (addr, token) = parse_url(url)?;
    let project = dir.join("project");
    std::fs::create_dir_all(&project).map_err(|e| format!("{}: {e}", project.display()))?;
    let mut runner = Runner::default();
    loop {
        let mut stream = match TcpStream::connect(&addr) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[dev-client] cannot reach {addr}: {e}; retrying");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        let mut hello = Vec::new();
        put_str(&mut hello, &token);
        put_str(&mut hello, device_name);
        put_str(&mut hello, env!("CARGO_PKG_VERSION"));
        if write_frame(&mut stream, KIND_HELLO, &hello).is_err() {
            continue;
        }
        eprintln!("[dev-client] linked to {addr}");
        let status_stream = stream.try_clone().map_err(|e| e.to_string())?;
        runner.status = Some(Arc::new(Mutex::new(status_stream)));
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
                Ok(_) => {}
                Err(_) => break,
            }
        }
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
    let roots = crate::deps::dependency_roots_for(project);
    let mut loaded = crate::loader::load_with_deps(&entry, &roots).map_err(|e| e.message)?;
    crate::checker::check(&mut loaded.program).map_err(|e| format!("type error at {}:{}: {}", e.line, e.col, e.msg))?;
    crate::compiler::compile_program(&loaded.program).map_err(|e| e.to_string())
}

/// M330 D2: la entrada C-llamable de la **librería de desarrollo** (el shell móvil la llama
/// en vez de `ray_start`): `url` es la `ray-dev://…` que imprime `ray dev --device`, `dir` un
/// directorio escribible del sandbox de la app. Retorna 0 con el enlace corriendo en su hilo
/// (1 = argumentos inválidos). Contrato de strings como el del shell: NUL-terminated, copiados
/// durante la llamada.
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
    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn signal(s: i32, h: usize) -> usize;
        }
        signal(13, 1); // SIGPIPE → SIG_IGN, como el ray_start emitido (un cdylib no pasa por el shim de main)
    }
    let name = if cfg!(target_os = "ios") { "iPhone" } else if cfg!(target_os = "android") { "Android" } else { "device" };
    match std::thread::Builder::new().name("ray-dev-link".into()).spawn(move || {
        if let Err(e) = run_device(&url, Path::new(&dir), name) {
            eprintln!("[dev-client] {e}");
        }
    }) {
        Ok(_) => 0,
        Err(_) => 1,
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
        assert!(parse_url("http://x/y").is_err());
        assert!(parse_url("ray-dev://host:1/").is_err());
    }

    #[test]
    fn a_snapshot_survives_encoding_and_decoding() {
        let files = vec![("src/main.ray".to_string(), b"fn main() {}".to_vec()), ("ray.toml".to_string(), vec![]), ("a/b.bin".to_string(), vec![0, 255, 7])];
        let back = decode_snapshot(&encode_snapshot(&files)).unwrap();
        assert_eq!(back, files);
        assert!(decode_snapshot(&[0, 0, 0, 9]).is_none());
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
