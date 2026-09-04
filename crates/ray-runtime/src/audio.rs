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

//!   - Windows (M178, docs/windows.md W7): WASAPI en modo compartido, COM A MANO (vtables
//!     transcritas de mmdeviceapi.h/audioclient.h; ole32 y el resto siempre presentes). El pipe
//!     es el anónimo de Windows (`std::io::pipe`) y su extremo de escritura es BLOQUEANTE: el
//!     `audio.write` bloquea el hilo hasta que el alimentador consume (la contrapresión sin
//!     aparcar la fibra, como el stdin de un hijo en M175).
#![cfg(all(feature = "audio", any(unix, windows)))]

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(unix)]
unsafe extern "C" {
    fn pipe(fds: *mut i32) -> i32;
    fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
    fn close(fd: i32) -> i32;
    // Variádica a propósito (lección de arm64, como en watch.rs/term).
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

/// El control de una salida viva, para `drain`: cuántos octetos ha aceptado el alimentador que
/// aún no ha entregado al backend, y los parámetros para estimar la latencia del dispositivo.
pub struct Ctl {
    /// Octetos leídos del pipe y aún no entregados al backend (el "en vuelo" del alimentador).
    in_flight: AtomicI64,
    /// El extremo de LECTURA del pipe (para que `drain` consulte lo encolado: FIONREAD en unix,
    /// `PeekNamedPipe` en Windows): el fd, o el handle de Windows como entero.
    pipe_r: i64,
    /// M158 (§79b): frames REALMENTE reproducidos según el backend (-1 = aún sin dato). Lo
    /// refresca el hilo alimentador tras cada `play` con la API del backend (GetCurrentTime /
    /// snd_pcm_delay / getFramesRead) — mismo hilo que posee el dispositivo: sin carreras.
    played_frames: Arc<AtomicI64>,
    /// Frames por segundo (el sample rate), para convertir a ms en `played_ms`.
    sample_rate: i64,
}

/// El mapa extremo-de-escritura → control, para que `drain(key)` encuentre su salida. La clave
/// es el fd (unix) o el handle de Windows, como entero (`AsRawFd`/`AsRawHandle` del `File`).
fn ctls() -> &'static Mutex<HashMap<i64, Arc<Ctl>>> {
    static CTLS: OnceLock<Mutex<HashMap<i64, Arc<Ctl>>>> = OnceLock::new();
    CTLS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Abre una salida PCM s16le (`sample_rate` Hz, `channels` canales) y devuelve el extremo de
/// escritura del pipe (no-bloqueante, listo para el registro de handles) — el llamador escribe
/// samples ahí y el hilo alimentador los toca. `RAY_AUDIO_SINK=null` → sumidero de tiempo real.
pub fn open(sample_rate: i64, channels: i64, latency_ms: i64) -> Result<std::fs::File, String> {
    if !(8000..=192000).contains(&sample_rate) {
        return Err(format!("audio: unsupported sample rate {sample_rate} (8000–192000)"));
    }
    if !(1..=8).contains(&channels) {
        return Err(format!("audio: unsupported channel count {channels} (1–8)"));
    }
    // M158 (§79b): el hint de latencia dimensiona anillo/buffers/chunk. 0 = default (200 ms,
    // el comportamiento de M145); explícito se acota a [20, 1000] — por debajo de 20 ms el
    // keepalive comería el margen, por encima de 1 s es un búfer, no una latencia.
    if latency_ms != 0 && !(20..=1000).contains(&latency_ms) {
        return Err(format!("audio: unsupported latency {latency_ms} ms (20–1000, or 0 = default)"));
    }
    let latency_ms = if latency_ms == 0 { 200 } else { latency_ms };
    let played_frames = Arc::new(AtomicI64::new(-1));
    // El backend se abre ANTES del pipe: un dispositivo ausente falla en `open`, no a mitad.
    let sink = make_sink(sample_rate, channels, latency_ms, played_frames.clone())?;

    // El chunk del alimentador es ~latencia/4: reactivo sin syscalls de más.
    let bytes_per_sec = sample_rate * channels * 2;
    let chunk = ((bytes_per_sec * latency_ms / 4000).max(256) as usize) & !1;
    #[cfg(unix)]
    {
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
        let key = fd_w as i64;
        let ctl = Arc::new(Ctl {
            in_flight: AtomicI64::new(0),
            pipe_r: fd_r as i64,
            played_frames,
            sample_rate,
        });
        ctls().lock().unwrap().insert(key, ctl.clone());
        // El alimentador: lee el pipe (bloqueante) y empuja al backend; EOF (close del handle) →
        // drena el backend y termina.
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
            ctls().lock().unwrap().remove(&key);
        });
        // SAFETY: fd_w es nuestro; File toma la propiedad (su Drop = close = EOF del alimentador).
        Ok(unsafe { std::os::unix::io::FromRawFd::from_raw_fd(fd_w) })
    }
    #[cfg(windows)]
    {
        // M178: el pipe anónimo de Windows. El extremo de escritura queda BLOQUEANTE (no hay
        // modo no bloqueante para pipes anónimos): `audio.write` bloquea el hilo mientras el
        // alimentador consume — la contrapresión, sin aparcar la fibra.
        use std::io::Read;
        use std::os::windows::io::{AsRawHandle, OwnedHandle};
        let (r, w) = std::io::pipe().map_err(|e| format!("audio: could not create the pipe: {e}"))?;
        let mut reader = std::fs::File::from(OwnedHandle::from(r));
        let writer = std::fs::File::from(OwnedHandle::from(w));
        let key = writer.as_raw_handle() as i64;
        let ctl = Arc::new(Ctl {
            in_flight: AtomicI64::new(0),
            pipe_r: reader.as_raw_handle() as i64,
            played_frames,
            sample_rate,
        });
        ctls().lock().unwrap().insert(key, ctl.clone());
        let ctl_thread = ctl.clone();
        let reader_handle = reader.as_raw_handle() as i64;
        std::thread::spawn(move || {
            let mut sink = sink;
            let mut buf = vec![0u8; chunk];
            loop {
                // Los pipes anónimos son SÍNCRONOS: un `ReadFile` bloqueado serializa detrás de él
                // cualquier `PeekNamedPipe` de otro hilo (el de `drain`) → interbloqueo. Por eso el
                // alimentador nunca bloquea en la lectura: sondea lo disponible y lee solo eso.
                let avail = match peek_pipe(reader_handle) {
                    Err(()) => break, // el escritor cerró (ERROR_BROKEN_PIPE): fin de la sesión
                    Ok(0) => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    }
                    Ok(n) => n,
                };
                let want = avail.min(buf.len());
                let n = match reader.read(&mut buf[..want]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                ctl_thread.in_flight.fetch_add(n as i64, Ordering::SeqCst);
                sink.play(&buf[..n]);
                ctl_thread.in_flight.fetch_sub(n as i64, Ordering::SeqCst);
            }
            sink.finish();
            drop(reader);
            ctls().lock().unwrap().remove(&key);
        });
        Ok(writer)
    }
}

/// Espera a que TODO lo escrito suene: pipe vacío + alimentador sin nada en vuelo + un margen de
/// la latencia del dispositivo. Bloquea el hilo (uso raro, al final de una sesión) — el margen
/// es aproximado por diseño: el "de verdad sonó" exacto es del backend y v1 no lo persigue.
pub fn drain(key: i64) -> Result<(), String> {
    let ctl = match ctls().lock().unwrap().get(&key) {
        Some(c) => c.clone(),
        None => return Err("audio: not an open audio output".to_string()),
    };
    loop {
        let queued = pipe_pending(ctl.pipe_r);
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
/// M158 (§79b): la posición REAL de reproducción en ms — lo que el backend confirma sonado
/// (AudioQueueGetCurrentTime / snd_pcm_delay / AAudioStream_getFramesRead), refrescado por el
/// alimentador tras cada chunk (~latencia/4 de granularidad). 0 = aún no sonó nada.
pub fn played_ms(key: i64) -> Result<i64, String> {
    let ctl = ctls()
        .lock()
        .unwrap()
        .get(&key)
        .cloned()
        .ok_or_else(|| "audio: not an open audio output".to_string())?;
    let frames = ctl.played_frames.load(Ordering::SeqCst);
    if frames <= 0 {
        return Ok(0);
    }
    Ok(frames * 1000 / ctl.sample_rate)
}

#[cfg(unix)]
fn pipe_pending(fd: i64) -> i64 {
    let fd = fd as i32;
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
// M178 (Windows): `PeekNamedPipe` sobre el handle de lectura (el mismo que usa `std/io` en M173).
#[cfg(windows)]
fn pipe_pending(handle: i64) -> i64 {
    peek_pipe(handle).map_or(0, |n| n as i64)
}
/// Octetos disponibles en el pipe sin bloquear; `Err(())` si el escritor cerró (EOF) o el handle
/// no sirve. Lo usan `drain` y el alimentador (que por esto nunca bloquea en `ReadFile`).
#[cfg(windows)]
fn peek_pipe(handle: i64) -> Result<usize, ()> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn PeekNamedPipe(handle: usize, buf: *mut core::ffi::c_void, len: u32, read: *mut u32, avail: *mut u32, left: *mut u32) -> i32;
    }
    let mut avail = 0u32;
    // SAFETY: sin buffer; solo se pide `avail`, un u32 propio; el handle es nuestro.
    let ok = unsafe { PeekNamedPipe(handle as usize, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut()) };
    if ok != 0 { Ok(avail as usize) } else { Err(()) }
}

// ── Los backends ─────────────────────────────────────────────────────────────

/// Un sumidero PCM: recibe s16le entrelazado y lo hace sonar (o lo consume). `play` puede
/// BLOQUEAR (es la contrapresión: el pipe se llena aguas arriba y la fibra aparca).
trait Sink: Send {
    fn play(&mut self, data: &[u8]);
    fn finish(&mut self);
}

fn make_sink(
    rate: i64,
    channels: i64,
    latency_ms: i64,
    played: Arc<AtomicI64>,
) -> Result<Box<dyn Sink>, String> {
    if std::env::var("RAY_AUDIO_SINK").as_deref() == Ok("null") {
        return Ok(Box::new(NullSink {
            bytes_per_sec: rate * channels * 2,
            frame_bytes: (channels * 2) as usize,
            consumed_frames: 0,
            played,
        }));
    }
    #[cfg(target_os = "macos")]
    {
        coreaudio::open(rate, channels, latency_ms, played)
    }
    #[cfg(target_os = "linux")]
    {
        alsa::open(rate, channels, latency_ms, played)
    }
    // M158: Android — AAudio por dlopen (API 26+; el patrón ALSA).
    #[cfg(target_os = "android")]
    {
        aaudio::open(rate, channels, latency_ms, played)
    }
    // M178: Windows — WASAPI por COM a mano.
    #[cfg(windows)]
    {
        wasapi::open(rate, channels, latency_ms, played)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android", windows)))]
    {
        let _ = (rate, channels, latency_ms, played);
        Err("audio: no backend for this platform (macOS/Linux/Android/Windows; RAY_AUDIO_SINK=null works anywhere)".to_string())
    }
}

/// El sumidero nulo: consume a ritmo de TIEMPO REAL (duerme lo que duraría el audio). Da la
/// misma contrapresión que un dispositivo — la vía de los tests en CI sin tarjeta de sonido.
struct NullSink {
    bytes_per_sec: i64,
    frame_bytes: usize,
    consumed_frames: i64,
    played: Arc<AtomicI64>,
}

impl Sink for NullSink {
    fn play(&mut self, data: &[u8]) {
        let ms = data.len() as i64 * 1000 / self.bytes_per_sec;
        std::thread::sleep(std::time::Duration::from_millis(ms.max(1) as u64));
        // El sumidero de tiempo real ES el dispositivo: lo consumido ya "sonó" (exacto).
        self.consumed_frames += (data.len() / self.frame_bytes) as i64;
        self.played.store(self.consumed_frames, Ordering::SeqCst);
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
        // M158: la posición real de reproducción (sample time de la cola).
        fn AudioQueueGetCurrentTime(
            q: AudioQueueRef,
            timeline: *const std::ffi::c_void,
            out_time: *mut AudioTimeStamp,
            discontinuity: *mut u8,
        ) -> i32;
        fn AudioQueueStop(q: AudioQueueRef, immediate: u8) -> i32;
        fn AudioQueueDispose(q: AudioQueueRef, immediate: u8) -> i32;
    }

    /// El AudioTimeStamp de CoreAudio (solo se lee mSampleTime y mFlags).
    #[repr(C)]
    struct AudioTimeStamp {
        sample_time: f64,
        host_time: u64,
        rate_scalar: f64,
        word_clock_time: u64,
        smpte_time: [u8; 24], // SMPTETime: 4×i16 + 3×u32 + 4×i16 = 24 octetos
        flags: u32,
        reserved: u32,
    }
    const TS_SAMPLE_TIME_VALID: u32 = 1;

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
        played: Arc<super::AtomicI64>,
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

    pub fn open(
        rate: i64,
        channels: i64,
        latency_ms: i64,
        played: Arc<super::AtomicI64>,
    ) -> Result<Box<dyn Sink>, String> {
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
        // M158: el anillo guarda ~la latencia pedida (default 200 ms); cada buffer, ~1/4.
        let frame_bytes = bytes_per_frame as usize;
        let bytes_per_sec = (rate * channels * 2) as usize;
        let cap =
            (bytes_per_sec * latency_ms as usize / 1000).max(4096) / frame_bytes * frame_bytes;
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
        Ok(Box::new(CoreAudioSink { q, shared, played }))
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
            drop(ring);
            // M158: refrescar la posición real (mismo hilo que posee la cola — sin carreras).
            let mut ts = AudioTimeStamp {
                sample_time: 0.0,
                host_time: 0,
                rate_scalar: 0.0,
                word_clock_time: 0,
                smpte_time: [0; 24],
                flags: 0,
                reserved: 0,
            };
            // SAFETY: q válido hasta Dispose; ts es nuestro buffer.
            let st = unsafe {
                AudioQueueGetCurrentTime(self.q, std::ptr::null(), &mut ts, std::ptr::null_mut())
            };
            if st == 0 && ts.flags & TS_SAMPLE_TIME_VALID != 0 && ts.sample_time >= 0.0 {
                self.played.store(ts.sample_time as i64, super::Ordering::SeqCst);
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
    use std::sync::Arc;

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
    type FnDelay = unsafe extern "C" fn(Pcm, *mut i64) -> i32;

    const SND_PCM_STREAM_PLAYBACK: i32 = 0;
    const SND_PCM_FORMAT_S16_LE: i32 = 2;
    const SND_PCM_ACCESS_RW_INTERLEAVED: i32 = 3;

    pub struct AlsaSink {
        pcm: Pcm,
        writei: FnWritei,
        recover: FnRecover,
        drain: FnDrain,
        close: FnClose,
        delay: FnDelay,
        frame_bytes: usize,
        written_frames: i64,
        played: Arc<super::AtomicI64>,
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

    pub fn open(
        rate: i64,
        channels: i64,
        latency_ms: i64,
        played: Arc<super::AtomicI64>,
    ) -> Result<Box<dyn Sink>, String> {
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
            // M158: la posición real = escritos − delay (frames aún en el búfer del dispositivo).
            let delay: FnDelay = std::mem::transmute(sym(lib, b"snd_pcm_delay\0")?);
            let close: FnClose = std::mem::transmute(sym(lib, b"snd_pcm_close\0")?);
            let mut pcm: Pcm = std::ptr::null_mut();
            let st = f_open(&mut pcm, b"default\0".as_ptr(), SND_PCM_STREAM_PLAYBACK, 0);
            if st != 0 {
                return Err(format!("audio: snd_pcm_open failed ({st})"));
            }
            // M158: la latencia del dispositivo sigue el hint (default 200 ms → 100 ms aquí,
            // el valor validado de rallyx; con hint explícito, la mitad del hint acotada).
            let dev_latency_us = if latency_ms == 200 {
                100_000
            } else {
                ((latency_ms * 1000 / 2).clamp(10_000, 500_000)) as u32
            };
            let st = f_params(
                pcm,
                SND_PCM_FORMAT_S16_LE,
                SND_PCM_ACCESS_RW_INTERLEAVED,
                channels as u32,
                rate as u32,
                1,
                dev_latency_us,
            );
            if st != 0 {
                close(pcm);
                return Err(format!("audio: snd_pcm_set_params failed ({st})"));
            }
            Ok(Box::new(AlsaSink {
                pcm,
                writei,
                recover,
                drain,
                close,
                delay,
                frame_bytes: channels as usize * 2,
                written_frames: 0,
                played,
            }))
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
                    self.written_frames += n;
                }
            }
            // M158: posición real = escritos − delay, desde el MISMO hilo que posee el pcm.
            let mut in_device: i64 = 0;
            // SAFETY: pcm válido; delay escribe un i64 (snd_pcm_sframes_t).
            if unsafe { (self.delay)(self.pcm, &mut in_device) } == 0 {
                let played_now = (self.written_frames - in_device).max(0);
                self.played.store(played_now, super::Ordering::SeqCst);
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

// ── Android: AAudio por dlopen (API 26+; el patrón ALSA — sin headers de build) ─────────────
#[cfg(target_os = "android")]
mod aaudio {
    use super::Sink;
    use std::sync::Arc;

    unsafe extern "C" {
        fn dlopen(path: *const u8, flags: i32) -> *mut std::ffi::c_void;
        fn dlsym(lib: *mut std::ffi::c_void, name: *const u8) -> *mut std::ffi::c_void;
    }
    const RTLD_NOW: i32 = 2;

    type Builder = *mut std::ffi::c_void;
    type Stream = *mut std::ffi::c_void;
    type FnCreateBuilder = unsafe extern "C" fn(*mut Builder) -> i32;
    type FnSetI32 = unsafe extern "C" fn(Builder, i32);
    type FnOpenStream = unsafe extern "C" fn(Builder, *mut Stream) -> i32;
    type FnBuilderDelete = unsafe extern "C" fn(Builder) -> i32;
    type FnRequest = unsafe extern "C" fn(Stream) -> i32;
    type FnWrite = unsafe extern "C" fn(Stream, *const std::ffi::c_void, i32, i64) -> i32;
    type FnFramesRead = unsafe extern "C" fn(Stream) -> i64;
    type FnClose = unsafe extern "C" fn(Stream) -> i32;

    const AAUDIO_FORMAT_PCM_I16: i32 = 1;
    const AAUDIO_PERFORMANCE_MODE_NONE: i32 = 10;
    const AAUDIO_PERFORMANCE_MODE_LOW_LATENCY: i32 = 12;

    pub struct AAudioSink {
        stream: Stream,
        write: FnWrite,
        frames_read: FnFramesRead,
        request_stop: FnRequest,
        close: FnClose,
        frame_bytes: usize,
        played: Arc<super::AtomicI64>,
    }
    // SAFETY: el stream se usa solo desde el hilo alimentador (write bloqueante).
    unsafe impl Send for AAudioSink {}

    fn sym(lib: *mut std::ffi::c_void, name: &[u8]) -> Result<*mut std::ffi::c_void, String> {
        // SAFETY: name termina en NUL (literales b"...\0").
        let p = unsafe { dlsym(lib, name.as_ptr()) };
        if p.is_null() {
            Err(format!("audio: libaaudio without {}", String::from_utf8_lossy(&name[..name.len() - 1])))
        } else {
            Ok(p)
        }
    }

    pub fn open(
        rate: i64,
        channels: i64,
        latency_ms: i64,
        played: Arc<super::AtomicI64>,
    ) -> Result<Box<dyn Sink>, String> {
        // SAFETY: dlopen/dlsym con literales; las firmas replican aaudio/AAudio.h (NDK).
        unsafe {
            let lib = dlopen(b"libaaudio.so\0".as_ptr(), RTLD_NOW);
            if lib.is_null() {
                return Err("audio: libaaudio.so not found (AAudio needs Android 8.0+)".to_string());
            }
            let create: FnCreateBuilder = std::mem::transmute(sym(lib, b"AAudio_createStreamBuilder\0")?);
            let set_rate: FnSetI32 = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_setSampleRate\0")?);
            let set_channels: FnSetI32 = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_setChannelCount\0")?);
            let set_format: FnSetI32 = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_setFormat\0")?);
            let set_perf: FnSetI32 = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_setPerformanceMode\0")?);
            let open_stream: FnOpenStream = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_openStream\0")?);
            let builder_delete: FnBuilderDelete = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_delete\0")?);
            let request_start: FnRequest = std::mem::transmute(sym(lib, b"AAudioStream_requestStart\0")?);
            let request_stop: FnRequest = std::mem::transmute(sym(lib, b"AAudioStream_requestStop\0")?);
            let write: FnWrite = std::mem::transmute(sym(lib, b"AAudioStream_write\0")?);
            let frames_read: FnFramesRead = std::mem::transmute(sym(lib, b"AAudioStream_getFramesRead\0")?);
            let close: FnClose = std::mem::transmute(sym(lib, b"AAudioStream_close\0")?);

            let mut builder: Builder = std::ptr::null_mut();
            let st = create(&mut builder);
            if st != 0 {
                return Err(format!("audio: AAudio_createStreamBuilder failed ({st})"));
            }
            set_rate(builder, rate as i32);
            set_channels(builder, channels as i32);
            set_format(builder, AAUDIO_FORMAT_PCM_I16);
            // M158: el hint decide el modo — por debajo de 50 ms se pide LOW_LATENCY.
            set_perf(
                builder,
                if latency_ms <= 50 { AAUDIO_PERFORMANCE_MODE_LOW_LATENCY } else { AAUDIO_PERFORMANCE_MODE_NONE },
            );
            let mut stream: Stream = std::ptr::null_mut();
            let st = open_stream(builder, &mut stream);
            builder_delete(builder);
            if st != 0 {
                return Err(format!("audio: AAudioStreamBuilder_openStream failed ({st})"));
            }
            let st = request_start(stream);
            if st != 0 {
                close(stream);
                return Err(format!("audio: AAudioStream_requestStart failed ({st})"));
            }
            Ok(Box::new(AAudioSink {
                stream,
                write,
                frames_read,
                request_stop,
                close,
                frame_bytes: (channels * 2) as usize,
                played,
            }))
        }
    }

    impl Sink for AAudioSink {
        fn play(&mut self, data: &[u8]) {
            let mut off = 0;
            while off + self.frame_bytes <= data.len() {
                let frames = ((data.len() - off) / self.frame_bytes) as i32;
                // SAFETY: buffer válido; write con timeout "infinito" práctico (10 s) BLOQUEA
                // hasta aceptar — la contrapresión, como writei de ALSA.
                let n = unsafe {
                    (self.write)(
                        self.stream,
                        data[off..].as_ptr() as *const std::ffi::c_void,
                        frames,
                        10_000_000_000,
                    )
                };
                if n <= 0 {
                    return; // error del stream: la sesión sigue sin audio (audio de app, no HA)
                }
                off += n as usize * self.frame_bytes;
            }
            // M158: la posición real — frames que el DISPOSITIVO ya consumió.
            // SAFETY: stream válido hasta close.
            let played_now = unsafe { (self.frames_read)(self.stream) };
            if played_now >= 0 {
                self.played.store(played_now, super::Ordering::SeqCst);
            }
        }

        fn finish(&mut self) {
            // SAFETY: stream válido; stop drena lo pendiente y close libera.
            unsafe {
                (self.request_stop)(self.stream);
                (self.close)(self.stream);
            }
        }
    }
}

// ── Windows: WASAPI en modo compartido, COM a mano (M178) ──────────────────────
//
// Sin crates: las vtables de `IMMDeviceEnumerator`/`IMMDevice`/`IAudioClient`/`IAudioRenderClient`
// se transcriben de mmdeviceapi.h/audioclient.h (ABI COM: puntero al vtable como primer campo,
// métodos `extern "system"` en orden, `IUnknown` delante). Formato: s16le entrelazado con
// `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM` (Windows 10+: el motor remuestrea/convierte al mix del
// dispositivo, así el contrato del módulo se conserva sin negociar formatos). Modo compartido
// por eventos NO: el alimentador ya marca el ritmo — se sondea `GetCurrentPadding` y se duerme
// ~latencia/8 cuando el búfer está lleno (la contrapresión). La posición real = escritos −
// padding (frames aún en el búfer del motor), como `snd_pcm_delay` en ALSA. Los objetos COM
// son *free-threaded* (WASAPI es MTA-agile): se crean en el hilo que llama a `open` y se usan
// desde el alimentador; cada hilo hace su `CoInitializeEx(MULTITHREADED)`.
#[cfg(windows)]
mod wasapi {
    use super::Sink;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

    type Ptr = *mut core::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        d1: u32,
        d2: u16,
        d3: u16,
        d4: [u8; 8],
    }
    const CLSID_MM_DEVICE_ENUMERATOR: Guid =
        Guid { d1: 0xBCDE_0395, d2: 0xE52F, d3: 0x467C, d4: [0x8E, 0x3D, 0xC4, 0x57, 0x92, 0x91, 0x69, 0x2E] };
    const IID_IMM_DEVICE_ENUMERATOR: Guid =
        Guid { d1: 0xA956_64D2, d2: 0x9614, d3: 0x4F35, d4: [0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6] };
    const IID_IAUDIO_CLIENT: Guid =
        Guid { d1: 0x1CB9_AD4C, d2: 0xDBFA, d3: 0x4C32, d4: [0xB1, 0x78, 0xC2, 0xF5, 0x68, 0xA7, 0x03, 0xB2] };
    const IID_IAUDIO_RENDER_CLIENT: Guid =
        Guid { d1: 0xF294_ACFC, d2: 0x3146, d3: 0x4483, d4: [0xA7, 0xBF, 0xAD, 0xDC, 0xA7, 0xC2, 0x60, 0xE2] };

    const COINIT_MULTITHREADED: u32 = 0x0;
    const CLSCTX_ALL: u32 = 0x17;
    const E_RENDER: u32 = 0; // EDataFlow::eRender
    const E_CONSOLE: u32 = 0; // ERole::eConsole
    const AUDCLNT_SHAREMODE_SHARED: u32 = 0;
    const AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM: u32 = 0x8000_0000;
    const AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
    const WAVE_FORMAT_PCM: u16 = 1;

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: Ptr, coinit: u32) -> i32;
        fn CoCreateInstance(clsid: *const Guid, outer: Ptr, ctx: u32, iid: *const Guid, out: *mut Ptr) -> i32;
    }

    /// `WAVEFORMATEX` (mmreg.h, empaquetado a 1 byte: 18 bytes).
    #[repr(C, packed)]
    struct WaveFormatEx {
        format_tag: u16,
        channels: u16,
        samples_per_sec: u32,
        avg_bytes_per_sec: u32,
        block_align: u16,
        bits_per_sample: u16,
        cb_size: u16,
    }

    type Hr = i32;
    #[repr(C)]
    struct IUnknownVtbl {
        query_interface: unsafe extern "system" fn(Ptr, *const Guid, *mut Ptr) -> Hr,
        add_ref: unsafe extern "system" fn(Ptr) -> u32,
        release: unsafe extern "system" fn(Ptr) -> u32,
    }
    #[repr(C)]
    struct IMMDeviceEnumeratorVtbl {
        base: IUnknownVtbl,
        enum_audio_endpoints: usize,
        get_default_audio_endpoint: unsafe extern "system" fn(Ptr, u32, u32, *mut Ptr) -> Hr,
        get_device: usize,
        register_endpoint_notification_callback: usize,
        unregister_endpoint_notification_callback: usize,
    }
    #[repr(C)]
    struct IMMDeviceVtbl {
        base: IUnknownVtbl,
        activate: unsafe extern "system" fn(Ptr, *const Guid, u32, Ptr, *mut Ptr) -> Hr,
        open_property_store: usize,
        get_id: usize,
        get_state: usize,
    }
    #[repr(C)]
    struct IAudioClientVtbl {
        base: IUnknownVtbl,
        initialize: unsafe extern "system" fn(Ptr, u32, u32, i64, i64, *const WaveFormatEx, *const Guid) -> Hr,
        get_buffer_size: unsafe extern "system" fn(Ptr, *mut u32) -> Hr,
        get_stream_latency: usize,
        get_current_padding: unsafe extern "system" fn(Ptr, *mut u32) -> Hr,
        is_format_supported: usize,
        get_mix_format: usize,
        get_device_period: usize,
        start: unsafe extern "system" fn(Ptr) -> Hr,
        stop: unsafe extern "system" fn(Ptr) -> Hr,
        reset: usize,
        set_event_handle: usize,
        get_service: unsafe extern "system" fn(Ptr, *const Guid, *mut Ptr) -> Hr,
    }
    #[repr(C)]
    struct IAudioRenderClientVtbl {
        base: IUnknownVtbl,
        get_buffer: unsafe extern "system" fn(Ptr, u32, *mut *mut u8) -> Hr,
        release_buffer: unsafe extern "system" fn(Ptr, u32, u32) -> Hr,
    }

    /// El vtable de un objeto COM (su primer campo es el puntero al vtable).
    unsafe fn vtbl<T>(obj: Ptr) -> &'static T {
        // SAFETY: todo objeto COM empieza por el puntero a su vtable; `T` es el layout transcrito.
        unsafe { &**(obj as *const *const T) }
    }
    fn release(obj: Ptr) {
        if !obj.is_null() {
            // SAFETY: `IUnknown::Release` sobre un puntero COM vivo que poseemos.
            unsafe { (vtbl::<IUnknownVtbl>(obj).release)(obj) };
        }
    }

    pub struct WasapiSink {
        enumerator: Ptr,
        device: Ptr,
        client: Ptr,
        render: Ptr,
        buffer_frames: u32,
        frame_bytes: usize,
        written_frames: i64,
        rate: i64,
        latency_ms: i64,
        played: Arc<AtomicI64>,
    }
    // SAFETY: los objetos de WASAPI son free-threaded; se usan solo desde el hilo alimentador
    // tras la creación (cada hilo inicializa COM en MTA).
    unsafe impl Send for WasapiSink {}

    fn com_init() {
        // SAFETY: llamada sin punteros. S_FALSE (ya inicializado) y RPC_E_CHANGED_MODE (el hilo
        // ya era STA) no impiden usar objetos free-threaded.
        unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_MULTITHREADED) };
    }

    pub fn open(rate: i64, channels: i64, latency_ms: i64, played: Arc<AtomicI64>) -> Result<Box<dyn Sink>, String> {
        com_init();
        let mut enumerator: Ptr = std::ptr::null_mut();
        // SAFETY: CoCreateInstance escribe un puntero COM en `enumerator` si devuelve S_OK.
        let hr = unsafe {
            CoCreateInstance(&CLSID_MM_DEVICE_ENUMERATOR, std::ptr::null_mut(), CLSCTX_ALL, &IID_IMM_DEVICE_ENUMERATOR, &mut enumerator)
        };
        if hr < 0 || enumerator.is_null() {
            return Err(format!("audio: could not create the WASAPI device enumerator (0x{:08X})", hr as u32));
        }
        let mut device: Ptr = std::ptr::null_mut();
        // SAFETY: método del vtable transcrito sobre un objeto vivo; escribe `device`.
        let hr = unsafe { (vtbl::<IMMDeviceEnumeratorVtbl>(enumerator).get_default_audio_endpoint)(enumerator, E_RENDER, E_CONSOLE, &mut device) };
        if hr < 0 || device.is_null() {
            release(enumerator);
            return Err("audio: no output device (WASAPI: no default render endpoint)".to_string());
        }
        let mut client: Ptr = std::ptr::null_mut();
        // SAFETY: ídem; `Activate` escribe la interfaz pedida.
        let hr = unsafe { (vtbl::<IMMDeviceVtbl>(device).activate)(device, &IID_IAUDIO_CLIENT, CLSCTX_ALL, std::ptr::null_mut(), &mut client) };
        if hr < 0 || client.is_null() {
            release(device);
            release(enumerator);
            return Err(format!("audio: could not activate the audio client (0x{:08X})", hr as u32));
        }
        let block_align = (channels * 2) as u16;
        let fmt = WaveFormatEx {
            format_tag: WAVE_FORMAT_PCM,
            channels: channels as u16,
            samples_per_sec: rate as u32,
            avg_bytes_per_sec: (rate * channels * 2) as u32,
            block_align,
            bits_per_sample: 16,
            cb_size: 0,
        };
        // La latencia pedida al motor sigue el hint (200 ms default → 100 ms, como ALSA), en
        // unidades de 100 ns. El motor puede dar más: `GetBufferSize` dice cuánto.
        let hns = (latency_ms.max(20) / 2).clamp(10, 500) * 10_000;
        let flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        // SAFETY: `fmt` vive durante la llamada; sin GUID de sesión (nulo).
        let hr = unsafe { (vtbl::<IAudioClientVtbl>(client).initialize)(client, AUDCLNT_SHAREMODE_SHARED, flags, hns, 0, &fmt, std::ptr::null()) };
        if hr < 0 {
            release(client);
            release(device);
            release(enumerator);
            return Err(format!("audio: could not initialize the WASAPI stream (0x{:08X})", hr as u32));
        }
        let mut buffer_frames = 0u32;
        // SAFETY: escribe un u32 propio.
        unsafe { (vtbl::<IAudioClientVtbl>(client).get_buffer_size)(client, &mut buffer_frames) };
        let mut render: Ptr = std::ptr::null_mut();
        // SAFETY: `GetService` escribe la interfaz pedida.
        let hr = unsafe { (vtbl::<IAudioClientVtbl>(client).get_service)(client, &IID_IAUDIO_RENDER_CLIENT, &mut render) };
        if hr < 0 || render.is_null() || buffer_frames == 0 {
            release(client);
            release(device);
            release(enumerator);
            return Err(format!("audio: could not get the render client (0x{:08X})", hr as u32));
        }
        // SAFETY: arranca el flujo; el búfer vacío suena como silencio hasta el primer `play`.
        let hr = unsafe { (vtbl::<IAudioClientVtbl>(client).start)(client) };
        if hr < 0 {
            release(render);
            release(client);
            release(device);
            release(enumerator);
            return Err(format!("audio: could not start the WASAPI stream (0x{:08X})", hr as u32));
        }
        Ok(Box::new(WasapiSink {
            enumerator,
            device,
            client,
            render,
            buffer_frames,
            frame_bytes: channels as usize * 2,
            written_frames: 0,
            rate,
            latency_ms,
            played,
        }))
    }

    impl WasapiSink {
        fn padding(&self) -> u32 {
            let mut pad = 0u32;
            // SAFETY: escribe un u32 propio; el cliente está vivo.
            let hr = unsafe { (vtbl::<IAudioClientVtbl>(self.client).get_current_padding)(self.client, &mut pad) };
            if hr < 0 { 0 } else { pad }
        }
        fn nap(&self) {
            let ms = (self.latency_ms / 8).clamp(2, 50) as u64;
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }

    impl Sink for WasapiSink {
        fn play(&mut self, data: &[u8]) {
            com_init();
            let mut off = 0usize;
            while off + self.frame_bytes <= data.len() {
                let free = self.buffer_frames.saturating_sub(self.padding());
                if free == 0 {
                    self.nap(); // búfer lleno: la contrapresión
                    continue;
                }
                let want = ((data.len() - off) / self.frame_bytes) as u32;
                let n = want.min(free);
                let mut dst: *mut u8 = std::ptr::null_mut();
                // SAFETY: `GetBuffer` presta `n` frames escribibles; se copian exactamente y se liberan.
                let hr = unsafe { (vtbl::<IAudioRenderClientVtbl>(self.render).get_buffer)(self.render, n, &mut dst) };
                if hr < 0 || dst.is_null() {
                    self.nap();
                    continue;
                }
                let bytes = n as usize * self.frame_bytes;
                // SAFETY: `dst` tiene sitio para `n` frames; el origen es nuestro slice.
                unsafe {
                    std::ptr::copy_nonoverlapping(data[off..].as_ptr(), dst, bytes);
                    (vtbl::<IAudioRenderClientVtbl>(self.render).release_buffer)(self.render, n, 0);
                }
                off += bytes;
                self.written_frames += n as i64;
            }
            // Posición real = escritos − lo que aún espera en el búfer del motor.
            let played_now = (self.written_frames - self.padding() as i64).max(0);
            self.played.store(played_now, Ordering::SeqCst);
        }
        fn finish(&mut self) {
            // Drena: espera a que el motor consuma lo entregado (acotado a ~2 s por si acaso).
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while self.padding() > 0 && std::time::Instant::now() < deadline {
                self.nap();
            }
            let _ = self.rate;
            // SAFETY: para el flujo y suelta los objetos COM (orden inverso de creación).
            unsafe { (vtbl::<IAudioClientVtbl>(self.client).stop)(self.client) };
            release(self.render);
            release(self.client);
            release(self.device);
            release(self.enumerator);
            self.render = std::ptr::null_mut();
            self.client = std::ptr::null_mut();
            self.device = std::ptr::null_mut();
            self.enumerator = std::ptr::null_mut();
        }
    }
}
