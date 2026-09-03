//! M172 (W3, `docs/windows.md` §3.2) — la **capa de sistema operativo del supervisor** de
//! `ray dev` y `ray test --watch`: lanzar al hijo de forma que se le pueda pedir un cierre
//! ORDENADO, terminarlo con drenado y escalada, arrastrarlo si el supervisor muere, y pasarle el
//! socket de escucha retenido (socket-activation, M92.3). Cada primitivo tiene su variante unix
//! (la de siempre: SIGTERM, `kill` desde el handler, `dup2` al fd 3) y su variante Windows:
//!
//! | Necesidad | unix | Windows |
//! |---|---|---|
//! | cierre ordenado del hijo | `kill(pid, SIGTERM)` | `CREATE_NEW_PROCESS_GROUP` al lanzar + `GenerateConsoleCtrlEvent(CTRL_BREAK, pid)` — el handler de consola del hijo (M168) lo entrega como `2` y `serve_graceful` drena |
//! | sin huérfanos si el supervisor muere | handler SIGTERM/SIGINT que reenvía SIGTERM (`kill` por pid); Ctrl-C mata al grupo | un **Job Object** con `KILL_ON_JOB_CLOSE`: el hijo entra al job y, cuando el último handle (el del supervisor) se cierra por la razón que sea, Windows lo mata |
//! | Ctrl-C en el supervisor | el handler reenvía SIGTERM y sale | `SetConsoleCtrlHandler` reenvía CTRL_BREAK al grupo del hijo, espera su salida (≤ 3 s) y sale |
//! | socket-activation | `dup2(fd, 3)` en `pre_exec` + `RAY_LISTEN_FD=3` | el handle del socket se marca HEREDABLE (`SetHandleInformation`) y su valor viaja en `RAY_LISTEN_FD`; el hijo lo adopta con `from_raw_socket` |
//!
//! Detalle de Windows que importa: `CREATE_NEW_PROCESS_GROUP` deja al hijo IGNORANDO el Ctrl-C de
//! la consola (así lo define el sistema), de modo que el hijo solo recibe lo que el supervisor le
//! reenvía — exactamente lo que se quiere: el supervisor decide. Sin consola (stdio redirigido
//! desde un servicio) `GenerateConsoleCtrlEvent` falla y se cae al `TerminateProcess` de antes,
//! avisando. El Job Object es anidable desde Windows 8, así que funciona aunque el propio
//! supervisor viva dentro de otro job (un runner de CI, un IDE).

use std::process;

/// Prepara el `Command` del hijo supervisado. Windows: grupo de procesos propio, requisito de
/// `GenerateConsoleCtrlEvent`. Unix: nada (el hijo hereda el grupo; Ctrl-C de terminal lo mata
/// junto al supervisor, que es lo deseado).
pub fn prepare(cmd: &mut process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(win::CREATE_NEW_PROCESS_GROUP);
    }
    let _ = &cmd;
}

/// Registra al hijo recién lanzado como supervisado. Windows: lo mete en el Job Object del
/// supervisor (kill-on-close); un fallo de asignación no es fatal (se avisa una vez: el hijo
/// funciona, solo pierde la garantía anti-huérfanos). Unix: nada que hacer.
pub fn adopt(child: &process::Child) {
    #[cfg(windows)]
    win::assign_to_job(child);
    let _ = child;
}

/// Termina el hijo con una petición de cierre ORDENADO (SIGTERM / CTRL_BREAK) — un servidor con
/// `serve_graceful` drena sus conexiones — y, si a los ~3 s sigue vivo, escala al kill duro.
pub fn terminate_gracefully(child: &mut process::Child) {
    let requested = request_shutdown(child.id());
    if requested {
        for _ in 0..30 {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        eprintln!("[dev] the program did not drain in time; forced termination");
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Pide al hijo `pid` que termine ordenadamente. `true` si la petición se entregó (y merece
/// esperar el drenado); `false` si en esta plataforma/situación no hay forma (→ kill directo).
fn request_shutdown(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        const SIGTERM: i32 = 15;
        // SAFETY: `kill` a un pid que lanzamos nosotros y aún no hemos recogido (`wait`).
        unsafe { kill(pid as i32, SIGTERM) == 0 }
    }
    #[cfg(windows)]
    {
        win::send_ctrl_break(pid)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

/// Instala la limpieza "si el supervisor muere, el hijo también": el handler de SIGTERM/SIGINT
/// (unix) o de control de consola (Windows) reenvía la petición de cierre al hijo en curso —
/// cuyo pid lee de `child_pid` — y sale con 130. Windows además crea el Job Object (ver el
/// módulo): un `TerminateProcess` al supervisor, donde ningún handler corre, también arrastra
/// al hijo.
pub fn install_cleanup_on_death(child_pid: &'static std::sync::atomic::AtomicI32) {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn signal(sig: i32, handler: usize) -> usize;
        }
        // El handler es async-signal-safe: `kill` + `_exit`, sin asignar. Solo puede leer un
        // estático: el pid se comparte por el atómico global que registra `cli`.
        static PID: std::sync::atomic::AtomicPtr<std::sync::atomic::AtomicI32> =
            std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
        extern "C" fn on_death(_sig: i32) {
            unsafe extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
                fn _exit(code: i32) -> !;
            }
            let p = PID.load(std::sync::atomic::Ordering::SeqCst);
            // SAFETY: `p` apunta a un `AtomicI32` `'static` (registrado en `install_cleanup_on_death`).
            let pid = if p.is_null() { 0 } else { unsafe { (*p).load(std::sync::atomic::Ordering::SeqCst) } };
            if pid > 0 {
                // SAFETY: `kill` a un hijo nuestro. SIGTERM: drena (serve_graceful) o muere por defecto.
                unsafe {
                    kill(pid, 15);
                }
            }
            // SAFETY: `_exit` es async-signal-safe.
            unsafe { _exit(130) }
        }
        PID.store(
            child_pid as *const _ as *mut std::sync::atomic::AtomicI32,
            std::sync::atomic::Ordering::SeqCst,
        );
        const SIGINT: i32 = 2;
        const SIGTERM: i32 = 15;
        // SAFETY: instalar un handler `extern "C"` válido para dos señales estándar.
        unsafe {
            signal(SIGINT, on_death as *const () as usize);
            signal(SIGTERM, on_death as *const () as usize);
        }
    }
    #[cfg(windows)]
    win::install_cleanup(child_pid);
    let _ = child_pid;
}

/// Pasa al hijo el socket de escucha retenido (socket-activation): lo hace visible en el hijo y
/// le deja en `RAY_LISTEN_FD`/`RAY_LISTEN_ADDR` cómo adoptarlo (`adopt_or_bind`, builtins).
/// Unix: `dup2` al fd 3 en `pre_exec` (sin CLOEXEC). Windows: el handle del socket se marca
/// heredable y `RAY_LISTEN_FD` lleva su VALOR (los handles heredados conservan el número).
pub fn pass_listener(cmd: &mut process::Command, listener: &std::net::TcpListener, addr: &str) {
    cmd.env("RAY_LISTEN_ADDR", addr);
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        use std::os::unix::process::CommandExt;
        const TARGET_FD: i32 = 3; // convención systemd (SD_LISTEN_FDS_START)
        let fd = listener.as_raw_fd();
        cmd.env("RAY_LISTEN_FD", TARGET_FD.to_string());
        // SAFETY: `pre_exec` corre en el hijo tras `fork` y antes de `exec`; solo se llama a
        // `dup2`/`fcntl` (async-signal-safe). `fd` (el listener del supervisor) es válido en el
        // hijo por herencia del fork. Se limpia CLOEXEC en el fd destino EXPLÍCITAMENTE: si `fd`
        // ya ERA 3 (típico: primer libre tras stdio), `dup2(3,3)` es un no-op que NO limpia
        // CLOEXEC → sin esto, el fd 3 se cerraría en el exec.
        unsafe {
            cmd.pre_exec(move || {
                unsafe extern "C" {
                    fn dup2(oldfd: i32, newfd: i32) -> i32;
                    // VARIÁDICA, como la declaración de builtins.rs (aridad fija = UB en arm64 y
                    // `clashing_extern_declarations` entre ambas).
                    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
                }
                const F_SETFD: i32 = 2; // limpiar los flags del descriptor (quita FD_CLOEXEC)
                // (sin `unsafe` interior: el closure ya corre dentro del bloque unsafe de pre_exec)
                if dup2(fd, TARGET_FD) < 0 || fcntl(TARGET_FD, F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        let sock = listener.as_raw_socket();
        if win::set_inheritable(sock as usize, true) {
            cmd.env("RAY_LISTEN_FD", sock.to_string());
        } else {
            eprintln!("[dev] could not make the listener inheritable; the program will re-bind");
            cmd.env_remove("RAY_LISTEN_ADDR");
        }
    }
    let _ = (listener, addr);
}

/// ¿Esta plataforma sabe pasar el socket retenido al hijo? (Para el aviso de `ray dev --port`.)
pub fn supports_socket_activation() -> bool {
    cfg!(any(unix, windows))
}

#[cfg(windows)]
pub mod win {
    use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicUsize, Ordering};

    pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CTRL_BREAK_EVENT: u32 = 1;
    const HANDLE_FLAG_INHERIT: u32 = 0x1;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const WAIT_OBJECT_0: u32 = 0;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;

    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    struct ExtendedLimitInformation {
        basic: BasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attrs: *const core::ffi::c_void, name: *const u16) -> usize;
        fn SetInformationJobObject(job: usize, class: u32, info: *const core::ffi::c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: usize, process: usize) -> i32;
        fn GenerateConsoleCtrlEvent(event: u32, process_group_id: u32) -> i32;
        fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
        fn SetHandleInformation(handle: usize, mask: u32, flags: u32) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> usize;
        fn WaitForSingleObject(handle: usize, ms: u32) -> u32;
        fn CloseHandle(handle: usize) -> i32;
        fn ExitProcess(code: u32) -> !;
    }

    /// El Job Object del supervisor (0 = aún no creado; `usize::MAX` = no se pudo crear).
    static JOB: AtomicUsize = AtomicUsize::new(0);
    /// El atómico con el pid del hijo en curso (lo registra `install_cleanup`).
    static CHILD_PID: AtomicPtr<AtomicI32> = AtomicPtr::new(std::ptr::null_mut());

    /// Crea (una vez) el job con kill-on-close. El handle se retiene para toda la vida del
    /// supervisor y NO es heredable: cuando este proceso muere, es el último handle y Windows
    /// mata a todo lo asignado.
    fn job() -> usize {
        let cur = JOB.load(Ordering::SeqCst);
        if cur != 0 {
            return cur;
        }
        let created = create_kill_on_close_job().unwrap_or(usize::MAX);
        match JOB.compare_exchange(0, created, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => created,
            Err(other) => {
                // Carrera perdida (no ocurre: el supervisor es monohilo aquí); se conserva el primero.
                if created != usize::MAX {
                    // SAFETY: cerrar un handle propio que no se va a usar.
                    unsafe {
                        CloseHandle(created);
                    }
                }
                other
            }
        }
    }

    /// Crea un Job Object con `KILL_ON_JOB_CLOSE`: todo proceso asignado muere cuando se cierra
    /// el último handle del job. `None` si el sistema lo niega. El handle NO es heredable.
    pub fn create_kill_on_close_job() -> Option<usize> {
        // SAFETY: llamadas a kernel32 con una estructura `repr(C)` inicializada a cero salvo el
        // flag; si la configuración falla, el handle se cierra aquí mismo.
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h == 0 {
                return None;
            }
            let mut info: ExtendedLimitInformation = std::mem::zeroed();
            info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<ExtendedLimitInformation>() as u32,
            );
            if ok == 0 {
                CloseHandle(h);
                return None;
            }
            Some(h)
        }
    }

    /// Asigna un proceso hijo a un job. `false` si Windows lo niega.
    pub fn assign_process(job: usize, child: &std::process::Child) -> bool {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: handles válidos (el del job lo creó `create_kill_on_close_job`; el del hijo lo
        // posee `Child`).
        unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as usize) != 0 }
    }

    /// Cierra un handle del kernel (para los tests: cerrar el job y ver morir al hijo).
    pub fn close_handle(handle: usize) {
        // SAFETY: cerrar un handle propio.
        unsafe {
            CloseHandle(handle);
        }
    }

    /// Mete al hijo en el job del supervisor. Avisa (una sola vez) si no se puede.
    pub fn assign_to_job(child: &std::process::Child) {
        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let job = job();
        let ok = job != usize::MAX && assign_process(job, child);
        if !ok && !WARNED.swap(true, Ordering::SeqCst) {
            eprintln!(
                "[dev] could not attach the program to the supervisor's job object ({}); \
                 if the supervisor is killed, the program may outlive it",
                std::io::Error::last_os_error()
            );
        }
    }

    /// Envía CTRL_BREAK al grupo de procesos del hijo (`pid`, líder por `CREATE_NEW_PROCESS_GROUP`).
    /// `false` si no se pudo (sin consola compartida: p. ej. lanzado por un servicio).
    pub fn send_ctrl_break(pid: u32) -> bool {
        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        // SAFETY: llamada a kernel32 sin punteros.
        let ok = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) } != 0;
        if !ok && !WARNED.swap(true, Ordering::SeqCst) {
            eprintln!(
                "[dev] could not send Ctrl-Break to the program ({}); restarts will terminate it without draining",
                std::io::Error::last_os_error()
            );
        }
        ok
    }

    /// Marca (o desmarca) un handle como heredable por los procesos hijos.
    pub fn set_inheritable(handle: usize, inherit: bool) -> bool {
        // SAFETY: llamada a kernel32 sobre un handle válido del llamador.
        unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, if inherit { HANDLE_FLAG_INHERIT } else { 0 }) != 0 }
    }

    /// Espera hasta `ms` a que el proceso `pid` termine. `true` si terminó.
    fn wait_pid(pid: u32, ms: u32) -> bool {
        // SAFETY: abrir un proceso solo para sincronizar; el handle se cierra aquí mismo.
        unsafe {
            let h = OpenProcess(SYNCHRONIZE, 0, pid);
            if h == 0 {
                return true; // ya no existe
            }
            let r = WaitForSingleObject(h, ms);
            CloseHandle(h);
            r == WAIT_OBJECT_0
        }
    }

    /// El cuerpo del handler de consola del supervisor, separado para poder probarlo: reenvía
    /// CTRL_BREAK al hijo en curso, le da hasta 3 s para drenar y devuelve el código de salida
    /// con el que el supervisor debe morir. Ante un evento ajeno devuelve `None`.
    pub fn on_console_event(event: u32) -> Option<u32> {
        // 0 CTRL_C, 1 CTRL_BREAK, 2 CTRL_CLOSE, 5 CTRL_LOGOFF, 6 CTRL_SHUTDOWN
        if !matches!(event, 0 | 1 | 2 | 5 | 6) {
            return None;
        }
        let p = CHILD_PID.load(Ordering::SeqCst);
        // SAFETY: `p` es nulo o apunta a un `AtomicI32` `'static` registrado en `install_cleanup`.
        let pid = if p.is_null() { 0 } else { unsafe { (*p).load(Ordering::SeqCst) } };
        if pid > 0 && send_ctrl_break(pid as u32) {
            wait_pid(pid as u32, 3000);
        }
        Some(130)
    }

    unsafe extern "system" fn on_ctrl(event: u32) -> i32 {
        match on_console_event(event) {
            // SAFETY: salir del proceso desde el hilo del handler es el uso previsto.
            Some(code) => unsafe { ExitProcess(code) },
            None => 0,
        }
    }

    /// Instala el handler de consola del supervisor y crea el job (para que exista ANTES del
    /// primer hijo: la ventana entre lanzar y asignar es de microsegundos, pero el job no debe
    /// fallar en ese momento por falta de memoria o permisos sin que se vea).
    pub fn install_cleanup(child_pid: &'static AtomicI32) {
        CHILD_PID.store(child_pid as *const _ as *mut AtomicI32, Ordering::SeqCst);
        let _ = job();
        // SAFETY: registrar un handler `extern "system"` válido.
        if unsafe { SetConsoleCtrlHandler(Some(on_ctrl), 1) } == 0 {
            eprintln!("[dev] could not install the console control handler; Ctrl-C will not forward to the program");
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn socket_activation_is_available_on_desktop_platforms() {
        assert!(super::supports_socket_activation());
    }

    #[cfg(windows)]
    #[test]
    fn console_events_are_forwarded_and_the_supervisor_exits_130() {
        // Sin hijo registrado: el handler no reenvía nada y pide salir con 130 ante los eventos
        // de cierre; un evento ajeno se ignora (Windows sigue con la acción por defecto).
        for ev in [0u32, 1, 2, 5, 6] {
            assert_eq!(super::win::on_console_event(ev), Some(130), "evento {ev}");
        }
        assert_eq!(super::win::on_console_event(99), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_child_in_the_job_dies_when_the_job_handle_closes() {
        // Un hijo inerte (`cmd /c pause` sin stdin: espera para siempre) entra al job; cerrar el
        // ÚLTIMO handle del job debe matarlo. Se usa un job PROPIO del test (no el estático del
        // supervisor) para poder cerrarlo.
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "pause"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("lanza cmd");
        let job = super::win::create_kill_on_close_job().expect("crea el job");
        assert!(super::win::assign_process(job, &child), "asigna al hijo");
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(child.try_wait().unwrap().is_none(), "el hijo sigue vivo dentro del job");
        super::win::close_handle(job);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "el hijo no murió al cerrar el job");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}
