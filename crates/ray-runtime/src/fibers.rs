//! F1 del arco de concurrencia nativa (`docs/diseno-concurrencia-nativa.md` §5): scheduler M:N de
//! **fibras** para el binario transpilado, sustituto del hilo-por-conexión (medido: 1002 hilos y
//! ~265 KB/conexión a `-c 1000`, contra los 13 hilos y 23 KB del mismo modelo en la VM).
//!
//! Piezas:
//! - **Corrutinas con pila propia** (`corosensei`): el código generado sigue pareciendo bloqueante
//!   (cero coloreado); la pila es `mmap` con página de guarda, así que la RESERVA (128 KiB por
//!   defecto) es virtual y solo cuestan las páginas tocadas (medido: 4-12 KiB en `net/webserver`).
//! - **Workers**: N hilos (= cores, o `RAYLANG_THREADS`) que sacan fibras listas de una cola
//!   compartida y las reanudan. Una fibra puede despertar en un worker DISTINTO del que la aparcó.
//! - **Reactor**: un hilo con kqueue/epoll **persistente** (a diferencia de `src/poll.rs` de la VM,
//!   que crea y destruye el poller en cada llamada) + una tubería de despertar (CLOEXEC, como la
//!   auditoría de IDEAS §53.4) + temporizadores para `sleep`.
//!
//! La cesión "profunda" (aparcar desde dentro de `__ray_socket_read`, a N marcos del arranque de la
//! fibra) usa un TLS con el yielder de la fibra en ejecución, repuesto tras cada reanudación porque
//! el worker puede ser otro (ver `suspend`).
//!
//! **Seguridad del movimiento entre workers**: el binario transpilado usa `Rc` en sus valores; mover
//! una fibra suspendida a otro hilo es correcto por el MISMO argumento que en la VM (M38): los
//! valores de una fibra no se comparten con otras (aislamiento del modelo de actores), así que sus
//! contadores `Rc` solo los toca la fibra dueña, esté en el hilo que esté.

use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult, Yielder};
use std::cell::Cell;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Motivo por el que una fibra devuelve el control al scheduler.
enum Park {
    /// Aparca hasta que el fd esté listo para leer.
    Read(i32),
    /// Aparca hasta que el fd esté listo para escribir.
    Write(i32),
    /// Aparca hasta el instante dado (`sleep`).
    SleepUntil(Instant),
    /// Cede el turno y vuelve al final de la cola de listas.
    Yield,
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    Read,
    Write,
}

type FiberCo = Coroutine<(), Park, ()>;

/// Una fibra en vuelo: la corrutina + la celda donde se publica su resultado.
struct Task {
    co: FiberCo,
    done: Arc<DoneCell>,
}

// SAFETY: corosensei NO marca `Coroutine` como Send a propósito — una pila suspendida puede contener
// valores no-Send (aquí: los `Rc` del binario transpilado) y el sistema de tipos ya no los ve. Este
// impl es la decisión consciente que habilita el modelo M:N, y su corrección se apoya en dos cosas:
// (1) EXCLUSIÓN: una Task es propiedad de exactamente un dueño a la vez (un worker mientras corre,
//     la cola/el reactor mientras espera); el movimiento entre hilos pasa por Mutex → hay
//     happens-before entre el último toque en un hilo y el primero en el siguiente.
// (2) AISLAMIENTO (el argumento de M38 en la VM): los valores de una fibra no se comparten con otras
//     fibras — los contadores `Rc` de su pila solo los toca la fibra dueña, esté en el worker que
//     esté. Lo que cruza fibras cruza por canales (F3), que imponen su propia sincronización.
unsafe impl Send for Task {}

/// Celda de terminación de una fibra: `None` mientras corre; `Ok`/`Err(mensaje del panic)` al acabar.
struct DoneCell {
    state: Mutex<Option<Result<(), String>>>,
    cv: Condvar,
}

/// Asa de espera de una fibra. `join` bloquea el HILO llamador (pensado para `main` y los tests);
/// la espera fibra-a-fibra (join/canales sin bloquear el worker) llega en F3.
pub struct JoinHandle {
    done: Arc<DoneCell>,
}

impl JoinHandle {
    pub fn join(self) -> Result<(), String> {
        let mut st = self.done.state.lock().unwrap();
        while st.is_none() {
            st = self.done.cv.wait(st).unwrap();
        }
        st.take().unwrap()
    }
}

/// Operaciones que los workers encargan al reactor (via buzón + tubería de despertar).
enum Op {
    Wait(i32, Dir, Task),
    Timer(Instant, Task),
}

struct Scheduler {
    /// Cola global de fibras listas. Suficiente en F1; si la contención aparece en el bench, el
    /// sharding por worker (como `__RAY_POOL`) es una mejora local de F5.
    runq: Mutex<VecDeque<Task>>,
    runq_cv: Condvar,
    /// Buzón del reactor: los workers dejan aquí los aparcados y tocan la tubería.
    inbox: Mutex<Vec<Op>>,
    /// Extremo de escritura de la tubería de despertar del reactor.
    wake_wr: i32,
}

impl Scheduler {
    fn enqueue(&self, t: Task) {
        self.runq.lock().unwrap().push_back(t);
        self.runq_cv.notify_one();
    }

    fn to_reactor(&self, op: Op) {
        self.inbox.lock().unwrap().push(op);
        sys::wake(self.wake_wr);
    }
}

/// Tamaño de RESERVA de la pila de cada fibra. Reserva virtual: con página de guarda, solo cuestan
/// las páginas tocadas. `RAY_FIBER_STACK_KIB` lo ajusta (mínimo 32 KiB, por seguridad).
fn fiber_stack_size() -> usize {
    static SIZE: OnceLock<usize> = OnceLock::new();
    *SIZE.get_or_init(|| {
        std::env::var("RAY_FIBER_STACK_KIB")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map(|k| k.max(32) * 1024)
            .unwrap_or(128 * 1024)
    })
}

fn sched() -> &'static Scheduler {
    static S: OnceLock<&'static Scheduler> = OnceLock::new();
    S.get_or_init(|| {
        let (wake_rd, wake_wr) = sys::wake_pipe();
        let s: &'static Scheduler = Box::leak(Box::new(Scheduler {
            runq: Mutex::new(VecDeque::new()),
            runq_cv: Condvar::new(),
            inbox: Mutex::new(Vec::new()),
            wake_wr,
        }));
        // Mismo mando que la VM: RAYLANG_THREADS acota los workers (1 = ejecución M:1).
        let workers = std::env::var("RAYLANG_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n >= 1)
            .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4));
        for i in 0..workers {
            std::thread::Builder::new()
                .name(format!("ray-fiber-worker-{i}"))
                .spawn(move || worker_loop(s))
                .expect("could not start a fiber worker");
        }
        std::thread::Builder::new()
            .name("ray-fiber-reactor".into())
            .spawn(move || reactor_loop(s, wake_rd))
            .expect("could not start the fiber reactor");
        s
    })
}

thread_local! {
    /// Yielder de la fibra actualmente en ejecución en ESTE worker (nulo fuera de fibra). Puntero
    /// crudo porque el tipo lleva lifetime; ver los SAFETY de `suspend`.
    static CURRENT: Cell<*const ()> = const { Cell::new(std::ptr::null()) };
}

/// Lanza `f` como fibra. Llamable desde cualquier hilo (incluida otra fibra).
pub fn spawn(f: impl FnOnce() + Send + 'static) -> JoinHandle {
    let done = Arc::new(DoneCell { state: Mutex::new(None), cv: Condvar::new() });
    let stack = DefaultStack::new(fiber_stack_size()).expect("could not map a fiber stack");
    let co = Coroutine::with_stack(stack, move |y: &Yielder<(), Park>, ()| {
        // Prólogo: deja el yielder a mano para la cesión profunda (park desde N marcos más abajo).
        CURRENT.with(|c| c.set(y as *const Yielder<(), Park> as *const ()));
        f();
    });
    let task = Task { co, done: done.clone() };
    sched().enqueue(task);
    JoinHandle { done }
}

/// ¿Está este hilo ejecutando una fibra ahora mismo? (El runtime lo usa para decidir aparcar-fibra
/// contra bloquear-hilo, p. ej. en el hilo `main`, que no es una fibra.)
pub fn in_fiber() -> bool {
    CURRENT.with(|c| !c.get().is_null())
}

/// Cede el control del worker con el motivo dado. Solo dentro de una fibra.
fn suspend(park: Park) {
    let y = CURRENT.with(|c| c.get());
    assert!(!y.is_null(), "fiber park outside of a fiber");
    // SAFETY: `y` apunta al `Yielder` de la corrutina EN EJECUCIÓN en este hilo: vive en la pila de
    // la corrutina (viva hasta que retorna) y solo lo usa la propia fibra. `suspend` devuelve el
    // control al `resume` del worker; cuando la fibra despierte, la ejecución sigue aquí.
    unsafe { (*(y as *const Yielder<(), Park>)).suspend(park) };
    // Al volver podemos estar en OTRO worker: reponer su TLS (el prólogo solo corre una vez).
    CURRENT.with(|c| c.set(y));
}

/// Aparca la fibra hasta que `fd` esté listo para **leer**.
pub fn park_readable(fd: i32) {
    suspend(Park::Read(fd));
}

/// Aparca la fibra hasta que `fd` esté listo para **escribir**.
pub fn park_writable(fd: i32) {
    suspend(Park::Write(fd));
}

/// Duerme la fibra (no el worker) `ms` milisegundos.
pub fn fiber_sleep(ms: i64) {
    suspend(Park::SleepUntil(Instant::now() + Duration::from_millis(ms.max(0) as u64)));
}

/// Cede el turno: la fibra vuelve al final de la cola de listas.
pub fn yield_now() {
    suspend(Park::Yield);
}

fn finish(done: &DoneCell, result: Result<(), String>) {
    *done.state.lock().unwrap() = Some(result);
    done.cv.notify_all();
}

fn panic_msg(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

fn worker_loop(s: &'static Scheduler) {
    loop {
        let mut task = {
            let mut q = s.runq.lock().unwrap();
            loop {
                if let Some(t) = q.pop_front() {
                    break t;
                }
                q = s.runq_cv.wait(q).unwrap();
            }
        };
        // El catch_unwind delimita el panic de LA FIBRA (corosensei lo propaga a través de resume):
        // se publica como Err en su celda y el worker sigue con la siguiente.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task.co.resume(())));
        CURRENT.with(|c| c.set(std::ptr::null()));
        match r {
            Ok(CoroutineResult::Yield(park)) => match park {
                Park::Read(fd) => s.to_reactor(Op::Wait(fd, Dir::Read, task)),
                Park::Write(fd) => s.to_reactor(Op::Wait(fd, Dir::Write, task)),
                Park::SleepUntil(at) => s.to_reactor(Op::Timer(at, task)),
                Park::Yield => s.enqueue(task),
            },
            Ok(CoroutineResult::Return(())) => finish(&task.done, Ok(())),
            Err(p) => finish(&task.done, Err(panic_msg(&*p))),
        }
    }
}

/// Esperas registradas sobre un fd: fibras aparcadas por lectura y por escritura.
#[derive(Default)]
struct FdWaiters {
    read: Vec<Task>,
    write: Vec<Task>,
}

fn reactor_loop(s: &'static Scheduler, wake_rd: i32) {
    let poller = sys::Poller::new();
    let mut fds: HashMap<i32, FdWaiters> = HashMap::new();
    // Temporizadores como Vec con barrido lineal: F1 no espera miles de sleeps simultáneos; si el
    // bench dice lo contrario, un BinaryHeap es un cambio local.
    let mut timers: Vec<(Instant, Task)> = Vec::new();
    loop {
        // 1) Drena el buzón de los workers.
        for op in std::mem::take(&mut *s.inbox.lock().unwrap()) {
            match op {
                Op::Wait(fd, Dir::Read, t) => fds.entry(fd).or_default().read.push(t),
                Op::Wait(fd, Dir::Write, t) => fds.entry(fd).or_default().write.push(t),
                Op::Timer(at, t) => timers.push((at, t)),
            }
        }
        // 2) Despierta los sleeps vencidos y calcula el timeout hasta el siguiente.
        let now = Instant::now();
        let mut i = 0;
        while i < timers.len() {
            if timers[i].0 <= now {
                s.enqueue(timers.swap_remove(i).1);
            } else {
                i += 1;
            }
        }
        let timeout_ms: i32 = match timers.iter().map(|(at, _)| *at).min() {
            Some(at) => at.saturating_duration_since(now).as_millis().min(i32::MAX as u128) as i32,
            None => -1, // sin timers: espera infinita; la tubería interrumpe cuando llegue trabajo
        };
        // 3) Espera readiness de todos los fds con esperas (+ la tubería de despertar).
        let read_fds: Vec<i32> = fds.iter().filter(|(_, w)| !w.read.is_empty()).map(|(&fd, _)| fd).collect();
        let write_fds: Vec<i32> = fds.iter().filter(|(_, w)| !w.write.is_empty()).map(|(&fd, _)| fd).collect();
        let ready = poller.wait(wake_rd, &read_fds, &write_fds, timeout_ms);
        // 4) Reencola las fibras de los fds listos. Si varias esperaban el mismo (fd, dirección),
        //    despiertan todas y las no atendidas se re-aparcan solas: siempre hay progreso.
        for (fd, dir) in ready {
            if fd == wake_rd {
                sys::drain(wake_rd);
                continue;
            }
            if let Some(w) = fds.get_mut(&fd) {
                let woken = match dir {
                    Dir::Read => std::mem::take(&mut w.read),
                    Dir::Write => std::mem::take(&mut w.write),
                };
                for t in woken {
                    s.enqueue(t);
                }
                if w.read.is_empty() && w.write.is_empty() {
                    fds.remove(&fd);
                }
            }
        }
    }
}

// ─── Plataforma: poller persistente + tubería de despertar ─────────────────────────────────────
//
// Mismo principio que `src/poll.rs` de la VM (FFI directo a libSystem/libc, sin crate `libc`),
// con dos diferencias: la instancia es PERSISTENTE (un kqueue/epoll para toda la vida del reactor,
// no uno por llamada) y los intereses de fd son ONESHOT (se auto-limpian al dispararse, que es
// exactamente la semántica de "aparcar hasta que esté listo una vez").

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
mod sys {
    use super::Dir;

    const EVFILT_READ: i16 = -1;
    const EVFILT_WRITE: i16 = -2;
    const EV_ADD: u16 = 0x0001;
    const EV_ONESHOT: u16 = 0x0010;

    /// `struct kevent` de Darwin/BSD (64-bit): 32 bytes, alineación 8 (ver `src/poll.rs`).
    #[repr(C)]
    #[allow(dead_code)]
    struct Kevent {
        ident: usize,
        filter: i16,
        flags: u16,
        fflags: u32,
        data: isize,
        udata: *mut core::ffi::c_void,
    }

    #[repr(C)]
    struct Timespec {
        tv_sec: isize,
        tv_nsec: isize,
    }

    unsafe extern "C" {
        fn kqueue() -> i32;
        fn kevent(
            kq: i32,
            changelist: *const Kevent,
            nchanges: i32,
            eventlist: *mut Kevent,
            nevents: i32,
            timeout: *const Timespec,
        ) -> i32;
        fn pipe(fds: *mut i32) -> i32;
        // OJO: fcntl es VARIÁDICA — declararla con aridad fija es UB en arm64 (los varargs van
        // por la pila en la convención de Apple; la lección ya está aprendida en builtins.rs).
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
        fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    }

    pub struct Poller {
        kq: i32,
    }

    impl Poller {
        pub fn new() -> Poller {
            // SAFETY: syscall sin argumentos; kqueue(2) NO se hereda por fork (no necesita CLOEXEC).
            let kq = unsafe { kqueue() };
            assert!(kq >= 0, "could not create the fiber reactor (kqueue)");
            Poller { kq }
        }

        /// Espera hasta que algo esté listo. Registra los intereses como ONESHOT en la misma llamada
        /// (changelist+eventlist en un solo syscall); re-registrar un (fd, filtro) vivo es idempotente
        /// (lo reemplaza). La tubería se registra SIN oneshot: es permanente.
        pub fn wait(&self, wake_rd: i32, read_fds: &[i32], write_fds: &[i32], timeout_ms: i32) -> Vec<(i32, Dir)> {
            let mut changes: Vec<Kevent> = Vec::with_capacity(read_fds.len() + write_fds.len() + 1);
            let ev = |fd: i32, filter: i16, flags: u16| Kevent {
                ident: fd as usize,
                filter,
                flags,
                fflags: 0,
                data: 0,
                udata: core::ptr::null_mut(),
            };
            changes.push(ev(wake_rd, EVFILT_READ, EV_ADD));
            for &fd in read_fds {
                changes.push(ev(fd, EVFILT_READ, EV_ADD | EV_ONESHOT));
            }
            for &fd in write_fds {
                changes.push(ev(fd, EVFILT_WRITE, EV_ADD | EV_ONESHOT));
            }
            let mut events: Vec<Kevent> = (0..changes.len()).map(|_| ev(0, 0, 0)).collect();
            let ts = Timespec {
                tv_sec: (timeout_ms as isize) / 1000,
                tv_nsec: ((timeout_ms as isize) % 1000) * 1_000_000,
            };
            let tsp = if timeout_ms < 0 { core::ptr::null() } else { &ts as *const Timespec };
            // SAFETY: como en `src/poll.rs` — buffers locales del tamaño declarado, fds de sockets
            // vivos (una fibra aparcada no cierra su fd), `tsp` nulo o apuntando a `ts` viva.
            let n = unsafe {
                kevent(self.kq, changes.as_ptr(), changes.len() as i32, events.as_mut_ptr(), events.len() as i32, tsp)
            };
            if n < 0 {
                return Vec::new(); // EINTR u otro transitorio: el bucle del reactor reintenta
            }
            events[..n as usize]
                .iter()
                .map(|e| (e.ident as i32, if e.filter == EVFILT_WRITE { Dir::Write } else { Dir::Read }))
                .collect()
        }
    }

    /// Crea la tubería de despertar: no-bloqueante y CLOEXEC en ambos extremos (IDEAS §53.4:
    /// `pipe(2)` nace sin CLOEXEC y `F_SETFL` toca los flags de estado, no los del descriptor).
    pub fn wake_pipe() -> (i32, i32) {
        const F_SETFD: i32 = 2;
        const FD_CLOEXEC: i32 = 1;
        const F_SETFL: i32 = 4;
        const O_NONBLOCK: i32 = 0x0004;
        let mut fds = [0i32; 2];
        // SAFETY: `pipe`/`fcntl` sobre un array local; fds recién creados, propiedad de este módulo.
        unsafe {
            assert!(pipe(fds.as_mut_ptr()) == 0, "could not create the reactor wake pipe");
            for fd in fds {
                let _ = fcntl(fd, F_SETFL, O_NONBLOCK);
                let _ = fcntl(fd, F_SETFD, FD_CLOEXEC);
            }
        }
        (fds[0], fds[1])
    }

    /// Toca la tubería (un byte; si está llena, el reactor ya tiene un despertar pendiente).
    pub fn wake(wake_wr: i32) {
        // SAFETY: escribe 1 byte de un buffer local a un fd propio no-bloqueante.
        unsafe {
            let b = 1u8;
            let _ = write(wake_wr, &b as *const u8, 1);
        }
    }

    /// Vacía la tubería tras un despertar (lecturas no-bloqueantes hasta agotar).
    pub fn drain(wake_rd: i32) {
        // SAFETY: lee a un buffer local desde un fd propio no-bloqueante.
        unsafe {
            let mut buf = [0u8; 64];
            while read(wake_rd, buf.as_mut_ptr(), buf.len()) > 0 {}
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod sys {
    use super::Dir;
    use std::collections::HashSet;

    const EPOLL_CTL_ADD: i32 = 1;
    const EPOLL_CTL_MOD: i32 = 3;
    const EPOLL_CLOEXEC: i32 = 0o2000000;
    const EPOLLIN: u32 = 0x001;
    const EPOLLOUT: u32 = 0x004;
    const EPOLLERR: u32 = 0x008;
    const EPOLLHUP: u32 = 0x010;
    const EPOLLONESHOT: u32 = 1 << 30;

    // Empaquetado solo en x86_64, como en `src/poll.rs`.
    #[cfg(target_arch = "x86_64")]
    #[repr(C, packed)]
    #[allow(dead_code)]
    struct EpollEvent {
        events: u32,
        data: u64,
    }
    #[cfg(not(target_arch = "x86_64"))]
    #[repr(C)]
    #[allow(dead_code)]
    struct EpollEvent {
        events: u32,
        data: u64,
    }

    unsafe extern "C" {
        fn epoll_create1(flags: i32) -> i32;
        fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: *mut EpollEvent) -> i32;
        fn epoll_wait(epfd: i32, events: *mut EpollEvent, maxevents: i32, timeout: i32) -> i32;
        fn pipe(fds: *mut i32) -> i32;
        // OJO: fcntl es VARIÁDICA — declararla con aridad fija es UB en arm64 (los varargs van
        // por la pila en la convención de Apple; la lección ya está aprendida en builtins.rs).
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
        fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    }

    pub struct Poller {
        ep: i32,
        /// fds que este epoll ya conoce (en epoll re-ADD da EEXIST: hay que alternar con MOD). Un
        /// ONESHOT disparado sigue "conocido" pero desarmado; se re-arma con MOD. Celda interior
        /// porque `wait` toma `&self`.
        known: std::cell::RefCell<HashSet<i32>>,
    }

    impl Poller {
        pub fn new() -> Poller {
            // SAFETY: syscall directa; CLOEXEC como en el epoll de la VM (auditoría IDEAS §53.4).
            let ep = unsafe { epoll_create1(EPOLL_CLOEXEC) };
            assert!(ep >= 0, "could not create the fiber reactor (epoll)");
            let p = Poller { ep, known: std::cell::RefCell::new(HashSet::new()) };
            p.arm(super::sys_wake_rd(), EPOLLIN); // la tubería, permanente (sin ONESHOT)
            p
        }

        /// Arma (o re-arma) el interés de un fd. Ante cualquier fallo de `epoll_ctl` se degrada a
        /// "listo ya" en el llamador: nunca se pierde una fibra, como mucho despierta de más.
        fn arm(&self, fd: i32, events: u32) -> bool {
            let mut ev = EpollEvent { events, data: fd as u64 };
            let mut known = self.known.borrow_mut();
            let (first, second) =
                if known.contains(&fd) { (EPOLL_CTL_MOD, EPOLL_CTL_ADD) } else { (EPOLL_CTL_ADD, EPOLL_CTL_MOD) };
            // SAFETY: `ev` vive durante la llamada; el fd viene de un socket vivo del programa.
            unsafe {
                if epoll_ctl(self.ep, first, fd, &mut ev as *mut EpollEvent) == 0
                    || epoll_ctl(self.ep, second, fd, &mut ev as *mut EpollEvent) == 0
                {
                    known.insert(fd);
                    return true;
                }
            }
            known.remove(&fd);
            false
        }

        pub fn wait(&self, wake_rd: i32, read_fds: &[i32], write_fds: &[i32], timeout_ms: i32) -> Vec<(i32, Dir)> {
            // La tubería quedó armada permanente en `new`; aquí se arman los intereses ONESHOT.
            // Interés combinado por fd (un fd puede tener esperas de lectura Y de escritura).
            let mut interest: std::collections::HashMap<i32, u32> = std::collections::HashMap::new();
            for &fd in read_fds {
                *interest.entry(fd).or_insert(0) |= EPOLLIN;
            }
            for &fd in write_fds {
                *interest.entry(fd).or_insert(0) |= EPOLLOUT;
            }
            let mut ready: Vec<(i32, Dir)> = Vec::new();
            for (&fd, &evs) in &interest {
                if !self.arm(fd, evs | EPOLLONESHOT) {
                    // No se pudo armar (fd cerrado en carrera, etc.): despierta ya, la fibra decide.
                    if evs & EPOLLIN != 0 {
                        ready.push((fd, Dir::Read));
                    }
                    if evs & EPOLLOUT != 0 {
                        ready.push((fd, Dir::Write));
                    }
                }
            }
            if !ready.is_empty() {
                return ready;
            }
            let cap = interest.len() + 1;
            let mut events: Vec<EpollEvent> = (0..cap).map(|_| EpollEvent { events: 0, data: 0 }).collect();
            // SAFETY: buffer local del tamaño declarado; el timeout es el contrato de epoll_wait.
            let n = unsafe { epoll_wait(self.ep, events.as_mut_ptr(), cap as i32, timeout_ms) };
            if n < 0 {
                return Vec::new(); // EINTR: el bucle del reactor reintenta
            }
            for e in &events[..n as usize] {
                let evs = e.events;
                let fd = { e.data } as i32; // copia (struct empaquetado en x86_64)
                // ERR/HUP despiertan AMBAS direcciones: la fibra hará la syscall y verá el error real.
                if evs & (EPOLLIN | EPOLLERR | EPOLLHUP) != 0 {
                    ready.push((fd, Dir::Read));
                }
                if evs & (EPOLLOUT | EPOLLERR | EPOLLHUP) != 0 {
                    ready.push((fd, Dir::Write));
                }
            }
            ready
        }
    }

    // La tubería se crea antes que el Poller pero epoll necesita armarla en `new`: se memoriza aquí.
    static WAKE_RD: std::sync::OnceLock<i32> = std::sync::OnceLock::new();

    pub(super) fn sys_wake_rd() -> i32 {
        *WAKE_RD.get().expect("wake_pipe() must run before Poller::new()")
    }

    pub fn wake_pipe() -> (i32, i32) {
        const F_SETFD: i32 = 2;
        const FD_CLOEXEC: i32 = 1;
        const F_SETFL: i32 = 4;
        const O_NONBLOCK: i32 = 0o4000;
        let mut fds = [0i32; 2];
        // SAFETY: `pipe`/`fcntl` sobre un array local; fds recién creados, propiedad de este módulo.
        unsafe {
            assert!(pipe(fds.as_mut_ptr()) == 0, "could not create the reactor wake pipe");
            for fd in fds {
                let _ = fcntl(fd, F_SETFL, O_NONBLOCK);
                let _ = fcntl(fd, F_SETFD, FD_CLOEXEC);
            }
        }
        let _ = WAKE_RD.set(fds[0]);
        (fds[0], fds[1])
    }

    pub fn wake(wake_wr: i32) {
        // SAFETY: escribe 1 byte de un buffer local a un fd propio no-bloqueante.
        unsafe {
            let b = 1u8;
            let _ = write(wake_wr, &b as *const u8, 1);
        }
    }

    pub fn drain(wake_rd: i32) {
        // SAFETY: lee a un buffer local desde un fd propio no-bloqueante.
        unsafe {
            let mut buf = [0u8; 64];
            while read(wake_rd, buf.as_mut_ptr(), buf.len()) > 0 {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::os::fd::AsRawFd;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Lee aparcando la fibra en WouldBlock (el patrón que usará el runtime transpilado en F2).
    fn read_parked(s: &mut TcpStream, buf: &mut [u8]) -> usize {
        loop {
            match s.read(buf) {
                Ok(n) => return n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => park_readable(s.as_raw_fd()),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => panic!("read: {e}"),
            }
        }
    }

    fn write_parked(s: &mut TcpStream, mut buf: &[u8]) {
        while !buf.is_empty() {
            match s.write(buf) {
                Ok(0) => panic!("write: connection closed"),
                Ok(n) => buf = &buf[n..],
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => park_writable(s.as_raw_fd()),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => panic!("write: {e}"),
            }
        }
    }

    #[test]
    fn thousands_of_fibers_run_to_completion_on_few_workers() {
        static DONE: AtomicUsize = AtomicUsize::new(0);
        let handles: Vec<_> = (0..2000)
            .map(|_| {
                spawn(|| {
                    yield_now(); // fuerza al menos una vuelta por el scheduler
                    DONE.fetch_add(1, Ordering::Relaxed);
                })
            })
            .collect();
        for h in handles {
            h.join().expect("la fibra termina sin panic");
        }
        assert_eq!(DONE.load(Ordering::Relaxed), 2000);
    }

    #[test]
    fn parked_io_echoes_across_two_hundred_concurrent_connections() {
        const N: usize = 200;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        listener.set_nonblocking(true).expect("nonblocking listener");

        // Servidor: una fibra acepta (aparcada en el listener) y lanza una fibra de eco por conexión.
        let server = spawn(move || {
            let mut echoes = Vec::new();
            for _ in 0..N {
                let mut conn = loop {
                    match listener.accept() {
                        Ok((c, _)) => break c,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            park_readable(listener.as_raw_fd())
                        }
                        Err(e) => panic!("accept: {e}"),
                    }
                };
                echoes.push(spawn(move || {
                    conn.set_nonblocking(true).expect("nonblocking conn");
                    let mut buf = [0u8; 32];
                    let n = read_parked(&mut conn, &mut buf);
                    write_parked(&mut conn, &buf[..n]);
                }));
            }
            for e in echoes {
                e.join().expect("la fibra de eco termina");
            }
        });

        // Clientes: N fibras concurrentes; cada una escribe su marca y verifica el eco.
        //
        // ⚠️ Lección de este mismo test: el `connect` BLOQUEANTE aquí construye un livelock real.
        // Con N > backlog del listener (128), los SYN sobrantes se retransmiten DENTRO del worker;
        // si los N workers acaban clavados en `connect`, el servidor — listo en la cola — no tiene
        // dónde correr, nadie acepta, y todo muere por timeout. Es la propiedad documentada del
        // modelo ("una syscall bloqueante retiene al worker"): las syscalls bloqueantes dentro de
        // una fibra deben ser ACOTADAS. De ahí el connect con timeout corto + reintento con cesión
        // (el connect real del runtime irá no-bloqueante con `park_writable` en F2).
        let clients: Vec<_> = (0..N)
            .map(|i| {
                spawn(move || {
                    let mut s = loop {
                        match TcpStream::connect_timeout(&addr, Duration::from_millis(50)) {
                            Ok(s) => break s,
                            Err(_) => yield_now(), // backlog lleno: cede y deja correr al servidor
                        }
                    };
                    s.set_nonblocking(true).expect("nonblocking client");
                    let msg = format!("fiber-{i}");
                    write_parked(&mut s, msg.as_bytes());
                    let mut buf = [0u8; 32];
                    let mut got = Vec::new();
                    while got.len() < msg.len() {
                        let n = read_parked(&mut s, &mut buf);
                        assert!(n > 0, "el servidor cerró antes del eco completo");
                        got.extend_from_slice(&buf[..n]);
                    }
                    assert_eq!(got, msg.as_bytes());
                })
            })
            .collect();
        for c in clients {
            c.join().expect("la fibra cliente termina");
        }
        server.join().expect("la fibra servidora termina");
    }

    #[test]
    fn sleeping_fibers_wake_in_deadline_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let handles: Vec<_> = [(30u64, 30u8), (10, 10), (20, 20)]
            .into_iter()
            .map(|(ms, tag)| {
                let order = order.clone();
                spawn(move || {
                    fiber_sleep(ms as i64);
                    order.lock().unwrap().push(tag);
                })
            })
            .collect();
        for h in handles {
            h.join().expect("la fibra durmiente termina");
        }
        assert_eq!(*order.lock().unwrap(), vec![10, 20, 30]);
    }

    #[test]
    fn a_panicking_fiber_reports_the_message_and_the_scheduler_survives() {
        let bad = spawn(|| panic!("boom in fiber"));
        assert_eq!(bad.join(), Err("boom in fiber".to_string()));
        // El scheduler sigue vivo: una fibra posterior corre con normalidad.
        let ok = spawn(|| {});
        assert_eq!(ok.join(), Ok(()));
    }

    #[test]
    fn a_fiber_can_spawn_other_fibers() {
        static DONE: AtomicUsize = AtomicUsize::new(0);
        let h = spawn(|| {
            let inner: Vec<_> = (0..50)
                .map(|_| {
                    spawn(|| {
                        DONE.fetch_add(1, Ordering::Relaxed);
                    })
                })
                .collect();
            // join desde una fibra BLOQUEA el worker (la espera fibra-a-fibra es de F3); con
            // workers ≥ 2 no interbloquea y para el test basta. Con RAYLANG_THREADS=1 este test
            // no debe correrse tal cual.
            for i in inner {
                i.join().expect("la fibra interior termina");
            }
        });
        h.join().expect("la fibra exterior termina");
        assert_eq!(DONE.load(Ordering::Relaxed), 50);
    }
}
