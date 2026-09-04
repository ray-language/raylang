//! M115.4 — watch de filesystem por eventos de kernel (IDEAS §69: la pieza que CINCO apps
//! reimplementaban sondeando mtimes). Sobre el crate `notify` (FSEvents en macOS, inotify en
//! Linux, kqueue en BSD, `ReadDirectoryChangesW` en Windows desde M181): la recursividad sobre
//! árboles llega resuelta — kqueue crudo exige un fd por archivo, inviable para un árbol de
//! fuentes.
//!
//! Los eventos llegan en un hilo del propio notify. El puente al mundo del programa es una
//! **cola compartida** (`Mutex` + `Condvar`): el callback encola y avisa; quien bloquea el hilo
//! (intérprete, nativo sin fibras, `ray dev`) espera en la condvar con plazo. Para APARCAR una
//! fibra hace falta además algo que el poller entienda: en unix un **self-pipe** (el truco de
//! `signals()` M88.1) — el callback escribe un octeto y la VM/el scheduler nativo aparcan por
//! readiness del extremo de lectura, como un socket. En Windows (M181) no hay pipe: `fd()` es
//! -1 y el scheduler de la VM, que ya sabe de esperas sin fd (M170/M177), consulta
//! `has_pending()` antes de despertar la fibra — el patrón de la cola de eventos de `std/ui`.
#![cfg(all(feature = "watch", any(unix, windows)))]

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

use notify::{EventKind, RecursiveMode, Watcher};

// Sin el crate `libc` (política del runtime: std + externs a mano, como process.rs).
#[cfg(unix)]
unsafe extern "C" {
    fn pipe(fds: *mut i32) -> i32;
    fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
    fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    fn close(fd: i32) -> i32;
    // Variádica a propósito (lección de arm64: con aridad fija los varargs van mal por la pila).
    #[link_name = "fcntl"]
    fn fcntl_raw(fd: i32, cmd: i32, ...) -> i32;
}
#[cfg(unix)]
const F_GETFL: i32 = 3;
#[cfg(unix)]
const F_SETFL: i32 = 4;
#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NONBLOCK: i32 = 0o4000; // M156: bionic también es 0o4000 (android es unix, no "linux")
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const O_NONBLOCK: i32 = 0x0004;

/// La cola de eventos ya traducidos, compartida entre el callback de notify y el dueño del
/// watch. `Arc` para que una espera bloqueante pueda sostenerla SIN retener el registro de
/// handles (un `close` concurrente no debe colgar a nadie).
pub struct Queue {
    events: Mutex<VecDeque<(String, String)>>,
    ready: Condvar,
}

impl Queue {
    /// ¿Hay un evento esperando? Sin bloquear (el scheduler de la VM lo consulta a cada vuelta).
    pub fn has_pending(&self) -> bool {
        !self.events.lock().unwrap().is_empty()
    }

    /// Espera hasta que haya un evento o venza `timeout_ms` (`<= 0` = sin plazo). Devuelve si
    /// hay evento. No lo consume: quien espera vuelve a `try_next` con el registro en mano.
    pub fn wait(&self, timeout_ms: i64) -> bool {
        let mut events = self.events.lock().unwrap();
        if timeout_ms <= 0 {
            while events.is_empty() {
                events = self.ready.wait(events).unwrap();
            }
            return true;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
        while events.is_empty() {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            events = self.ready.wait_timeout(events, left).unwrap().0;
        }
        true
    }
}

/// Un watch vivo: el watcher de notify (su `Drop` detiene los hilos), la cola y, en unix, el
/// self-pipe cuyo extremo de lectura es el fd por el que aparca la fibra.
pub struct FsWatcher {
    queue: Arc<Queue>,
    #[cfg(unix)]
    pipe_rd: i32,
    _watcher: notify::RecommendedWatcher,
}

// Sonoro: la cola va tras Mutex/Condvar y el watcher de notify solo se toca desde el dueño del
// handle (el registro va tras un Mutex global).
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
    #[cfg(unix)]
    let (pipe_rd, pipe_wr) = make_pipe()?;
    let queue = Arc::new(Queue { events: Mutex::new(VecDeque::new()), ready: Condvar::new() });
    let producer = queue.clone();
    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        if let Ok(ev) = res {
            if let Some(kind) = kind_str(&ev.kind) {
                for p in &ev.paths {
                    producer.events.lock().unwrap().push_back((kind.to_string(), p.to_string_lossy().into_owned()));
                    producer.ready.notify_one();
                    // Un octeto por evento; con el pipe LLENO se omite (queda legible igual, y el
                    // lector drena octetos huérfanos al vaciar la cola).
                    #[cfg(unix)]
                    unsafe {
                        write(pipe_wr, [1u8].as_ptr(), 1)
                    };
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
    Ok(FsWatcher {
        queue,
        #[cfg(unix)]
        pipe_rd,
        _watcher: watcher,
    })
}

impl FsWatcher {
    /// El fd por el que aparcar la fibra (extremo de lectura del self-pipe, no-bloqueante). En
    /// Windows no hay fd: -1, y la espera va por `queue().has_pending()` (VM) o `wait` (hilos).
    pub fn fd(&self) -> i32 {
        #[cfg(unix)]
        {
            self.pipe_rd
        }
        #[cfg(windows)]
        {
            -1
        }
    }

    /// La cola compartida, para esperar sin retener el registro de handles.
    pub fn queue(&self) -> Arc<Queue> {
        self.queue.clone()
    }

    /// ¿Hay un evento esperando? Sin bloquear ni consumir.
    pub fn has_pending(&self) -> bool {
        self.queue.has_pending()
    }

    /// El siguiente evento si ya hay uno, SIN bloquear. En unix consume un octeto del pipe por
    /// evento entregado y, con la cola vacía, drena los octetos huérfanos (para no despertar en
    /// falso).
    pub fn try_next(&self) -> Option<(String, String)> {
        let ev = self.queue.events.lock().unwrap().pop_front();
        #[cfg(unix)]
        self.drain_pipe(if ev.is_some() { 1 } else { usize::MAX });
        ev
    }

    /// El siguiente evento BLOQUEANDO el hilo (intérprete / nativo sin fibras / `ray dev`).
    /// `ms <= 0` = sin plazo; `Ok(None)` = plazo vencido.
    pub fn next_timeout(&self, ms: i64) -> Result<Option<(String, String)>, String> {
        if let Some(ev) = self.try_next() {
            return Ok(Some(ev));
        }
        if !self.queue.wait(ms) {
            return Ok(None);
        }
        Ok(self.try_next())
    }

    /// Lee hasta `max` octetos del pipe (no-bloqueante) y los descarta.
    #[cfg(unix)]
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

#[cfg(unix)]
impl Drop for FsWatcher {
    fn drop(&mut self) {
        unsafe { close(self.pipe_rd) };
        // El extremo de escritura lo posee el closure del watcher; al caer `_watcher` muere el
        // hilo de notify y con él el closure — el fd de escritura se queda hasta entonces (los
        // writes a un pipe sin lector fallan con EPIPE, que el callback ignora).
    }
}

/// ¿Hay lectura pendiente en `fd` dentro de `timeout_ms`? Para la espera del binario nativo SIN
/// fibras (hilo-por-tarea) en unix: sondear la cola + poll(2) del fd, sin retener el lock del
/// registro. (Windows espera en la cola: `Queue::wait`.)
#[cfg(unix)]
pub fn fd_ready(fd: i32, timeout_ms: i32) -> bool {
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    // nfds_t es u64 en Linux/Android (bionic LP64) y u32 en los demás unix (mismo cfg que
    // process.rs/fibers.rs — y mismo tipo, para no chocar con su extern de `poll`).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    type Nfds = u64;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    type Nfds = u32;
    unsafe extern "C" {
        fn poll(fds: *mut PollFd, nfds: Nfds, timeout_ms: i32) -> i32;
    }
    let mut pfd = PollFd { fd, events: 0x0001, revents: 0 };
    // SAFETY: un solo PollFd bien formado; poll no retiene el puntero tras volver.
    unsafe { poll(&mut pfd, 1, timeout_ms) > 0 }
}

/// Crea el self-pipe con ambos extremos NO-bloqueantes.
#[cfg(unix)]
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
