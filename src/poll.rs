//! M17 — *readiness* de E/S del SO (`kqueue`/`epoll`), para el scheduler de la VM.
//!
//! M15.5 implementó la concurrencia de red con un **busy-poll cooperativo**: cuando ninguna fibra
//! está lista pero hay fibras esperando E/S, el scheduler dormía ~1 ms y las re-encolaba todas para
//! que reintentaran. Funciona y es cero-deps, pero malgasta CPU y añade latencia. M17 lo sustituye
//! por **notificación de readiness del SO**: el scheduler se **bloquea** hasta que algún socket esté
//! realmente listo para leer y despierta **solo** las fibras de esos descriptores.
//!
//! **Invariante de cero dependencias de Cargo.** `std` no expone `kqueue`/`epoll`/`poll`, así que en
//! vez de traer el crate `libc` declaramos aquí los pocos `extern "C"` que necesitamos: viven en
//! libSystem (macOS/BSD) / libc (Linux), **siempre enlazados** → no son una dependencia, solo FFI con
//! `unsafe` acotado. Los descriptores (`RawFd`, un `i32` en Unix) salen de `std` vía
//! `AsRawFd::as_raw_fd()` (ver `builtins::raw_fd`).
//!
//! **API**: `wait(read_fds, write_fds, timeout_ms)` bloquea hasta que al menos uno de `read_fds` esté
//! listo para **leer** o uno de `write_fds` para **escribir** (o venza el timeout; `timeout_ms < 0` =
//! infinito) y devuelve `Ready(listos)` con los fds que quedaron listos. La cesión en `socket_write`
//! (post-M19.4) usa el interés de escritura: una fibra que llena el buffer de envío se aparca hasta que
//! el socket vuelva a ser escribible. Windows tiene su backend desde M174 (`WSAPoll`, la misma forma
//! que `poll(2)`); en una plataforma sin poller devuelve `Unsupported` y el scheduler cae al
//! busy-poll de M15.5. Un error transitorio (EINTR) se mapea a
//! `Ready(vacío)`, que el scheduler también resuelve cayendo al busy-poll → **siempre hay progreso**.

/// Resultado de esperar readiness. `Ready` con la lista (posiblemente vacía) de fds listos (de lectura
/// o de escritura); `Unsupported` si esta plataforma no tiene poller (el llamador debe hacer busy-poll).
pub enum PollResult {
    Ready(Vec<i32>),
    Unsupported,
}

/// Bloquea hasta que algún `read_fds` esté listo para **leer** o algún `write_fds` para **escribir** (o
/// venza `timeout_ms`; negativo = infinito). Despacha a la implementación de la plataforma.
pub fn wait(read_fds: &[i32], write_fds: &[i32], timeout_ms: i32) -> PollResult {
    sys::wait(read_fds, write_fds, timeout_ms)
}

/// Duerme el hilo `ms` milisegundos **con precisión** (M119). `ms <= 0` no duerme.
///
/// Por qué no `thread::sleep`: en macOS su `nanosleep` está sujeto a *timer coalescing* y se pasa
/// varios ms (medido: `sleep(33)` → ~37 ms), lo que descuadra el pacing de un juego o un
/// muestreador (§72). `poll(2)` con **cero descriptores** honra el timeout por la vía de eventos del
/// kernel, mucho más ajustada (medido: ~34 ms), y es portable en Unix. Reintenta ante `EINTR` hasta
/// cubrir el plazo, así que garantiza dormir *al menos* lo pedido (semántica de `thread::sleep`).
/// En Windows (M174) usa un *waitable timer* de alta resolución; en plataformas sin nada de eso cae
/// a `thread::sleep`.
pub fn sleep_ms(ms: i64) {
    if ms <= 0 {
        return;
    }
    sys::sleep_ms(ms as u64);
}

/// `struct pollfd`. Solo se usa para dar a la declaración de `poll` el MISMO tipo de puntero que la
/// otra declaración del crate (`builtins::watch_mod`), y así no disparar `clashing_extern_declarations`
/// (dos `extern` del mismo símbolo con firmas ABI-distintas). En [`sleep_ms`] siempre se pasa un
/// puntero nulo con `nfds = 0` (espera pura por timeout), así que los campos no se tocan.
#[cfg(unix)]
#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

/// Núcleo común de [`sleep_ms`] para las plataformas con `poll(2)`: llama `poll_once(timeout_ms)`
/// (que ejecuta `poll(NULL, 0, timeout_ms)`) y, si vuelve por `EINTR` (retorno < 0) antes de cubrir
/// el plazo, reintenta con el tiempo restante. Así se garantiza dormir **al menos** `ms`.
#[cfg(unix)]
fn poll_sleep(ms: u64, poll_once: impl Fn(i32) -> i32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return;
        }
        // +1: redondeo hacia arriba para no despertar un pelo antes del plazo y girar de más.
        let timeout_ms = (remaining.as_millis().min(i32::MAX as u128 - 1) as i32) + 1;
        if poll_once(timeout_ms) >= 0 {
            return; // timeout cumplido (n == 0) o algún fd listo (imposible con 0 fds)
        }
        // n < 0 → EINTR (una señal): reintenta el resto del plazo.
    }
}

// ─── macOS / BSD: kqueue ────────────────────────────────────────────────────────────────────────
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
mod sys {
    use super::PollResult;

    const EVFILT_READ: i16 = -1;
    const EVFILT_WRITE: i16 = -2;
    const EV_ADD: u16 = 0x0001;

    /// `struct kevent` de Darwin/BSD (64-bit): 32 bytes, alineación 8. El kernel lee/escribe todos los
    /// campos; nosotros solo leemos `ident` (el fd listo) de los eventos de salida.
    #[repr(C)]
    #[allow(dead_code)]
    struct Kevent {
        ident: usize,  // uintptr_t — el descriptor
        filter: i16,   // int16_t   — EVFILT_READ
        flags: u16,    // uint16_t  — EV_ADD, …
        fflags: u32,   // uint32_t
        data: isize,   // intptr_t
        udata: *mut core::ffi::c_void,
    }

    /// `struct timespec` (64-bit): segundos + nanosegundos, ambos `long`.
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
        fn close(fd: i32) -> i32;
        // `poll(NULL, 0, ms)` = espera precisa (M119). Firma ABI-idéntica a `builtins::watch_mod`
        // (mismo `*mut PollFd` y `nfds: u64`) para no disparar `clashing_extern_declarations`.
        fn poll(fds: *mut super::PollFd, nfds: u64, timeout: i32) -> i32;
    }

    pub fn sleep_ms(ms: u64) {
        super::poll_sleep(ms, |t| unsafe { poll(core::ptr::null_mut(), 0, t) });
    }

    pub fn wait(read_fds: &[i32], write_fds: &[i32], timeout_ms: i32) -> PollResult {
        if read_fds.is_empty() && write_fds.is_empty() {
            return PollResult::Ready(Vec::new());
        }
        // SAFETY: `kqueue`/`kevent` son syscalls de libSystem. Los fds vienen de sockets vivos del
        // registro de handles; en el modelo M:1 cooperativo nadie los cierra mientras esperamos. Los
        // buffers (`changes`/`events`) viven en este marco durante toda la llamada y tienen el tamaño
        // que se pasa. `tsp` es nulo (infinito) o apunta a `ts`, vivo aquí.
        unsafe {
            let kq = kqueue();
            if kq < 0 {
                return PollResult::Unsupported;
            }
            // Una sola llamada a kevent registra (changelist) y espera (eventlist) → un solo syscall.
            // Un fd puede pedir lectura (EVFILT_READ) y/o escritura (EVFILT_WRITE) con eventos separados.
            let mut changes: Vec<Kevent> = Vec::with_capacity(read_fds.len() + write_fds.len());
            for &fd in read_fds {
                changes.push(Kevent { ident: fd as usize, filter: EVFILT_READ, flags: EV_ADD, fflags: 0, data: 0, udata: core::ptr::null_mut() });
            }
            for &fd in write_fds {
                changes.push(Kevent { ident: fd as usize, filter: EVFILT_WRITE, flags: EV_ADD, fflags: 0, data: 0, udata: core::ptr::null_mut() });
            }
            let mut events: Vec<Kevent> = (0..changes.len())
                .map(|_| Kevent {
                    ident: 0,
                    filter: 0,
                    flags: 0,
                    fflags: 0,
                    data: 0,
                    udata: core::ptr::null_mut(),
                })
                .collect();
            let ts = Timespec {
                tv_sec: (timeout_ms as isize) / 1000,
                tv_nsec: ((timeout_ms as isize) % 1000) * 1_000_000,
            };
            let tsp = if timeout_ms < 0 {
                core::ptr::null()
            } else {
                &ts as *const Timespec
            };
            let n = kevent(
                kq,
                changes.as_ptr(),
                changes.len() as i32,
                events.as_mut_ptr(),
                events.len() as i32,
                tsp,
            );
            close(kq);
            if n < 0 {
                // EINTR u otro error transitorio: que el scheduler reintente vía el busy-poll de respaldo.
                return PollResult::Ready(Vec::new());
            }
            let ready = events[..n as usize].iter().map(|e| e.ident as i32).collect();
            PollResult::Ready(ready)
        }
    }
}

// ─── Linux / Android: epoll ─────────────────────────────────────────────────────────────────────
#[cfg(any(target_os = "linux", target_os = "android"))]
mod sys {
    use super::PollResult;

    const EPOLL_CTL_ADD: i32 = 1;
    /// `EPOLL_CLOEXEC` == `O_CLOEXEC` (0o2000000 en Linux): el fd del epoll no sobrevive a un `exec`.
    const EPOLL_CLOEXEC: i32 = 0o2000000;
    const EPOLLIN: u32 = 0x001;
    const EPOLLOUT: u32 = 0x004;

    // En glibc, `struct epoll_event` está **empaquetada** solo en x86_64 (12 bytes); en otras arqui-
    // tecturas lleva la alineación natural del `u64` (16 bytes). Reproducimos ambos casos.
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
        fn close(fd: i32) -> i32;
        // `poll(NULL, 0, ms)` = espera precisa (M119). Firma ABI-idéntica a `builtins::watch_mod`
        // (`nfds` es `unsigned long`/u64 en Linux LP64) para no disparar `clashing_extern_declarations`.
        fn poll(fds: *mut super::PollFd, nfds: u64, timeout: i32) -> i32;
    }

    pub fn sleep_ms(ms: u64) {
        super::poll_sleep(ms, |t| unsafe { poll(core::ptr::null_mut(), 0, t) });
    }

    pub fn wait(read_fds: &[i32], write_fds: &[i32], timeout_ms: i32) -> PollResult {
        if read_fds.is_empty() && write_fds.is_empty() {
            return PollResult::Ready(Vec::new());
        }
        // SAFETY: `epoll_*` son syscalls de libc. Igual que en kqueue: fds vivos, buffers locales del
        // tamaño declarado, llamadas bien formadas.
        unsafe {
            // EPOLL_CLOEXEC (auditoría jul 2026, IDEAS §53.4): sin él, el fd del epoll se filtraría a
            // un hijo lanzado por exec DURANTE esta llamada. La ventana es estrecha (el epoll nace y
            // muere dentro de `wait`) y hoy no hay vía de fuga, pero el flag es gratis y es lo
            // correcto. En macOS/BSD no hace falta el equivalente: kqueue(2) garantiza que el
            // descriptor NO se hereda por `fork`.
            let ep = epoll_create1(EPOLL_CLOEXEC);
            if ep < 0 {
                return PollResult::Unsupported;
            }
            // Interés por fd: lectura (EPOLLIN), escritura (EPOLLOUT) o ambos. Se combinan en un solo
            // `EPOLL_CTL_ADD` por fd (añadir el mismo fd dos veces daría EEXIST).
            let mut interest: std::collections::HashMap<i32, u32> = std::collections::HashMap::new();
            for &fd in read_fds { *interest.entry(fd).or_insert(0) |= EPOLLIN; }
            for &fd in write_fds { *interest.entry(fd).or_insert(0) |= EPOLLOUT; }
            for (&fd, &evs) in &interest {
                let mut ev = EpollEvent {
                    events: evs,
                    data: fd as u64, // guardamos el fd para recuperarlo del evento listo
                };
                epoll_ctl(ep, EPOLL_CTL_ADD, fd, &mut ev as *mut EpollEvent);
            }
            let mut events: Vec<EpollEvent> = (0..interest.len())
                .map(|_| EpollEvent { events: 0, data: 0 })
                .collect();
            let n = epoll_wait(ep, events.as_mut_ptr(), events.len() as i32, timeout_ms);
            close(ep);
            if n < 0 {
                return PollResult::Ready(Vec::new());
            }
            let ready = events[..n as usize]
                .iter()
                .map(|e| {
                    let d = e.data; // copia: no se puede referenciar un campo de un struct empaquetado
                    d as i32
                })
                .collect();
            PollResult::Ready(ready)
        }
    }
}

// ─── Windows: WSAPoll (M174, docs/windows.md W5) ────────────────────────────────────────────────
// `WSAPoll` (ws2_32, Vista+) es `poll(2)` sobre SOCKETs: la misma forma que este módulo, sin crates
// (wepoll habría sido la alternativa; IOCP es el arco largo de las fibras nativas, W7). Los "fds" que
// llegan son los SOCKET del registro (`builtins::raw_fd`, que en Windows devuelve el handle como
// i32) y el pseudo-fd 0 de stdin (M107.2), que NO es un socket: WSAPoll falla entero con
// WSAENOTSOCK ante un handle ajeno, así que stdin se atiende aparte, sondeando `stdin_ready(0)`
// (M173) a cuantos de 5 ms mientras se espera a los sockets. Un socket con error/cierre
// (POLLERR/POLLHUP/POLLNVAL) cuenta como listo: la fibra reintenta y recoge el error real.
#[cfg(windows)]
mod sys {
    use super::PollResult;

    const POLLRDNORM: i16 = 0x0100;
    const POLLWRNORM: i16 = 0x0010;
    const POLLERR: i16 = 0x0001;
    const POLLHUP: i16 = 0x0002;
    const POLLNVAL: i16 = 0x0004;

    /// `WSAPOLLFD`: el SOCKET (usize), interés y resultado.
    #[repr(C)]
    struct WsaPollFd {
        fd: usize,
        events: i16,
        revents: i16,
    }

    #[link(name = "ws2_32")]
    unsafe extern "system" {
        fn WSAPoll(fds: *mut WsaPollFd, nfds: u32, timeout: i32) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateWaitableTimerExW(attrs: *const core::ffi::c_void, name: *const u16, flags: u32, access: u32) -> usize;
        fn SetWaitableTimer(timer: usize, due: *const i64, period: i32, routine: *const core::ffi::c_void, arg: *const core::ffi::c_void, resume: i32) -> i32;
        fn WaitForSingleObject(handle: usize, ms: u32) -> u32;
        fn CloseHandle(handle: usize) -> i32;
    }
    const CREATE_WAITABLE_TIMER_HIGH_RESOLUTION: u32 = 0x0000_0002;
    const TIMER_ALL_ACCESS: u32 = 0x001F_0003;
    const INFINITE: u32 = 0xFFFF_FFFF;

    /// Sueño fino (M174): un *waitable timer* de ALTA RESOLUCIÓN (Windows 10 1803+), que no está
    /// sujeto al tick de 15,6 ms del planificador — `thread::sleep(1)` dormía ~15 ms y descuadraba el
    /// pacing de juegos/audio y `time.sleep_ms`. El timer se crea una vez por hilo. Si el sistema no
    /// lo ofrece, `thread::sleep` (imprecisión asumida). Garantiza dormir AL MENOS `ms`.
    pub fn sleep_ms(ms: u64) {
        thread_local! {
            static TIMER: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        }
        let timer = TIMER.with(|t| {
            if t.get() == 0 {
                // SAFETY: llamada sin punteros salvo nulos; el handle se retiene para el hilo.
                let h = unsafe { CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS) };
                t.set(if h == 0 { usize::MAX } else { h });
            }
            t.get()
        });
        if timer == usize::MAX {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            return;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return;
            }
            // Plazo relativo en unidades de 100 ns (negativo = relativo).
            let due: i64 = -((remaining.as_nanos() / 100) as i64).max(1);
            // SAFETY: `due` vive durante la llamada; sin rutina de completado; el timer es de este hilo.
            let armed = unsafe { SetWaitableTimer(timer, &due, 0, std::ptr::null(), std::ptr::null(), 0) };
            if armed == 0 {
                std::thread::sleep(remaining);
                return;
            }
            // SAFETY: espera sobre un handle propio.
            unsafe { WaitForSingleObject(timer, INFINITE) };
        }
    }

    /// Cierra el timer de un hilo (no se llama: los hilos del scheduler viven lo que el proceso).
    #[allow(dead_code)]
    fn close_timer(h: usize) {
        // SAFETY: cerrar un handle propio.
        unsafe { CloseHandle(h) };
    }

    pub fn wait(read_fds: &[i32], write_fds: &[i32], timeout_ms: i32) -> PollResult {
        if read_fds.is_empty() && write_fds.is_empty() {
            return PollResult::Ready(Vec::new());
        }
        let has_stdin = read_fds.contains(&crate::builtins::STDIN_PSEUDO_HANDLE_FD);
        // Interés por socket: lectura, escritura o ambos (un socket dos veces daría dos entradas;
        // WSAPoll lo tolera, pero se combinan para devolverlo una sola vez).
        let mut fds: Vec<WsaPollFd> = Vec::with_capacity(read_fds.len() + write_fds.len());
        let mut add = |fd: i32, ev: i16| {
            if fd == crate::builtins::STDIN_PSEUDO_HANDLE_FD || fd < 0 {
                return;
            }
            match fds.iter_mut().find(|p| p.fd == fd as usize) {
                Some(p) => p.events |= ev,
                None => fds.push(WsaPollFd { fd: fd as usize, events: ev, revents: 0 }),
            }
        };
        for &fd in read_fds {
            add(fd, POLLRDNORM);
        }
        for &fd in write_fds {
            add(fd, POLLWRNORM);
        }
        let deadline = (timeout_ms >= 0).then(|| std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64));
        loop {
            // Con stdin en espera, se sondea a cuantos de 5 ms; sin él, la espera entera va al poller.
            let slice_ms: i32 = match deadline {
                Some(d) => {
                    let left = d.saturating_duration_since(std::time::Instant::now());
                    let left_ms = (left.as_millis().min(i32::MAX as u128 - 1) as i32) + 1;
                    if has_stdin { left_ms.min(5) } else { left_ms }
                }
                None => if has_stdin { 5 } else { -1 },
            };
            let mut ready: Vec<i32> = Vec::new();
            if fds.is_empty() {
                sleep_ms(slice_ms.max(0) as u64);
            } else {
                for p in fds.iter_mut() {
                    p.revents = 0;
                }
                // SAFETY: `fds` es un arreglo de WSAPOLLFD bien formado del tamaño declarado; los
                // SOCKET vienen del registro de handles y nadie los cierra mientras esperamos.
                let n = unsafe { WSAPoll(fds.as_mut_ptr(), fds.len() as u32, slice_ms) };
                if n < 0 {
                    // WSAENOTSOCK u otro error: que el scheduler caiga al busy-poll de respaldo.
                    return PollResult::Ready(Vec::new());
                }
                if n > 0 {
                    for p in &fds {
                        let want_read = p.events & POLLRDNORM != 0 && p.revents & (POLLRDNORM | POLLERR | POLLHUP | POLLNVAL) != 0;
                        let want_write = p.events & POLLWRNORM != 0 && p.revents & (POLLWRNORM | POLLERR | POLLHUP | POLLNVAL) != 0;
                        if want_read || want_write {
                            ready.push(p.fd as i32);
                        }
                    }
                }
            }
            if has_stdin && crate::builtins::stdin_ready(0) {
                ready.push(crate::builtins::STDIN_PSEUDO_HANDLE_FD);
            }
            if !ready.is_empty() {
                return PollResult::Ready(ready);
            }
            if let Some(d) = deadline
                && std::time::Instant::now() >= d
            {
                return PollResult::Ready(Vec::new());
            }
        }
    }
}

// ─── Otras plataformas: sin poller → busy-poll de M15.5 ──────────────────────────────────────────
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "linux",
    target_os = "android",
    windows
)))]
mod sys {
    use super::PollResult;
    pub fn wait(_read_fds: &[i32], _write_fds: &[i32], _timeout_ms: i32) -> PollResult {
        PollResult::Unsupported
    }
    /// Sin `poll(2)` (p. ej. Windows): respaldo a `thread::sleep` (imprecisión asumida).
    pub fn sleep_ms(ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M119: `sleep_ms` duerme AL MENOS lo pedido (regresión del wrapper viejo, que con listas
    /// vacías retornaba al instante). Solo cota INFERIOR: el techo depende de la carga de la máquina
    /// y haría el test flaky.
    #[test]
    fn sleep_ms_waits_at_least_the_requested_time() {
        let t0 = std::time::Instant::now();
        sleep_ms(30);
        let dt = t0.elapsed().as_millis();
        assert!(dt >= 28, "sleep_ms(30) debería dormir ~30ms, durmió {dt}ms");
    }

    /// M174: readiness REAL en las tres plataformas (kqueue/epoll/WSAPoll): un listener sin
    /// conexiones no está listo (la espera vence: `Ready(vacío)` tras ~el plazo), y con una
    /// conexión pendiente en el backlog está listo para leer (`accept` no bloquearía).
    #[cfg(any(unix, windows))]
    #[test]
    fn a_listener_is_ready_only_with_a_pending_connection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        #[cfg(unix)]
        let fd = {
            use std::os::unix::io::AsRawFd;
            listener.as_raw_fd()
        };
        #[cfg(windows)]
        let fd = {
            use std::os::windows::io::AsRawSocket;
            i32::try_from(listener.as_raw_socket()).unwrap()
        };
        let t0 = std::time::Instant::now();
        match wait(&[fd], &[], 60) {
            PollResult::Ready(r) => assert!(r.is_empty(), "sin conexiones no hay readiness: {r:?}"),
            PollResult::Unsupported => panic!("esta plataforma debe tener poller"),
        }
        assert!(t0.elapsed().as_millis() >= 50, "la espera honra el plazo");
        let _client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        match wait(&[fd], &[], 2000) {
            PollResult::Ready(r) => assert_eq!(r, vec![fd], "el listener está listo con una conexión pendiente"),
            PollResult::Unsupported => panic!("esta plataforma debe tener poller"),
        }
    }

    /// `ms <= 0` no duerme (retorno inmediato).
    #[test]
    fn sleep_ms_zero_or_negative_is_a_noop() {
        let t0 = std::time::Instant::now();
        sleep_ms(0);
        sleep_ms(-5);
        assert!(t0.elapsed().as_millis() < 20, "un sleep no positivo no debe bloquear");
    }
}
