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
//! el socket vuelva a ser escribible. En una plataforma sin poller (p. ej. Windows) devuelve
//! `Unsupported` y el scheduler cae al busy-poll de M15.5. Un error transitorio (EINTR) se mapea a
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

// ─── Otras plataformas (p. ej. Windows): sin poller → busy-poll de M15.5 ─────────────────────────
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "linux",
    target_os = "android"
)))]
mod sys {
    use super::PollResult;
    pub fn wait(_read_fds: &[i32], _write_fds: &[i32], _timeout_ms: i32) -> PollResult {
        PollResult::Unsupported
    }
}
