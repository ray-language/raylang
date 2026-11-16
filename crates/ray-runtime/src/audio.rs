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
//!   - macOS e iOS (M324): AudioQueue (AudioToolbox.framework, SIEMPRE presente — se enlaza al
//!     build; en iOS el shell generado por `ray bundle --ios` añade `-framework AudioToolbox`).
//!   - Linux: ALSA por `dlopen("libasound.so.2")` EN RUNTIME (sin headers; sin la lib → `Err`).
//!   - `RAY_AUDIO_SINK=null`: un sumidero que consume a ritmo de tiempo real SIN hardware — la
//!     vía de los tests (CI no tiene tarjeta de sonido) y de los benchmarks.
//!
//! Formato de v1: PCM **s16le entrelazado** (el mínimo común de los backends). M398 (IDEAS §111,
//! rayplay #167): también `s24le`, `s32le` y `f32le` — nativos donde el backend los acepta
//! (CoreAudio todos; ALSA/AAudio según dispositivo) y convertidos a s16le en el alimentador
//! donde no (`Convert`); `flush` (rayplay #166) descarta lo encolado sin cerrar la salida.

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
    fn socketpair(domain: i32, ty: i32, protocol: i32, fds: *mut i32) -> i32;
    fn setsockopt(fd: i32, level: i32, name: i32, value: *const core::ffi::c_void, len: u32) -> i32;
    fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
    /// M463: la fuente de entrada escribe lo capturado en el extremo no bloqueante del pipe.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn send(fd: i32, buf: *const u8, n: usize, flags: i32) -> isize;
    fn close(fd: i32) -> i32;
    /// M463: falla (ENOTCONN) cuando el otro extremo del par de datagramas ya se cerró.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    fn getpeername(fd: i32, addr: *mut u8, len: *mut u32) -> i32;
    /// M398: el alimentador espera con `poll` (20 ms) en vez de un `read` bloqueante, para
    /// atender la bandera de `flush` aunque el programa no escriba nada más.
    fn poll(fds: *mut PollFd, n: u32, timeout_ms: i32) -> i32;
    // Variádica a propósito (lección de arm64, como en watch.rs/term).
    #[link_name = "fcntl"]
    fn fcntl_raw(fd: i32, cmd: i32, ...) -> i32;
}
#[cfg(unix)]
#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}
#[cfg(unix)]
const POLLIN: i16 = 1;

#[cfg(unix)]
const F_GETFL: i32 = 3;
#[cfg(unix)]
const F_SETFL: i32 = 4;
#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NONBLOCK: i32 = 0o4000; // M156: bionic también es 0o4000 (android es unix, no "linux")
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const O_NONBLOCK: i32 = 0x0004;
// M298 (findings 1.27.11 #21): el "pipe" es un `socketpair` AF_UNIX/SOCK_STREAM con los buffers
// acotados a la latencia pedida (SO_SNDBUF/SO_RCVBUF). Un pipe del SO tiene un buffer fijo
// (64 KiB en macOS) que a tasas bajas es un suelo enorme: a 22050 Hz mono, 1,5 s en cola
// hicieran lo que hicieran `open_latency` y el anillo del backend.
#[cfg(unix)]
const AF_UNIX: i32 = 1;
#[cfg(unix)]
const SOCK_STREAM: i32 = 1;
/// M463: la entrada usa DATAGRAMAS — cada lectura del programa es un bloque de frames enteros.
/// M468: en Linux y Android, `SOCK_SEQPACKET`: lo mismo, y además el programa ve el fin de
/// flujo si la fuente muere (un dispositivo desenchufado); con datagramas se quedaba esperando.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const INPUT_SOCKET: i32 = 2; // SOCK_DGRAM
#[cfg(any(target_os = "linux", target_os = "android"))]
const INPUT_SOCKET: i32 = 5; // SOCK_SEQPACKET
#[cfg(any(target_os = "linux", target_os = "android"))]
const SOL_SOCKET: i32 = 1;
#[cfg(any(target_os = "linux", target_os = "android"))]
const SO_SNDBUF: i32 = 7;
#[cfg(any(target_os = "linux", target_os = "android"))]
const SO_RCVBUF: i32 = 8;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const SOL_SOCKET: i32 = 0xffff;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const SO_SNDBUF: i32 = 0x1001;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const SO_RCVBUF: i32 = 0x1002;
#[cfg(any(target_os = "macos", target_os = "ios"))]
const SO_NOSIGPIPE: i32 = 0x1022;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const SO_SNDLOWAT: i32 = 0x1003;
/// M398 (IDEAS §111, rayplay #167): el formato de muestra de una salida. `write` recibe octetos
/// entrelazados en este formato; los backends que no lo aceptan nativo reciben s16le vía `Convert`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    S16,
    S24,
    S32,
    F32,
}

impl Format {
    pub fn parse(name: &str) -> Result<Format, String> {
        match name {
            "s16le" => Ok(Format::S16),
            "s24le" => Ok(Format::S24),
            "s32le" => Ok(Format::S32),
            "f32le" => Ok(Format::F32),
            other => Err(format!("audio: unsupported format '{other}' (s16le, s24le, s32le, f32le)")),
        }
    }
    /// Octetos por muestra (por canal).
    pub fn sample_bytes(self) -> usize {
        match self {
            Format::S16 => 2,
            Format::S24 => 3,
            Format::S32 | Format::F32 => 4,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Format::S16 => "s16le",
            Format::S24 => "s24le",
            Format::S32 => "s32le",
            Format::F32 => "f32le",
        }
    }
}

/// M468 (ray-daw #424): un dispositivo de audio del sistema.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceInfo {
    /// Lo que se pasa para abrirlo: el UID de CoreAudio, el nombre PCM de ALSA, el id del
    /// endpoint de WASAPI o el id numérico de Android. Estable entre reconexiones donde el
    /// sistema lo da así (CoreAudio, WASAPI).
    pub id: String,
    pub name: String,
    /// Canales de entrada (0 = no graba) y de salida (0 = no suena).
    pub input_channels: i64,
    pub output_channels: i64,
    pub default_input: bool,
    pub default_output: bool,
    /// La frecuencia de muestreo actual del dispositivo, o 0 si no se sabe.
    pub sample_rate: i64,
    /// Cómo está conectado: "builtin", "usb", "bluetooth", "hdmi", "network", "virtual",
    /// "aggregate", "wired", o "" si no se sabe.
    pub transport: String,
}

/// Los dispositivos de audio presentes ahora. En iOS la lista sale vacía: el sistema enruta el
/// audio por su cuenta y solo existe «el dispositivo actual».
pub fn devices() -> Result<Vec<DeviceInfo>, String> {
    #[cfg(target_os = "macos")]
    {
        coreaudio::devices()
    }
    #[cfg(target_os = "linux")]
    {
        alsa::devices()
    }
    #[cfg(windows)]
    {
        wasapi::devices()
    }
    #[cfg(target_os = "android")]
    {
        aaudio::devices()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android", windows)))]
    {
        Ok(Vec::new())
    }
}

/// El listado de `devices` como texto, una línea por dispositivo con sus campos separados por
/// tabuladores (id, nombre, canales de entrada, de salida, por defecto de entrada, de salida,
/// frecuencia, transporte): lo que cruza al programa por el primitivo `__audio_open`.
pub fn devices_text() -> Result<String, String> {
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    Ok(devices()?
        .iter()
        .map(|d| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                clean(&d.id),
                clean(&d.name),
                d.input_channels,
                d.output_channels,
                d.default_input as u8,
                d.default_output as u8,
                d.sample_rate,
                clean(&d.transport)
            )
        })
        .collect())
}

/// Parte `formato[@dispositivo]`: el formato de muestra y el id del dispositivo ("" = el
/// dispositivo por defecto). El id va detrás de la PRIMERA arroba y puede contener otras.
fn split_device(format: &str) -> (&str, &str) {
    format.split_once('@').unwrap_or((format, ""))
}

/// Convierte muestras entrelazadas de `from` a s16le (lo que entienden todos los backends). Un
/// resto de octetos que no completa una muestra se descarta. Pura.
pub fn to_s16le(from: Format, data: &[u8], out: &mut Vec<u8>) {
    match from {
        Format::S16 => out.extend_from_slice(&data[..data.len() / 2 * 2]),
        // s24le → los 16 bits altos (octetos 1 y 2 de cada muestra).
        Format::S24 => {
            for s in data.chunks_exact(3) {
                out.extend_from_slice(&[s[1], s[2]]);
            }
        }
        Format::S32 => {
            for s in data.chunks_exact(4) {
                out.extend_from_slice(&[s[2], s[3]]);
            }
        }
        Format::F32 => {
            for s in data.chunks_exact(4) {
                let v = f32::from_le_bytes([s[0], s[1], s[2], s[3]]).clamp(-1.0, 1.0);
                out.extend_from_slice(&((v * 32767.0) as i16).to_le_bytes());
            }
        }
    }
}

/// La inversa de `to_s16le` (M465): lleva muestras s16le entrelazadas al formato `to`, para una
/// ENTRADA cuyo dispositivo solo entrega s16le. Los enteros se desplazan (los bits bajos a cero)
/// y el flotante se escala a [-1, 1). Pura.
#[allow(dead_code)] // CoreAudio captura en los cuatro formatos y no la usa
pub fn from_s16le(to: Format, data: &[u8], out: &mut Vec<u8>) {
    match to {
        Format::S16 => out.extend_from_slice(&data[..data.len() / 2 * 2]),
        Format::S24 => {
            for s in data.chunks_exact(2) {
                out.extend_from_slice(&[0, s[0], s[1]]);
            }
        }
        Format::S32 => {
            for s in data.chunks_exact(2) {
                out.extend_from_slice(&[0, 0, s[0], s[1]]);
            }
        }
        Format::F32 => {
            for s in data.chunks_exact(2) {
                let v = i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0;
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
}

/// Escala IN PLACE la muestra `s` (un canal) por `k` en su formato. Para las rampas anti-clic.
#[allow(dead_code)] // solo CoreAudio tiene rampa propia; el resto de backends no la usa
fn scale_sample(s: &mut [u8], fmt: Format, k: f32) {
    match fmt {
        Format::S16 => {
            let v = i16::from_le_bytes([s[0], s[1]]);
            s.copy_from_slice(&((v as f32 * k) as i16).to_le_bytes());
        }
        Format::S24 => {
            let v = (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f64 * k as f64;
            let b = (v as i32).to_le_bytes();
            s.copy_from_slice(&[b[0], b[1], b[2]]);
        }
        Format::S32 => {
            let v = i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64 * k as f64;
            s.copy_from_slice(&(v as i32).to_le_bytes());
        }
        Format::F32 => {
            let v = f32::from_le_bytes([s[0], s[1], s[2], s[3]]) * k;
            s.copy_from_slice(&v.to_le_bytes());
        }
    }
}

/// M378 (1942 #108): la cola entre el programa y el alimentador en octetos — la mitad de la
/// latencia pedida y nunca menos de dos chunks del alimentador (el coste por syscall ya lo acota
/// el suelo de 256 B del chunk). Antes había además un suelo de 2 KiB que a 22 050 Hz mono valía
/// 46 ms: con los otros dos suelos en bytes de CoreAudio, ninguna pista bajaba de ~130 ms. Pura.
fn queue_budget_bytes(bytes_per_sec: i64, latency_ms: i64, chunk: usize) -> i32 {
    // Suelo de 20 ms de audio en el socket: lo que tarda en despertar la fibra que escribe
    // (medido a 22 050 Hz mono con el anillo ya sano: con 10 ms la VM cortaba la salida — 3 s
    // de audio tardaban 5,3 s —; con 20 ms, cero huecos en 22/44,1/48 kHz, mono y estéreo).
    (bytes_per_sec * latency_ms / 2000).max(bytes_per_sec * 20 / 1000).max(chunk as i64 * 2) as i32
}

/// M469: por debajo de este hint (ms) una salida es de BAJA LATENCIA.
#[cfg(unix)]
const LOW_LATENCY_BELOW_MS: i64 = 20;

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
    /// M398 (rayplay #166): `flush` la levanta; el alimentador descarta lo encolado en el pipe y
    /// en el backend, y la baja. Mientras está alta, nada de lo escrito suena.
    discard: std::sync::atomic::AtomicBool,
}

/// El mapa extremo-de-escritura → control, para que `drain(key)` encuentre su salida. La clave
/// es el fd (unix) o el handle de Windows, como entero (`AsRawFd`/`AsRawHandle` del `File`).
fn ctls() -> &'static Mutex<HashMap<i64, Arc<Ctl>>> {
    static CTLS: OnceLock<Mutex<HashMap<i64, Arc<Ctl>>>> = OnceLock::new();
    CTLS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// M298 (findings 1.27.11 #22): el alimentador retira SU entrada del mapa al terminar — solo si
/// sigue siendo la suya. El SO reutiliza el fd/handle del extremo de escritura en cuanto se
/// cierra, así que una salida abierta justo después heredaba la misma clave y el alimentador
/// viejo, al acabar de drenar, borraba la entrada de la NUEVA: `played_ms` decía "not an open
/// audio output" durante toda su vida mientras `write` seguía funcionando.
fn forget_ctl(key: i64, own: &Arc<Ctl>) {
    let mut map = ctls().lock().unwrap();
    if map.get(&key).is_some_and(|c| Arc::ptr_eq(c, own)) {
        map.remove(&key);
    }
}

/// Abre una salida PCM (`sample_rate` Hz, `channels` canales, muestras en `format` — "s16le",
/// "s24le", "s32le" o "f32le", M398) y devuelve el extremo de escritura del pipe (no-bloqueante,
/// listo para el registro de handles) — el llamador escribe samples ahí y el hilo alimentador los
/// toca. `RAY_AUDIO_SINK=null` → sumidero de tiempo real.
pub fn open(sample_rate: i64, channels: i64, latency_ms: i64, format: &str) -> Result<std::fs::File, String> {
    // M468: `formato@dispositivo` abre un dispositivo concreto (de `devices`).
    let (format, device) = split_device(format);
    let fmt = Format::parse(format)?;
    if !(8000..=192000).contains(&sample_rate) {
        return Err(format!("audio: unsupported sample rate {sample_rate} (8000–192000)"));
    }
    if !(1..=8).contains(&channels) {
        return Err(format!("audio: unsupported channel count {channels} (1–8)"));
    }
    // M158 (§79b): el hint de latencia dimensiona anillo/buffers/chunk. 0 = default (200 ms,
    // el comportamiento de M145); explícito se acota a [20, 1000] — por debajo de 20 ms el
    // keepalive comería el margen, por encima de 1 s es un búfer, no una latencia.
    // M469 (ray-daw #424): de 3 a 19 ms es la petición de BAJA LATENCIA. En macOS abre el
    // camino AudioUnit HAL (búferes de dispositivo de 32–128 frames); en el resto se acepta y
    // cada backend la sube a su suelo.
    if latency_ms != 0 && !(3..=1000).contains(&latency_ms) {
        return Err(format!("audio: unsupported latency {latency_ms} ms (3–1000, or 0 = default)"));
    }
    let latency_ms = if latency_ms == 0 { 200 } else { latency_ms };
    #[cfg(unix)]
    let null_sink = std::env::var("RAY_AUDIO_SINK").as_deref() == Ok("null");
    // M469 (ray-daw #431): una salida Bluetooth crepita con menos de ~80 ms encolados (el
    // fallo ocurre dentro de CoreAudio, invisible desde aquí): el hint se sube a ese suelo.
    #[cfg(target_os = "macos")]
    let latency_ms = if null_sink { latency_ms } else { coreaudio::raise_for_transport(device, latency_ms) };
    let played_frames = Arc::new(AtomicI64::new(-1));
    // El backend se abre ANTES del pipe: un dispositivo ausente falla en `open`, no a mitad.
    let sink = make_sink(sample_rate, channels, latency_ms, fmt, played_frames.clone(), device)?;

    // El chunk del alimentador es ~latencia/4: reactivo sin syscalls de más (alineado a frame).
    let frame_bytes = (channels as usize) * fmt.sample_bytes();
    let bytes_per_sec = sample_rate * channels * fmt.sample_bytes() as i64;
    let chunk = ((bytes_per_sec * latency_ms / 4000).max(256) as usize).max(frame_bytes) / frame_bytes * frame_bytes;
    #[cfg(unix)]
    {
        let mut fds = [0i32; 2];
        // SAFETY: socketpair escribe dos fds válidos.
        if unsafe { socketpair(AF_UNIX, SOCK_STREAM, 0, fds.as_mut_ptr()) } != 0 {
            return Err(format!("audio: could not create the pipe: {}", std::io::Error::last_os_error()));
        }
        let (fd_r, fd_w) = (fds[0], fds[1]);
        // M298 (#21): la cola entre el programa y el alimentador se acota (antes el pipe del SO
        // ponía un suelo de 64 KiB). Los dos buffers, en los dos extremos: en BSD manda el de
        // recepción del par; en Linux, el de envío del emisor.
        // M326 (findings #103): la latencia pedida es el TOTAL encolado, y hay tres etapas — este
        // socket, el anillo del backend y los buffers del dispositivo. M298 daba la latencia entera
        // al socket y el backend guardaba otro tanto (CoreAudio: anillo 1× + 3 buffers de ¼), así
        // que `open_latency(…, 200)` encolaba ~600 ms. Reparto: socket ½, backend ½ (anillo ¼ +
        // buffers ¼ en CoreAudio; ALSA/WASAPI ya pedían ½ al dispositivo). Nunca menos de dos
        // chunks del alimentador, para que el flujo no se corte por el propio reparto.
        // M469: en baja latencia (macOS, hint < 20) el suelo de 20 ms del socket sería toda la
        // latencia pedida: la cola es la mitad del hint y nunca menos de dos chunks.
        let low_latency = cfg!(target_os = "macos") && !null_sink && latency_ms < LOW_LATENCY_BELOW_MS;
        let budget = if low_latency {
            (bytes_per_sec * latency_ms / 2000).max(chunk as i64 * 2) as i32
        } else {
            queue_budget_bytes(bytes_per_sec, latency_ms, chunk)
        };
        unsafe {
            let v = &budget as *const i32 as *const core::ffi::c_void;
            for fd in [fd_r, fd_w] {
                setsockopt(fd, SOL_SOCKET, SO_SNDBUF, v, 4);
                setsockopt(fd, SOL_SOCKET, SO_RCVBUF, v, 4);
            }
            // Un `write` con el alimentador ya muerto debe dar EPIPE, no matar el proceso.
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            {
                let one: i32 = 1;
                setsockopt(fd_w, SOL_SOCKET, SO_NOSIGPIPE, &one as *const i32 as *const core::ffi::c_void, 4);
            }
            // M378 (1942 #108): en BSD/macOS el socket solo se reporta ESCRIBIBLE (kqueue, la
            // espera de la fibra) con al menos SO_SNDLOWAT libre — 2 KiB por defecto, recortado
            // al tamaño del buffer si este es menor —, así que con un buffer de ~30 ms la fibra
            // solo despertaba con el socket VACÍO y el anillo + los búferes (25 ms) no cubrían
            // el despertar: silencio. Con la marca en un chunk, despierta en cuanto cabe uno.
            #[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
            {
                let lowat: i32 = chunk as i32;
                setsockopt(fd_w, SOL_SOCKET, SO_SNDLOWAT, &lowat as *const i32 as *const core::ffi::c_void, 4);
            }
        }
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
            discard: std::sync::atomic::AtomicBool::new(false),
        });
        ctls().lock().unwrap().insert(key, ctl.clone());
        // El alimentador: lee el pipe (bloqueante) y empuja al backend; EOF (close del handle) →
        // drena el backend y termina.
        let ctl_thread = ctl.clone();
        std::thread::spawn(move || {
            // M332: en Apple el alimentador pide la clase de QoS interactiva — en iOS un hilo
            // de prioridad normal puede tardar más que un búfer de la AudioQueue en despertar
            // (y al bloquear la pantalla, bastante más): el anillo se vaciaba y crepitaba.
            // SAFETY: llamada de libSystem sobre el hilo actual; un fallo solo deja la QoS por defecto.
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            unsafe {
                unsafe extern "C" {
                    fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
                }
                const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
                pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
            }
            let mut sink = sink;
            let mut buf = vec![0u8; chunk];
            loop {
                // M398 (rayplay #166): `flush` — vacía el pipe sin tocarlo y descarta lo del backend.
                if ctl_thread.discard.load(Ordering::SeqCst) {
                    while pipe_pending(fd_r as i64) > 0 {
                        // SAFETY: hay octetos pendientes, así que el read no bloquea; buf es nuestro.
                        if unsafe { read(fd_r, buf.as_mut_ptr(), buf.len()) } <= 0 {
                            break;
                        }
                    }
                    sink.flush();
                    ctl_thread.discard.store(false, Ordering::SeqCst);
                    continue;
                }
                // Espera legible con un tope de 20 ms, para volver a mirar la bandera de arriba
                // aunque el programa no escriba nada más (un `read` bloqueante no la vería).
                let mut pfd = PollFd { fd: fd_r, events: POLLIN, revents: 0 };
                // SAFETY: pfd es nuestro y vive durante la llamada.
                if unsafe { poll(&mut pfd, 1, 20) } <= 0 {
                    continue; // tiempo agotado o EINTR: nada que leer todavía
                }
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
            forget_ctl(key, &ctl_thread);
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
            discard: std::sync::atomic::AtomicBool::new(false),
        });
        ctls().lock().unwrap().insert(key, ctl.clone());
        let ctl_thread = ctl.clone();
        let reader_handle = reader.as_raw_handle() as i64;
        std::thread::spawn(move || {
            let mut sink = sink;
            let mut buf = vec![0u8; chunk];
            loop {
                // M398 (rayplay #166): `flush` — vacía el pipe sin tocarlo y descarta lo del backend.
                if ctl_thread.discard.load(Ordering::SeqCst) {
                    while let Ok(n) = peek_pipe(reader_handle) {
                        let want = n.min(buf.len());
                        if n == 0 || reader.read(&mut buf[..want]).unwrap_or(0) == 0 {
                            break;
                        }
                    }
                    sink.flush();
                    ctl_thread.discard.store(false, Ordering::SeqCst);
                    continue;
                }
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
            forget_ctl(key, &ctl_thread);
        });
        Ok(writer)
    }
}

/// Espera a que TODO lo escrito suene: pipe vacío + alimentador sin nada en vuelo + un margen de
/// la latencia del dispositivo. Bloquea el hilo (uso raro, al final de una sesión) — el margen
/// es aproximado por diseño: el "de verdad sonó" exacto es del backend y v1 no lo persigue.
/// M332: apaga con rampa las salidas vivas antes de que el proceso muera (iOS:
/// `applicationWillTerminate`). Fuera de CoreAudio no hace nada.
pub fn quiesce() {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    coreaudio::quiesce();
}

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

/// M398 (rayplay #166): descarta lo escrito y aún no sonado — el pipe y lo que el backend tenga
/// en su anillo (CoreAudio funde la cola con la rampa anti-clic) — SIN cerrar la salida: lo que
/// se escriba después suena en cuanto llega. Lo que ya está en los búferes del dispositivo
/// (unos pocos ms) termina de sonar. Bloquea el hilo hasta que el alimentador lo confirma
/// (como mucho ~un chunk, acotado a 1 s).
pub fn flush(key: i64) -> Result<(), String> {
    let ctl = match ctls().lock().unwrap().get(&key) {
        Some(c) => c.clone(),
        None => return Err("audio: not an open audio output".to_string()),
    };
    ctl.discard.store(true, Ordering::SeqCst);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while ctl.discard.load(Ordering::SeqCst) {
        if std::time::Instant::now() > deadline {
            return Err("audio: flush timed out (the output is not being consumed)".to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
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

/// Un sumidero PCM: recibe muestras entrelazadas (en el formato con que se abrió) y las hace
/// sonar (o las consume). `play` puede BLOQUEAR (es la contrapresión: el pipe se llena aguas
/// arriba y la fibra aparca). `flush` (M398) descarta lo que tenga encolado, si tiene.
// =====================================================================================
// M463 (ray-daw #422) — ENTRADA de audio: grabar del micrófono o de una interfaz
// =====================================================================================
//
// El espejo de la salida. `open_input` crea el mismo par de sockets, pero el sentido es el
// contrario: la FUENTE (el callback del dispositivo, o un hilo en la fuente de pruebas) escribe
// lo capturado en un extremo y el programa lee del otro, que se registra como un pipe de lectura
// ordinario — `audio.read` aparca la fibra hasta que haya datos, igual que la lectura del stdout
// de un proceso hijo. El `close(h)` del programa cierra ese extremo; la fuente lo ve y se para.
//
// El par es de DATAGRAMAS: cada bloque capturado viaja entero y cada `audio.read` devuelve uno,
// así que el programa siempre recibe frames enteros sea cual sea su tamaño (con un flujo, la
// lectura de 64 KiB del runtime partía los frames de 3 o 6 octetos).
//
// La entrada NO tiene contrapresión: el dispositivo captura a su ritmo, lo lea alguien o no. La
// cola guarda hasta `INPUT_QUEUE_MS` de audio; si el programa no lee a tiempo, el bloque que no
// cabe se DESCARTA entero (un desbordamiento, contado) — nunca se bloquea el hilo del
// dispositivo ni se desalinea el flujo.
//
// Fuentes: AudioQueue de entrada en macOS e iOS; ALSA en Linux (M465: un hilo que lee el
// dispositivo de captura por defecto en bloques); `RAY_AUDIO_SOURCE=null` (silencio) o `=tone`
// (un La de 440 Hz) a ritmo de tiempo real y sin hardware, para tests y CI, en cualquier unix.
// Android (M465): AAudio en dirección de entrada, con el mismo hilo lector; antes de abrir pide
// al shell el permiso de micrófono (`RECORD_AUDIO`, que el bundle declara con
// `[android] record_audio = true`). Windows (M465): WASAPI en modo compartido; el pipe es uno con
// nombre en modo mensaje (`win_pipe`), que da lo mismo que el par de datagramas.

/// Cuánto audio capturado se guarda sin leer antes de descartar.
const INPUT_QUEUE_MS: i64 = 1000;

/// El estado de una entrada abierta, compartido entre la fuente y `recorded_ms`.
pub struct InCtl {
    /// Frames que el dispositivo entregó desde que se abrió, se hayan leído o no: el reloj de
    /// captura (`recorded_ms`).
    captured_frames: AtomicI64,
    /// Frames descartados porque el programa no leyó a tiempo.
    dropped_frames: AtomicI64,
    sample_rate: i64,
    frame_bytes: usize,
}

fn in_ctls() -> &'static Mutex<HashMap<i64, Arc<InCtl>>> {
    static CTLS: OnceLock<Mutex<HashMap<i64, Arc<InCtl>>>> = OnceLock::new();
    CTLS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Como `forget_ctl`: la fuente retira su entrada del mapa solo si sigue siendo la suya (el SO
/// reutiliza el fd en cuanto se cierra).
fn forget_in_ctl(key: i64, own: &Arc<InCtl>) {
    let mut map = in_ctls().lock().unwrap();
    if map.get(&key).is_some_and(|c| Arc::ptr_eq(c, own)) {
        map.remove(&key);
    }
    if std::env::var_os("RAY_AUDIO_TRACE").is_some() {
        eprintln!(
            "audio input: captured {} frames, dropped {}",
            own.captured_frames.load(Ordering::SeqCst),
            own.dropped_frames.load(Ordering::SeqCst)
        );
    }
}

/// El reloj de captura de la entrada `key`, en ms: cuánto audio ha entregado el dispositivo
/// desde que se abrió. Avanza aunque el programa no lea.
pub fn recorded_ms(key: i64) -> Result<i64, String> {
    let ctl = in_ctls().lock().unwrap().get(&key).cloned().ok_or_else(|| "audio: not an open audio input".to_string())?;
    Ok(ctl.captured_frames.load(Ordering::SeqCst) * 1000 / ctl.sample_rate)
}

/// El datagrama más largo que se entrega: cabe en el búfer de lectura del runtime (64 KiB).
const INPUT_DATAGRAM_MAX: usize = 32 * 1024;

/// El extremo de la FUENTE en el pipe de una entrada: un fd en unix (un socket de datagramas),
/// un HANDLE en Windows (M465: el lado servidor de un pipe con nombre en modo mensaje).
#[cfg(unix)]
type SourceEnd = i32;
#[cfg(windows)]
pub(crate) type SourceEnd = isize;

/// El resultado de entregar un bloque al programa.
#[cfg_attr(unix, allow(dead_code))]
pub(crate) enum Wrote {
    Done,
    /// La cola está llena: el bloque se descarta.
    Full,
    /// El programa cerró la entrada.
    Closed,
}

#[cfg(unix)]
fn write_block(end: SourceEnd, block: &[u8]) -> Wrote {
    loop {
        // SAFETY: el tramo vive durante la llamada; `end` es de la fuente.
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let n = unsafe { write(end, block.as_ptr(), block.len()) };
        // Con SOCK_SEQPACKET, escribir a un par cerrado levanta SIGPIPE: MSG_NOSIGNAL lo evita.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let n = unsafe { send(end, block.as_ptr(), block.len(), 0x4000) };
        if n >= 0 {
            return Wrote::Done;
        }
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        // Cola llena: WouldBlock, o ENOBUFS en un socket de datagramas (55 en macOS, 105 en Linux).
        if err.kind() == std::io::ErrorKind::WouldBlock || matches!(err.raw_os_error(), Some(55) | Some(105)) {
            return Wrote::Full;
        }
        return Wrote::Closed;
    }
}

#[cfg(unix)]
fn close_end(end: SourceEnd) {
    // SAFETY: `end` es de la fuente que lo cierra, una sola vez.
    unsafe { close(end) };
}

#[cfg(windows)]
pub(crate) mod win_pipe {
    //! M465: el pipe de una entrada en Windows. Un pipe ANÓNIMO es un flujo de octetos y no tiene
    //! modo no bloqueante; aquí hacen falta las dos cosas que en unix da el par de datagramas:
    //! que cada lectura del programa sea un bloque de frames enteros y que la fuente nunca espere.
    //! Un pipe CON NOMBRE en modo mensaje (`PIPE_TYPE_MESSAGE`) con el lado de la fuente en
    //! `PIPE_NOWAIT` da ambas: `ReadFile` devuelve un mensaje, y `WriteFile` sin sitio vuelve con
    //! cero octetos escritos. Una sola instancia, conectada en el acto por este proceso: nadie
    //! más puede abrirlo.
    //!
    //! M466: `std/midi` usa el mismo pipe para sus entradas, y pregunta con `peer_alive` si el
    //! programa cerró la suya (por eso el lado de la fuente se abre dúplex: `PeekNamedPipe`
    //! exige acceso de lectura, aunque por él nunca llegue nada).
    use super::{SourceEnd, Wrote};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateNamedPipeW(name: *const u16, open_mode: u32, pipe_mode: u32, max_instances: u32, out_size: u32, in_size: u32, timeout: u32, security: *const core::ffi::c_void) -> isize;
        fn CreateFileW(name: *const u16, access: u32, share: u32, security: *const core::ffi::c_void, disposition: u32, flags: u32, template: isize) -> isize;
        fn SetNamedPipeHandleState(handle: isize, mode: *const u32, max_collection: *const u32, collect_timeout: *const u32) -> i32;
        fn WriteFile(handle: isize, buf: *const u8, len: u32, written: *mut u32, overlapped: *mut core::ffi::c_void) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetCurrentProcessId() -> u32;
        // El handle como `usize`, igual que la otra declaración del crate.
        fn PeekNamedPipe(handle: usize, buf: *mut core::ffi::c_void, len: u32, read: *mut u32, avail: *mut u32, left: *mut u32) -> i32;
    }
    const INVALID_HANDLE_VALUE: isize = -1;
    const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
    const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
    const PIPE_TYPE_MESSAGE: u32 = 0x4;
    const PIPE_NOWAIT: u32 = 0x1;
    const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x8;
    const PIPE_READMODE_MESSAGE: u32 = 0x2;
    const GENERIC_READ: u32 = 0x8000_0000;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
    const OPEN_EXISTING: u32 = 3;

    /// Crea el pipe con sitio para `budget` octetos: (extremo de lectura del programa, extremo
    /// de la fuente).
    pub fn create(budget: i32) -> Result<(std::fs::File, SourceEnd), String> {
        create_named("audio", budget).map_err(|e| format!("audio: {e}"))
    }

    /// Como `create`, con la etiqueta del subsistema en el nombre del pipe y el error sin prefijo.
    pub fn create_named(label: &str, budget: i32) -> Result<(std::fs::File, SourceEnd), String> {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // SAFETY: llamada sin punteros.
        let pid = unsafe { GetCurrentProcessId() };
        let name: Vec<u16> = format!(r"\\.\pipe\ray-{label}-in-{pid}-{seq}").encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `name` termina en NUL y vive durante las llamadas; los handles devueltos son nuestros.
        unsafe {
            let server = CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_MESSAGE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                budget as u32,
                0,
                0,
                std::ptr::null(),
            );
            if server == INVALID_HANDLE_VALUE {
                return Err(format!("could not create the pipe: {}", std::io::Error::last_os_error()));
            }
            // FILE_WRITE_ATTRIBUTES: lo exige SetNamedPipeHandleState en el lado cliente.
            let client = CreateFileW(name.as_ptr(), GENERIC_READ | FILE_WRITE_ATTRIBUTES, 0, std::ptr::null(), OPEN_EXISTING, 0, 0);
            if client == INVALID_HANDLE_VALUE {
                let e = std::io::Error::last_os_error();
                CloseHandle(server);
                return Err(format!("could not open the pipe: {e}"));
            }
            let mode = PIPE_READMODE_MESSAGE;
            if SetNamedPipeHandleState(client, &mode, std::ptr::null(), std::ptr::null()) == 0 {
                let e = std::io::Error::last_os_error();
                CloseHandle(client);
                CloseHandle(server);
                return Err(format!("could not configure the pipe: {e}"));
            }
            let reader: std::fs::File = std::os::windows::io::FromRawHandle::from_raw_handle(client as *mut core::ffi::c_void);
            Ok((reader, server))
        }
    }

    pub fn write_block(end: SourceEnd, block: &[u8]) -> Wrote {
        let mut written = 0u32;
        // SAFETY: el tramo vive durante la llamada; `end` es de la fuente.
        let ok = unsafe { WriteFile(end, block.as_ptr(), block.len() as u32, &mut written, std::ptr::null_mut()) };
        if ok == 0 {
            return Wrote::Closed; // ERROR_NO_DATA: el programa cerró su extremo
        }
        if written == 0 { Wrote::Full } else { Wrote::Done }
    }

    /// ¿Sigue abierto el extremo del programa? (`PeekNamedPipe` falla con ERROR_BROKEN_PIPE
    /// cuando el cliente cerró.)
    pub fn peer_alive(end: SourceEnd) -> bool {
        let mut avail = 0u32;
        // SAFETY: sin búfer; solo se pide `avail`, un u32 propio; `end` es de la fuente.
        unsafe { PeekNamedPipe(end as usize, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut()) != 0 }
    }

    pub fn close_end(end: SourceEnd) {
        // SAFETY: `end` es de la fuente que lo cierra, una sola vez.
        unsafe { CloseHandle(end) };
    }
}
#[cfg(windows)]
use win_pipe::{close_end, write_block};

/// Entrega `data` (frames enteros recién capturados) al programa, en bloques de frames enteros.
/// `false` = el programa cerró la entrada: la fuente debe parar. Nunca bloquea: el bloque que no
/// cabe se descarta entero.
fn push_captured(end: SourceEnd, ctl: &InCtl, data: &[u8]) -> bool {
    let fb = ctl.frame_bytes;
    ctl.captured_frames.fetch_add((data.len() / fb) as i64, Ordering::SeqCst);
    let piece = (INPUT_DATAGRAM_MAX / fb).max(1) * fb;
    for block in data.chunks(piece) {
        match write_block(end, block) {
            Wrote::Done => {}
            Wrote::Full => {
                ctl.dropped_frames.fetch_add((block.len() / fb) as i64, Ordering::SeqCst);
            }
            Wrote::Closed => return false,
        }
    }
    true
}

/// El pipe de una entrada con sitio para `budget` octetos: (el extremo de lectura del programa,
/// no bloqueante; el extremo de la fuente, que nunca espera).
#[cfg(unix)]
fn input_pipe(budget: i32) -> Result<(std::fs::File, SourceEnd), String> {
    let mut fds = [0i32; 2];
    // SAFETY: socketpair escribe dos fds válidos.
    if unsafe { socketpair(AF_UNIX, INPUT_SOCKET, 0, fds.as_mut_ptr()) } != 0 {
        return Err(format!("audio: could not create the pipe: {}", std::io::Error::last_os_error()));
    }
    let (fd_r, fd_w) = (fds[0], fds[1]);
    // SAFETY: fds recién creados y nuestros; los valores viven durante cada llamada.
    unsafe {
        let v = &budget as *const i32 as *const core::ffi::c_void;
        for fd in [fd_r, fd_w] {
            setsockopt(fd, SOL_SOCKET, SO_SNDBUF, v, 4);
            setsockopt(fd, SOL_SOCKET, SO_RCVBUF, v, 4);
        }
        // Una escritura con el programa ya cerrado debe dar EPIPE, no matar el proceso.
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            let one: i32 = 1;
            setsockopt(fd_w, SOL_SOCKET, SO_NOSIGPIPE, &one as *const i32 as *const core::ffi::c_void, 4);
        }
        // Los dos extremos no bloqueantes: la fuente nunca espera, y la lectura del programa
        // aparca la fibra (WouldBlock) en vez de bloquear el hilo.
        for fd in [fd_r, fd_w] {
            let fl = fcntl_raw(fd, F_GETFL);
            fcntl_raw(fd, F_SETFL, fl | O_NONBLOCK);
        }
    }
    // SAFETY: fd_r es nuestro; File toma la propiedad (su Drop = close = fin de la captura).
    Ok((unsafe { std::os::unix::io::FromRawFd::from_raw_fd(fd_r) }, fd_w))
}
#[cfg(windows)]
fn input_pipe(budget: i32) -> Result<(std::fs::File, SourceEnd), String> {
    win_pipe::create(budget)
}

/// La clave de una entrada en el mapa de controles: el fd o el HANDLE de su extremo de lectura,
/// como entero (lo mismo que calcula el registro de handles del llamador).
fn input_key(f: &std::fs::File) -> i64 {
    #[cfg(unix)]
    {
        std::os::unix::io::AsRawFd::as_raw_fd(f) as i64
    }
    #[cfg(windows)]
    {
        std::os::windows::io::AsRawHandle::as_raw_handle(f) as i64
    }
}

/// Abre una ENTRADA PCM (`sample_rate` Hz, `channels` canales, muestras en `format`) y devuelve
/// el extremo de lectura del pipe, no bloqueante y listo para el registro de handles: el
/// llamador lee de ahí los frames entrelazados que captura el dispositivo. `latency_ms` (20–1000,
/// o 0 = 100) es el tamaño de los bloques en que llegan.
pub fn open_input(sample_rate: i64, channels: i64, latency_ms: i64, format: &str) -> Result<std::fs::File, String> {
    // M468: `formato@dispositivo` graba de un dispositivo concreto (de `devices`).
    let (format, device) = split_device(format);
    let fmt = Format::parse(format)?;
    if !(8000..=192000).contains(&sample_rate) {
        return Err(format!("audio: unsupported sample rate {sample_rate} (8000–192000)"));
    }
    if !(1..=8).contains(&channels) {
        return Err(format!("audio: unsupported channel count {channels} (1–8)"));
    }
    // M469: de 3 a 19 ms, baja latencia (bloques más pequeños; en macOS, además, el búfer de
    // E/S del dispositivo).
    if latency_ms != 0 && !(3..=1000).contains(&latency_ms) {
        return Err(format!("audio: unsupported latency {latency_ms} ms (3–1000, or 0 = default)"));
    }
    let latency_ms = if latency_ms == 0 { 100 } else { latency_ms };
    let frame_bytes = channels as usize * fmt.sample_bytes();
    let bytes_per_sec = sample_rate as usize * frame_bytes;
    let source = std::env::var("RAY_AUDIO_SOURCE").ok();
    let test_source = matches!(source.as_deref(), Some("null") | Some("tone"));
    if !test_source && !cfg!(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "android", windows)) {
        return Err("audio: no input backend for this platform (macOS/iOS/Linux/Android/Windows; RAY_AUDIO_SOURCE=null gives a silent real-time source anywhere)".to_string());
    }

    // La cola (y con ella el bloque más largo que admite el pipe): 1 s de audio, y nunca menos
    // de dos bloques máximos.
    let budget = ((bytes_per_sec as i64 * INPUT_QUEUE_MS / 1000).max(2 * INPUT_DATAGRAM_MAX as i64)).min(i32::MAX as i64) as i32;
    let (reader, end) = input_pipe(budget)?;
    let key = input_key(&reader);
    let ctl = Arc::new(InCtl {
        captured_frames: AtomicI64::new(0),
        dropped_frames: AtomicI64::new(0),
        sample_rate,
        frame_bytes,
    });
    let _ = device; // la fuente de pruebas no tiene dispositivos
    let started = if test_source {
        test_source_start(end, key, ctl.clone(), sample_rate, channels, latency_ms, fmt, source.as_deref() == Some("tone"));
        Ok(())
    } else {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            coreaudio::open_input(sample_rate, channels, latency_ms, fmt, end, key, ctl.clone(), device)
        }
        // M465: Linux — ALSA por dlopen, con un hilo que lee el dispositivo en bloques.
        #[cfg(target_os = "linux")]
        {
            alsa::open_input(sample_rate, channels, latency_ms, fmt, end, key, ctl.clone(), device)
        }
        // M465: Android — AAudio en dirección de entrada (pide antes el permiso de micrófono).
        #[cfg(target_os = "android")]
        {
            aaudio::open_input(sample_rate, channels, latency_ms, fmt, end, key, ctl.clone(), device)
        }
        // M465: Windows — WASAPI en modo compartido sobre el dispositivo de captura por defecto.
        #[cfg(windows)]
        {
            wasapi::open_input(sample_rate, channels, latency_ms, fmt, end, key, ctl.clone(), device)
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "android", windows)))]
        {
            let _ = device;
            Err("audio: no input backend for this platform".to_string())
        }
    };
    if let Err(e) = started {
        // Los dos extremos siguen siendo nuestros (la fuente no llegó a arrancar).
        close_end(end);
        return Err(e); // `reader` se cierra al salir
    }
    in_ctls().lock().unwrap().insert(key, ctl);
    Ok(reader)
}

/// La fuente de pruebas (`RAY_AUDIO_SOURCE=null|tone`): un hilo que entrega bloques de
/// `latency_ms / 4` a ritmo de tiempo real — silencio, o un La de 440 Hz a media escala.
#[allow(clippy::too_many_arguments)]
fn test_source_start(end: SourceEnd, key: i64, ctl: Arc<InCtl>, rate: i64, channels: i64, latency_ms: i64, fmt: Format, tone: bool) {
    std::thread::spawn(move || {
        let block_frames = ((rate * latency_ms / 4000).max(64)) as usize;
        let sb = fmt.sample_bytes();
        let mut buf = vec![0u8; block_frames * channels as usize * sb];
        let started = std::time::Instant::now();
        let mut frame = 0u64;
        loop {
            if tone {
                for i in 0..block_frames {
                    let t = (frame + i as u64) as f64 / rate as f64;
                    let v = (t * 440.0 * std::f64::consts::TAU).sin() * 0.5;
                    for c in 0..channels as usize {
                        let at = (i * channels as usize + c) * sb;
                        match fmt {
                            Format::S16 => buf[at..at + 2].copy_from_slice(&((v * 32767.0) as i16).to_le_bytes()),
                            Format::S24 => buf[at..at + 3].copy_from_slice(&((v * 8_388_607.0) as i32).to_le_bytes()[..3]),
                            Format::S32 => buf[at..at + 4].copy_from_slice(&((v * 2_147_483_647.0) as i32).to_le_bytes()),
                            Format::F32 => buf[at..at + 4].copy_from_slice(&(v as f32).to_le_bytes()),
                        }
                    }
                }
            }
            frame += block_frames as u64;
            // El bloque se entrega cuando «ha sonado»: al final de su duración.
            let due = started + std::time::Duration::from_micros(frame * 1_000_000 / rate as u64);
            let now = std::time::Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
            if !push_captured(end, &ctl, &buf) {
                break;
            }
        }
        close_end(end);
        forget_in_ctl(key, &ctl);
    });
}

trait Sink: Send {
    fn play(&mut self, data: &[u8]);
    fn finish(&mut self);
    fn flush(&mut self) {}
}

fn make_sink(
    rate: i64,
    channels: i64,
    latency_ms: i64,
    fmt: Format,
    played: Arc<AtomicI64>,
    device: &str,
) -> Result<Box<dyn Sink>, String> {
    if std::env::var("RAY_AUDIO_SINK").as_deref() == Ok("null") {
        return Ok(Box::new(NullSink {
            bytes_per_sec: rate * channels * fmt.sample_bytes() as i64,
            frame_bytes: channels as usize * fmt.sample_bytes(),
            consumed_frames: 0,
            played,
        }));
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        coreaudio::open(rate, channels, latency_ms, fmt, played, device)
    }
    #[cfg(target_os = "linux")]
    {
        alsa::open(rate, channels, latency_ms, fmt, played, device)
    }
    // M158: Android — AAudio por dlopen (API 26+; el patrón ALSA).
    #[cfg(target_os = "android")]
    {
        aaudio::open(rate, channels, latency_ms, fmt, played, device)
    }
    // M178: Windows — WASAPI por COM a mano (s16le; otros formatos convertidos en el alimentador).
    #[cfg(windows)]
    {
        Ok(Convert::wrap(fmt, wasapi::open(rate, channels, latency_ms, played, device)?))
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "android", windows)))]
    {
        let _ = (rate, channels, latency_ms, fmt, played, device);
        Err("audio: no backend for this platform (macOS/iOS/Linux/Android/Windows; RAY_AUDIO_SINK=null works anywhere)".to_string())
    }
}

/// M398: el sumidero que convierte a s16le en el alimentador, para un backend que no acepta el
/// formato pedido (ALSA/AAudio según dispositivo, WASAPI siempre). Transparente: `Format::S16`
/// no envuelve.
#[allow(dead_code)] // en macOS/iOS CoreAudio acepta los cuatro formatos y no se usa
struct Convert {
    inner: Box<dyn Sink>,
    from: Format,
    scratch: Vec<u8>,
}

impl Convert {
    #[allow(dead_code)]
    fn wrap(from: Format, inner: Box<dyn Sink>) -> Box<dyn Sink> {
        if from == Format::S16 {
            inner
        } else {
            Box::new(Convert { inner, from, scratch: Vec::new() })
        }
    }
}

impl Sink for Convert {
    fn play(&mut self, data: &[u8]) {
        self.scratch.clear();
        to_s16le(self.from, data, &mut self.scratch);
        self.inner.play(&self.scratch);
    }
    fn finish(&mut self) {
        self.inner.finish()
    }
    fn flush(&mut self) {
        self.inner.flush()
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

// ── macOS e iOS: AudioQueue (AudioToolbox.framework, enlazado — siempre presente) ─────
// M324 (ray808 #18): el mismo backend sirve en iOS — AudioQueue es API de ambos; el shell
// generado enlaza el framework (un staticlib no arrastra sus `#[link]`).
#[cfg(any(target_os = "macos", target_os = "ios"))]
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
        /// M463: la cola de ENTRADA. Con `run_loop` nulo el callback corre en un hilo interno
        /// de AudioQueue, como el de salida.
        fn AudioQueueNewInput(
            format: *const AudioStreamBasicDescription,
            callback: extern "C" fn(*mut std::ffi::c_void, AudioQueueRef, AudioQueueBufferRef, *const AudioTimeStamp, u32, *const std::ffi::c_void),
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
        fn AudioQueueSetParameter(q: AudioQueueRef, param: u32, value: f32) -> i32;
        /// M468: `kAudioQueueProperty_CurrentDevice` elige el dispositivo de la cola.
        #[cfg(target_os = "macos")]
        fn AudioQueueSetProperty(q: AudioQueueRef, id: u32, data: *const std::ffi::c_void, size: u32) -> i32;
    }

    // ── M468 (ray-daw #424): los dispositivos del sistema (HAL de CoreAudio, solo macOS) ──
    //
    // iOS no tiene esta API: el sistema enruta el audio y la app solo ve «el dispositivo
    // actual». En macOS cada dispositivo es un `AudioObjectID` con propiedades que se leen por
    // (selector, ámbito, elemento); el UID es la cadena estable con la que se abre.
    #[cfg(target_os = "macos")]
    mod hal {
        use std::ffi::c_void;

        #[repr(C)]
        struct PropertyAddress {
            selector: u32,
            scope: u32,
            element: u32,
        }
        type CFStringRef = *const c_void;

        #[link(name = "CoreAudio", kind = "framework")]
        unsafe extern "C" {
            fn AudioObjectGetPropertyDataSize(object: u32, address: *const PropertyAddress, qualifier_size: u32, qualifier: *const c_void, size: *mut u32) -> i32;
            fn AudioObjectGetPropertyData(object: u32, address: *const PropertyAddress, qualifier_size: u32, qualifier: *const c_void, size: *mut u32, data: *mut c_void) -> i32;
        }
        #[link(name = "CoreFoundation", kind = "framework")]
        unsafe extern "C" {
            fn CFStringCreateWithBytes(alloc: *const c_void, bytes: *const u8, len: isize, encoding: u32, external: u8) -> CFStringRef;
            fn CFStringGetLength(s: CFStringRef) -> isize;
            fn CFStringGetMaximumSizeForEncoding(len: isize, encoding: u32) -> isize;
            fn CFStringGetCString(s: CFStringRef, buf: *mut u8, len: isize, encoding: u32) -> u8;
            fn CFRelease(obj: *const c_void);
        }
        const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8

        const fn fourcc(code: &[u8; 4]) -> u32 {
            u32::from_be_bytes(*code)
        }
        const SYSTEM_OBJECT: u32 = 1; // kAudioObjectSystemObject
        const SCOPE_GLOBAL: u32 = fourcc(b"glob");
        const SCOPE_INPUT: u32 = fourcc(b"inpt");
        const SCOPE_OUTPUT: u32 = fourcc(b"outp");
        const DEVICES: u32 = fourcc(b"dev#"); // kAudioHardwarePropertyDevices
        const DEFAULT_INPUT: u32 = fourcc(b"dIn "); // kAudioHardwarePropertyDefaultInputDevice
        const DEFAULT_OUTPUT: u32 = fourcc(b"dOut"); // kAudioHardwarePropertyDefaultOutputDevice
        const NAME: u32 = fourcc(b"lnam"); // kAudioObjectPropertyName
        const DEVICE_UID: u32 = fourcc(b"uid "); // kAudioDevicePropertyDeviceUID
        const STREAM_CONFIGURATION: u32 = fourcc(b"slay"); // kAudioDevicePropertyStreamConfiguration
        const NOMINAL_SAMPLE_RATE: u32 = fourcc(b"nsrt"); // kAudioDevicePropertyNominalSampleRate
        const TRANSPORT_TYPE: u32 = fourcc(b"tran"); // kAudioDevicePropertyTransportType

        /// Lee una propiedad de tamaño fijo en `out`; `false` si el objeto no la tiene.
        fn get<T>(object: u32, selector: u32, scope: u32, out: &mut T) -> bool {
            let address = PropertyAddress { selector, scope, element: 0 };
            let mut size = std::mem::size_of::<T>() as u32;
            // SAFETY: `out` es nuestro y tiene exactamente `size` octetos.
            unsafe { AudioObjectGetPropertyData(object, &address, 0, std::ptr::null(), &mut size, out as *mut T as *mut c_void) == 0 }
        }

        /// Lee una propiedad de tamaño variable entera.
        fn get_bytes(object: u32, selector: u32, scope: u32) -> Vec<u8> {
            let address = PropertyAddress { selector, scope, element: 0 };
            let mut size = 0u32;
            // SAFETY: consulta de tamaño; después el búfer tiene los octetos que CoreAudio pidió.
            unsafe {
                if AudioObjectGetPropertyDataSize(object, &address, 0, std::ptr::null(), &mut size) != 0 || size == 0 {
                    return Vec::new();
                }
                // Alineado a 8: las listas llevan punteros.
                let mut buf = vec![0u64; (size as usize).div_ceil(8)];
                if AudioObjectGetPropertyData(object, &address, 0, std::ptr::null(), &mut size, buf.as_mut_ptr() as *mut c_void) != 0 {
                    return Vec::new();
                }
                std::slice::from_raw_parts(buf.as_ptr() as *const u8, size as usize).to_vec()
            }
        }

        /// Una propiedad de texto (un CFString que poseemos y soltamos).
        fn get_text(object: u32, selector: u32) -> String {
            let mut s: CFStringRef = std::ptr::null();
            if !get(object, selector, SCOPE_GLOBAL, &mut s) || s.is_null() {
                return String::new();
            }
            // SAFETY: `s` es un CFString vivo; el búfer tiene el tamaño que CF pide para UTF-8.
            unsafe {
                let cap = CFStringGetMaximumSizeForEncoding(CFStringGetLength(s), UTF8) + 1;
                let mut buf = vec![0u8; cap.max(1) as usize];
                let ok = CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, UTF8) != 0;
                CFRelease(s);
                if !ok {
                    return String::new();
                }
                let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                String::from_utf8_lossy(&buf[..end]).into_owned()
            }
        }

        /// Los canales de un dispositivo en un ámbito: la suma de los de sus búferes
        /// (`AudioBufferList`: un `u32` de cuenta y, alineados a 8, búferes de 16 octetos que
        /// empiezan por su número de canales).
        fn channels(device: u32, scope: u32) -> i64 {
            let list = get_bytes(device, STREAM_CONFIGURATION, scope);
            if list.len() < 8 {
                return 0;
            }
            let count = u32::from_ne_bytes([list[0], list[1], list[2], list[3]]) as usize;
            (0..count)
                .filter_map(|i| list.get(8 + i * 16..12 + i * 16))
                .map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]) as i64)
                .sum()
        }

        fn transport(device: u32) -> &'static str {
            let mut code = 0u32;
            get(device, TRANSPORT_TYPE, SCOPE_GLOBAL, &mut code);
            match &code.to_be_bytes() {
                b"bltn" => "builtin",
                b"usb " => "usb",
                b"blue" | b"blea" => "bluetooth",
                b"hdmi" | b"dprt" => "hdmi",
                b"airp" | b"eavb" => "network",
                b"virt" => "virtual",
                b"grup" => "aggregate",
                b"1394" | b"thun" | b"pci " => "wired",
                _ => "",
            }
        }

        pub fn devices() -> Result<Vec<super::super::DeviceInfo>, String> {
            let ids = get_bytes(SYSTEM_OBJECT, DEVICES, SCOPE_GLOBAL);
            let (mut default_in, mut default_out) = (0u32, 0u32);
            get(SYSTEM_OBJECT, DEFAULT_INPUT, SCOPE_GLOBAL, &mut default_in);
            get(SYSTEM_OBJECT, DEFAULT_OUTPUT, SCOPE_GLOBAL, &mut default_out);
            let mut out = Vec::new();
            for id in ids.chunks_exact(4).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]])) {
                let uid = get_text(id, DEVICE_UID);
                if uid.is_empty() {
                    continue;
                }
                let mut rate = 0f64;
                get(id, NOMINAL_SAMPLE_RATE, SCOPE_GLOBAL, &mut rate);
                out.push(super::super::DeviceInfo {
                    id: uid,
                    name: get_text(id, NAME),
                    input_channels: channels(id, SCOPE_INPUT),
                    output_channels: channels(id, SCOPE_OUTPUT),
                    default_input: id == default_in,
                    default_output: id == default_out,
                    sample_rate: rate as i64,
                    transport: transport(id).to_string(),
                });
            }
            Ok(out)
        }

        const BUFFER_FRAME_SIZE: u32 = fourcc(b"fsiz"); // kAudioDevicePropertyBufferFrameSize
        const BUFFER_FRAME_SIZE_RANGE: u32 = fourcc(b"fsz#"); // kAudioDevicePropertyBufferFrameSizeRange

        #[link(name = "CoreAudio", kind = "framework")]
        unsafe extern "C" {
            fn AudioObjectSetPropertyData(object: u32, address: *const PropertyAddress, qualifier_size: u32, qualifier: *const c_void, size: u32, data: *const c_void) -> i32;
        }

        /// El `AudioDeviceID` de salida de UID `uid`, o el de salida por defecto si es "".
        pub fn output_device(uid: &str) -> Option<u32> {
            device_id(uid, false)
        }

        /// El `AudioDeviceID` de UID `uid`, o el por defecto de esa dirección si es "".
        pub fn device_id(uid: &str, input: bool) -> Option<u32> {
            if uid.is_empty() {
                let mut id = 0u32;
                let selector = if input { DEFAULT_INPUT } else { DEFAULT_OUTPUT };
                return (get(SYSTEM_OBJECT, selector, SCOPE_GLOBAL, &mut id) && id != 0).then_some(id);
            }
            get_bytes(SYSTEM_OBJECT, DEVICES, SCOPE_GLOBAL)
                .chunks_exact(4)
                .map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
                .find(|&id| get_text(id, DEVICE_UID) == uid)
        }

        pub fn input_channels(device: u32) -> i64 {
            channels(device, SCOPE_INPUT)
        }

        pub fn device_transport(device: u32) -> &'static str {
            transport(device)
        }

        pub fn device_rate(device: u32) -> f64 {
            let mut rate = 0f64;
            get(device, NOMINAL_SAMPLE_RATE, SCOPE_GLOBAL, &mut rate);
            rate
        }

        /// Pide al dispositivo un búfer de E/S de `frames` (acotado al rango que admite) y
        /// devuelve el que quedó. El tamaño es por proceso: la HAL usa el menor de sus clientes.
        pub fn set_buffer_frames(device: u32, frames: u32) -> u32 {
            let mut range = [0f64; 2];
            let wanted = if get(device, BUFFER_FRAME_SIZE_RANGE, SCOPE_GLOBAL, &mut range) && range[1] >= range[0] && range[0] > 0.0 {
                (frames as f64).clamp(range[0], range[1]) as u32
            } else {
                frames
            };
            let address = PropertyAddress { selector: BUFFER_FRAME_SIZE, scope: SCOPE_GLOBAL, element: 0 };
            // SAFETY: la propiedad es un u32; el puntero es a uno nuestro del tamaño declarado.
            unsafe { AudioObjectSetPropertyData(device, &address, 0, std::ptr::null(), 4, &wanted as *const u32 as *const c_void) };
            let mut got = 0u32;
            if get(device, BUFFER_FRAME_SIZE, SCOPE_GLOBAL, &mut got) && got > 0 { got } else { wanted }
        }

        /// Un CFString con el UID, para `kAudioQueueProperty_CurrentDevice`. Hay que soltarlo.
        pub fn uid_string(uid: &str) -> CFStringRef {
            // SAFETY: el puntero y la longitud describen el `str`; CF copia los octetos.
            unsafe { CFStringCreateWithBytes(std::ptr::null(), uid.as_ptr(), uid.len() as isize, UTF8, 0) }
        }

        pub fn release(s: CFStringRef) {
            // SAFETY: `s` lo creó `uid_string`.
            unsafe { CFRelease(s) };
        }
    }

    #[cfg(target_os = "macos")]
    pub fn devices() -> Result<Vec<super::DeviceInfo>, String> {
        hal::devices()
    }

    /// M469 (ray-daw #431): el suelo de latencia de una salida Bluetooth.
    #[cfg(target_os = "macos")]
    const BLUETOOTH_MIN_LATENCY_MS: i64 = 80;

    /// M469 (ray-daw #431): el hint que de verdad se usa para la salida `device` ("" = la de
    /// por defecto). Una salida Bluetooth necesita ~80 ms encolados; con menos crepita dentro
    /// de CoreAudio sin que ninguna cuenta de aquí lo vea. `RAY_AUDIO_TRACE` avisa de la subida.
    #[cfg(target_os = "macos")]
    pub fn raise_for_transport(device: &str, latency_ms: i64) -> i64 {
        if latency_ms >= BLUETOOTH_MIN_LATENCY_MS {
            return latency_ms;
        }
        match hal::output_device(device) {
            Some(id) if hal::device_transport(id) == "bluetooth" => {
                if std::env::var_os("RAY_AUDIO_TRACE").is_some() {
                    eprintln!("[audio] coreaudio: Bluetooth output: latency hint raised from {latency_ms} to {BLUETOOTH_MIN_LATENCY_MS} ms");
                }
                BLUETOOTH_MIN_LATENCY_MS
            }
            _ => latency_ms,
        }
    }

    // ── M469 (ray-daw #424): BAJA LATENCIA — AudioUnit HAL (solo macOS) ────────────────────
    //
    // AudioQueue no baja de ~50 ms: reparte la latencia en tres búferes propios y no deja
    // tocar el del dispositivo. El AudioUnit de salida de la HAL (`kAudioUnitSubType_HALOutput`)
    // sí: el dispositivo pide audio en un callback cada búfer de E/S, y ese búfer se puede
    // bajar a 32–128 frames (0,7–2,7 ms a 48 kHz). Es el camino de un hint por debajo de 20 ms.
    //
    // El callback saca del anillo lo que haya y rellena con silencio lo que falte (un hueco,
    // contado). No hay rampas anti-clic ni silencio de mantenimiento: aquí manda la latencia, y
    // quien la pide escribe bloques pequeños a tiempo. El AudioUnit convierte el formato del
    // programa (y su frecuencia) al del dispositivo.
    #[cfg(target_os = "macos")]
    mod auhal {
        use super::super::Sink;
        use super::{AudioStreamBasicDescription, AudioTimeStamp, hal};
        use std::collections::VecDeque;
        use std::ffi::c_void;
        use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
        use std::sync::{Arc, Condvar, Mutex};

        type Unit = *mut c_void;

        #[repr(C)]
        struct ComponentDescription {
            kind: u32,
            sub_kind: u32,
            manufacturer: u32,
            flags: u32,
            flags_mask: u32,
        }
        #[repr(C)]
        struct AudioBuffer {
            channels: u32,
            byte_size: u32,
            data: *mut u8,
        }
        #[repr(C)]
        struct AudioBufferList {
            count: u32,
            buffers: [AudioBuffer; 1],
        }
        type RenderProc = extern "C" fn(*mut c_void, *mut u32, *const AudioTimeStamp, u32, u32, *mut AudioBufferList) -> i32;
        #[repr(C)]
        struct RenderCallback {
            callback: RenderProc,
            user: *mut c_void,
        }

        #[link(name = "AudioToolbox", kind = "framework")]
        unsafe extern "C" {
            fn AudioComponentFindNext(after: *mut c_void, description: *const ComponentDescription) -> *mut c_void;
            fn AudioComponentInstanceNew(component: *mut c_void, out: *mut Unit) -> i32;
            fn AudioComponentInstanceDispose(unit: Unit) -> i32;
            fn AudioUnitSetProperty(unit: Unit, id: u32, scope: u32, element: u32, data: *const c_void, size: u32) -> i32;
            fn AudioUnitInitialize(unit: Unit) -> i32;
            fn AudioUnitUninitialize(unit: Unit) -> i32;
            fn AudioOutputUnitStart(unit: Unit) -> i32;
            fn AudioOutputUnitStop(unit: Unit) -> i32;
        }
        const KIND_OUTPUT: u32 = u32::from_be_bytes(*b"auou"); // kAudioUnitType_Output
        const SUB_KIND_HAL_OUTPUT: u32 = u32::from_be_bytes(*b"ahal"); // kAudioUnitSubType_HALOutput
        const MANUFACTURER_APPLE: u32 = u32::from_be_bytes(*b"appl");
        const PROP_STREAM_FORMAT: u32 = 8; // kAudioUnitProperty_StreamFormat
        const PROP_MAX_FRAMES_PER_SLICE: u32 = 14; // kAudioUnitProperty_MaximumFramesPerSlice
        const PROP_SET_RENDER_CALLBACK: u32 = 23; // kAudioUnitProperty_SetRenderCallback
        const PROP_CURRENT_DEVICE: u32 = 2000; // kAudioOutputUnitProperty_CurrentDevice
        const SCOPE_GLOBAL: u32 = 0;
        const SCOPE_INPUT: u32 = 1;

        /// Lo que comparten el alimentador (que llena el anillo) y el callback (que lo vacía).
        struct Shared {
            ring: Mutex<VecDeque<u8>>,
            space: Condvar,
            cap: usize,
            frame_bytes: usize,
            /// Frames de audio del programa ya entregados al dispositivo.
            rendered_frames: AtomicI64,
            /// Veces que el callback encontró el anillo corto con el flujo en marcha.
            gaps: AtomicUsize,
            /// El callback anterior ya se quedó corto (para contar un hueco, no cada callback).
            starved: AtomicBool,
            /// Ya llegó audio alguna vez (el silencio previo no es un hueco).
            started: AtomicBool,
            /// La salida se está cerrando: el anillo que se vacía ya no es un hueco.
            closing: AtomicBool,
        }

        /// El callback de render, en el hilo de tiempo real de la HAL: copia del anillo y
        /// completa con silencio.
        extern "C" fn on_render(user: *mut c_void, _flags: *mut u32, _time: *const AudioTimeStamp, _bus: u32, frames: u32, io: *mut AudioBufferList) -> i32 {
            // SAFETY: `user` es el `Arc<Shared>` filtrado al abrir, vivo hasta que `finish`
            // para y desmonta el AudioUnit; `io` trae un búfer entrelazado de `frames` frames.
            unsafe {
                let shared = &*(user as *const Shared);
                let buffer = &mut (*io).buffers[0];
                let want = (frames as usize * shared.frame_bytes).min(buffer.byte_size as usize);
                let out = std::slice::from_raw_parts_mut(buffer.data, want);
                let mut ring = shared.ring.lock().unwrap_or_else(|e| e.into_inner());
                let take = ring.len().min(want) / shared.frame_bytes * shared.frame_bytes;
                let (front, back) = ring.as_slices();
                let from_front = take.min(front.len());
                out[..from_front].copy_from_slice(&front[..from_front]);
                out[from_front..take].copy_from_slice(&back[..take - from_front]);
                ring.drain(..take);
                drop(ring);
                out[take..].fill(0);
                if take > 0 {
                    shared.started.store(true, Ordering::Relaxed);
                    shared.rendered_frames.fetch_add((take / shared.frame_bytes) as i64, Ordering::Relaxed);
                    shared.space.notify_all();
                }
                if take < want && shared.started.load(Ordering::Relaxed) && !shared.closing.load(Ordering::Relaxed) {
                    if !shared.starved.swap(true, Ordering::Relaxed) {
                        shared.gaps.fetch_add(1, Ordering::Relaxed);
                    }
                } else if take == want {
                    shared.starved.store(false, Ordering::Relaxed);
                }
            }
            0
        }

        pub struct LowLatencySink {
            unit: Unit,
            user: *const Shared,
            shared: Arc<Shared>,
            played: Arc<AtomicI64>,
            device_frames: u32,
        }
        // SAFETY: el AudioUnit se crea en el hilo que abre y después solo lo toca el alimentador.
        unsafe impl Send for LowLatencySink {}

        pub fn open(
            rate: i64,
            channels: i64,
            latency_ms: i64,
            fmt: super::super::Format,
            played: Arc<AtomicI64>,
            device: &str,
        ) -> Result<Box<dyn Sink>, String> {
            let Some(device_id) = hal::output_device(device) else {
                return Err(if device.is_empty() {
                    "audio: no output device".to_string()
                } else {
                    format!("audio: no output device with id '{device}' (was it disconnected?); list them with audio.devices()")
                });
            };
            let sb = fmt.sample_bytes();
            let frame_bytes = channels as usize * sb;
            let desc = AudioStreamBasicDescription {
                sample_rate: rate as f64,
                format_id: super::FORMAT_LINEAR_PCM,
                format_flags: if fmt == super::super::Format::F32 { super::FLAG_IS_FLOAT | super::FLAG_IS_PACKED } else { super::FLAG_IS_SIGNED_INTEGER | super::FLAG_IS_PACKED },
                bytes_per_packet: frame_bytes as u32,
                frames_per_packet: 1,
                bytes_per_frame: frame_bytes as u32,
                channels_per_frame: channels as u32,
                bits_per_channel: (sb * 8) as u32,
                reserved: 0,
            };
            // El búfer de E/S del dispositivo: un cuarto del hint, redondeado hacia abajo a una
            // potencia de dos y nunca menos de 32 frames (10 ms → 64; 5 ms → 32; 19 ms → 128).
            let device_rate = match hal::device_rate(device_id) {
                r if r > 0.0 => r,
                _ => rate as f64,
            };
            let target = (device_rate * latency_ms as f64 / 4000.0) as u32;
            let device_frames = hal::set_buffer_frames(device_id, (1u32 << (31 - target.max(32).leading_zeros())).max(32));
            // El anillo: dos búferes del dispositivo (en frames del programa) y nunca menos de
            // un cuarto del hint — lo que el alimentador entrega de una vez.
            let bytes_per_sec = rate as usize * frame_bytes;
            let two_buffers = (2.0 * device_frames as f64 / device_rate * bytes_per_sec as f64) as usize;
            let cap = two_buffers.max(bytes_per_sec * latency_ms as usize / 4000).max(frame_bytes * 16) / frame_bytes * frame_bytes;
            let shared = Arc::new(Shared {
                ring: Mutex::new(VecDeque::with_capacity(cap * 2)),
                space: Condvar::new(),
                cap,
                frame_bytes,
                rendered_frames: AtomicI64::new(0),
                gaps: AtomicUsize::new(0),
                starved: AtomicBool::new(false),
                started: AtomicBool::new(false),
                closing: AtomicBool::new(false),
            });
            let user = Arc::into_raw(shared.clone());
            let description = ComponentDescription { kind: KIND_OUTPUT, sub_kind: SUB_KIND_HAL_OUTPUT, manufacturer: MANUFACTURER_APPLE, flags: 0, flags_mask: 0 };
            let mut unit: Unit = std::ptr::null_mut();
            // SAFETY: descripción y salida válidas; cada propiedad recibe un valor nuestro del
            // tamaño declarado; el callback y `user` viven hasta que `finish` desmonta la unidad.
            let step = unsafe {
                let component = AudioComponentFindNext(std::ptr::null_mut(), &description);
                if component.is_null() || AudioComponentInstanceNew(component, &mut unit) != 0 || unit.is_null() {
                    Err(("AudioComponentInstanceNew", -1))
                } else {
                    let set = |id: u32, scope: u32, data: *const c_void, size: usize| AudioUnitSetProperty(unit, id, scope, 0, data, size as u32);
                    let callback = RenderCallback { callback: on_render, user: user as *mut c_void };
                    let max_frames: u32 = 4096;
                    let steps: [(&str, i32); 6] = [
                        ("CurrentDevice", set(PROP_CURRENT_DEVICE, SCOPE_GLOBAL, &device_id as *const u32 as *const c_void, 4)),
                        ("StreamFormat", set(PROP_STREAM_FORMAT, SCOPE_INPUT, &desc as *const _ as *const c_void, std::mem::size_of::<AudioStreamBasicDescription>())),
                        ("MaximumFramesPerSlice", set(PROP_MAX_FRAMES_PER_SLICE, SCOPE_GLOBAL, &max_frames as *const u32 as *const c_void, 4)),
                        ("SetRenderCallback", set(PROP_SET_RENDER_CALLBACK, SCOPE_INPUT, &callback as *const _ as *const c_void, std::mem::size_of::<RenderCallback>())),
                        ("AudioUnitInitialize", AudioUnitInitialize(unit)),
                        ("AudioOutputUnitStart", 0),
                    ];
                    match steps.iter().find(|(_, st)| *st != 0) {
                        Some(&(name, st)) => Err((name, st)),
                        None => match AudioOutputUnitStart(unit) {
                            0 => Ok(()),
                            st => {
                                AudioUnitUninitialize(unit);
                                Err(("AudioOutputUnitStart", st))
                            }
                        },
                    }
                }
            };
            if let Err((name, st)) = step {
                // SAFETY: la unidad no llegó a arrancar; se suelta, y con ella el Arc filtrado.
                unsafe {
                    if !unit.is_null() {
                        AudioComponentInstanceDispose(unit);
                    }
                    drop(Arc::from_raw(user));
                }
                return Err(format!("audio: could not open the low-latency output ({name}, OSStatus {st})"));
            }
            Ok(Box::new(LowLatencySink { unit, user, shared, played, device_frames }))
        }

        // ── La entrada de baja latencia ───────────────────────────────────────────────────
        //
        // La cola de entrada de AudioQueue entrega en ráfagas de ~20 ms pida lo que se le
        // pida. El mismo AudioUnit de la HAL, con la E/S de entrada activada y la de salida
        // apagada, avisa en cada búfer del dispositivo: el callback pide los frames con
        // `AudioUnitRender` y los entrega al programa. No convierte la frecuencia de muestreo:
        // solo sirve si el programa pide la del dispositivo (si no, `open_input` cae a la cola).
        type InputProc = extern "C" fn(*mut c_void, *mut u32, *const AudioTimeStamp, u32, u32, *mut AudioBufferList) -> i32;
        #[repr(C)]
        struct InputCallback {
            callback: InputProc,
            user: *mut c_void,
        }
        unsafe extern "C" {
            fn AudioUnitRender(unit: Unit, flags: *mut u32, time: *const AudioTimeStamp, bus: u32, frames: u32, data: *mut AudioBufferList) -> i32;
        }
        const PROP_ENABLE_IO: u32 = 2003; // kAudioOutputUnitProperty_EnableIO
        const PROP_SET_INPUT_CALLBACK: u32 = 2005; // kAudioOutputUnitProperty_SetInputCallback
        const SCOPE_OUTPUT: u32 = 2;
        const INPUT_BUS: u32 = 1;

        struct Capture {
            unit: std::sync::atomic::AtomicUsize,
            fd_w: i32,
            ctl: Arc<super::super::InCtl>,
            closed: AtomicBool,
            /// El búfer donde `AudioUnitRender` deja cada bloque (solo lo toca el callback).
            scratch: Mutex<Vec<u8>>,
        }

        extern "C" fn on_input(user: *mut c_void, flags: *mut u32, time: *const AudioTimeStamp, _bus: u32, frames: u32, _io: *mut AudioBufferList) -> i32 {
            // SAFETY: `user` es el `Arc<Capture>` filtrado al abrir, vivo hasta que el vigilante
            // para y desmonta la unidad; el búfer es nuestro y tiene sitio para `frames`.
            unsafe {
                let capture = &*(user as *const Capture);
                if capture.closed.load(Ordering::SeqCst) {
                    return 0;
                }
                let unit = capture.unit.load(Ordering::SeqCst) as Unit;
                let bytes = frames as usize * capture.ctl.frame_bytes;
                let mut scratch = capture.scratch.lock().unwrap_or_else(|e| e.into_inner());
                if scratch.len() < bytes {
                    scratch.resize(bytes, 0);
                }
                let mut list = AudioBufferList { count: 1, buffers: [AudioBuffer { channels: 0, byte_size: bytes as u32, data: scratch.as_mut_ptr() }] };
                if AudioUnitRender(unit, flags, time, INPUT_BUS, frames, &mut list) != 0 {
                    return 0;
                }
                let got = (list.buffers[0].byte_size as usize).min(bytes) / capture.ctl.frame_bytes * capture.ctl.frame_bytes;
                if got > 0 && !super::super::push_captured(capture.fd_w, &capture.ctl, &scratch[..got]) {
                    capture.closed.store(true, Ordering::SeqCst);
                }
            }
            0
        }

        /// Abre la entrada de baja latencia. `Err` si el dispositivo no puede servirla tal
        /// cual (otra frecuencia de muestreo, otro número de canales): el llamador cae a la
        /// cola de AudioQueue.
        pub fn open_input(
            rate: i64,
            channels: i64,
            latency_ms: i64,
            fmt: super::super::Format,
            fd_w: i32,
            key: i64,
            ctl: Arc<super::super::InCtl>,
            device: &str,
        ) -> Result<(), String> {
            let device_id = hal::device_id(device, true).ok_or_else(|| "no input device".to_string())?;
            if hal::device_rate(device_id) as i64 != rate {
                return Err("the device runs at another sample rate".to_string());
            }
            // Tampoco reparte canales: un micrófono mono pedido en estéreo dejaría el segundo
            // canal en silencio, y la cola lo duplica.
            if hal::input_channels(device_id) != channels {
                return Err("the device has another channel count".to_string());
            }
            let sb = fmt.sample_bytes();
            let frame_bytes = channels as usize * sb;
            let desc = AudioStreamBasicDescription {
                sample_rate: rate as f64,
                format_id: super::FORMAT_LINEAR_PCM,
                format_flags: if fmt == super::super::Format::F32 { super::FLAG_IS_FLOAT | super::FLAG_IS_PACKED } else { super::FLAG_IS_SIGNED_INTEGER | super::FLAG_IS_PACKED },
                bytes_per_packet: frame_bytes as u32,
                frames_per_packet: 1,
                bytes_per_frame: frame_bytes as u32,
                channels_per_frame: channels as u32,
                bits_per_channel: (sb * 8) as u32,
                reserved: 0,
            };
            let target = (rate as f64 * latency_ms as f64 / 4000.0) as u32;
            hal::set_buffer_frames(device_id, (1u32 << (31 - target.max(32).leading_zeros())).max(32));
            let capture = Arc::new(Capture {
                unit: std::sync::atomic::AtomicUsize::new(0),
                fd_w,
                ctl: ctl.clone(),
                closed: AtomicBool::new(false),
                scratch: Mutex::new(vec![0u8; 4096 * frame_bytes]),
            });
            let user = Arc::into_raw(capture.clone());
            let description = ComponentDescription { kind: KIND_OUTPUT, sub_kind: SUB_KIND_HAL_OUTPUT, manufacturer: MANUFACTURER_APPLE, flags: 0, flags_mask: 0 };
            let mut unit: Unit = std::ptr::null_mut();
            // SAFETY: como en `open`: valores nuestros del tamaño declarado; el callback y
            // `user` viven hasta que el vigilante desmonta la unidad.
            let step = unsafe {
                let component = AudioComponentFindNext(std::ptr::null_mut(), &description);
                if component.is_null() || AudioComponentInstanceNew(component, &mut unit) != 0 || unit.is_null() {
                    Err(("AudioComponentInstanceNew", -1))
                } else {
                    capture.unit.store(unit as usize, Ordering::SeqCst);
                    let (on, off): (u32, u32) = (1, 0);
                    let callback = InputCallback { callback: on_input, user: user as *mut c_void };
                    let max_frames: u32 = 4096;
                    let steps: [(&str, i32); 7] = [
                        ("EnableIO input", AudioUnitSetProperty(unit, PROP_ENABLE_IO, SCOPE_INPUT, INPUT_BUS, &on as *const u32 as *const c_void, 4)),
                        ("EnableIO output", AudioUnitSetProperty(unit, PROP_ENABLE_IO, SCOPE_OUTPUT, 0, &off as *const u32 as *const c_void, 4)),
                        ("CurrentDevice", AudioUnitSetProperty(unit, PROP_CURRENT_DEVICE, SCOPE_GLOBAL, 0, &device_id as *const u32 as *const c_void, 4)),
                        ("StreamFormat", AudioUnitSetProperty(unit, PROP_STREAM_FORMAT, SCOPE_OUTPUT, INPUT_BUS, &desc as *const _ as *const c_void, std::mem::size_of::<AudioStreamBasicDescription>() as u32)),
                        ("MaximumFramesPerSlice", AudioUnitSetProperty(unit, PROP_MAX_FRAMES_PER_SLICE, SCOPE_GLOBAL, 0, &max_frames as *const u32 as *const c_void, 4)),
                        ("SetInputCallback", AudioUnitSetProperty(unit, PROP_SET_INPUT_CALLBACK, SCOPE_GLOBAL, 0, &callback as *const _ as *const c_void, std::mem::size_of::<InputCallback>() as u32)),
                        ("AudioUnitInitialize", AudioUnitInitialize(unit)),
                    ];
                    match steps.iter().find(|(_, st)| *st != 0) {
                        Some(&(name, st)) => Err((name, st)),
                        None => match AudioOutputUnitStart(unit) {
                            0 => Ok(()),
                            st => {
                                AudioUnitUninitialize(unit);
                                Err(("AudioOutputUnitStart", st))
                            }
                        },
                    }
                }
            };
            if let Err((name, st)) = step {
                // SAFETY: la unidad no llegó a arrancar; se suelta, y con ella el Arc filtrado.
                unsafe {
                    if !unit.is_null() {
                        AudioComponentInstanceDispose(unit);
                    }
                    drop(Arc::from_raw(user));
                }
                return Err(format!("{name}, OSStatus {st}"));
            }
            // El vigilante: cuando el programa cierra la entrada, para y desmonta la unidad
            // (no se puede desde el callback).
            let unit_addr = unit as usize;
            let user_addr = user as usize;
            std::thread::spawn(move || {
                while !capture.closed.load(Ordering::SeqCst) {
                    let mut addr = [0u8; 128];
                    let mut len = addr.len() as u32;
                    // SAFETY: `addr` y `len` son nuestros y describen el búfer.
                    if unsafe { super::super::getpeername(fd_w, addr.as_mut_ptr(), &mut len) } != 0 {
                        capture.closed.store(true, Ordering::SeqCst);
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                // SAFETY: parar y desinicializar esperan al callback en curso; después nadie
                // más usa `user` ni `fd_w`.
                unsafe {
                    let unit = unit_addr as Unit;
                    AudioOutputUnitStop(unit);
                    AudioUnitUninitialize(unit);
                    AudioComponentInstanceDispose(unit);
                    drop(Arc::from_raw(user_addr as *const Capture));
                    super::super::close(fd_w);
                }
                super::super::forget_in_ctl(key, &ctl);
            });
            Ok(())
        }

        impl Sink for LowLatencySink {
            fn play(&mut self, data: &[u8]) {
                let mut at = 0;
                let mut ring = self.shared.ring.lock().unwrap_or_else(|e| e.into_inner());
                while at < data.len() {
                    // Contrapresión: anillo lleno → espera a que el callback haga sitio.
                    while ring.len() >= self.shared.cap {
                        ring = self.shared.space.wait(ring).unwrap_or_else(|e| e.into_inner());
                    }
                    let n = (self.shared.cap - ring.len()).min(data.len() - at);
                    ring.extend(&data[at..at + n]);
                    at += n;
                }
                drop(ring);
                self.played.store(self.shared.rendered_frames.load(Ordering::Relaxed), Ordering::SeqCst);
            }

            fn flush(&mut self) {
                self.shared.ring.lock().unwrap_or_else(|e| e.into_inner()).clear();
                self.shared.space.notify_all();
            }

            fn finish(&mut self) {
                // Espera (acotado) a que suene lo que queda, y un búfer del dispositivo más.
                self.shared.closing.store(true, Ordering::Relaxed);
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while !self.shared.ring.lock().unwrap_or_else(|e| e.into_inner()).is_empty() && std::time::Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
                if std::env::var_os("RAY_AUDIO_TRACE").is_some() {
                    eprintln!(
                        "[audio] coreaudio low latency: device buffer {} frames, ring cap {} B, gaps {} (1 = only the tail, after the last write)",
                        self.device_frames,
                        self.shared.cap,
                        self.shared.gaps.load(Ordering::Relaxed)
                    );
                }
                // SAFETY: parar y desinicializar esperan al callback en curso; después nadie
                // más usa `user`, que se recupera y se suelta.
                unsafe {
                    AudioOutputUnitStop(self.unit);
                    AudioUnitUninitialize(self.unit);
                    AudioComponentInstanceDispose(self.unit);
                    drop(Arc::from_raw(self.user));
                }
            }
        }
    }

    /// M468: dirige la cola `q` (aún sin arrancar) al dispositivo `device`, o no hace nada si
    /// es "" (el dispositivo por defecto, que además sigue los cambios del sistema).
    fn select_device(q: AudioQueueRef, device: &str, input: bool) -> Result<(), String> {
        if device.is_empty() {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        {
            const CURRENT_DEVICE: u32 = u32::from_be_bytes(*b"aqcd"); // kAudioQueueProperty_CurrentDevice
            // La cola acepta cualquier UID y falla al arrancar con un código opaco: se comprueba
            // antes que el dispositivo existe y sirve para esta dirección.
            let direction = if input { "input" } else { "output" };
            match hal::devices()?.iter().find(|d| d.id == device) {
                None => return Err(format!("audio: no {direction} device with id '{device}' (was it disconnected?); list them with audio.devices()")),
                Some(d) if (if input { d.input_channels } else { d.output_channels }) == 0 => {
                    return Err(format!("audio: the device '{}' has no {direction} channels", d.name));
                }
                Some(_) => {}
            }
            let uid = hal::uid_string(device);
            // SAFETY: `q` es una cola viva; la propiedad es un CFStringRef (se pasa su dirección).
            let st = unsafe { AudioQueueSetProperty(q, CURRENT_DEVICE, &uid as *const _ as *const std::ffi::c_void, std::mem::size_of_val(&uid) as u32) };
            hal::release(uid);
            if st != 0 {
                return Err(format!(
                    "audio: no {} device with id '{device}' (OSStatus {st}); list them with audio.devices()",
                    if input { "input" } else { "output" }
                ));
            }
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (q, input);
            Err(format!("audio: iOS routes audio by itself: the device '{device}' cannot be chosen (open the default one)"))
        }
    }
    const PARAM_VOLUME: u32 = 1; // kAudioQueueParam_Volume
    const PARAM_VOLUME_RAMP_TIME: u32 = 4; // kAudioQueueParam_VolumeRampTime (segundos)

    /// M332: las colas vivas, para apagarlas con rampa cuando el SO va a matar el proceso
    /// (`quiesce`): en iOS cerrar la app desde el selector mata el proceso con la cola a mitad
    /// de onda — un clic que ninguna rampa sobre nuestros datos puede evitar.
    fn live_queues() -> &'static Mutex<Vec<usize>> {
        static Q: std::sync::OnceLock<Mutex<Vec<usize>>> = std::sync::OnceLock::new();
        Q.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// Baja a cero el volumen de todas las colas vivas con una rampa de CoreAudio de 30 ms y
    /// espera a que termine. Para llamar justo antes de que el proceso muera (el shell iOS en
    /// `applicationWillTerminate`). Idempotente; sin colas no hace nada.
    pub fn quiesce() {
        let queues: Vec<usize> = live_queues().lock().unwrap_or_else(|e| e.into_inner()).clone();
        if queues.is_empty() {
            return;
        }
        for q in &queues {
            // SAFETY: refs de colas vivas (se retiran del registro antes de Dispose).
            unsafe {
                AudioQueueSetParameter(*q as AudioQueueRef, PARAM_VOLUME_RAMP_TIME, 0.03);
                AudioQueueSetParameter(*q as AudioQueueRef, PARAM_VOLUME, 0.0);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(60));
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
    const FLAG_IS_FLOAT: u32 = 1 << 0;
    const FLAG_IS_SIGNED_INTEGER: u32 = 1 << 2;
    const FLAG_IS_PACKED: u32 = 1 << 3;

    /// El estado compartido con el callback: un anillo de PCM pendiente. El callback SACA; el
    /// alimentador METE (bloqueando con la condvar cuando el anillo está lleno — contrapresión).
    struct Shared {
        ring: Mutex<VecDeque<u8>>,
        space: Condvar,
        cap: usize,
        /// Octetos por frame (canales × octetos por muestra): los buffers se entregan SIEMPRE
        /// alineados a frame.
        frame_bytes: usize,
        /// M398: el formato de muestra (la rampa anti-clic escala en ese formato).
        fmt: super::Format,
        /// El silencio de mantener-viva-la-cola con el anillo seco: ~8 ms, alineado a frame.
        /// Todo lo encolado se INSERTA en la línea de tiempo — el silencio es latencia
        /// permanente, así que se encola el mínimo que evita que la cola muera (rallyx).
        keepalive_bytes: usize,
        /// M332: ¿la salida anterior terminó en silencio (anillo seco o rampa de salida)? Si
        /// sí, el siguiente bloque con datos arranca con una rampa de entrada. Solo lo toca el
        /// callback (un hilo), con el anillo bloqueado.
        after_gap: std::sync::atomic::AtomicBool,
        /// M378: contadores de diagnóstico (`RAY_AUDIO_TRACE=1` los imprime al cerrar): octetos de
        /// silencio de keepalive insertados (anillo seco) y salidas fundidas (anillo casi seco).
        silence_bytes: std::sync::atomic::AtomicUsize,
        fade_outs: std::sync::atomic::AtomicUsize,
        /// M378: transiciones dato → silencio (huecos). Un flujo limpio tiene exactamente uno:
        /// la cola final. Cada hueco a mitad de flujo es un underrun audible.
        gaps: std::sync::atomic::AtomicUsize,
        /// M400 (rayplay #171/#173): cortes de `flush` (la traza los cuenta aparte de `fade_outs`,
        /// que solo cuenta las colas fundidas por el callback al quedarse sin datos).
        flushes: std::sync::atomic::AtomicUsize,
        /// M400: octetos de cola YA FUNDIDA que el callback debe dejar pasar antes de aplicar la
        /// rampa de entrada de `after_gap`. Tras un `flush` el anillo guarda la cola atenuada y,
        /// detrás, lo nuevo que el programa escriba: la rampa de entrada va sobre lo nuevo, no
        /// sobre la cola (que ya es ~silencio). Lo pone `flush_ring` y lo consume el callback.
        fade_in_skip: std::sync::atomic::AtomicUsize,
        /// M332: fotogramas de la rampa anti-clic (10 ms). Un corte a mitad de onda — `close`,
        /// cambio de pista, o el anillo vaciándose porque iOS congeló el proceso al bloquear la
        /// pantalla — suena como un clic; atenuado a cero en unos milisegundos es un silencio
        /// limpio. El callback RETIENE siempre esta cola del anillo y solo la entrega atenuada
        /// cuando no ha llegado nada más (fin real o hueco real): vaciar el anillo en una vuelta
        /// no significa que venga silencio, y fundir ahí a ciegas sonaba como un ruido al cambiar
        /// de pista. No añade ni quita un octeto: la línea de tiempo no cambia (la regla de
        /// rallyx de arriba se mantiene).
        ramp_frames: usize,
    }

    /// Rampa lineal sobre `frames` fotogramas de `out` (en el formato `fmt`) desde el fotograma
    /// `from` (`fade_in`: de 0 a 1; si no, de 1 a 0). Todos los canales del fotograma por igual.
    fn ramp(out: &mut [u8], fmt: super::Format, frame_bytes: usize, from: usize, frames: usize, fade_in: bool) {
        let sb = fmt.sample_bytes();
        for i in 0..frames {
            let k = if fade_in { i as f32 / frames as f32 } else { 1.0 - (i + 1) as f32 / frames as f32 };
            let base = (from + i) * frame_bytes;
            let mut off = 0;
            while off + sb <= frame_bytes {
                let p = base + off;
                if p + sb > out.len() {
                    return;
                }
                super::scale_sample(&mut out[p..p + sb], fmt, k);
                off += sb;
            }
        }
    }

    #[cfg(test)]
    pub(super) fn ramp_for_tests(out: &mut [u8], frame_bytes: usize, from: usize, frames: usize, fade_in: bool) {
        ramp(out, super::Format::S16, frame_bytes, from, frames, fade_in)
    }

    /// M400: un anillo de pruebas (s16le estéreo, rampa de `ramp_frames`) sin AudioQueue, y las dos
    /// operaciones que lo tocan, para probar `flush` sobre las MUESTRAS.
    #[cfg(test)]
    pub(super) struct TestRing(Arc<Shared>);
    #[cfg(test)]
    impl TestRing {
        pub(super) fn new(ramp_frames: usize, cap_frames: usize) -> TestRing {
            TestRing(Arc::new(Shared {
                ring: Mutex::new(VecDeque::new()),
                space: Condvar::new(),
                cap: cap_frames * 4,
                frame_bytes: 4,
                fmt: super::Format::S16,
                keepalive_bytes: 64,
                after_gap: std::sync::atomic::AtomicBool::new(false),
                silence_bytes: std::sync::atomic::AtomicUsize::new(0),
                fade_outs: std::sync::atomic::AtomicUsize::new(0),
                gaps: std::sync::atomic::AtomicUsize::new(0),
                flushes: std::sync::atomic::AtomicUsize::new(0),
                fade_in_skip: std::sync::atomic::AtomicUsize::new(0),
                ramp_frames,
            }))
        }
        /// Encola `frames` fotogramas estéreo con el valor `v` en ambos canales.
        pub(super) fn push(&self, frames: usize, v: i16) {
            let mut ring = self.0.ring.lock().unwrap();
            for _ in 0..frames {
                ring.extend(v.to_le_bytes());
                ring.extend(v.to_le_bytes());
            }
        }
        pub(super) fn flush(&self) {
            flush_ring(&self.0)
        }
        /// Una vuelta del callback con un búfer de `cap_frames`: los fotogramas (canal izquierdo).
        pub(super) fn fill(&self, cap_frames: usize) -> Vec<i16> {
            let mut out = vec![0u8; cap_frames * 4];
            let n = fill(&self.0, &mut out);
            (0..n / 4).map(|i| i16::from_le_bytes([out[i * 4], out[i * 4 + 1]])).collect()
        }
        pub(super) fn queued_frames(&self) -> usize {
            self.0.ring.lock().unwrap().len() / 4
        }
        pub(super) fn counters(&self) -> (usize, usize) {
            use std::sync::atomic::Ordering::Relaxed;
            (self.0.flushes.load(Relaxed), self.0.fade_outs.load(Relaxed))
        }
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
            (*buf).audio_data_byte_size = fill(shared, out) as u32;
            AudioQueueEnqueueBuffer(q, buf, 0, std::ptr::null());
        }
    }

    /// El cuerpo del callback, sin AudioQueue (así se prueba sobre muestras): saca del anillo a
    /// `out` y devuelve los octetos escritos. byte_size = EXACTAMENTE lo tomado, alineado a
    /// frame — jamás rellenar un buffer parcial con silencio: todo octeto encolado se inserta en
    /// la línea de tiempo y un relleno es LATENCIA PERMANENTE (hallazgo de rallyx: 150 ms de
    /// cebado + 50 ms por underrun con el diseño anterior).
    fn fill(shared: &Shared, out: &mut [u8]) -> usize {
        use std::sync::atomic::Ordering::Relaxed;
        let cap = out.len();
        let mut ring = shared.ring.lock().unwrap();
        let fb = shared.frame_bytes;
        let avail = (ring.len() / fb) * fb;
        let hold = shared.ramp_frames * fb;
        // M332: si esta vuelta vaciaría el anillo, retén la cola de la rampa (salvo que ya
        // sea solo la cola: entonces es el fin o un hueco real, y sale fundida).
        let (take, fading_out) = if avail > cap {
            (cap / fb * fb, false)
        } else if avail > hold {
            (avail - hold, false)
        } else {
            (avail, avail > 0)
        };
        if take > 0 {
            for slot in out.iter_mut().take(take) {
                *slot = ring.pop_front().unwrap_or(0);
            }
            drop(ring);
            let frames = take / fb;
            if shared.after_gap.swap(false, Relaxed) {
                // M400: tras un `flush`, los primeros `skip` octetos son la cola ya fundida; la
                // rampa de entrada empieza donde empieza lo nuevo. Si esta vuelta no pasa de la
                // cola, la entrada queda pendiente para la siguiente.
                let skip = shared.fade_in_skip.swap(0, Relaxed);
                if skip >= take {
                    shared.after_gap.store(true, Relaxed);
                    shared.fade_in_skip.store(skip - take, Relaxed);
                } else {
                    let from = skip / fb;
                    ramp(out, shared.fmt, fb, from, shared.ramp_frames.min(frames - from), true);
                }
            }
            if fading_out {
                shared.fade_outs.fetch_add(1, Relaxed);
                let n = shared.ramp_frames.min(frames);
                ramp(out, shared.fmt, fb, frames - n, n, false);
                shared.after_gap.store(true, Relaxed);
                shared.fade_in_skip.store(0, Relaxed);
            }
            shared.space.notify_all();
            take
        } else {
            drop(ring);
            // Anillo seco: el MÍNIMO de silencio que mantiene viva la cola (~8 ms) — un
            // buffer sin encolar sale de la rotación y la cola muere.
            let silence = shared.keepalive_bytes.min(cap).max(fb);
            shared.silence_bytes.fetch_add(silence, Relaxed);
            for slot in out.iter_mut().take(silence) {
                *slot = 0;
            }
            if !shared.after_gap.swap(true, Relaxed) {
                shared.gaps.fetch_add(1, Relaxed);
            }
            shared.fade_in_skip.store(0, Relaxed);
            shared.space.notify_all();
            silence
        }
    }

    /// M400 (rayplay #171/#173): el corte de `flush`. Descarta el anillo salvo la cola de la rampa
    /// y la FUNDE AQUÍ, en el anillo, antes de que el callback la toque; lo siguiente que llegue
    /// arranca con rampa de entrada justo detrás (`after_gap` + `fade_in_skip`). M398 solo
    /// truncaba y confiaba en que el callback viera «solo queda la cola» y la fundiera: pero el
    /// programa escribe lo nuevo nada más volver `flush`, el callback encontraba cola + datos y
    /// cortaba en seco — un clic a cualquier frecuencia (a 88,2 kHz y más lo salvaba el azar del
    /// reparto de tiempos, y dejó de salvarlo con el scheduler de 1.27.54).
    fn flush_ring(shared: &Shared) {
        use std::sync::atomic::Ordering::Relaxed;
        let mut ring = shared.ring.lock().unwrap();
        let fb = shared.frame_bytes;
        let hold = shared.ramp_frames * fb;
        if ring.len() > hold {
            ring.truncate(hold);
        }
        let kept = ring.len() / fb * fb;
        ring.truncate(kept);
        if kept > 0 {
            let tail = ring.make_contiguous();
            ramp(tail, shared.fmt, fb, 0, kept / fb, false);
        }
        shared.fade_in_skip.store(kept, Relaxed);
        shared.after_gap.store(true, Relaxed);
        shared.flushes.fetch_add(1, Relaxed);
        drop(ring);
        shared.space.notify_all();
    }

    pub fn open(
        rate: i64,
        channels: i64,
        latency_ms: i64,
        fmt: super::Format,
        played: Arc<super::AtomicI64>,
        device: &str,
    ) -> Result<Box<dyn Sink>, String> {
        // M469: un hint por debajo de 20 ms es de baja latencia — AudioUnit HAL (solo macOS).
        #[cfg(target_os = "macos")]
        if latency_ms < super::LOW_LATENCY_BELOW_MS {
            return auhal::open(rate, channels, latency_ms, fmt, played, device);
        }
        // M398: AudioQueue acepta los cuatro formatos tal cual (enteros empaquetados o float).
        let sb = fmt.sample_bytes();
        let bytes_per_frame = (channels as usize * sb) as u32;
        let desc = AudioStreamBasicDescription {
            sample_rate: rate as f64,
            format_id: FORMAT_LINEAR_PCM,
            format_flags: if fmt == super::Format::F32 { FLAG_IS_FLOAT | FLAG_IS_PACKED } else { FLAG_IS_SIGNED_INTEGER | FLAG_IS_PACKED },
            bytes_per_packet: bytes_per_frame,
            frames_per_packet: 1,
            bytes_per_frame,
            channels_per_frame: channels as u32,
            bits_per_channel: (sb * 8) as u32,
            reserved: 0,
        };
        // M158: el anillo y los buffers se dimensionan con la latencia pedida (default 200 ms).
        // M326 (findings #103): el backend recibe la MITAD del presupuesto — el anillo ¼ y los tres
        // buffers de AudioQueue ¼ entre los tres (~1/12 cada uno) — porque el socket del programa
        // ya guarda la otra mitad (ver `open`). Suelos: 4 KiB de anillo y 1 KiB por buffer.
        let frame_bytes = bytes_per_frame as usize;
        let bytes_per_sec = rate as usize * channels as usize * sb;
        // M332 (rayplay en iPhone): en iOS la AudioQueue no perdona búferes de 10 ms con un
        // consumidor que no es de tiempo real — con la latencia por defecto se vaciaba y
        // crepitaba. Y al bloquear o desbloquear la pantalla iOS CONGELA los hilos de la app
        // unos 200 ms (medido: el `write` del programa se bloquea 130–206 ms y `played_ms`
        // salta otro tanto) mientras la cola sigue sonando: lo único que la alimenta entonces
        // es lo que ya tiene encolado DENTRO. Suelo en iOS: 4 búferes de 60 ms (240 ms
        // encolados) y un anillo de al menos 100 ms, independientes de la latencia pedida (que
        // sigue mandando por encima). En macOS sobra CPU y se respeta la petición tal cual,
        // con suelos de 5 ms de anillo y 5 ms por búfer.
        // M378 (1942 #108): los suelos van en MILISEGUNDOS, no en octetos. Los antiguos (4 KiB de
        // anillo, 1 KiB por búfer) a 22 050 Hz mono eran 93 + 70 ms: con ellos y el del socket,
        // `open_latency(…, 20)` encolaba ~130 ms, lo mismo que `…, 100`. Queda un suelo absoluto
        // mínimo (256 B) para que un búfer de AudioQueue nunca sea de un puñado de frames.
        // La rampa anti-clic (M332) se ACORTA con la pista: 10 ms con 80 ms o más, lat/8 por
        // debajo (20 → 2,5 ms; nunca menos de 2). Y el anillo nunca baja de rampa + 3 búferes:
        // el callback retiene la rampa en el anillo para poder fundir si el flujo se corta, y
        // con un anillo igual a la rampa (10 ms a pista 20, el caso del hallazgo) CADA callback
        // lo vaciaba con fundido de salida (321 fundidos en 3 s) y el siguiente entraba con
        // rampa: señal troceada y anillo seco.
        let (min_buf_ms, min_ring_ms): (usize, usize) = if cfg!(target_os = "ios") { (60, 100) } else { (5, 0) };
        let ramp_ms = (latency_ms as usize / 8).clamp(2, 10);
        let buf_ms_floor = latency_ms as usize / 12;
        let buf_size = ((bytes_per_sec * buf_ms_floor / 1000)
            .max(bytes_per_sec * min_buf_ms / 1000)
            .max(256)
            / frame_bytes
            * frame_bytes) as u32;
        let ring_floor = bytes_per_sec * ramp_ms / 1000 + 3 * buf_size as usize;
        let cap = (bytes_per_sec * latency_ms as usize / 4000)
            .max(bytes_per_sec * min_ring_ms / 1000)
            .max(ring_floor)
            / frame_bytes
            * frame_bytes;
        // ~8 ms de silencio de keepalive (el cebado son 3 → ~24 ms de retraso inicial, no 150).
        let keepalive_bytes = (bytes_per_sec / 125).max(frame_bytes) / frame_bytes * frame_bytes;
        let shared = Arc::new(Shared {
            ring: Mutex::new(VecDeque::new()),
            space: Condvar::new(),
            cap,
            frame_bytes,
            fmt,
            keepalive_bytes,
            after_gap: std::sync::atomic::AtomicBool::new(true), // la primera salida arranca suave
            silence_bytes: std::sync::atomic::AtomicUsize::new(0),
            fade_outs: std::sync::atomic::AtomicUsize::new(0),
            gaps: std::sync::atomic::AtomicUsize::new(0),
            flushes: std::sync::atomic::AtomicUsize::new(0),
            fade_in_skip: std::sync::atomic::AtomicUsize::new(0),
            ramp_frames: (rate as usize * ramp_ms / 1000).max(16),
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
        if let Err(e) = select_device(q, device, false) {
            // SAFETY: la cola no llegó a arrancar; se desmonta y se recupera el Arc filtrado.
            unsafe {
                AudioQueueDispose(q, 1);
                drop(Arc::from_raw(user as *const Shared));
            }
            return Err(e);
        }
        let buffers = if cfg!(target_os = "ios") { 4 } else { 3 };
        unsafe {
            for _ in 0..buffers {
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
        live_queues().lock().unwrap_or_else(|e| e.into_inner()).push(q as usize);
        Ok(Box::new(CoreAudioSink { q, shared, played }))
    }

    /// M463: lo que el callback de entrada necesita — a dónde escribir y si ya debe parar.
    struct InputState {
        fd_w: i32,
        ctl: Arc<super::InCtl>,
        closed: std::sync::atomic::AtomicBool,
    }

    /// El callback de la cola de entrada: entrega el búfer capturado y lo devuelve a la cola.
    /// Si el programa cerró la entrada no lo reencola: la cola se queda sin búferes y el
    /// vigilante la desmonta.
    extern "C" fn on_input(
        user: *mut std::ffi::c_void,
        q: AudioQueueRef,
        buf: AudioQueueBufferRef,
        _start: *const AudioTimeStamp,
        _packets: u32,
        _descs: *const std::ffi::c_void,
    ) {
        // SAFETY: `user` es el `Arc<InputState>` filtrado en `open_input`, vivo hasta que el
        // vigilante desmonta la cola (Dispose síncrono: ya no hay callbacks después).
        let st = unsafe { &*(user as *const InputState) };
        if st.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        // SAFETY: AudioQueue entrega un búfer válido con `audio_data_byte_size` octetos.
        let data = unsafe { std::slice::from_raw_parts((*buf).audio_data, (*buf).audio_data_byte_size as usize) };
        let whole = data.len() / st.ctl.frame_bytes * st.ctl.frame_bytes;
        if whole > 0 && !super::push_captured(st.fd_w, &st.ctl, &data[..whole]) {
            st.closed.store(true, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        // SAFETY: el búfer es de esta cola; se devuelve para la siguiente captura.
        unsafe { AudioQueueEnqueueBuffer(q, buf, 0, std::ptr::null()) };
    }

    /// Abre la cola de entrada del dispositivo por defecto y la arranca. La primera vez, macOS
    /// pide permiso de micrófono para la app que aloja el proceso (la terminal, o el `.app`);
    /// sin permiso la cola entrega silencio. En iOS hace falta además
    /// `NSMicrophoneUsageDescription` y una sesión de audio que permita grabar.
    pub fn open_input(
        rate: i64,
        channels: i64,
        latency_ms: i64,
        fmt: super::Format,
        fd_w: i32,
        key: i64,
        ctl: Arc<super::InCtl>,
        device: &str,
    ) -> Result<(), String> {
        // M469: un hint de baja latencia va por el AudioUnit de la HAL cuando el dispositivo
        // puede servirlo tal cual; si no, sigue por la cola con bloques pequeños.
        #[cfg(target_os = "macos")]
        if latency_ms < super::LOW_LATENCY_BELOW_MS {
            match auhal::open_input(rate, channels, latency_ms, fmt, fd_w, key, ctl.clone(), device) {
                Ok(()) => return Ok(()),
                Err(why) => {
                    if std::env::var_os("RAY_AUDIO_TRACE").is_some() {
                        eprintln!("[audio] coreaudio: low-latency input not available ({why}); using the queue");
                    }
                }
            }
        }
        let sb = fmt.sample_bytes();
        let bytes_per_frame = (channels as usize * sb) as u32;
        let desc = AudioStreamBasicDescription {
            sample_rate: rate as f64,
            format_id: FORMAT_LINEAR_PCM,
            format_flags: if fmt == super::Format::F32 { FLAG_IS_FLOAT | FLAG_IS_PACKED } else { FLAG_IS_SIGNED_INTEGER | FLAG_IS_PACKED },
            bytes_per_packet: bytes_per_frame,
            frames_per_packet: 1,
            bytes_per_frame,
            channels_per_frame: channels as u32,
            bits_per_channel: (sb * 8) as u32,
            reserved: 0,
        };
        // Tres búferes de un cuarto de la latencia pedida (5 ms como poco): el bloque en que
        // llega la captura. M469: con un hint de baja latencia el suelo baja a 1 ms y, en
        // macOS, se pide al dispositivo un búfer de E/S igual de pequeño (por defecto son 512
        // frames, ~11 ms, y ningún bloque llegaría antes).
        let frame_bytes = bytes_per_frame as usize;
        let bytes_per_sec = rate as usize * frame_bytes;
        let low_latency = latency_ms < super::LOW_LATENCY_BELOW_MS;
        let floor = if low_latency { bytes_per_sec / 1000 } else { bytes_per_sec / 200 };
        let buf_size = ((bytes_per_sec * latency_ms as usize / 4000).max(floor).max(frame_bytes * 16) / frame_bytes * frame_bytes) as u32;
        #[cfg(target_os = "macos")]
        if low_latency && let Some(id) = hal::device_id(device, true) {
            let target = (hal::device_rate(id).max(rate as f64) * latency_ms as f64 / 4000.0) as u32;
            hal::set_buffer_frames(id, (1u32 << (31 - target.max(32).leading_zeros())).max(32));
        }
        let state = Arc::new(InputState { fd_w, ctl: ctl.clone(), closed: std::sync::atomic::AtomicBool::new(false) });
        let user = Arc::into_raw(state.clone()) as *mut std::ffi::c_void;
        let mut q: AudioQueueRef = std::ptr::null_mut();
        // SAFETY: desc/out válidos; el callback y `user` viven hasta Dispose.
        let st = unsafe { AudioQueueNewInput(&desc, on_input, user, std::ptr::null(), std::ptr::null(), 0, &mut q) };
        if st != 0 {
            // SAFETY: recupera el Arc filtrado (la cola no llegó a existir).
            unsafe { drop(Arc::from_raw(user as *const InputState)) };
            return Err(format!("audio: could not open the input device (AudioQueueNewInput, OSStatus {st}); is there a microphone or audio interface?"));
        }
        if let Err(e) = select_device(q, device, true) {
            // SAFETY: la cola no llegó a arrancar; se desmonta y se recupera el Arc filtrado.
            unsafe {
                AudioQueueDispose(q, 1);
                drop(Arc::from_raw(user as *const InputState));
            }
            return Err(e);
        }
        // SAFETY: `q` es la cola recién creada; los búferes son suyos.
        unsafe {
            for _ in 0..3 {
                let mut b: AudioQueueBufferRef = std::ptr::null_mut();
                if AudioQueueAllocateBuffer(q, buf_size, &mut b) == 0 {
                    AudioQueueEnqueueBuffer(q, b, 0, std::ptr::null());
                }
            }
            let st = AudioQueueStart(q, std::ptr::null());
            if st != 0 {
                AudioQueueDispose(q, 1);
                drop(Arc::from_raw(user as *const InputState));
                return Err(format!("audio: could not start recording (AudioQueueStart, OSStatus {st}); check the microphone permission and, on iOS, the audio session"));
            }
        }
        // El vigilante: cuando el programa cierra la entrada (el otro extremo del par desaparece,
        // o el callback vio EPIPE), para y desmonta la cola. No se puede hacer desde el callback.
        let q_addr = q as usize;
        let user_addr = user as usize;
        std::thread::spawn(move || {
            loop {
                if state.closed.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                // ¿Sigue ahí el extremo del programa? (Un par de datagramas no avisa con POLLHUP.)
                let mut addr = [0u8; 128];
                let mut len = addr.len() as u32;
                // SAFETY: `addr` y `len` son nuestros y describen el búfer.
                if unsafe { super::getpeername(fd_w, addr.as_mut_ptr(), &mut len) } != 0 {
                    state.closed.store(true, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            // SAFETY: la cola y `user` son de esta entrada; Stop/Dispose inmediatos esperan a que
            // el callback en curso termine, así que después nadie más usa `user` ni `fd_w`.
            unsafe {
                let q = q_addr as AudioQueueRef;
                AudioQueueStop(q, 1);
                AudioQueueDispose(q, 1);
                drop(Arc::from_raw(user_addr as *const InputState));
                super::close(fd_w);
            }
            super::forget_in_ctl(key, &ctl);
        });
        Ok(())
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

        /// M398 (rayplay #166) / M400 (#171, #173): descarta el anillo salvo la cola de la rampa,
        /// que sale fundida (ver `flush_ring`); lo siguiente que llegue arranca con rampa de
        /// entrada. Los búferes ya en la cola de AudioQueue (3 × ~lat/12) terminan de sonar.
        fn flush(&mut self) {
            flush_ring(&self.shared);
        }

        fn finish(&mut self) {
            // Espera a que el anillo se vacíe y para la cola (Stop no-inmediato drena sus buffers).
            loop {
                if self.shared.ring.lock().unwrap().is_empty() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            live_queues().lock().unwrap_or_else(|e| e.into_inner()).retain(|&q| q != self.q as usize);
            if std::env::var_os("RAY_AUDIO_TRACE").is_some() {
                eprintln!(
                    "[audio] coreaudio: ring cap {} B, keepalive silence {} B (priming + tail included), fade-outs {}, flushes {}, gaps {} (1 = only the tail)",
                    self.shared.cap,
                    self.shared.silence_bytes.load(std::sync::atomic::Ordering::Relaxed),
                    self.shared.fade_outs.load(std::sync::atomic::Ordering::Relaxed),
                    self.shared.flushes.load(std::sync::atomic::Ordering::Relaxed),
                    self.shared.gaps.load(std::sync::atomic::Ordering::Relaxed)
                );
            }
            unsafe {
                AudioQueueStop(self.q, 0);
                AudioQueueDispose(self.q, 0);
            }
        }
    }

}

// ── Linux: ALSA por dlopen (sin headers de build; sin libasound → Err claro) ────
#[cfg(all(test, any(target_os = "macos", target_os = "ios")))]
mod coreaudio_tests {
    use super::coreaudio::TestRing;

    /// M400 (rayplay #171/#173): `flush` deja en el anillo SOLO la cola de la rampa, ya fundida a
    /// cero, y lo que el programa escriba después entra con rampa de subida detrás de esa cola —
    /// aunque llegue antes de que el callback dé la vuelta (el caso real: `flush` y la escritura
    /// de la nueva posición van seguidas). Antes el callback encontraba cola + datos nuevos, no
    /// fundía nada y el corte se oía como un clic.
    #[test]
    fn flush_fades_the_tail_in_the_ring_and_the_next_data_fades_in_after_it() {
        let ring = TestRing::new(16, 512);
        ring.push(400, 1000); // un «tono» constante: 400 fotogramas a 1000
        ring.flush();
        assert_eq!(ring.queued_frames(), 16, "solo queda la cola de la rampa");
        assert_eq!(ring.counters(), (1, 0), "un flush, ningún fundido del callback");
        // Lo nuevo llega ANTES de la siguiente vuelta del callback.
        ring.push(100, 1000);
        let out = ring.fill(512); // avail 116 > hold 16 → entrega 100 (retiene la cola de 16)
        assert_eq!(out.len(), 100);
        let tail = &out[..16];
        assert!(tail[0] < 1000 && tail[0] > tail[8] && tail[8] > tail[15], "cola decreciente: {tail:?}");
        assert!(tail[15].abs() <= 1000 / 16, "la cola termina ~en silencio: {}", tail[15]);
        let head = &out[16..32];
        assert_eq!(head[0], 0, "la entrada arranca desde cero justo detrás de la cola");
        assert!(head[1] > 0 && head[1] < head[8] && head[8] < head[15], "entrada creciente: {head:?}");
        assert_eq!(out[32], 1000, "tras la rampa de entrada, la señal íntegra");
        assert_eq!(ring.counters(), (1, 0));
    }

    /// M400: si el callback da una vuelta corta que no pasa de la cola fundida, la rampa de
    /// entrada queda pendiente y se aplica a lo nuevo en la vuelta siguiente; y un `flush` con el
    /// anillo casi vacío no inventa nada: funde lo que haya y lo siguiente entra en rampa.
    #[test]
    fn the_pending_fade_in_survives_a_short_callback_round_and_an_empty_flush() {
        let ring = TestRing::new(16, 512);
        ring.push(400, 1000);
        ring.flush();
        ring.push(200, 1000);
        let first = ring.fill(8); // solo 8 fotogramas: todos de la cola fundida
        assert_eq!(first.len(), 8);
        assert!(first.iter().all(|&v| v < 1000), "todo cola: {first:?}");
        let second = ring.fill(512); // 8 de cola + 184 nuevos (retiene 16)
        assert_eq!(second.len(), 192);
        assert_eq!(second[8], 0, "la entrada empieza donde empieza lo nuevo");
        assert!(second[9] > 0 && second[9] < second[20], "y sube: {:?}", &second[8..24]);
        assert_eq!(second[40], 1000);
        // Flush con menos de una rampa en el anillo (4 fotogramas).
        let small = TestRing::new(16, 512);
        small.push(4, 1000);
        small.flush();
        assert_eq!(small.queued_frames(), 4);
        small.push(50, 1000);
        let out = small.fill(512);
        assert_eq!(out.len(), 38); // 54 − 16 retenidos
        assert!(out[3] < 500, "los 4 de cola, fundidos: {:?}", &out[..4]);
        assert_eq!(out[4], 0, "entrada detrás de la cola corta");
        assert_eq!(out[30], 1000);
        assert_eq!(small.counters().0, 1);
    }
    /// M332: la rampa atenúa a cero (salida) o desde cero (entrada) sin mover un octeto, por
    /// fotograma y para todos los canales.
    #[test]
    fn the_ramp_fades_frames_in_place() {
        let frame_bytes = 4; // estéreo s16le
        let mut out = Vec::new();
        for _ in 0..8 {
            out.extend_from_slice(&1000i16.to_le_bytes());
            out.extend_from_slice(&(-1000i16).to_le_bytes());
        }
        let before = out.len();
        super::coreaudio::ramp_for_tests(&mut out, frame_bytes, 4, 4, false);
        assert_eq!(out.len(), before);
        fn l(o: &[u8], f: usize) -> i16 {
            i16::from_le_bytes([o[f * 4], o[f * 4 + 1]])
        }
        fn r(o: &[u8], f: usize) -> i16 {
            i16::from_le_bytes([o[f * 4 + 2], o[f * 4 + 3]])
        }
        assert_eq!((l(&out, 3), r(&out, 3)), (1000, -1000), "untouched before the ramp");
        let tail = [l(&out, 4), l(&out, 5), l(&out, 6), l(&out, 7)];
        assert!(tail[0] < 1000 && tail[0] > tail[1] && tail[1] > tail[2] && tail[2] > tail[3], "decreasing: {tail:?}");
        assert_eq!((l(&out, 7), r(&out, 7)), (0, 0), "ends at silence");
        super::coreaudio::ramp_for_tests(&mut out, frame_bytes, 0, 4, true);
        let head = [l(&out, 0), l(&out, 1), l(&out, 2), l(&out, 3)];
        assert_eq!(head[0], 0);
        assert!(head[0] < head[1] && head[1] < head[2] && head[2] < head[3], "increasing: {head:?}");
    }
}

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
    type FnDrop = unsafe extern "C" fn(Pcm) -> i32;

    const SND_PCM_STREAM_PLAYBACK: i32 = 0;
    const SND_PCM_FORMAT_S16_LE: i32 = 2;
    const SND_PCM_FORMAT_S32_LE: i32 = 10;
    const SND_PCM_FORMAT_FLOAT_LE: i32 = 14;
    const SND_PCM_FORMAT_S24_3LE: i32 = 32;
    const SND_PCM_ACCESS_RW_INTERLEAVED: i32 = 3;

    fn alsa_format(fmt: super::Format) -> i32 {
        match fmt {
            super::Format::S16 => SND_PCM_FORMAT_S16_LE,
            super::Format::S24 => SND_PCM_FORMAT_S24_3LE,
            super::Format::S32 => SND_PCM_FORMAT_S32_LE,
            super::Format::F32 => SND_PCM_FORMAT_FLOAT_LE,
        }
    }

    pub struct AlsaSink {
        pcm: Pcm,
        writei: FnWritei,
        recover: FnRecover,
        drain: FnDrain,
        close: FnClose,
        delay: FnDelay,
        /// M398: `snd_pcm_drop` + `snd_pcm_prepare` para `flush` (opcionales en la lib).
        drop_prepare: Option<(FnDrop, FnDrop)>,
        frame_bytes: usize,
        written_frames: i64,
        played: Arc<super::AtomicI64>,
    }
    // SAFETY: el pcm se usa solo desde el hilo alimentador.
    unsafe impl Send for AlsaSink {}

    /// El manejador de errores de alsa-lib: no imprime nada.
    unsafe extern "C" fn quiet_alsa(_file: *const u8, _line: i32, _function: *const u8, _err: i32, _fmt: *const u8) {}

    /// M466: alsa-lib escribe sus errores en stderr por su cuenta («Unknown PCM default»); aquí
    /// cada fallo ya vuelve como `Err`, así que se le pone un manejador mudo (una vez).
    fn quiet(lib: *mut std::ffi::c_void) {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            // SAFETY: el símbolo, si está, es `snd_lib_error_set_handler(handler)`; el manejador
            // es variádico en C y este ignora todos sus argumentos.
            unsafe {
                let set = dlsym(lib, b"snd_lib_error_set_handler\0".as_ptr());
                if !set.is_null() {
                    let set: unsafe extern "C" fn(unsafe extern "C" fn(*const u8, i32, *const u8, i32, *const u8)) -> i32 = std::mem::transmute(set);
                    set(quiet_alsa);
                }
            }
        });
    }

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
        fmt: super::Format,
        played: Arc<super::AtomicI64>,
        device: &str,
    ) -> Result<Box<dyn Sink>, String> {
        // SAFETY: literal NUL-terminado; dlopen es seguro de llamar.
        let lib = unsafe { dlopen(b"libasound.so.2\0".as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            return Err("audio: libasound.so.2 not found (install ALSA, e.g. libasound2)".to_string());
        }
        quiet(lib);
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
            let drop_prepare: Option<(FnDrop, FnDrop)> = match (sym(lib, b"snd_pcm_drop\0"), sym(lib, b"snd_pcm_prepare\0")) {
                (Ok(d), Ok(p)) => Some((std::mem::transmute(d), std::mem::transmute(p))),
                _ => None,
            };
            let mut pcm: Pcm = std::ptr::null_mut();
            let name = pcm_name(device);
            let st = f_open(&mut pcm, name.as_ptr(), SND_PCM_STREAM_PLAYBACK, 0);
            if st != 0 {
                return Err(if device.is_empty() {
                    format!("audio: snd_pcm_open failed ({st})")
                } else {
                    format!("audio: could not open the output device '{device}' (snd_pcm_open, {st}); list them with audio.devices()")
                });
            }
            // M158: la latencia del dispositivo sigue el hint (default 200 ms → 100 ms aquí,
            // el valor validado de rallyx; con hint explícito, la mitad del hint acotada).
            let dev_latency_us = if latency_ms == 200 {
                100_000
            } else {
                ((latency_ms * 1000 / 2).clamp(10_000, 500_000)) as u32
            };
            // M398: el formato pedido tal cual; si el dispositivo no lo acepta, s16le y se
            // convierte en el alimentador (`Convert`).
            let mut native = fmt;
            let mut st = f_params(pcm, alsa_format(fmt), SND_PCM_ACCESS_RW_INTERLEAVED, channels as u32, rate as u32, 1, dev_latency_us);
            if st != 0 && fmt != super::Format::S16 {
                native = super::Format::S16;
                st = f_params(pcm, SND_PCM_FORMAT_S16_LE, SND_PCM_ACCESS_RW_INTERLEAVED, channels as u32, rate as u32, 1, dev_latency_us);
            }
            if st != 0 {
                close(pcm);
                return Err(format!("audio: snd_pcm_set_params failed ({st})"));
            }
            let sink = Box::new(AlsaSink {
                pcm,
                writei,
                recover,
                drain,
                close,
                delay,
                drop_prepare,
                frame_bytes: channels as usize * native.sample_bytes(),
                written_frames: 0,
                played,
            });
            Ok(if native == fmt { sink } else { super::Convert::wrap(fmt, sink) })
        }
    }

    /// El nombre PCM de ALSA de `device` ("" = `default`), terminado en NUL.
    fn pcm_name(device: &str) -> Vec<u8> {
        let name = if device.is_empty() { "default" } else { device };
        name.bytes().filter(|&b| b != 0).chain(std::iter::once(0)).collect()
    }

    // ── M468 (ray-daw #424): los dispositivos de ALSA ──────────────────────────────────────
    //
    // ALSA no tiene «dispositivos» sino nombres PCM, y `snd_device_name_hint` da decenas por
    // tarjeta (`front:`, `surround51:`, `dmix:`…). Se listan los que un programa abriría: el
    // `default` (y `pipewire` / `pulse` si existen) y, por cada tarjeta, su `plughw:` — el
    // hardware directo con conversión de formato. El id es ese nombre PCM.
    type FnNameHint = unsafe extern "C" fn(i32, *const u8, *mut *mut *mut std::ffi::c_void) -> i32;
    type FnGetHint = unsafe extern "C" fn(*const std::ffi::c_void, *const u8) -> *mut std::ffi::c_char;
    type FnFreeHint = unsafe extern "C" fn(*mut *mut std::ffi::c_void) -> i32;
    type FnParamsMalloc = unsafe extern "C" fn(*mut *mut std::ffi::c_void) -> i32;
    type FnParamsFree = unsafe extern "C" fn(*mut std::ffi::c_void);
    type FnParamsAny = unsafe extern "C" fn(Pcm, *mut std::ffi::c_void) -> i32;
    type FnParamsChannelsMax = unsafe extern "C" fn(*const std::ffi::c_void, *mut u32) -> i32;
    const SND_PCM_NONBLOCK: i32 = 1;

    unsafe extern "C" {
        fn free(p: *mut std::ffi::c_void);
    }

    /// Una pista de `snd_device_name_hint` como texto (alsa-lib la reserva con malloc).
    fn hint_text(get: FnGetHint, hint: *const std::ffi::c_void, id: &[u8]) -> Option<String> {
        // SAFETY: `hint` es una entrada viva de la lista; `id` termina en NUL; la cadena
        // devuelta es nuestra y se libera con `free`.
        unsafe {
            let p = get(hint, id.as_ptr());
            if p.is_null() {
                return None;
            }
            let text = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
            free(p as *mut std::ffi::c_void);
            Some(text)
        }
    }

    /// Cómo está conectada la tarjeta de un nombre `…:CARD=<id>,…`: el bus de su dispositivo
    /// en sysfs.
    fn card_transport(name: &str) -> &'static str {
        let Some(card) = name.split_once("CARD=").map(|(_, rest)| rest.split(',').next().unwrap_or("")) else {
            return "";
        };
        // La tarjeta cuyo `id` de sysfs es ese: `/sys/class/sound/cardN/id`.
        let Some(dir) = std::fs::read_dir("/sys/class/sound").ok().and_then(|entries| {
            entries.flatten().map(|e| e.path()).find(|p| std::fs::read_to_string(p.join("id")).is_ok_and(|id| id.trim() == card))
        }) else {
            return "";
        };
        let Ok(bus) = std::fs::read_link(dir.join("device/subsystem")) else {
            return "";
        };
        match bus.file_name().and_then(|n| n.to_str()) {
            Some("usb") => "usb",
            Some("pci") | Some("platform") => "builtin",
            Some("hdmi") => "hdmi",
            Some("bluetooth") => "bluetooth",
            _ => "",
        }
    }

    pub fn devices() -> Result<Vec<super::DeviceInfo>, String> {
        // SAFETY: literal NUL-terminado; dlopen es seguro de llamar.
        let lib = unsafe { dlopen(b"libasound.so.2\0".as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            return Ok(Vec::new()); // sin ALSA no hay dispositivos que listar
        }
        quiet(lib);
        let mut out = Vec::new();
        // SAFETY de los transmutes: las firmas replican el header de ALSA (API estable).
        unsafe {
            let name_hint: FnNameHint = std::mem::transmute(sym(lib, b"snd_device_name_hint\0")?);
            let get_hint: FnGetHint = std::mem::transmute(sym(lib, b"snd_device_name_get_hint\0")?);
            let free_hint: FnFreeHint = std::mem::transmute(sym(lib, b"snd_device_name_free_hint\0")?);
            let f_open: FnOpen = std::mem::transmute(sym(lib, b"snd_pcm_open\0")?);
            let f_close: FnClose = std::mem::transmute(sym(lib, b"snd_pcm_close\0")?);
            let params_malloc: FnParamsMalloc = std::mem::transmute(sym(lib, b"snd_pcm_hw_params_malloc\0")?);
            let params_free: FnParamsFree = std::mem::transmute(sym(lib, b"snd_pcm_hw_params_free\0")?);
            let params_any: FnParamsAny = std::mem::transmute(sym(lib, b"snd_pcm_hw_params_any\0")?);
            let channels_max: FnParamsChannelsMax = std::mem::transmute(sym(lib, b"snd_pcm_hw_params_get_channels_max\0")?);
            // Los canales del hardware en una dirección: se abre `hw:` sin bloquear y se
            // pregunta. Si otro programa lo tiene cogido no se puede saber: estéreo. Si no se
            // puede abrir por otra causa, la tarjeta no tiene esa dirección.
            let probe = |plug_name: &str, stream: i32| -> i64 {
                let hw = pcm_name(plug_name.strip_prefix("plug").unwrap_or(plug_name));
                let mut pcm: Pcm = std::ptr::null_mut();
                match f_open(&mut pcm, hw.as_ptr(), stream, SND_PCM_NONBLOCK) {
                    0 => {}
                    -16 => return 2, // EBUSY: existe, pero lo tiene otro programa
                    _ => return 0,   // la tarjeta no tiene esta dirección
                }
                let mut params: *mut std::ffi::c_void = std::ptr::null_mut();
                let mut max = 0u32;
                if params_malloc(&mut params) == 0 {
                    if params_any(pcm, params) >= 0 {
                        channels_max(params, &mut max);
                    }
                    params_free(params);
                }
                f_close(pcm);
                if max == 0 { 2 } else { max.min(64) as i64 }
            };
            let mut hints: *mut *mut std::ffi::c_void = std::ptr::null_mut();
            if name_hint(-1, b"pcm\0".as_ptr(), &mut hints) != 0 || hints.is_null() {
                return Ok(out);
            }
            let mut at = hints;
            while !(*at).is_null() {
                let hint = *at as *const std::ffi::c_void;
                at = at.add(1);
                let Some(name) = hint_text(get_hint, hint, b"NAME\0") else { continue };
                let card = name.starts_with("plughw:");
                if !card && !matches!(name.as_str(), "default" | "pipewire" | "pulse") {
                    continue;
                }
                // IOID: «Input», «Output», o ausente si sirve para las dos direcciones.
                let io = hint_text(get_hint, hint, b"IOID\0");
                let (input, output) = (io.as_deref() != Some("Output"), io.as_deref() != Some("Input"));
                let desc = hint_text(get_hint, hint, b"DESC\0").unwrap_or_default();
                let label = desc.lines().next().unwrap_or("").trim();
                let is_default = name == "default";
                out.push(super::DeviceInfo {
                    name: if label.is_empty() { name.clone() } else { label.to_string() },
                    input_channels: if !input { 0 } else if card { probe(&name, SND_PCM_STREAM_CAPTURE) } else { 2 },
                    output_channels: if !output { 0 } else if card { probe(&name, SND_PCM_STREAM_PLAYBACK) } else { 2 },
                    default_input: is_default && input,
                    default_output: is_default && output,
                    sample_rate: 0,
                    transport: card_transport(&name).to_string(),
                    id: name,
                });
            }
            free_hint(hints);
        }
        // En un ALSA sin servidor de sonido `default` no trae descripción y no sale en las
        // pistas, pero existe siempre que haya una tarjeta: es lo que abre `open`.
        if !out.is_empty() && !out.iter().any(|d| d.id == "default") {
            out.insert(
                0,
                super::DeviceInfo {
                    id: "default".to_string(),
                    name: "Default".to_string(),
                    input_channels: if out.iter().any(|d| d.input_channels > 0) { 2 } else { 0 },
                    output_channels: if out.iter().any(|d| d.output_channels > 0) { 2 } else { 0 },
                    default_input: true,
                    default_output: true,
                    ..Default::default()
                },
            );
        }
        Ok(out)
    }

    const SND_PCM_STREAM_CAPTURE: i32 = 1;
    type FnReadi = unsafe extern "C" fn(Pcm, *mut u8, u64) -> i64;

    /// M465: abre el dispositivo de CAPTURA por defecto y lanza el hilo que lo lee en bloques de
    /// un cuarto de la latencia pedida y los entrega al programa. Si el dispositivo no acepta el
    /// formato pedido, captura en s16le y convierte (`from_s16le`). El hilo termina cuando el
    /// programa cierra la entrada (la escritura en el par falla) o el dispositivo deja de dar.
    pub fn open_input(
        rate: i64,
        channels: i64,
        latency_ms: i64,
        fmt: super::Format,
        fd_w: i32,
        key: i64,
        ctl: Arc<super::InCtl>,
        device: &str,
    ) -> Result<(), String> {
        // SAFETY: literal NUL-terminado; dlopen es seguro de llamar.
        let lib = unsafe { dlopen(b"libasound.so.2\0".as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            return Err("audio: libasound.so.2 not found (install ALSA, e.g. libasound2)".to_string());
        }
        quiet(lib);
        // SAFETY de los transmutes: las firmas replican el header de ALSA (API estable).
        let (pcm, native, readi, recover, close, start) = unsafe {
            let f_open: FnOpen = std::mem::transmute(sym(lib, b"snd_pcm_open\0")?);
            let f_params: FnSetParams = std::mem::transmute(sym(lib, b"snd_pcm_set_params\0")?);
            let readi: FnReadi = std::mem::transmute(sym(lib, b"snd_pcm_readi\0")?);
            let start: FnDrain = std::mem::transmute(sym(lib, b"snd_pcm_start\0")?);
            let recover: FnRecover = std::mem::transmute(sym(lib, b"snd_pcm_recover\0")?);
            let close: FnClose = std::mem::transmute(sym(lib, b"snd_pcm_close\0")?);
            let mut pcm: Pcm = std::ptr::null_mut();
            let name = pcm_name(device);
            let st = f_open(&mut pcm, name.as_ptr(), SND_PCM_STREAM_CAPTURE, 0);
            if st != 0 {
                return Err(if device.is_empty() {
                    format!("audio: could not open the input device (snd_pcm_open, {st}); is there a microphone or capture device?")
                } else {
                    format!("audio: could not open the input device '{device}' (snd_pcm_open, {st}); list them with audio.devices()")
                });
            }
            // El búfer del dispositivo: la latencia pedida entera (los bloques son un cuarto).
            let dev_latency_us = (latency_ms * 1000).clamp(20_000, 1_000_000) as u32;
            let mut native = fmt;
            let mut st = f_params(pcm, alsa_format(fmt), SND_PCM_ACCESS_RW_INTERLEAVED, channels as u32, rate as u32, 1, dev_latency_us);
            if st != 0 && fmt != super::Format::S16 {
                native = super::Format::S16;
                st = f_params(pcm, SND_PCM_FORMAT_S16_LE, SND_PCM_ACCESS_RW_INTERLEAVED, channels as u32, rate as u32, 1, dev_latency_us);
            }
            if st != 0 {
                close(pcm);
                return Err(format!("audio: the input device does not accept {rate} Hz, {channels} channel(s) (snd_pcm_set_params, {st})"));
            }
            // La captura no arranca sola: `set_params` deja el umbral de arranque en el búfer
            // entero y una lectura de un bloque (un cuarto) no lo alcanza en un `hw:` directo.
            start(pcm);
            (pcm as usize, native, readi, recover, close, start)
        };
        std::thread::spawn(move || {
            let pcm = pcm as Pcm;
            let native_frame = channels as usize * native.sample_bytes();
            let block_frames = ((rate * latency_ms / 4000).max(64)) as usize;
            let mut buf = vec![0u8; block_frames * native_frame];
            let mut converted = Vec::new();
            loop {
                // SAFETY: `buf` tiene sitio para `block_frames`; readi BLOQUEA hasta tenerlos.
                let n = unsafe { readi(pcm, buf.as_mut_ptr(), block_frames as u64) };
                if n < 0 {
                    // Desbordamiento (el hilo llegó tarde) u otro tropiezo: recover, volver a
                    // arrancar y seguir.
                    // SAFETY: pcm válido hasta el close de abajo.
                    if unsafe { recover(pcm, n as i32, 1) } < 0 {
                        if std::env::var_os("RAY_AUDIO_TRACE").is_some() {
                            eprintln!("audio input: the device stopped (snd_pcm_readi, {n})");
                        }
                        break;
                    }
                    // SAFETY: ídem.
                    unsafe { start(pcm) };
                    continue;
                }
                let got = &buf[..n as usize * native_frame];
                let data = if native == fmt {
                    got
                } else {
                    converted.clear();
                    super::from_s16le(fmt, got, &mut converted);
                    &converted[..]
                };
                if !super::push_captured(fd_w, &ctl, data) {
                    break;
                }
            }
            // SAFETY: el pcm y `fd_w` son de este hilo desde que arrancó.
            unsafe {
                close(pcm);
                super::close(fd_w);
            }
            super::forget_in_ctl(key, &ctl);
        });
        Ok(())
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

        /// M398: `snd_pcm_drop` vacía el búfer del dispositivo; `prepare` lo deja listo para seguir.
        fn flush(&mut self) {
            if let Some((drop, prepare)) = self.drop_prepare {
                // SAFETY: pcm válido; las dos funciones vienen de la lib cargada.
                unsafe {
                    drop(self.pcm);
                    prepare(self.pcm);
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
    const AAUDIO_FORMAT_PCM_FLOAT: i32 = 2;
    const AAUDIO_FORMAT_PCM_I24_PACKED: i32 = 3; // API 31+
    const AAUDIO_FORMAT_PCM_I32: i32 = 4; // API 31+
    const AAUDIO_PERFORMANCE_MODE_NONE: i32 = 10;

    fn aaudio_format(fmt: super::Format) -> i32 {
        match fmt {
            super::Format::S16 => AAUDIO_FORMAT_PCM_I16,
            super::Format::S24 => AAUDIO_FORMAT_PCM_I24_PACKED,
            super::Format::S32 => AAUDIO_FORMAT_PCM_I32,
            super::Format::F32 => AAUDIO_FORMAT_PCM_FLOAT,
        }
    }
    const AAUDIO_PERFORMANCE_MODE_LOW_LATENCY: i32 = 12;

    pub struct AAudioSink {
        stream: Stream,
        write: FnWrite,
        frames_read: FnFramesRead,
        request_stop: FnRequest,
        close: FnClose,
        /// M398: pause + flush + start para `flush` (opcionales en la lib).
        pause_flush_start: Option<(FnRequest, FnRequest, FnRequest)>,
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
        fmt: super::Format,
        played: Arc<super::AtomicI64>,
        device: &str,
    ) -> Result<Box<dyn Sink>, String> {
        let device = device_id(device, false)?;
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
            let set_device: FnSetI32 = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_setDeviceId\0")?);
            let open_stream: FnOpenStream = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_openStream\0")?);
            let builder_delete: FnBuilderDelete = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_delete\0")?);
            let request_start: FnRequest = std::mem::transmute(sym(lib, b"AAudioStream_requestStart\0")?);
            let request_stop: FnRequest = std::mem::transmute(sym(lib, b"AAudioStream_requestStop\0")?);
            let write: FnWrite = std::mem::transmute(sym(lib, b"AAudioStream_write\0")?);
            let frames_read: FnFramesRead = std::mem::transmute(sym(lib, b"AAudioStream_getFramesRead\0")?);
            let close: FnClose = std::mem::transmute(sym(lib, b"AAudioStream_close\0")?);
            let pause_flush_start: Option<(FnRequest, FnRequest, FnRequest)> =
                match (sym(lib, b"AAudioStream_requestPause\0"), sym(lib, b"AAudioStream_requestFlush\0")) {
                    (Ok(p), Ok(f)) => Some((std::mem::transmute(p), std::mem::transmute(f), request_start)),
                    _ => None,
                };

            // M398: el formato pedido; si el stream no abre con él (I24/I32 piden API 31), s16le y
            // conversión en el alimentador (`Convert`).
            let mut native = fmt;
            let mut stream: Stream = std::ptr::null_mut();
            let mut st;
            loop {
                let mut builder: Builder = std::ptr::null_mut();
                st = create(&mut builder);
                if st != 0 {
                    return Err(format!("audio: AAudio_createStreamBuilder failed ({st})"));
                }
                if let Some(id) = device {
                    set_device(builder, id);
                }
                set_rate(builder, rate as i32);
                set_channels(builder, channels as i32);
                set_format(builder, aaudio_format(native));
                // M158: el hint decide el modo — por debajo de 50 ms se pide LOW_LATENCY.
                set_perf(
                    builder,
                    if latency_ms <= 50 { AAUDIO_PERFORMANCE_MODE_LOW_LATENCY } else { AAUDIO_PERFORMANCE_MODE_NONE },
                );
                st = open_stream(builder, &mut stream);
                builder_delete(builder);
                if st == 0 || native == super::Format::S16 {
                    break;
                }
                native = super::Format::S16;
            }
            if st != 0 {
                return Err(format!("audio: AAudioStreamBuilder_openStream failed ({st})"));
            }
            let st = request_start(stream);
            if st != 0 {
                close(stream);
                return Err(format!("audio: AAudioStream_requestStart failed ({st})"));
            }
            let sink = Box::new(AAudioSink {
                stream,
                write,
                frames_read,
                request_stop,
                close,
                pause_flush_start,
                frame_bytes: channels as usize * native.sample_bytes(),
                played,
            });
            Ok(if native == fmt { sink } else { super::Convert::wrap(fmt, sink) })
        }
    }

    /// M468 (ray-daw #424): los dispositivos de audio de Android. La lista es de `AudioManager`,
    /// en Java: la da el shell (`audio_devices`), ya en el formato de `devices_text`. Sin shell,
    /// vacía. El id es el número que acepta `AAudioStreamBuilder_setDeviceId`.
    pub fn devices() -> Result<Vec<super::DeviceInfo>, String> {
        #[cfg(feature = "ui")]
        {
            let text = crate::ui::shell_request("audio_devices", "")?;
            Ok(text
                .lines()
                .filter_map(|line| {
                    let f: Vec<&str> = line.split('\t').collect();
                    if f.len() < 8 {
                        return None;
                    }
                    Some(super::DeviceInfo {
                        id: f[0].to_string(),
                        name: f[1].to_string(),
                        input_channels: f[2].parse().unwrap_or(0),
                        output_channels: f[3].parse().unwrap_or(0),
                        default_input: f[4] == "1",
                        default_output: f[5] == "1",
                        sample_rate: f[6].parse().unwrap_or(0),
                        transport: f[7].to_string(),
                    })
                })
                .collect())
        }
        #[cfg(not(feature = "ui"))]
        {
            Ok(Vec::new())
        }
    }

    /// El id numérico de `device` para AAudio; `None` = el dispositivo por defecto.
    fn device_id(device: &str, input: bool) -> Result<Option<i32>, String> {
        if device.is_empty() {
            return Ok(None);
        }
        device.parse::<i32>().map(Some).map_err(|_| {
            format!("audio: no {} device with id '{device}'; list them with audio.devices()", if input { "input" } else { "output" })
        })
    }

    const AAUDIO_DIRECTION_INPUT: i32 = 1;
    type FnRead = unsafe extern "C" fn(Stream, *mut std::ffi::c_void, i32, i64) -> i32;

    /// M465: el permiso de micrófono. `RECORD_AUDIO` es un permiso «peligroso»: además de
    /// declararse en el manifest (`[android] record_audio = true`) el usuario lo concede en un
    /// diálogo, que el shell abre la primera vez. Bloquea hasta la respuesta.
    fn microphone_permission() -> Result<(), String> {
        #[cfg(feature = "ui")]
        {
            match crate::ui::shell_request("audio_permission", "") {
                Ok(v) if v == "granted" => Ok(()),
                Ok(v) if v == "undeclared" => Err("audio: the app does not declare the microphone permission (add `record_audio = true` to [android] in ray.toml and rebuild the bundle)".to_string()),
                Ok(_) => Err("audio: the user denied the microphone permission".to_string()),
                Err(e) => Err(format!("audio: could not ask for the microphone permission: {e}")),
            }
        }
        #[cfg(not(feature = "ui"))]
        {
            Ok(()) // sin shell no hay a quién pedirlo: el stream dirá si se puede
        }
    }

    /// M465: abre un stream de ENTRADA y lanza el hilo que lo lee en bloques de un cuarto de la
    /// latencia pedida. Como en la salida, si el stream no abre con el formato pedido (I24/I32
    /// piden API 31) captura en s16le y convierte.
    pub fn open_input(
        rate: i64,
        channels: i64,
        latency_ms: i64,
        fmt: super::Format,
        fd_w: i32,
        key: i64,
        ctl: Arc<super::InCtl>,
        device: &str,
    ) -> Result<(), String> {
        let device = device_id(device, true)?;
        microphone_permission()?;
        // SAFETY: dlopen/dlsym con literales; las firmas replican aaudio/AAudio.h (NDK).
        let (stream, native, read, request_stop, close) = unsafe {
            let lib = dlopen(b"libaaudio.so\0".as_ptr(), RTLD_NOW);
            if lib.is_null() {
                return Err("audio: libaaudio.so not found (AAudio needs Android 8.0+)".to_string());
            }
            let create: FnCreateBuilder = std::mem::transmute(sym(lib, b"AAudio_createStreamBuilder\0")?);
            let set_rate: FnSetI32 = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_setSampleRate\0")?);
            let set_channels: FnSetI32 = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_setChannelCount\0")?);
            let set_format: FnSetI32 = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_setFormat\0")?);
            let set_direction: FnSetI32 = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_setDirection\0")?);
            let set_perf: FnSetI32 = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_setPerformanceMode\0")?);
            let set_device: FnSetI32 = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_setDeviceId\0")?);
            let open_stream: FnOpenStream = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_openStream\0")?);
            let builder_delete: FnBuilderDelete = std::mem::transmute(sym(lib, b"AAudioStreamBuilder_delete\0")?);
            let request_start: FnRequest = std::mem::transmute(sym(lib, b"AAudioStream_requestStart\0")?);
            let request_stop: FnRequest = std::mem::transmute(sym(lib, b"AAudioStream_requestStop\0")?);
            let read: FnRead = std::mem::transmute(sym(lib, b"AAudioStream_read\0")?);
            let close: FnClose = std::mem::transmute(sym(lib, b"AAudioStream_close\0")?);
            let mut native = fmt;
            let mut stream: Stream = std::ptr::null_mut();
            let mut st;
            loop {
                let mut builder: Builder = std::ptr::null_mut();
                st = create(&mut builder);
                if st != 0 {
                    return Err(format!("audio: AAudio_createStreamBuilder failed ({st})"));
                }
                set_direction(builder, AAUDIO_DIRECTION_INPUT);
                if let Some(id) = device {
                    set_device(builder, id);
                }
                set_rate(builder, rate as i32);
                set_channels(builder, channels as i32);
                set_format(builder, aaudio_format(native));
                set_perf(
                    builder,
                    if latency_ms <= 50 { AAUDIO_PERFORMANCE_MODE_LOW_LATENCY } else { AAUDIO_PERFORMANCE_MODE_NONE },
                );
                st = open_stream(builder, &mut stream);
                builder_delete(builder);
                if st == 0 || native == super::Format::S16 {
                    break;
                }
                native = super::Format::S16;
            }
            if st != 0 {
                return Err(format!("audio: could not open the input device (AAudioStreamBuilder_openStream, {st}); is the microphone permission granted?"));
            }
            let st = request_start(stream);
            if st != 0 {
                close(stream);
                return Err(format!("audio: could not start recording (AAudioStream_requestStart, {st})"));
            }
            (stream as usize, native, read, request_stop, close)
        };
        std::thread::spawn(move || {
            let stream = stream as Stream;
            let native_frame = channels as usize * native.sample_bytes();
            let block_frames = ((rate * latency_ms / 4000).max(64)) as usize;
            let mut buf = vec![0u8; block_frames * native_frame];
            let mut converted = Vec::new();
            loop {
                // SAFETY: `buf` tiene sitio para `block_frames`; read espera hasta 1 s por ellos.
                let n = unsafe { read(stream, buf.as_mut_ptr() as *mut std::ffi::c_void, block_frames as i32, 1_000_000_000) };
                if n < 0 {
                    break; // el stream se desconectó (cambio de dispositivo) o falló
                }
                if n == 0 {
                    continue;
                }
                let got = &buf[..n as usize * native_frame];
                let data = if native == fmt {
                    got
                } else {
                    converted.clear();
                    super::from_s16le(fmt, got, &mut converted);
                    &converted[..]
                };
                if !super::push_captured(fd_w, &ctl, data) {
                    break;
                }
            }
            // SAFETY: el stream y `fd_w` son de este hilo desde que arrancó.
            unsafe {
                request_stop(stream);
                close(stream);
                super::close(fd_w);
            }
            super::forget_in_ctl(key, &ctl);
        });
        Ok(())
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

        /// M398: AAudio solo vacía en pausa: pause → flush → start.
        fn flush(&mut self) {
            if let Some((pause, flush, start)) = self.pause_flush_start {
                // SAFETY: stream válido; las funciones vienen de la lib cargada.
                unsafe {
                    pause(self.stream);
                    flush(self.stream);
                    start(self.stream);
                }
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
    const E_CAPTURE: u32 = 1; // EDataFlow::eCapture
    const E_CONSOLE: u32 = 0; // ERole::eConsole
    const AUDCLNT_SHAREMODE_SHARED: u32 = 0;
    const AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM: u32 = 0x8000_0000;
    const AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
    const WAVE_FORMAT_PCM: u16 = 1;

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: Ptr, coinit: u32) -> i32;
        fn CoCreateInstance(clsid: *const Guid, outer: Ptr, ctx: u32, iid: *const Guid, out: *mut Ptr) -> i32;
        fn CoTaskMemFree(p: Ptr);
        fn PropVariantClear(value: *mut PropVariant) -> i32;
    }

    // ── M468 (ray-daw #424): los endpoints de WASAPI ───────────────────────────────────────
    //
    // Un endpoint es de salida o de entrada (nunca los dos), y su id es una cadena estable
    // («{0.0.0.00000000}.{guid}»). El nombre y el bus salen de su almacén de propiedades; los
    // canales y la frecuencia, del formato de mezcla del motor.
    #[repr(C)]
    struct PropertyKey {
        fmtid: Guid,
        pid: u32,
    }
    /// `PROPVARIANT` (24 octetos en 64 bits): el tipo y, para `VT_LPWSTR`, un puntero.
    #[repr(C)]
    struct PropVariant {
        vt: u16,
        reserved: [u16; 3],
        pointer: *mut u16,
        padding: usize,
    }
    const VT_LPWSTR: u16 = 31;
    const DEVPKEY_GUID: Guid = Guid { d1: 0xA45C_254E, d2: 0xDF1C, d3: 0x4EFD, d4: [0x80, 0x20, 0x67, 0xD1, 0x46, 0xA8, 0x50, 0xE0] };
    const PKEY_FRIENDLY_NAME: PropertyKey = PropertyKey { fmtid: DEVPKEY_GUID, pid: 14 };
    const PKEY_ENUMERATOR_NAME: PropertyKey = PropertyKey { fmtid: DEVPKEY_GUID, pid: 24 };
    const DEVICE_STATE_ACTIVE: u32 = 1;

    #[repr(C)]
    struct IMMDeviceCollectionVtbl {
        base: IUnknownVtbl,
        get_count: unsafe extern "system" fn(Ptr, *mut u32) -> Hr,
        item: unsafe extern "system" fn(Ptr, u32, *mut Ptr) -> Hr,
    }
    #[repr(C)]
    struct IPropertyStoreVtbl {
        base: IUnknownVtbl,
        get_count: usize,
        get_at: usize,
        get_value: unsafe extern "system" fn(Ptr, *const PropertyKey, *mut PropVariant) -> Hr,
        set_value: usize,
        commit: usize,
    }
    type FnEnumEndpoints = unsafe extern "system" fn(Ptr, u32, u32, *mut Ptr) -> Hr;
    type FnGetDevice = unsafe extern "system" fn(Ptr, *const u16, *mut Ptr) -> Hr;
    type FnOpenPropertyStore = unsafe extern "system" fn(Ptr, u32, *mut Ptr) -> Hr;
    type FnGetId = unsafe extern "system" fn(Ptr, *mut *mut u16) -> Hr;
    type FnGetMixFormat = unsafe extern "system" fn(Ptr, *mut *mut WaveFormatEx) -> Hr;

    /// Una cadena ancha terminada en NUL, copiada.
    unsafe fn wide_text(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        // SAFETY (del llamador): `p` apunta a una cadena ancha terminada en NUL.
        unsafe {
            let mut len = 0;
            while *p.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
        }
    }

    fn enumerator() -> Result<Ptr, String> {
        com_init();
        let mut enumerator: Ptr = std::ptr::null_mut();
        // SAFETY: CoCreateInstance escribe un puntero COM si devuelve S_OK.
        let hr = unsafe { CoCreateInstance(&CLSID_MM_DEVICE_ENUMERATOR, std::ptr::null_mut(), CLSCTX_ALL, &IID_IMM_DEVICE_ENUMERATOR, &mut enumerator) };
        if hr < 0 || enumerator.is_null() {
            return Err(format!("audio: could not create the WASAPI device enumerator (0x{:08X})", hr as u32));
        }
        Ok(enumerator)
    }

    /// El id de un endpoint (la cadena la reserva COM y se libera aquí).
    fn endpoint_id(device: Ptr) -> String {
        let mut id: *mut u16 = std::ptr::null_mut();
        // SAFETY: `GetId` escribe una cadena reservada con CoTaskMemAlloc.
        unsafe {
            let get_id: FnGetId = std::mem::transmute(vtbl::<IMMDeviceVtbl>(device).get_id);
            if get_id(device, &mut id) < 0 {
                return String::new();
            }
            let text = wide_text(id);
            CoTaskMemFree(id as Ptr);
            text
        }
    }

    fn property_text(store: Ptr, key: &PropertyKey) -> String {
        let mut value = PropVariant { vt: 0, reserved: [0; 3], pointer: std::ptr::null_mut(), padding: 0 };
        // SAFETY: `GetValue` rellena el PROPVARIANT; `PropVariantClear` libera lo que reservó.
        unsafe {
            if (vtbl::<IPropertyStoreVtbl>(store).get_value)(store, key, &mut value) < 0 {
                return String::new();
            }
            let text = if value.vt == VT_LPWSTR { wide_text(value.pointer) } else { String::new() };
            PropVariantClear(&mut value);
            text
        }
    }

    /// El endpoint por defecto de un flujo (`E_RENDER` / `E_CAPTURE`), o el de id `device`.
    fn endpoint(enumerator: Ptr, flow: u32, device: &str) -> Result<Ptr, String> {
        let direction = if flow == E_CAPTURE { "input" } else { "output" };
        let mut found: Ptr = std::ptr::null_mut();
        if device.is_empty() {
            // SAFETY: método del vtable transcrito sobre un objeto vivo; escribe `found`.
            let hr = unsafe { (vtbl::<IMMDeviceEnumeratorVtbl>(enumerator).get_default_audio_endpoint)(enumerator, flow, E_CONSOLE, &mut found) };
            if hr < 0 || found.is_null() {
                return Err(if flow == E_CAPTURE {
                    "audio: could not open the input device (WASAPI: no default capture endpoint); is there a microphone or audio interface?".to_string()
                } else {
                    "audio: no output device (WASAPI: no default render endpoint)".to_string()
                });
            }
            return Ok(found);
        }
        // Un endpoint de la otra dirección se activaría sin queja y fallaría más abajo con un
        // código opaco: se comprueba antes contra la lista.
        match devices()?.iter().find(|d| d.id == device) {
            None => return Err(format!("audio: no {direction} device with id '{device}' (was it disconnected?); list them with audio.devices()")),
            Some(d) if (if flow == E_CAPTURE { d.input_channels } else { d.output_channels }) == 0 => {
                return Err(format!("audio: the device '{}' has no {direction} channels", d.name));
            }
            Some(_) => {}
        }
        let wide: Vec<u16> = device.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` termina en NUL; `GetDevice` escribe `found`.
        let hr = unsafe {
            let get_device: FnGetDevice = std::mem::transmute(vtbl::<IMMDeviceEnumeratorVtbl>(enumerator).get_device);
            get_device(enumerator, wide.as_ptr(), &mut found)
        };
        if hr < 0 || found.is_null() {
            return Err(format!("audio: no {direction} device with id '{device}' (0x{:08X})", hr as u32));
        }
        Ok(found)
    }

    pub fn devices() -> Result<Vec<super::DeviceInfo>, String> {
        let enumerator = enumerator()?;
        let mut out = Vec::new();
        for flow in [E_RENDER, E_CAPTURE] {
            // El id del endpoint por defecto de este flujo.
            let mut default: Ptr = std::ptr::null_mut();
            // SAFETY: método del vtable transcrito; escribe `default` o lo deja nulo.
            unsafe { (vtbl::<IMMDeviceEnumeratorVtbl>(enumerator).get_default_audio_endpoint)(enumerator, flow, E_CONSOLE, &mut default) };
            let default_id = if default.is_null() { String::new() } else { endpoint_id(default) };
            release(default);
            let mut collection: Ptr = std::ptr::null_mut();
            // SAFETY: `EnumAudioEndpoints` escribe la colección de endpoints activos.
            let hr = unsafe {
                let enum_endpoints: FnEnumEndpoints = std::mem::transmute(vtbl::<IMMDeviceEnumeratorVtbl>(enumerator).enum_audio_endpoints);
                enum_endpoints(enumerator, flow, DEVICE_STATE_ACTIVE, &mut collection)
            };
            if hr < 0 || collection.is_null() {
                continue;
            }
            let mut count = 0u32;
            // SAFETY: escribe un u32 propio.
            unsafe { (vtbl::<IMMDeviceCollectionVtbl>(collection).get_count)(collection, &mut count) };
            for i in 0..count {
                let mut device: Ptr = std::ptr::null_mut();
                // SAFETY: `Item` escribe el endpoint `i`.
                if unsafe { (vtbl::<IMMDeviceCollectionVtbl>(collection).item)(collection, i, &mut device) } < 0 || device.is_null() {
                    continue;
                }
                let id = endpoint_id(device);
                let (mut name, mut bus) = (String::new(), String::new());
                let mut store: Ptr = std::ptr::null_mut();
                // SAFETY: `OpenPropertyStore(STGM_READ)` escribe el almacén.
                unsafe {
                    let open_store: FnOpenPropertyStore = std::mem::transmute(vtbl::<IMMDeviceVtbl>(device).open_property_store);
                    if open_store(device, 0, &mut store) >= 0 && !store.is_null() {
                        name = property_text(store, &PKEY_FRIENDLY_NAME);
                        bus = property_text(store, &PKEY_ENUMERATOR_NAME);
                    }
                }
                release(store);
                // Canales y frecuencia: el formato de mezcla del motor para este endpoint.
                let (mut channels, mut rate) = (0i64, 0i64);
                let mut client: Ptr = std::ptr::null_mut();
                // SAFETY: `Activate` escribe el cliente; `GetMixFormat` reserva el formato con
                // CoTaskMemAlloc y se libera aquí.
                unsafe {
                    if (vtbl::<IMMDeviceVtbl>(device).activate)(device, &IID_IAUDIO_CLIENT, CLSCTX_ALL, std::ptr::null_mut(), &mut client) >= 0 && !client.is_null() {
                        let get_mix_format: FnGetMixFormat = std::mem::transmute(vtbl::<IAudioClientVtbl>(client).get_mix_format);
                        let mut format: *mut WaveFormatEx = std::ptr::null_mut();
                        if get_mix_format(client, &mut format) >= 0 && !format.is_null() {
                            channels = std::ptr::addr_of!((*format).channels).read_unaligned() as i64;
                            rate = std::ptr::addr_of!((*format).samples_per_sec).read_unaligned() as i64;
                            CoTaskMemFree(format as Ptr);
                        }
                    }
                }
                release(client);
                release(device);
                if id.is_empty() {
                    continue;
                }
                out.push(super::DeviceInfo {
                    default_input: flow == E_CAPTURE && id == default_id,
                    default_output: flow == E_RENDER && id == default_id,
                    input_channels: if flow == E_CAPTURE { channels.max(1) } else { 0 },
                    output_channels: if flow == E_RENDER { channels.max(1) } else { 0 },
                    sample_rate: rate,
                    transport: match bus.to_ascii_uppercase().as_str() {
                        "USB" => "usb",
                        "BTHENUM" | "BTHHFENUM" | "BTHLEDEVICE" => "bluetooth",
                        "HDAUDIO" | "INTELAUDIO" => "builtin",
                        "ROOT" | "SWD" => "virtual",
                        _ => "",
                    }
                    .to_string(),
                    name,
                    id,
                });
            }
            release(collection);
        }
        release(enumerator);
        Ok(out)
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

    pub fn open(rate: i64, channels: i64, latency_ms: i64, played: Arc<AtomicI64>, device_id: &str) -> Result<Box<dyn Sink>, String> {
        let enumerator = enumerator()?;
        let device = match endpoint(enumerator, E_RENDER, device_id) {
            Ok(d) => d,
            Err(e) => {
                release(enumerator);
                return Err(e);
            }
        };
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

    const IID_IAUDIO_CAPTURE_CLIENT: Guid =
        Guid { d1: 0xC8AD_BD64, d2: 0xE71E, d3: 0x48A0, d4: [0xA4, 0xDE, 0x18, 0x5C, 0x39, 0x5C, 0xD3, 0x17] };
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;
    const E_ACCESSDENIED: u32 = 0x8007_0005;

    #[repr(C)]
    struct IAudioCaptureClientVtbl {
        base: IUnknownVtbl,
        get_buffer: unsafe extern "system" fn(Ptr, *mut *mut u8, *mut u32, *mut u32, *mut u64, *mut u64) -> Hr,
        release_buffer: unsafe extern "system" fn(Ptr, u32) -> Hr,
        get_next_packet_size: unsafe extern "system" fn(Ptr, *mut u32) -> Hr,
    }

    /// Los objetos COM de una captura, para soltarlos juntos.
    struct Capture {
        enumerator: Ptr,
        device: Ptr,
        client: Ptr,
        capture: Ptr,
    }
    // SAFETY: los objetos de WASAPI son free-threaded; tras crearse solo los usa el hilo lector.
    unsafe impl Send for Capture {}
    impl Drop for Capture {
        fn drop(&mut self) {
            release(self.capture);
            release(self.client);
            release(self.device);
            release(self.enumerator);
        }
    }

    /// M465: abre el dispositivo de CAPTURA por defecto en modo compartido y lanza el hilo que
    /// recoge sus paquetes y los entrega en bloques de un cuarto de la latencia pedida. El motor
    /// convierte al formato pedido (`AUTOCONVERTPCM`); si lo rechaza, se captura en s16le y se
    /// convierte aquí.
    pub fn open_input(
        rate: i64,
        channels: i64,
        latency_ms: i64,
        fmt: super::Format,
        end: super::SourceEnd,
        key: i64,
        ctl: Arc<super::InCtl>,
        device: &str,
    ) -> Result<(), String> {
        let mut cap = Capture { enumerator: std::ptr::null_mut(), device: std::ptr::null_mut(), client: std::ptr::null_mut(), capture: std::ptr::null_mut() };
        cap.enumerator = enumerator()?;
        cap.device = endpoint(cap.enumerator, E_CAPTURE, device)?;
        // El búfer del motor: la latencia pedida entera (los bloques son un cuarto), en 100 ns.
        let hns = latency_ms.clamp(20, 1000) * 10_000;
        let flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        let mut native = fmt;
        let mut hr;
        loop {
            // Un cliente por intento: `Initialize` solo se puede llamar una vez sobre cada uno.
            release(cap.client);
            cap.client = std::ptr::null_mut();
            // SAFETY: `Activate` escribe la interfaz pedida.
            hr = unsafe { (vtbl::<IMMDeviceVtbl>(cap.device).activate)(cap.device, &IID_IAUDIO_CLIENT, CLSCTX_ALL, std::ptr::null_mut(), &mut cap.client) };
            if hr < 0 || cap.client.is_null() {
                return Err(format!("audio: could not activate the audio client (0x{:08X})", hr as u32));
            }
            let sb = native.sample_bytes();
            let wf = WaveFormatEx {
                format_tag: if native == super::Format::F32 { WAVE_FORMAT_IEEE_FLOAT } else { WAVE_FORMAT_PCM },
                channels: channels as u16,
                samples_per_sec: rate as u32,
                avg_bytes_per_sec: (rate * channels * sb as i64) as u32,
                block_align: (channels as usize * sb) as u16,
                bits_per_sample: (sb * 8) as u16,
                cb_size: 0,
            };
            // SAFETY: `wf` vive durante la llamada; sin GUID de sesión (nulo).
            hr = unsafe { (vtbl::<IAudioClientVtbl>(cap.client).initialize)(cap.client, AUDCLNT_SHAREMODE_SHARED, flags, hns, 0, &wf, std::ptr::null()) };
            if hr >= 0 || native == super::Format::S16 || hr as u32 == E_ACCESSDENIED {
                break;
            }
            native = super::Format::S16;
        }
        if hr as u32 == E_ACCESSDENIED {
            return Err("audio: Windows denied access to the microphone (Settings > Privacy & security > Microphone: allow desktop apps)".to_string());
        }
        if hr < 0 {
            return Err(format!("audio: the input device does not accept {rate} Hz, {channels} channel(s) (WASAPI Initialize, 0x{:08X})", hr as u32));
        }
        // SAFETY: `GetService` escribe la interfaz pedida.
        let hr = unsafe { (vtbl::<IAudioClientVtbl>(cap.client).get_service)(cap.client, &IID_IAUDIO_CAPTURE_CLIENT, &mut cap.capture) };
        if hr < 0 || cap.capture.is_null() {
            return Err(format!("audio: could not get the capture client (0x{:08X})", hr as u32));
        }
        // SAFETY: arranca la captura.
        let hr = unsafe { (vtbl::<IAudioClientVtbl>(cap.client).start)(cap.client) };
        if hr < 0 {
            return Err(format!("audio: could not start recording (WASAPI Start, 0x{:08X})", hr as u32));
        }
        std::thread::spawn(move || {
            com_init();
            let cap = cap;
            let native_frame = channels as usize * native.sample_bytes();
            let block_bytes = ((rate * latency_ms / 4000).max(64)) as usize * native_frame;
            let mut pending: Vec<u8> = Vec::with_capacity(block_bytes * 2);
            let mut converted = Vec::new();
            // El motor entrega paquetes de su periodo (~10 ms): se sondea a esa cadencia.
            let nap = std::time::Duration::from_millis((latency_ms / 8).clamp(2, 10) as u64);
            'capture: loop {
                loop {
                    let mut frames = 0u32;
                    // SAFETY: escribe un u32 propio; el cliente de captura está vivo.
                    let hr = unsafe { (vtbl::<IAudioCaptureClientVtbl>(cap.capture).get_next_packet_size)(cap.capture, &mut frames) };
                    if hr < 0 {
                        break 'capture; // el dispositivo se desconectó o cambió
                    }
                    if frames == 0 {
                        break;
                    }
                    let mut data: *mut u8 = std::ptr::null_mut();
                    let mut flags = 0u32;
                    // SAFETY: `GetBuffer` presta `frames` frames legibles hasta `ReleaseBuffer`.
                    let hr = unsafe {
                        (vtbl::<IAudioCaptureClientVtbl>(cap.capture).get_buffer)(cap.capture, &mut data, &mut frames, &mut flags, std::ptr::null_mut(), std::ptr::null_mut())
                    };
                    if hr < 0 {
                        break 'capture;
                    }
                    let bytes = frames as usize * native_frame;
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT != 0 || data.is_null() {
                        pending.resize(pending.len() + bytes, 0);
                    } else {
                        // SAFETY: `data` apunta a `bytes` octetos prestados por el motor.
                        pending.extend_from_slice(unsafe { std::slice::from_raw_parts(data, bytes) });
                    }
                    // SAFETY: devuelve el préstamo de `GetBuffer`.
                    unsafe { (vtbl::<IAudioCaptureClientVtbl>(cap.capture).release_buffer)(cap.capture, frames) };
                }
                while pending.len() >= block_bytes {
                    let delivered = {
                        let got = &pending[..block_bytes];
                        let data = if native == fmt {
                            got
                        } else {
                            converted.clear();
                            super::from_s16le(fmt, got, &mut converted);
                            &converted[..]
                        };
                        super::push_captured(end, &ctl, data)
                    };
                    if !delivered {
                        break 'capture;
                    }
                    pending.drain(..block_bytes);
                }
                std::thread::sleep(nap);
            }
            // SAFETY: para la captura; los objetos COM se sueltan con `cap`.
            unsafe { (vtbl::<IAudioClientVtbl>(cap.client).stop)(cap.client) };
            drop(cap);
            super::close_end(end);
            super::forget_in_ctl(key, &ctl);
        });
        Ok(())
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

#[cfg(test)]
mod device_tests {
    use super::{DeviceInfo, split_device};

    /// M468: el dispositivo viaja pegado al formato tras la primera arroba; un id puede llevar
    /// arrobas, llaves y puntos (los de WASAPI).
    #[test]
    fn the_device_id_rides_after_the_first_at_sign() {
        assert_eq!(split_device("s16le"), ("s16le", ""));
        assert_eq!(split_device("f32le@BuiltInSpeakerDevice"), ("f32le", "BuiltInSpeakerDevice"));
        assert_eq!(split_device("s24le@{0.0.0.00000000}.{6507-a3e7}"), ("s24le", "{0.0.0.00000000}.{6507-a3e7}"));
        assert_eq!(split_device("s16le@a@b"), ("s16le", "a@b"));
        assert_eq!(split_device("s16le@"), ("s16le", ""));
    }

    /// El listado no rompe aunque no haya tarjeta, y cada dispositivo tiene id y dirección.
    #[test]
    fn devices_have_an_id_and_a_direction() {
        for d in super::devices().unwrap_or_default() {
            let DeviceInfo { id, input_channels, output_channels, .. } = d;
            assert!(!id.is_empty());
            assert!(input_channels + output_channels > 0, "{id}");
        }
    }
}

#[cfg(test)]
mod convert_tests {
    use super::{Format, from_s16le, to_s16le};

    /// M465: una entrada que el dispositivo solo da en s16le llega al programa en el formato que
    /// pidió, y la conversión es la inversa de la de la salida.
    #[test]
    fn from_s16le_widens_each_sample_and_round_trips() {
        let src: Vec<u8> = [0i16, 1, -1, 12345, i16::MAX, i16::MIN].iter().flat_map(|v| v.to_le_bytes()).collect();
        for fmt in [Format::S16, Format::S24, Format::S32, Format::F32] {
            let mut wide = Vec::new();
            from_s16le(fmt, &src, &mut wide);
            assert_eq!(wide.len(), src.len() / 2 * fmt.sample_bytes(), "{fmt:?}");
            let mut back = Vec::new();
            to_s16le(fmt, &wide, &mut back);
            if fmt == Format::F32 {
                // El flotante se escala por 32768 a la ida y 32767 a la vuelta: un paso de error.
                for (a, b) in src.chunks(2).zip(back.chunks(2)) {
                    let (a, b) = (i16::from_le_bytes([a[0], a[1]]) as i32, i16::from_le_bytes([b[0], b[1]]) as i32);
                    assert!((a - b).abs() <= 1, "{a} vs {b}");
                }
            } else {
                assert_eq!(back, src, "{fmt:?}");
            }
        }
        let mut f = Vec::new();
        from_s16le(Format::F32, &i16::MIN.to_le_bytes(), &mut f);
        assert_eq!(f32::from_le_bytes([f[0], f[1], f[2], f[3]]), -1.0);
    }
}

#[cfg(all(test, unix))]
mod budget_tests {
    // M398: formatos y conversión a s16le (lo que recibe un backend sin el formato nativo).
    #[test]
    fn formats_parse_and_convert_to_s16le() {
        use super::{to_s16le, Format};
        assert_eq!(Format::parse("f32le"), Ok(Format::F32));
        assert!(Format::parse("u8").unwrap_err().contains("s16le, s24le, s32le, f32le"));
        assert_eq!(Format::S24.sample_bytes(), 3);
        let mut out = Vec::new();
        to_s16le(Format::F32, &[1.0f32.to_le_bytes(), (-0.5f32).to_le_bytes(), 2.0f32.to_le_bytes()].concat(), &mut out);
        assert_eq!(out, [32767i16.to_le_bytes(), (-16383i16).to_le_bytes(), 32767i16.to_le_bytes()].concat());
        out.clear();
        to_s16le(Format::S24, &[0x00, 0x34, 0x12, 0xff, 0xff, 0x7f, 0xaa], &mut out); // el 0xaa sobrante se descarta
        assert_eq!(out, vec![0x34, 0x12, 0xff, 0x7f]);
        out.clear();
        to_s16le(Format::S32, &0x1234_5678i32.to_le_bytes(), &mut out);
        assert_eq!(out, 0x1234i16.to_le_bytes());
        // La rampa escala en cada formato (una muestra a la mitad).
        let mut s = 0.8f32.to_le_bytes();
        super::scale_sample(&mut s, Format::F32, 0.5);
        assert!((f32::from_le_bytes(s) - 0.4).abs() < 1e-6);
        let mut s = [0x00, 0x00, 0x40]; // 0x400000 en s24
        super::scale_sample(&mut s, Format::S24, 0.5);
        assert_eq!(s, [0x00, 0x00, 0x20]);
    }

    use super::queue_budget_bytes;

    /// M378 (1942 #108): el presupuesto del socket sigue la pista pedida (½) desde 20 ms; el único
    /// suelo son dos chunks del alimentador. A 22 050 Hz mono, 20 ms ya no son 2 KiB (46 ms).
    #[test]
    fn the_queue_budget_follows_small_latency_hints() {
        let bps = 22_050 * 2;
        let chunk = |lat: i64| ((bps * lat / 4000).max(256) as usize) & !1;
        assert_eq!(queue_budget_bytes(bps, 20, chunk(20)), 882); // ½ = 441 → suelo de 20 ms
        assert_eq!(queue_budget_bytes(bps, 50, chunk(50)), 1102); // 25 ms
        assert_eq!(queue_budget_bytes(bps, 80, chunk(80)), 1764); // 40 ms
        assert_eq!(queue_budget_bytes(bps, 200, chunk(200)), 4410); // 100 ms
        assert!(queue_budget_bytes(bps, 20, chunk(20)) < 2048, "el suelo de 2 KiB ya no existe");
    }
}
