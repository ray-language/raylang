//! M145 — salida de audio PCM (IDEAS §79): el análogo de `term.*` para sonido.
//!
//! La forma es un PIPE + un hilo alimentador: `open` crea un pipe del SO, registra el extremo de
//! escritura como handle ordinario (variante `PipeW` del registro de builtins) y lanza un hilo
//! que LEE el extremo de lectura y empuja el PCM al dispositivo. Con eso `audio.write` reusa
//! VERBATIM la maquinaria de contrapresión de `Proc.write` (M100 v3): pipe lleno → la fibra
//! APARCA por interés de escritura — el *pacing* del programa sale del consumo real del
//! dispositivo, sin relojes. `close(h)` es el EOF: el alimentador drena lo pendiente y termina.
//!
//! Backends A MANO, cero crates (la decisión de IDEAS §79 se volteó al implementar: `cpal`
//! arrastra `alsa-sys`, que exige los headers de ALSA EN BUILD — rompería `cargo build` en
//! cualquier Linux pelado y en CI; ninguna dependencia del proyecto impone eso — rusqlite
//! vendorea, rustls/notify son puros):
//!   - macOS: AudioQueue (AudioToolbox.framework, SIEMPRE presente — se enlaza al build).
//!   - Linux: ALSA por `dlopen("libasound.so.2")` EN RUNTIME (sin headers; sin la lib → `Err`).
//!   - `RAY_AUDIO_SINK=null`: un sumidero que consume a ritmo de tiempo real SIN hardware — la
//!     vía de los tests (CI no tiene tarjeta de sonido) y de los benchmarks.
//!
//! Formato único de v1: PCM **s16le entrelazado** (el mínimo común de los tres backends).

#![cfg(all(feature = "audio", unix))]

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

unsafe extern "C" {
    fn pipe(fds: *mut i32) -> i32;
    fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
    fn close(fd: i32) -> i32;
    // Variádica a propósito (lección de arm64, como en watch.rs/term).
    #[link_name = "fcntl"]
    fn fcntl_raw(fd: i32, cmd: i32, ...) -> i32;
}
const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NONBLOCK: i32 = 0o4000; // M156: bionic también es 0o4000 (android es unix, no "linux")
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const O_NONBLOCK: i32 = 0x0004;

/// El control de una salida viva, para `drain`: cuántos octetos ha aceptado el alimentador que
/// aún no ha entregado al backend, y los parámetros para estimar la latencia del dispositivo.
pub struct Ctl {
    /// Octetos leídos del pipe y aún no entregados al backend (el "en vuelo" del alimentador).
    in_flight: AtomicI64,
    /// Octetos por segundo del formato (rate × channels × 2): para el margen de `drain`.
    bytes_per_sec: i64,
    /// El extremo de LECTURA del pipe (para que `drain` consulte lo encolado con FIONREAD).
    fd_r: i32,
}

/// El mapa fd-de-escritura → control, para que `drain(fd)` encuentre su salida.
fn ctls() -> &'static Mutex<HashMap<i32, Arc<Ctl>>> {
    static CTLS: OnceLock<Mutex<HashMap<i32, Arc<Ctl>>>> = OnceLock::new();
    CTLS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Abre una salida PCM s16le (`sample_rate` Hz, `channels` canales) y devuelve el extremo de
/// escritura del pipe (no-bloqueante, listo para el registro de handles) — el llamador escribe
/// samples ahí y el hilo alimentador los toca. `RAY_AUDIO_SINK=null` → sumidero de tiempo real.
pub fn open(sample_rate: i64, channels: i64) -> Result<std::fs::File, String> {
    if !(8000..=192000).contains(&sample_rate) {
        return Err(format!("audio: unsupported sample rate {sample_rate} (8000–192000)"));
    }
    if !(1..=8).contains(&channels) {
        return Err(format!("audio: unsupported channel count {channels} (1–8)"));
    }
    // El backend se abre ANTES del pipe: un dispositivo ausente falla en `open`, no a mitad.
    let sink = make_sink(sample_rate, channels)?;

    let mut fds = [0i32; 2];
    // SAFETY: pipe escribe dos fds válidos.
    if unsafe { pipe(fds.as_mut_ptr()) } != 0 {
        return Err(format!("audio: could not create the pipe: {}", std::io::Error::last_os_error()));
    }
    let (fd_r, fd_w) = (fds[0], fds[1]);
    // El extremo de escritura NO-bloqueante: el contrato de `socket_write_nb` (WouldBlock =
    // lleno → la fibra aparca). El de lectura queda bloqueante para el alimentador.
    unsafe {
        let fl = fcntl_raw(fd_w, F_GETFL);
        fcntl_raw(fd_w, F_SETFL, fl | O_NONBLOCK);
    }

    let ctl = Arc::new(Ctl {
        in_flight: AtomicI64::new(0),
        bytes_per_sec: sample_rate * channels * 2,
        fd_r,
    });
    ctls().lock().unwrap().insert(fd_w, ctl.clone());

    // El alimentador: lee el pipe (bloqueante) y empuja al backend; EOF (close del handle) →
    // drena el backend y termina. El chunk es ~50 ms de audio: latencia baja sin syscalls de más.
    let chunk = ((ctl.bytes_per_sec / 20).max(256) as usize) & !1;
    let ctl_thread = ctl.clone();
    std::thread::spawn(move || {
        let mut sink = sink;
        let mut buf = vec![0u8; chunk];
        loop {
            // SAFETY: buf vive durante la llamada; fd_r es nuestro hasta el close de abajo.
            let n = unsafe { read(fd_r, buf.as_mut_ptr(), buf.len()) };
            if n <= 0 {
                break; // EOF (el handle se cerró) o error: fin de la sesión
            }
            ctl_thread.in_flight.fetch_add(n as i64, Ordering::SeqCst);
            sink.play(&buf[..n as usize]);
            ctl_thread.in_flight.fetch_sub(n as i64, Ordering::SeqCst);
        }
        sink.finish();
        unsafe { close(fd_r) };
        ctls().lock().unwrap().remove(&fd_w);
    });

    // SAFETY: fd_w es nuestro; File toma la propiedad (su Drop = close = EOF del alimentador).
    Ok(unsafe { std::os::unix::io::FromRawFd::from_raw_fd(fd_w) })
}

/// Espera a que TODO lo escrito suene: pipe vacío + alimentador sin nada en vuelo + un margen de
/// la latencia del dispositivo. Bloquea el hilo (uso raro, al final de una sesión) — el margen
/// es aproximado por diseño: el "de verdad sonó" exacto es del backend y v1 no lo persigue.
pub fn drain(fd_w: i32) -> Result<(), String> {
    let ctl = match ctls().lock().unwrap().get(&fd_w) {
        Some(c) => c.clone(),
        None => return Err("audio: not an open audio output".to_string()),
    };
    loop {
        let queued = pipe_pending(ctl.fd_r);
        let flying = ctl.in_flight.load(Ordering::SeqCst);
        if queued == 0 && flying == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Margen: lo último entregado al backend aún suena (~su buffer). 250 ms cubre a los tres.
    std::thread::sleep(std::time::Duration::from_millis(250));
    Ok(())
}

// Octetos pendientes de lectura en el pipe (FIONREAD), 0 si no se puede saber.
fn pipe_pending(fd: i32) -> i64 {
    unsafe extern "C" {
        fn ioctl(fd: i32, req: u64, ...) -> i32;
    }
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    const FIONREAD: u64 = 0x4004_667F;
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd")))]
    const FIONREAD: u64 = 0x541B;
    let mut n: i32 = 0;
    // SAFETY: FIONREAD escribe un int.
    if unsafe { ioctl(fd, FIONREAD, &mut n as *mut i32) } == 0 { n as i64 } else { 0 }
}

// ── Los backends ─────────────────────────────────────────────────────────────

/// Un sumidero PCM: recibe s16le entrelazado y lo hace sonar (o lo consume). `play` puede
/// BLOQUEAR (es la contrapresión: el pipe se llena aguas arriba y la fibra aparca).
trait Sink: Send {
    fn play(&mut self, data: &[u8]);
    fn finish(&mut self);
}

fn make_sink(rate: i64, channels: i64) -> Result<Box<dyn Sink>, String> {
    if std::env::var("RAY_AUDIO_SINK").as_deref() == Ok("null") {
        return Ok(Box::new(NullSink { bytes_per_sec: rate * channels * 2 }));
    }
    #[cfg(target_os = "macos")]
    {
        coreaudio::open(rate, channels)
    }
    #[cfg(target_os = "linux")]
    {
        alsa::open(rate, channels)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (rate, channels);
        Err("audio: no backend for this platform (macOS/Linux; RAY_AUDIO_SINK=null works anywhere)".to_string())
    }
}

/// El sumidero nulo: consume a ritmo de TIEMPO REAL (duerme lo que duraría el audio). Da la
/// misma contrapresión que un dispositivo — la vía de los tests en CI sin tarjeta de sonido.
struct NullSink {
    bytes_per_sec: i64,
}

impl Sink for NullSink {
    fn play(&mut self, data: &[u8]) {
        let ms = data.len() as i64 * 1000 / self.bytes_per_sec;
        std::thread::sleep(std::time::Duration::from_millis(ms.max(1) as u64));
    }
    fn finish(&mut self) {}
}

// ── macOS: AudioQueue (AudioToolbox.framework, enlazado — siempre presente) ─────
#[cfg(target_os = "macos")]
mod coreaudio {
    use super::Sink;
    use std::collections::VecDeque;
    use std::sync::{Arc, Condvar, Mutex};

    // Los tipos opacos de AudioToolbox se manejan como punteros crudos.
    type AudioQueueRef = *mut std::ffi::c_void;
    type AudioQueueBufferRef = *mut AudioQueueBuffer;

    #[repr(C)]
    struct AudioStreamBasicDescription {
        sample_rate: f64,
        format_id: u32,
        format_flags: u32,
        bytes_per_packet: u32,
        frames_per_packet: u32,
        bytes_per_frame: u32,
        channels_per_frame: u32,
        bits_per_channel: u32,
        reserved: u32,
    }

    #[repr(C)]
    struct AudioQueueBuffer {
        audio_data_bytes_capacity: u32,
        audio_data: *mut u8,
        audio_data_byte_size: u32,
        user_data: *mut std::ffi::c_void,
        // El resto de la struct (packet descriptions) no se toca: se accede solo a la cabecera.
    }

    #[link(name = "AudioToolbox", kind = "framework")]
    unsafe extern "C" {
        fn AudioQueueNewOutput(
            format: *const AudioStreamBasicDescription,
            callback: extern "C" fn(*mut std::ffi::c_void, AudioQueueRef, AudioQueueBufferRef),
            user_data: *mut std::ffi::c_void,
            run_loop: *const std::ffi::c_void,
            run_loop_mode: *const std::ffi::c_void,
            flags: u32,
            out: *mut AudioQueueRef,
        ) -> i32;
        fn AudioQueueAllocateBuffer(q: AudioQueueRef, size: u32, out: *mut AudioQueueBufferRef) -> i32;
        fn AudioQueueEnqueueBuffer(
            q: AudioQueueRef,
            buf: AudioQueueBufferRef,
            n_packets: u32,
            packets: *const std::ffi::c_void,
        ) -> i32;
        fn AudioQueueStart(q: AudioQueueRef, start_time: *const std::ffi::c_void) -> i32;
        fn AudioQueueStop(q: AudioQueueRef, immediate: u8) -> i32;
        fn AudioQueueDispose(q: AudioQueueRef, immediate: u8) -> i32;
    }

    const FORMAT_LINEAR_PCM: u32 = 0x6C70_636D; // 'lpcm'
    const FLAG_IS_SIGNED_INTEGER: u32 = 1 << 2;
    const FLAG_IS_PACKED: u32 = 1 << 3;

    /// El estado compartido con el callback: un anillo de PCM pendiente. El callback SACA; el
    /// alimentador METE (bloqueando con la condvar cuando el anillo está lleno — contrapresión).
    struct Shared {
        ring: Mutex<VecDeque<u8>>,
        space: Condvar,
        cap: usize,
        /// Octetos por frame (canales × 2): los buffers se entregan SIEMPRE alineados a frame.
        frame_bytes: usize,
        /// El silencio de mantener-viva-la-cola con el anillo seco: ~8 ms, alineado a frame.
        /// Todo lo encolado se INSERTA en la línea de tiempo — el silencio es latencia
        /// permanente, así que se encola el mínimo que evita que la cola muera (rallyx).
        keepalive_bytes: usize,
    }

    pub struct CoreAudioSink {
        q: AudioQueueRef,
        shared: Arc<Shared>,
    }
    // SAFETY: AudioQueueRef se usa solo desde el hilo alimentador tras la creación; el callback
    // corre en el hilo interno de AudioToolbox y solo toca `shared` (sincronizado).
    unsafe impl Send for CoreAudioSink {}

    /// El callback de la cola: rellena el buffer con lo que haya en el anillo (silencio si está
    /// vacío — la cola no debe pararse entre writes del programa) y lo re-encola.
    extern "C" fn on_buffer(user: *mut std::ffi::c_void, q: AudioQueueRef, buf: AudioQueueBufferRef) {
        // SAFETY: `user` es el Arc<Shared> filtrado en open (vive hasta Dispose); `buf` es de la cola.
        let shared = unsafe { &*(user as *const Shared) };
        unsafe {
            let cap = (*buf).audio_data_bytes_capacity as usize;
            let out = std::slice::from_raw_parts_mut((*buf).audio_data, cap);
            let mut ring = shared.ring.lock().unwrap();
            // byte_size = EXACTAMENTE lo tomado, alineado a frame — jamás rellenar un buffer
            // parcial con silencio: todo octeto encolado se inserta en la línea de tiempo y un
            // relleno es LATENCIA PERMANENTE (hallazgo de rallyx: 150 ms de cebado + 50 ms por
            // underrun con el diseño anterior).
            let fb = shared.frame_bytes;
            let take = (ring.len().min(cap) / fb) * fb;
            if take > 0 {
                for slot in out.iter_mut().take(take) {
                    *slot = ring.pop_front().unwrap_or(0);
                }
                (*buf).audio_data_byte_size = take as u32;
            } else {
                // Anillo seco: el MÍNIMO de silencio que mantiene viva la cola (~8 ms) — un
                // buffer sin encolar sale de la rotación y la cola muere.
                let silence = shared.keepalive_bytes.min(cap).max(fb);
                for slot in out.iter_mut().take(silence) {
                    *slot = 0;
                }
                (*buf).audio_data_byte_size = silence as u32;
            }
            drop(ring);
            shared.space.notify_all();
            AudioQueueEnqueueBuffer(q, buf, 0, std::ptr::null());
        }
    }

    pub fn open(rate: i64, channels: i64) -> Result<Box<dyn Sink>, String> {
        let bytes_per_frame = (channels * 2) as u32;
        let desc = AudioStreamBasicDescription {
            sample_rate: rate as f64,
            format_id: FORMAT_LINEAR_PCM,
            format_flags: FLAG_IS_SIGNED_INTEGER | FLAG_IS_PACKED,
            bytes_per_packet: bytes_per_frame,
            frames_per_packet: 1,
            bytes_per_frame,
            channels_per_frame: channels as u32,
            bits_per_channel: 16,
            reserved: 0,
        };
        // El anillo guarda ~200 ms; cada buffer de la cola, ~50 ms (3 buffers, el clásico).
        let frame_bytes = bytes_per_frame as usize;
        let bytes_per_sec = (rate * channels * 2) as usize;
        let cap = (bytes_per_sec / 5).max(4096) / frame_bytes * frame_bytes;
        let buf_size = ((cap / 4).max(1024) / frame_bytes * frame_bytes) as u32;
        // ~8 ms de silencio de keepalive (el cebado son 3 → ~24 ms de retraso inicial, no 150).
        let keepalive_bytes = (bytes_per_sec / 125).max(frame_bytes) / frame_bytes * frame_bytes;
        let shared = Arc::new(Shared {
            ring: Mutex::new(VecDeque::new()),
            space: Condvar::new(),
            cap,
            frame_bytes,
            keepalive_bytes,
        });
        let user = Arc::into_raw(shared.clone()) as *mut std::ffi::c_void;
        let mut q: AudioQueueRef = std::ptr::null_mut();
        // SAFETY: desc/out válidos; el callback y `user` viven hasta Dispose.
        let st = unsafe {
            AudioQueueNewOutput(&desc, on_buffer, user, std::ptr::null(), std::ptr::null(), 0, &mut q)
        };
        if st != 0 {
            // Recupera el Arc filtrado para no filtrar memoria en el camino de error.
            unsafe { drop(Arc::from_raw(user as *const Shared)) };
            return Err(format!("audio: AudioQueueNewOutput failed (OSStatus {st})"));
        }
        unsafe {
            for _ in 0..3 {
                let mut b: AudioQueueBufferRef = std::ptr::null_mut();
                if AudioQueueAllocateBuffer(q, buf_size, &mut b) == 0 {
                    on_buffer(user, q, b); // se estrena con silencio y queda encolado
                }
            }
            let st = AudioQueueStart(q, std::ptr::null());
            if st != 0 {
                AudioQueueDispose(q, 1);
                drop(Arc::from_raw(user as *const Shared));
                return Err(format!("audio: AudioQueueStart failed (OSStatus {st})"));
            }
        }
        Ok(Box::new(CoreAudioSink { q, shared }))
    }

    impl Sink for CoreAudioSink {
        fn play(&mut self, data: &[u8]) {
            let mut ring = self.shared.ring.lock().unwrap();
            for &b in data {
                // Contrapresión: anillo lleno → espera a que el callback haga sitio.
                while ring.len() >= self.shared.cap {
                    ring = self.shared.space.wait(ring).unwrap();
                }
                ring.push_back(b);
            }
        }

        fn finish(&mut self) {
            // Espera a que el anillo se vacíe y para la cola (Stop no-inmediato drena sus buffers).
            loop {
                if self.shared.ring.lock().unwrap().is_empty() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            unsafe {
                AudioQueueStop(self.q, 0);
                AudioQueueDispose(self.q, 0);
            }
        }
    }

}

// ── Linux: ALSA por dlopen (sin headers de build; sin libasound → Err claro) ────
#[cfg(target_os = "linux")]
mod alsa {
    use super::Sink;

    unsafe extern "C" {
        fn dlopen(path: *const u8, flags: i32) -> *mut std::ffi::c_void;
        fn dlsym(handle: *mut std::ffi::c_void, name: *const u8) -> *mut std::ffi::c_void;
    }
    const RTLD_NOW: i32 = 2;

    type Pcm = *mut std::ffi::c_void;
    type FnOpen = unsafe extern "C" fn(*mut Pcm, *const u8, i32, i32) -> i32;
    type FnSetParams = unsafe extern "C" fn(Pcm, i32, i32, u32, u32, i32, u32) -> i32;
    type FnWritei = unsafe extern "C" fn(Pcm, *const u8, u64) -> i64;
    type FnRecover = unsafe extern "C" fn(Pcm, i32, i32) -> i32;
    type FnDrain = unsafe extern "C" fn(Pcm) -> i32;
    type FnClose = unsafe extern "C" fn(Pcm) -> i32;

    const SND_PCM_STREAM_PLAYBACK: i32 = 0;
    const SND_PCM_FORMAT_S16_LE: i32 = 2;
    const SND_PCM_ACCESS_RW_INTERLEAVED: i32 = 3;

    pub struct AlsaSink {
        pcm: Pcm,
        writei: FnWritei,
        recover: FnRecover,
        drain: FnDrain,
        close: FnClose,
        frame_bytes: usize,
    }
    // SAFETY: el pcm se usa solo desde el hilo alimentador.
    unsafe impl Send for AlsaSink {}

    fn sym(lib: *mut std::ffi::c_void, name: &[u8]) -> Result<*mut std::ffi::c_void, String> {
        // SAFETY: name termina en NUL (los llamadores usan literales b"...\0").
        let p = unsafe { dlsym(lib, name.as_ptr()) };
        if p.is_null() {
            Err(format!("audio: libasound without {}", String::from_utf8_lossy(&name[..name.len() - 1])))
        } else {
            Ok(p)
        }
    }

    pub fn open(rate: i64, channels: i64) -> Result<Box<dyn Sink>, String> {
        // SAFETY: literal NUL-terminado; dlopen es seguro de llamar.
        let lib = unsafe { dlopen(b"libasound.so.2\0".as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            return Err("audio: libasound.so.2 not found (install ALSA, e.g. libasound2)".to_string());
        }
        // SAFETY de los transmutes: las firmas replican el header de ALSA (API estable).
        unsafe {
            let f_open: FnOpen = std::mem::transmute(sym(lib, b"snd_pcm_open\0")?);
            let f_params: FnSetParams = std::mem::transmute(sym(lib, b"snd_pcm_set_params\0")?);
            let writei: FnWritei = std::mem::transmute(sym(lib, b"snd_pcm_writei\0")?);
            let recover: FnRecover = std::mem::transmute(sym(lib, b"snd_pcm_recover\0")?);
            let drain: FnDrain = std::mem::transmute(sym(lib, b"snd_pcm_drain\0")?);
            let close: FnClose = std::mem::transmute(sym(lib, b"snd_pcm_close\0")?);
            let mut pcm: Pcm = std::ptr::null_mut();
            let st = f_open(&mut pcm, b"default\0".as_ptr(), SND_PCM_STREAM_PLAYBACK, 0);
            if st != 0 {
                return Err(format!("audio: snd_pcm_open failed ({st})"));
            }
            // 100 ms de latencia del dispositivo: reactivo sin ser frágil (500 ms, lo primero
            // que se probó, insertaba medio segundo entre write y altavoz — hallazgo de rallyx;
            // los underruns los cubre recover).
            let st = f_params(
                pcm,
                SND_PCM_FORMAT_S16_LE,
                SND_PCM_ACCESS_RW_INTERLEAVED,
                channels as u32,
                rate as u32,
                1,
                100000,
            );
            if st != 0 {
                close(pcm);
                return Err(format!("audio: snd_pcm_set_params failed ({st})"));
            }
            Ok(Box::new(AlsaSink { pcm, writei, recover, drain, close, frame_bytes: channels as usize * 2 }))
        }
    }

    impl Sink for AlsaSink {
        fn play(&mut self, data: &[u8]) {
            let mut off = 0;
            while off + self.frame_bytes <= data.len() {
                let frames = ((data.len() - off) / self.frame_bytes) as u64;
                // SAFETY: buffer válido; writei BLOQUEA hasta aceptar (la contrapresión).
                let n = unsafe { (self.writei)(self.pcm, data[off..].as_ptr(), frames) };
                if n < 0 {
                    // XRUN u otro tropiezo: recover silencioso y se sigue (audio de dev, no HA).
                    // SAFETY: pcm válido.
                    if unsafe { (self.recover)(self.pcm, n as i32, 1) } < 0 {
                        return;
                    }
                } else {
                    off += n as usize * self.frame_bytes;
                }
            }
        }

        fn finish(&mut self) {
            // SAFETY: pcm válido; drain bloquea hasta que suene lo entregado.
            unsafe {
                (self.drain)(self.pcm);
                (self.close)(self.pcm);
            }
        }
    }
}
