//! D1 del arco de hot reload móvil (findings ray-apps #50/#106): **parada cooperativa** de la VM
//! desde fuera del programa. La librería de desarrollo que corre en el teléfono no puede
//! reiniciar el proceso (iOS no relanza una app que sale), así que el programa en curso tiene
//! que terminar *dentro* del proceso y dejar el runtime limpio para el siguiente.
//!
//! Mecanismo (reusa la fontanería de señales de M88.1): una bandera atómica global y un self-pipe
//! cuyo extremo de lectura entra SIEMPRE al conjunto del poller de `io_wait`. `request_stop`
//! (cualquier hilo) sube la bandera y escribe un octeto; el scheduler la ve en la siguiente
//! conmutación de fibra (`poll_next`) o, si el programa gira sin conmutar, cada 64 K instrucciones
//! (la máscara sobre el `fuel`), y fija `outcome = Err(STOP)` → todos los workers se detienen y
//! `run_program` devuelve un error distinguido ([`RuntimeError::is_stop`]). Sin fd (Windows,
//! wasm) el poller sondea a cuantos cortos y consulta la bandera, como con las señales.
//!
//! Es transporte de desarrollo/embedding: ningún builtin del lenguaje lo expone.

use std::sync::atomic::{AtomicBool, Ordering};

/// El mensaje del error de parada (lo reconoce `RuntimeError::is_stop`).
pub const STOP_MSG: &str = "program stopped";

static REQUESTED: AtomicBool = AtomicBool::new(false);

/// El self-pipe `(rd, wr)`, creado la primera vez que arranca una VM (`ensure`). En plataformas
/// sin pipe queda `(-1, -1)` y el scheduler se apoya en la bandera.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn pipe() -> &'static (i32, i32) {
    static P: std::sync::OnceLock<(i32, i32)> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        unsafe extern "C" {
            fn pipe(fds: *mut i32) -> i32;
            fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        }
        const F_GETFL: i32 = 3;
        const F_SETFL: i32 = 4;
        const F_SETFD: i32 = 2;
        const FD_CLOEXEC: i32 = 1;
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        const O_NONBLOCK: i32 = 0x4;
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        const O_NONBLOCK: i32 = 0o4000;
        let mut fds = [-1i32; 2];
        // SAFETY: `pipe` escribe dos fds válidos en el arreglo; si falla, quedan en -1 y la
        // parada se degrada a "solo bandera" (la próxima conmutación o sondeo la ve).
        unsafe {
            if pipe(fds.as_mut_ptr()) != 0 {
                return (-1, -1);
            }
            for fd in fds {
                let fl = fcntl(fd, F_GETFL);
                fcntl(fd, F_SETFL, fl | O_NONBLOCK);
                fcntl(fd, F_SETFD, FD_CLOEXEC);
            }
        }
        (fds[0], fds[1])
    })
}

/// Crea el self-pipe si aún no existe (lo llama el arranque de cada `run_program`).
pub(crate) fn ensure() {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        let _ = pipe();
    }
}

/// El fd de lectura del self-pipe para el poller (`-1` = sin fd: sondear la bandera).
pub(crate) fn fd() -> i32 {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        pipe().0
    }
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
    {
        -1
    }
}

/// ¿Hay una parada pedida? (lectura barata; no la consume — la consume `clear`, al arrancar el
/// siguiente programa, para que TODOS los workers la vean mientras se apagan).
#[inline]
pub(crate) fn requested() -> bool {
    REQUESTED.load(Ordering::Acquire)
}

/// Pide que el programa en curso termine (desde cualquier hilo; idempotente). La VM devuelve
/// `Err` con [`STOP_MSG`] en cuanto lo ve; si no hay programa corriendo, el siguiente
/// `run_program` la descarta al arrancar (no se guarda para él).
pub fn request_stop() {
    REQUESTED.store(true, Ordering::Release);
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        let wr = pipe().1;
        if wr >= 0 {
            unsafe extern "C" {
                fn write(fd: i32, buf: *const u8, n: usize) -> isize;
            }
            // SAFETY: un octeto a un fd nuestro no bloqueante; con el pipe lleno se omite (ya hay
            // octetos que despiertan al poller).
            unsafe { write(wr, [1u8].as_ptr(), 1) };
        }
    }
}

/// Baja la bandera y drena el self-pipe: el arranque de un programa nuevo.
pub(crate) fn clear() {
    REQUESTED.store(false, Ordering::Release);
    drain();
}

/// Vacía el self-pipe (los octetos son solo un despertador; la verdad es la bandera).
pub(crate) fn drain() {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        let rd = pipe().0;
        if rd >= 0 {
            unsafe extern "C" {
                fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
            }
            let mut buf = [0u8; 64];
            // SAFETY: lectura no bloqueante a un búfer propio hasta EAGAIN.
            while unsafe { read(rd, buf.as_mut_ptr(), buf.len()) } > 0 {}
        }
    }
}
