//! M115.4 — watch de filesystem por eventos de kernel (IDEAS §69: la pieza que CINCO apps
//! reimplementaban sondeando mtimes). Sobre el crate `notify` (FSEvents en macOS, inotify en
//! Linux, kqueue en BSD): la recursividad sobre árboles llega resuelta — kqueue crudo exige un
//! fd por archivo, inviable para un árbol de fuentes.
//!
//! Los eventos llegan en un hilo del propio notify. El puente al mundo de fibras es un
//! **self-pipe** (el truco de `signals()` M88.1): el callback encola el evento y escribe un
//! octeto al pipe; el lector (VM/nativo) aparca la fibra por readiness del extremo de lectura y
//! drena la cola al despertar. Así ni la VM ni el scheduler nativo saben nada de notify — solo
//! ven un fd legible, como un socket.
#![cfg(all(feature = "watch", unix))]

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Mutex;

use notify::{EventKind, RecursiveMode, Watcher};

// Sin el crate `libc` (política del runtime: std + externs a mano, como process.rs).
unsafe extern "C" {
    fn pipe(fds: *mut i32) -> i32;
    fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
    fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    fn close(fd: i32) -> i32;
    // Variádica a propósito (lección de arm64: con aridad fija los varargs van mal por la pila).
    #[link_name = "fcntl"]
    fn fcntl_raw(fd: i32, cmd: i32, ...) -> i32;
}
const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0o4000;
#[cfg(not(target_os = "linux"))]
const O_NONBLOCK: i32 = 0x0004;

/// Un watch vivo: el watcher de notify (su `Drop` detiene los hilos), la cola de eventos ya
/// traducidos y el self-pipe cuyo extremo de lectura es el fd por el que aparca la fibra.
pub struct FsWatcher {
    rx: Receiver<(String, String)>,
    /// Eventos ya extraídos de `rx` pero aún no entregados (un evento de notify puede traer
    /// varias rutas → varios eventos nuestros).
    pending: Mutex<VecDeque<(String, String)>>,
    pipe_rd: i32,
    _watcher: notify::RecommendedWatcher,
}

// Sonoro: `rx`/`pending` solo los toca el dueño del handle (el registro va tras un Mutex global)
// y el watcher de notify ya es Send.
unsafe impl Sync for FsWatcher {}

/// Traduce el `EventKind` de notify a la moneda estable del lenguaje. `Access` se descarta
/// (ruido: no cambia el contenido); un rename llega como `Modify(Name)`.
fn kind_str(k: &EventKind) -> Option<&'static str> {
    match k {
        EventKind::Create(_) => Some("create"),
        EventKind::Remove(_) => Some("remove"),
        EventKind::Modify(notify::event::ModifyKind::Name(_)) => Some("rename"),
        EventKind::Modify(_) => Some("modify"),
        EventKind::Access(_) => None,
        _ => Some("other"),
    }
}

/// Abre un watch sobre `path` (un directorio se observa RECURSIVO; un archivo, a sí mismo).
pub fn watch(path: &str) -> Result<FsWatcher, String> {
    let (pipe_rd, pipe_wr) = make_pipe()?;
    let (tx, rx) = std::sync::mpsc::channel::<(String, String)>();
    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        if let Ok(ev) = res {
            if let Some(kind) = kind_str(&ev.kind) {
                for p in &ev.paths {
                    if tx.send((kind.to_string(), p.to_string_lossy().into_owned())).is_ok() {
                        // Un octeto por evento; con el pipe LLENO se omite (queda legible igual,
                        // y el lector drena octetos huérfanos al vaciar la cola).
                        unsafe { write(pipe_wr, [1u8].as_ptr(), 1) };
                    }
                }
            }
        }
    })
    .map_err(|e| e.to_string())?;
    let p = std::path::Path::new(path);
    if !p.exists() {
        return Err(format!("watch: path does not exist: {}", path));
    }
    let mode = if p.is_dir() { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
    watcher.watch(p, mode).map_err(|e| e.to_string())?;
    Ok(FsWatcher { rx, pending: Mutex::new(VecDeque::new()), pipe_rd, _watcher: watcher })
}

impl FsWatcher {
    /// El fd por el que aparcar la fibra (extremo de lectura del self-pipe, no-bloqueante).
    pub fn fd(&self) -> i32 {
        self.pipe_rd
    }

    /// El siguiente evento si ya hay uno, SIN bloquear. Consume un octeto del pipe por evento
    /// entregado; con la cola vacía drena los octetos huérfanos (para no despertar en falso).
    pub fn try_next(&self) -> Option<(String, String)> {
        let mut pending = self.pending.lock().unwrap();
        loop {
            if let Some(ev) = pending.pop_front() {
                self.drain_pipe(1);
                return Some(ev);
            }
            match self.rx.try_recv() {
                Ok(ev) => pending.push_back(ev),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => {
                    self.drain_pipe(usize::MAX);
                    return None;
                }
            }
        }
    }

    /// El siguiente evento BLOQUEANDO el hilo (intérprete / nativo sin fibras). `ms <= 0` = sin
    /// plazo; `Ok(None)` = plazo vencido.
    pub fn next_timeout(&self, ms: i64) -> Result<Option<(String, String)>, String> {
        if let Some(ev) = self.try_next() {
            return Ok(Some(ev));
        }
        let r = if ms <= 0 {
            self.rx.recv().map_err(|_| "the watch was closed".to_string())?
        } else {
            match self.rx.recv_timeout(std::time::Duration::from_millis(ms as u64)) {
                Ok(ev) => ev,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("the watch was closed".to_string())
                }
            }
        };
        self.drain_pipe(1);
        Ok(Some(r))
    }

    /// Lee hasta `max` octetos del pipe (no-bloqueante) y los descarta.
    fn drain_pipe(&self, max: usize) {
        let mut buf = [0u8; 64];
        let mut left = max;
        while left > 0 {
            let want = buf.len().min(left);
            let n = unsafe { read(self.pipe_rd, buf.as_mut_ptr(), want) };
            if n <= 0 {
                break;
            }
            left -= n as usize;
        }
    }
}

impl Drop for FsWatcher {
    fn drop(&mut self) {
        unsafe { close(self.pipe_rd) };
        // El extremo de escritura lo posee el closure del watcher; al caer `_watcher` muere el
        // hilo de notify y con él el closure — el fd de escritura se queda hasta entonces (los
        // writes a un pipe sin lector fallan con EPIPE, que el callback ignora).
    }
}

/// ¿Hay lectura pendiente en `fd` dentro de `timeout_ms`? Para la espera del binario nativo SIN
/// fibras (hilo-por-tarea): sondear la cola + poll(2) del fd, sin retener el lock del registro.
pub fn fd_ready(fd: i32, timeout_ms: i32) -> bool {
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    // nfds_t es u64 en Linux y u32 en los demás unix (mismo cfg que process.rs — y mismo tipo,
    // para no chocar con su declaración extern de `poll` en el mismo crate).
    #[cfg(target_os = "linux")]
    type Nfds = u64;
    #[cfg(not(target_os = "linux"))]
    type Nfds = u32;
    unsafe extern "C" {
        fn poll(fds: *mut PollFd, nfds: Nfds, timeout_ms: i32) -> i32;
    }
    let mut pfd = PollFd { fd, events: 0x0001, revents: 0 };
    // SAFETY: un solo PollFd bien formado; poll no retiene el puntero tras volver.
    unsafe { poll(&mut pfd, 1, timeout_ms) > 0 }
}

/// Crea el self-pipe con ambos extremos NO-bloqueantes.
fn make_pipe() -> Result<(i32, i32), String> {
    let mut fds = [0i32; 2];
    if unsafe { pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    for fd in fds {
        unsafe {
            let fl = fcntl_raw(fd, F_GETFL);
            fcntl_raw(fd, F_SETFL, fl | O_NONBLOCK);
        }
    }
    Ok((fds[0], fds[1]))
}
