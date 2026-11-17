//! M464 (ray-daw #423) — MIDI: puertos de entrada y salida del sistema (`std/midi`).
//!
//! La misma forma que `audio`: lo que el sistema entrega en un callback se convierte en un
//! descriptor que el programa lee o escribe con la maquinaria de handles de siempre.
//!
//!   - **Entrada**: un par de sockets de DATAGRAMAS. El callback de CoreMIDI escribe un
//!     datagrama por mensaje (`[marca de tiempo en ns, 8 octetos LE][octetos MIDI]`) y el
//!     programa lee del otro extremo, registrado como un pipe de lectura: `midi.read` aparca la
//!     fibra hasta que llega un mensaje y cada lectura devuelve UNO entero (por eso datagramas:
//!     no hay que reensamblar nada en raylang).
//!   - **Salida**: un par de sockets de flujo. El programa escribe tramas `[longitud u16 LE]
//!     [octetos]` en su extremo (un pipe de escritura, como el stdin de un hijo) y un hilo las
//!     lee y las entrega al puerto. El `close(h)` del programa es el EOF que lo termina.
//!   - **Cambios**: `watch` da un extremo de lectura que recibe un datagrama cada vez que se
//!     conecta o desconecta un dispositivo.
//!
//! Backends A MANO, cero crates, como el audio: CoreMIDI en macOS e iOS (el framework está
//! siempre presente) y el secuenciador de ALSA en Linux (M466: `libasound.so.2` por dlopen; sin
//! la biblioteca o sin `/dev/snd/seq`, `Err`) y WinMM en Windows (M466: los dispositivos MIDI
//! clásicos; Windows no tiene puertos virtuales). En Android (M467) el MIDI del sistema solo
//! existe en Java (`android.media.midi`): el shell abre los dispositivos y este módulo le habla
//! por las peticiones genéricas del puente.
//!
//! Va tras la feature `audio` (un build sin audio tampoco tiene MIDI).
#![cfg(all(feature = "audio", any(unix, windows)))]

/// Un puerto MIDI del sistema: su id y su nombre. El id es el *unique ID* de CoreMIDI, o
/// `cliente << 8 | puerto` del secuenciador de ALSA: estable mientras el dispositivo siga
/// conectado.
pub type Port = (i64, String);

#[cfg(any(target_os = "macos", target_os = "ios"))]
use coremidi as backend;
// M466: Linux — el secuenciador de ALSA por dlopen.
#[cfg(target_os = "linux")]
use alsaseq as backend;

// M466: Windows — WinMM (los dispositivos MIDI clásicos; sin puertos virtuales).
#[cfg(windows)]
use winmm as backend;

// M467: Android — `android.media.midi` a través del shell.
#[cfg(target_os = "android")]
use android as backend;

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "android", windows)))]
mod backend {
    //! Sin backend: toda operación da `Err`.
    use super::Port;
    const UNSUPPORTED: &str = "midi: not supported on this platform (macOS, iOS, Linux, Android and Windows)";
    pub fn list(_inputs: bool) -> Result<Vec<Port>, String> {
        Err(UNSUPPORTED.to_string())
    }
    pub fn open_input(_id: Option<i64>, _name: &str) -> Result<std::fs::File, String> {
        Err(UNSUPPORTED.to_string())
    }
    pub fn open_output(_id: Option<i64>, _name: &str) -> Result<std::fs::File, String> {
        Err(UNSUPPORTED.to_string())
    }
    pub fn watch() -> Result<std::fs::File, String> {
        Err(UNSUPPORTED.to_string())
    }
}

/// Trocea los octetos de un paquete en mensajes MIDI y llama a `emit` con cada uno (ray-daw
/// #446). CoreMIDI junta en un solo paquete los mensajes que llegan a la vez —un acorde, un Note
/// Off y un Note On del mismo tick—, y `midi.read` promete un mensaje por lectura: sin trocear,
/// dos `send` seguidos llegaban como uno de seis octetos y la segunda lectura no volvía nunca.
/// Un SysEx se entrega entero si cabe en el paquete, o por tramos si viene partido (`in_sysex`
/// recuerda entre paquetes que se está dentro de uno); un octeto de tiempo real (0xF8–0xFF) va
/// solo, también en medio de otro mensaje; el *running status* se repone en cada mensaje.
#[cfg_attr(not(any(target_os = "macos", target_os = "ios", test)), allow(dead_code))]
fn split_messages(data: &[u8], in_sysex: &mut bool, mut emit: impl FnMut(&[u8])) {
    let short_len = |status: u8| -> usize {
        match status {
            0x80..=0xBF | 0xE0..=0xEF | 0xF2 => 3,
            0xC0..=0xDF | 0xF1 | 0xF3 => 2,
            _ => 1,
        }
    };
    let mut running = 0u8;
    let mut message: Vec<u8> = Vec::with_capacity(16);
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if b >= 0xF8 {
            emit(&[b]);
            i += 1;
        } else if *in_sysex || b == 0xF0 {
            // Hasta el 0xF7 (incluido), hasta otro estado (el SysEx quedó cortado) o hasta el
            // final del paquete (sigue en el siguiente); el tiempo real intercalado sale aparte.
            message.clear();
            let continuing = *in_sysex;
            *in_sysex = true;
            let mut first = !continuing;
            while i < data.len() {
                let c = data[i];
                if c >= 0xF8 {
                    emit(&[c]);
                } else if c == 0xF7 {
                    message.push(c);
                    *in_sysex = false;
                    i += 1;
                    break;
                } else if c >= 0x80 && !first {
                    *in_sysex = false;
                    break; // otro mensaje empieza aquí: se procesa en la vuelta siguiente
                } else {
                    message.push(c);
                }
                first = false;
                i += 1;
            }
            if !message.is_empty() {
                emit(&message);
            }
            running = 0;
        } else {
            let (status, mut need) = if b >= 0x80 {
                i += 1;
                (b, short_len(b) - 1)
            } else if running != 0 {
                (running, short_len(running) - 1)
            } else {
                // Datos sin estado conocido: se entregan tal cual hasta el siguiente estado.
                let start = i;
                while i < data.len() && data[i] < 0x80 {
                    i += 1;
                }
                emit(&data[start..i]);
                continue;
            };
            message.clear();
            message.push(status);
            while need > 0 && i < data.len() {
                let c = data[i];
                if c >= 0xF8 {
                    emit(&[c]);
                } else if c >= 0x80 {
                    break; // mensaje truncado
                } else {
                    message.push(c);
                    need -= 1;
                }
                i += 1;
            }
            running = if (0x80..0xF0).contains(&status) { status } else { 0 };
            emit(&message);
        }
    }
}

/// Los puertos de entrada (`inputs = true`: fuentes de las que se puede leer) o de salida
/// (destinos a los que se puede enviar) presentes ahora.
pub fn list(inputs: bool) -> Result<Vec<Port>, String> {
    backend::list(inputs)
}

/// Abre la entrada `id` (de `list(true)`) y devuelve el extremo de LECTURA, no bloqueante: un
/// datagrama por mensaje.
pub fn open_input(id: i64) -> Result<std::fs::File, String> {
    backend::open_input(Some(id), "")
}

/// Crea una entrada VIRTUAL llamada `name`: otros programas la ven como un destino MIDI y lo
/// que le envían se lee del extremo devuelto.
pub fn create_input(name: &str) -> Result<std::fs::File, String> {
    backend::open_input(None, name)
}

/// Abre la salida `id` (de `list(false)`) y devuelve el extremo de ESCRITURA: tramas
/// `[longitud u16 LE][octetos MIDI]`.
pub fn open_output(id: i64) -> Result<std::fs::File, String> {
    backend::open_output(Some(id), "")
}

/// Crea una salida VIRTUAL llamada `name`: otros programas la ven como una fuente MIDI y
/// reciben lo que se escribe en el extremo devuelto.
pub fn create_output(name: &str) -> Result<std::fs::File, String> {
    backend::open_output(None, name)
}

/// Un extremo de lectura que recibe un datagrama (un octeto) cada vez que cambia el conjunto de
/// puertos: se conectó o se desconectó un dispositivo.
pub fn watch() -> Result<std::fs::File, String> {
    backend::watch()
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod coremidi {
    use super::Port;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    type CFStringRef = *const c_void;
    /// `MIDIObjectRef` y sus especializaciones (cliente, puerto, extremo) son `UInt32`.
    type MidiRef = u32;

    #[repr(C)]
    struct MIDINotification {
        message_id: i32,
        message_size: u32,
    }
    const MSG_SETUP_CHANGED: i32 = 1; // kMIDIMsgSetupChanged

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithBytes(alloc: *const c_void, bytes: *const u8, len: isize, encoding: u32, external: u8) -> CFStringRef;
        fn CFStringGetLength(s: CFStringRef) -> isize;
        fn CFStringGetMaximumSizeForEncoding(len: isize, encoding: u32) -> isize;
        fn CFStringGetCString(s: CFStringRef, buf: *mut u8, len: isize, encoding: u32) -> u8;
        fn CFRelease(obj: *const c_void);
        fn CFRunLoopRun();
    }
    const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8

    type ReadProc = extern "C" fn(*const u8, *mut c_void, *mut c_void);

    #[link(name = "CoreMIDI", kind = "framework")]
    unsafe extern "C" {
        fn MIDIClientCreate(
            name: CFStringRef,
            notify: extern "C" fn(*const MIDINotification, *mut c_void),
            ref_con: *mut c_void,
            out: *mut MidiRef,
        ) -> i32;
        fn MIDIGetNumberOfSources() -> usize;
        fn MIDIGetSource(i: usize) -> MidiRef;
        fn MIDIGetNumberOfDestinations() -> usize;
        fn MIDIGetDestination(i: usize) -> MidiRef;
        fn MIDIObjectGetStringProperty(obj: MidiRef, prop: CFStringRef, out: *mut CFStringRef) -> i32;
        fn MIDIObjectGetIntegerProperty(obj: MidiRef, prop: CFStringRef, out: *mut i32) -> i32;
        fn MIDIInputPortCreate(client: MidiRef, name: CFStringRef, read: ReadProc, ref_con: *mut c_void, out: *mut MidiRef) -> i32;
        fn MIDIPortConnectSource(port: MidiRef, source: MidiRef, conn_ref_con: *mut c_void) -> i32;
        fn MIDIOutputPortCreate(client: MidiRef, name: CFStringRef, out: *mut MidiRef) -> i32;
        fn MIDIPortDispose(port: MidiRef) -> i32;
        fn MIDISend(port: MidiRef, dest: MidiRef, packets: *const u8) -> i32;
        fn MIDIDestinationCreate(client: MidiRef, name: CFStringRef, read: ReadProc, ref_con: *mut c_void, out: *mut MidiRef) -> i32;
        fn MIDISourceCreate(client: MidiRef, name: CFStringRef, out: *mut MidiRef) -> i32;
        fn MIDIReceived(source: MidiRef, packets: *const u8) -> i32;
        fn MIDIEndpointDispose(endpoint: MidiRef) -> i32;
        static kMIDIPropertyDisplayName: CFStringRef;
        static kMIDIPropertyName: CFStringRef;
        static kMIDIPropertyUniqueID: CFStringRef;
    }

    #[repr(C)]
    struct TimebaseInfo {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut TimebaseInfo) -> i32;
        fn socketpair(domain: i32, ty: i32, protocol: i32, fds: *mut i32) -> i32;
        fn setsockopt(fd: i32, level: i32, name: i32, value: *const c_void, len: u32) -> i32;
        fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
        fn write(fd: i32, buf: *const u8, n: usize) -> isize;
        fn close(fd: i32) -> i32;
        /// Falla con ENOTCONN cuando el otro extremo del par ya se cerró: la forma de saber,
        /// sin escribir nada, que el programa soltó una entrada.
        fn getpeername(fd: i32, addr: *mut u8, len: *mut u32) -> i32;
        #[link_name = "fcntl"]
        fn fcntl_raw(fd: i32, cmd: i32, ...) -> i32;
    }
    const AF_UNIX: i32 = 1;
    const SOCK_STREAM: i32 = 1;
    const SOCK_DGRAM: i32 = 2;
    const SOL_SOCKET: i32 = 0xffff;
    const SO_SNDBUF: i32 = 0x1001;
    const SO_RCVBUF: i32 = 0x1002;
    const SO_NOSIGPIPE: i32 = 0x1022;
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    const O_NONBLOCK: i32 = 0x0004;

    /// Los ticks de `mach_absolute_time` a nanosegundos.
    fn ticks_to_ns(ticks: u64) -> u64 {
        static BASE: OnceLock<(u64, u64)> = OnceLock::new();
        let (numer, denom) = *BASE.get_or_init(|| {
            let mut info = TimebaseInfo { numer: 1, denom: 1 };
            // SAFETY: `info` es nuestro y la llamada solo lo rellena.
            unsafe { mach_timebase_info(&mut info) };
            (info.numer.max(1) as u64, info.denom.max(1) as u64)
        });
        (ticks as u128 * numer as u128 / denom as u128) as u64
    }

    fn cfstring(s: &str) -> CFStringRef {
        // SAFETY: el puntero y la longitud describen el `str`; CF copia los octetos.
        unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, 0) }
    }

    /// El texto de un `CFString` (que NO se libera aquí).
    fn cfstring_text(s: CFStringRef) -> String {
        if s.is_null() {
            return String::new();
        }
        // SAFETY: `s` es un CFString vivo; el búfer tiene el tamaño que CF pide para UTF-8.
        unsafe {
            let cap = CFStringGetMaximumSizeForEncoding(CFStringGetLength(s), UTF8) + 1;
            let mut buf = vec![0u8; cap.max(1) as usize];
            if CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, UTF8) == 0 {
                return String::new();
            }
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            String::from_utf8_lossy(&buf[..end]).into_owned()
        }
    }

    /// Los extremos de escritura de los `watch` vivos.
    fn watchers() -> &'static Mutex<Vec<i32>> {
        static W: OnceLock<Mutex<Vec<i32>>> = OnceLock::new();
        W.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// El aviso de CoreMIDI: cuando cambia la configuración, un datagrama a cada `watch`. Uno
    /// cuyo lector ya cerró se retira.
    extern "C" fn on_notify(msg: *const MIDINotification, _ref_con: *mut c_void) {
        // SAFETY: CoreMIDI entrega una notificación válida durante la llamada.
        if unsafe { (*msg).message_id } != MSG_SETUP_CHANGED {
            return;
        }
        let mut ws = watchers().lock().unwrap_or_else(|e| e.into_inner());
        ws.retain(|&fd| {
            let one = [1u8];
            // SAFETY: `one` vive durante la llamada; `fd` es un extremo nuestro.
            let n = unsafe { write(fd, one.as_ptr(), 1) };
            // Una cola llena (el programa no ha leído los avisos anteriores) no es un lector
            // muerto: en macOS un socket de datagramas lleno da ENOBUFS, no WouldBlock.
            let alive = n == 1 || is_full(&std::io::Error::last_os_error());
            if !alive {
                // SAFETY: el extremo es nuestro y nadie más lo usa tras retirarlo.
                unsafe { close(fd) };
            }
            alive
        });
    }

    /// El cliente de CoreMIDI del proceso. Se crea en un hilo propio que se queda en su run
    /// loop: es donde CoreMIDI entrega los avisos y lo que mantiene al día la lista de
    /// dispositivos cuando se conectan en caliente.
    fn client() -> Result<MidiRef, String> {
        static CLIENT: OnceLock<Result<MidiRef, String>> = OnceLock::new();
        CLIENT
            .get_or_init(|| {
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let name = cfstring("raylang");
                    let mut client: MidiRef = 0;
                    // SAFETY: nombre y salida válidos; el callback es una función estática.
                    let st = unsafe { MIDIClientCreate(name, on_notify, std::ptr::null_mut(), &mut client) };
                    // SAFETY: `name` lo creamos nosotros.
                    unsafe { CFRelease(name) };
                    if st != 0 {
                        let _ = tx.send(Err(format!("midi: could not start CoreMIDI (MIDIClientCreate, OSStatus {st})")));
                        return;
                    }
                    let _ = tx.send(Ok(client));
                    // SAFETY: bucle de eventos de este hilo; no vuelve mientras viva el proceso.
                    unsafe { CFRunLoopRun() };
                });
                rx.recv().unwrap_or_else(|_| Err("midi: the CoreMIDI thread ended before starting".to_string()))
            })
            .clone()
    }

    fn endpoints(inputs: bool) -> Vec<MidiRef> {
        // SAFETY: llamadas de consulta de CoreMIDI, sin punteros nuestros.
        unsafe {
            if inputs {
                (0..MIDIGetNumberOfSources()).map(|i| MIDIGetSource(i)).collect()
            } else {
                (0..MIDIGetNumberOfDestinations()).map(|i| MIDIGetDestination(i)).collect()
            }
        }
    }

    fn unique_id(endpoint: MidiRef) -> i64 {
        let mut id: i32 = 0;
        // SAFETY: `id` es nuestro; la propiedad es una constante exportada por CoreMIDI.
        unsafe { MIDIObjectGetIntegerProperty(endpoint, kMIDIPropertyUniqueID, &mut id) };
        id as i64
    }

    fn display_name(endpoint: MidiRef) -> String {
        // SAFETY: cada propiedad devuelve un CFString que poseemos y liberamos.
        unsafe {
            for prop in [kMIDIPropertyDisplayName, kMIDIPropertyName] {
                let mut s: CFStringRef = std::ptr::null();
                if MIDIObjectGetStringProperty(endpoint, prop, &mut s) == 0 && !s.is_null() {
                    let text = cfstring_text(s);
                    CFRelease(s);
                    if !text.is_empty() {
                        return text;
                    }
                }
            }
        }
        String::new()
    }

    pub fn list(inputs: bool) -> Result<Vec<Port>, String> {
        client()?;
        sweep();
        Ok(endpoints(inputs).into_iter().filter(|&e| e != 0).map(|e| (unique_id(e), display_name(e))).collect())
    }

    fn find(inputs: bool, id: i64) -> Result<MidiRef, String> {
        endpoints(inputs)
            .into_iter()
            .find(|&e| e != 0 && unique_id(e) == id)
            .ok_or_else(|| format!("midi: no {} port with id {id} (was it disconnected?)", if inputs { "input" } else { "output" }))
    }

    /// Un par de sockets: `(lectura, escritura)`. El de lectura queda no bloqueante cuando es
    /// el del programa (`reader_nonblocking`); el de escritura, cuando es el del callback.
    fn pair(kind: i32, reader_nonblocking: bool, writer_nonblocking: bool, buffer: i32) -> Result<(i32, i32), String> {
        let mut fds = [0i32; 2];
        // SAFETY: socketpair escribe dos fds válidos.
        if unsafe { socketpair(AF_UNIX, kind, 0, fds.as_mut_ptr()) } != 0 {
            return Err(format!("midi: could not create the pipe: {}", std::io::Error::last_os_error()));
        }
        let (fd_r, fd_w) = (fds[0], fds[1]);
        // SAFETY: fds recién creados y nuestros; los valores viven durante cada llamada.
        unsafe {
            let v = &buffer as *const i32 as *const c_void;
            for fd in [fd_r, fd_w] {
                setsockopt(fd, SOL_SOCKET, SO_SNDBUF, v, 4);
                setsockopt(fd, SOL_SOCKET, SO_RCVBUF, v, 4);
            }
            let one: i32 = 1;
            setsockopt(fd_w, SOL_SOCKET, SO_NOSIGPIPE, &one as *const i32 as *const c_void, 4);
            for (fd, nonblocking) in [(fd_r, reader_nonblocking), (fd_w, writer_nonblocking)] {
                if nonblocking {
                    let fl = fcntl_raw(fd, F_GETFL);
                    fcntl_raw(fd, F_SETFL, fl | O_NONBLOCK);
                }
            }
        }
        Ok((fd_r, fd_w))
    }

    fn file(fd: i32) -> std::fs::File {
        // SAFETY: `fd` es nuestro; File toma la propiedad (su Drop lo cierra).
        unsafe { std::os::unix::io::FromRawFd::from_raw_fd(fd) }
    }

    // ---------------------------------------------------------------------------------
    // Entrada
    // ---------------------------------------------------------------------------------

    /// Lo que el callback de lectura necesita, y lo que hay que soltar al cerrar.
    struct Input {
        fd_w: i32,
        closed: AtomicBool,
        /// Dentro de un SysEx que sigue en el paquete siguiente (solo lo toca `on_read`).
        in_sysex: AtomicBool,
        /// El puerto de entrada (entrada real) o el destino virtual: lo que se desmonta.
        object: Mutex<(MidiRef, bool)>,
    }

    /// Las entradas vivas, para desmontar las que el programa cerró (`sweep`).
    fn inputs_alive() -> &'static Mutex<Vec<usize>> {
        static I: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
        I.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// El callback de lectura de CoreMIDI: un datagrama por MENSAJE, `[ns: 8 LE][octetos]` (un
    /// paquete puede traer varios: `split_messages`).
    /// Corre en un hilo de CoreMIDI; nunca bloquea (un mensaje que no cabe se pierde).
    extern "C" fn on_read(packets: *const u8, ref_con: *mut c_void, _conn: *mut c_void) {
        // SAFETY: `ref_con` es el `Arc<Input>` filtrado al abrir; vive hasta `sweep`, que solo
        // lo suelta después de desmontar el puerto (ya sin callbacks).
        let input = unsafe { &*(ref_con as *const Input) };
        if input.closed.load(Ordering::SeqCst) {
            return;
        }
        // SAFETY: CoreMIDI entrega una `MIDIPacketList` válida: `numPackets: u32` y, a
        // continuación (empaquetada a 4), paquetes `{ timeStamp: u64, length: u16, data }`.
        unsafe {
            let count = std::ptr::read_unaligned(packets as *const u32);
            let mut p = packets.add(4);
            let mut record = Vec::with_capacity(64);
            let mut in_sysex = input.in_sysex.load(Ordering::SeqCst);
            let mut gone = false;
            for _ in 0..count {
                let stamp = std::ptr::read_unaligned(p as *const u64);
                let len = std::ptr::read_unaligned(p.add(8) as *const u16) as usize;
                let data = std::slice::from_raw_parts(p.add(10), len);
                let ticks = if stamp == 0 { mach_absolute_time() } else { stamp };
                let ns = ticks_to_ns(ticks).to_le_bytes();
                super::split_messages(data, &mut in_sysex, |message| {
                    if gone {
                        return;
                    }
                    record.clear();
                    record.extend_from_slice(&ns);
                    record.extend_from_slice(message);
                    let n = write(input.fd_w, record.as_ptr(), record.len());
                    if n < 0 && !is_full(&std::io::Error::last_os_error()) {
                        gone = true; // el programa cerró la entrada
                    }
                });
                if gone {
                    input.closed.store(true, Ordering::SeqCst);
                    return;
                }
                // `MIDIPacketNext`: en arm64 los paquetes van alineados a 4 octetos.
                let next = p.add(10 + len);
                p = if cfg!(target_arch = "aarch64") { ((next as usize + 3) & !3) as *const u8 } else { next };
            }
            input.in_sysex.store(in_sysex, Ordering::SeqCst);
        }
    }

    /// ¿El error de una escritura es «cola llena» (y no «el lector se fue»)?
    fn is_full(err: &std::io::Error) -> bool {
        err.kind() == std::io::ErrorKind::WouldBlock || err.raw_os_error() == Some(55) // ENOBUFS
    }

    /// ¿Sigue abierto el otro extremo del par?
    fn peer_alive(fd: i32) -> bool {
        let mut addr = [0u8; 128];
        let mut len = addr.len() as u32;
        // SAFETY: `addr` y `len` son nuestros y describen el búfer.
        unsafe { getpeername(fd, addr.as_mut_ptr(), &mut len) == 0 }
    }

    /// Arranca (una vez) el hilo que desmonta las entradas que el programa cerró: sin él, un
    /// `close(h)` solo surtía efecto en la siguiente llamada a `std/midi`, y hasta entonces los
    /// demás programas seguían viendo —y enviando a— un puerto virtual muerto.
    fn start_sweeper() {
        static STARTED: OnceLock<()> = OnceLock::new();
        STARTED.get_or_init(|| {
            std::thread::spawn(|| loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
                sweep();
            });
        });
    }

    /// Desmonta las entradas cuyo lector ya cerró. Lo llama el barrendero cada 200 ms y cada
    /// operación al empezar.
    fn sweep() {
        let mut alive = inputs_alive().lock().unwrap_or_else(|e| e.into_inner());
        alive.retain(|&addr| {
            // SAFETY: cada dirección es un `Arc<Input>` filtrado y aún no soltado.
            let input = unsafe { &*(addr as *const Input) };
            if !input.closed.load(Ordering::SeqCst) {
                // Sin tráfico el callback no se entera del cierre: se pregunta al socket, sin
                // escribir nada (un sondeo con datagramas llenaba la cola de una entrada que
                // nadie leía y los mensajes de verdad se perdían).
                if peer_alive(input.fd_w) {
                    return true;
                }
                input.closed.store(true, Ordering::SeqCst);
            }
            let (object, virtual_endpoint) = *input.object.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: el objeto es de esta entrada; tras desmontarlo no hay más callbacks, así
            // que el `Arc` filtrado y el fd se pueden soltar.
            unsafe {
                if virtual_endpoint {
                    MIDIEndpointDispose(object);
                } else {
                    MIDIPortDispose(object);
                }
                close(input.fd_w);
                drop(Arc::from_raw(addr as *const Input));
            }
            false
        });
    }

    /// Abre la entrada con id `id`, o crea una virtual llamada `name` si `id` es `None`.
    pub fn open_input(id: Option<i64>, name: &str) -> Result<std::fs::File, String> {
        let client = client()?;
        sweep();
        let source = match id {
            Some(id) => Some(find(true, id)?),
            None => None,
        };
        // Datagramas: uno por mensaje. El búfer fija también el mensaje más largo (un SysEx).
        let (fd_r, fd_w) = pair(SOCK_DGRAM, true, true, 256 * 1024)?;
        let input = Arc::new(Input { fd_w, closed: AtomicBool::new(false), in_sysex: AtomicBool::new(false), object: Mutex::new((0, source.is_none())) });
        let ref_con = Arc::into_raw(input.clone()) as *mut c_void;
        let label = cfstring(if name.is_empty() { "raylang input" } else { name });
        let mut object: MidiRef = 0;
        // SAFETY: cliente, nombre y salida válidos; `ref_con` vive hasta `sweep`.
        let st = unsafe {
            let st = match source {
                Some(_) => MIDIInputPortCreate(client, label, on_read, ref_con, &mut object),
                None => MIDIDestinationCreate(client, label, on_read, ref_con, &mut object),
            };
            CFRelease(label);
            st
        };
        let connected = if st != 0 {
            Err(format!("midi: could not open the input (OSStatus {st})"))
        } else if let Some(source) = source {
            // SAFETY: puerto recién creado y fuente viva.
            match unsafe { MIDIPortConnectSource(object, source, std::ptr::null_mut()) } {
                0 => Ok(()),
                st => {
                    // SAFETY: el puerto es nuestro y aún no está registrado.
                    unsafe { MIDIPortDispose(object) };
                    Err(format!("midi: could not connect to the input (OSStatus {st})"))
                }
            }
        } else {
            Ok(())
        };
        if let Err(e) = connected {
            // SAFETY: nada más referencia el `Arc` filtrado ni los fds.
            unsafe {
                drop(Arc::from_raw(ref_con as *const Input));
                close(fd_r);
                close(fd_w);
            }
            return Err(e);
        }
        input.object.lock().unwrap().0 = object;
        inputs_alive().lock().unwrap().push(ref_con as usize);
        start_sweeper();
        Ok(file(fd_r))
    }

    // ---------------------------------------------------------------------------------
    // Salida
    // ---------------------------------------------------------------------------------

    /// Lee exactamente `buf.len()` octetos del extremo bloqueante; `false` en EOF o error.
    fn read_exact(fd: i32, buf: &mut [u8]) -> bool {
        let mut got = 0;
        while got < buf.len() {
            // SAFETY: el tramo es nuestro y vive durante la llamada.
            let n = unsafe { read(fd, buf[got..].as_mut_ptr(), buf.len() - got) };
            if n > 0 {
                got += n as usize;
            } else if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            } else {
                return false;
            }
        }
        true
    }

    /// Abre la salida con id `id`, o crea una virtual llamada `name` si `id` es `None`. Un hilo
    /// lee las tramas `[longitud u16 LE][octetos]` del programa y las entrega.
    pub fn open_output(id: Option<i64>, name: &str) -> Result<std::fs::File, String> {
        let client = client()?;
        sweep();
        let destination = match id {
            Some(id) => Some(find(false, id)?),
            None => None,
        };
        let label = cfstring(if name.is_empty() { "raylang output" } else { name });
        let mut object: MidiRef = 0;
        // SAFETY: cliente, nombre y salida válidos.
        let st = unsafe {
            let st = match destination {
                Some(_) => MIDIOutputPortCreate(client, label, &mut object),
                None => MIDISourceCreate(client, label, &mut object),
            };
            CFRelease(label);
            st
        };
        if st != 0 {
            return Err(format!("midi: could not open the output (OSStatus {st})"));
        }
        // Flujo: el programa escribe tramas (con contrapresión) y el hilo las lee bloqueando.
        let (fd_r, fd_w) = match pair(SOCK_STREAM, false, true, 64 * 1024) {
            Ok(p) => p,
            Err(e) => {
                // SAFETY: el objeto es nuestro y nadie más lo conoce.
                unsafe {
                    if destination.is_some() { MIDIPortDispose(object) } else { MIDIEndpointDispose(object) };
                }
                return Err(e);
            }
        };
        std::thread::spawn(move || {
            let mut header = [0u8; 2];
            let mut list = Vec::new();
            while read_exact(fd_r, &mut header) {
                let len = u16::from_le_bytes(header) as usize;
                // Una `MIDIPacketList` de un paquete: numPackets, timeStamp 0 (= ahora), length.
                list.clear();
                list.extend_from_slice(&1u32.to_ne_bytes());
                list.extend_from_slice(&0u64.to_ne_bytes());
                list.extend_from_slice(&(len as u16).to_ne_bytes());
                let at = list.len();
                list.resize(at + len, 0);
                if !read_exact(fd_r, &mut list[at..]) {
                    break;
                }
                // SAFETY: `list` es una lista bien formada que vive durante la llamada.
                unsafe {
                    match destination {
                        Some(dest) => MIDISend(object, dest, list.as_ptr()),
                        None => MIDIReceived(object, list.as_ptr()),
                    };
                }
            }
            // SAFETY: el objeto y el fd son de este hilo desde que arrancó.
            unsafe {
                if destination.is_some() { MIDIPortDispose(object) } else { MIDIEndpointDispose(object) };
                close(fd_r);
            }
        });
        Ok(file(fd_w))
    }

    pub fn watch() -> Result<std::fs::File, String> {
        client()?;
        let (fd_r, fd_w) = pair(SOCK_DGRAM, true, true, 4096)?;
        watchers().lock().unwrap().push(fd_w);
        Ok(file(fd_r))
    }
}

// ── Linux: el secuenciador de ALSA por dlopen (M466) ───────────────────────────────────────
//
// Cada puerto abierto es un CLIENTE propio del secuenciador con un solo puerto, usado desde un
// solo hilo: alsa-lib no promete que un `snd_seq_t` aguante dos hilos, y así no hace falta
// ningún cerrojo. Los mensajes viajan como eventos (`snd_seq_event_t`); `snd_midi_event_*`
// los convierte desde y hacia octetos MIDI.
//
// Las entradas usan un par SOCK_SEQPACKET en vez de datagramas: conserva los límites de cada
// mensaje y además avisa (POLLHUP) cuando el programa cierra su extremo, cosa que en Linux un
// par de datagramas no hace hasta que se le escribe.
#[cfg(target_os = "linux")]
mod alsaseq {
    use super::Port;
    use core::ffi::c_long;
    use std::ffi::c_void;
    use std::sync::{Mutex, OnceLock};

    unsafe extern "C" {
        fn dlopen(path: *const u8, flags: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, name: *const u8) -> *mut c_void;
        fn socketpair(domain: i32, ty: i32, protocol: i32, fds: *mut i32) -> i32;
        fn setsockopt(fd: i32, level: i32, name: i32, value: *const c_void, len: u32) -> i32;
        fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
        fn send(fd: i32, buf: *const u8, n: usize, flags: i32) -> isize;
        fn close(fd: i32) -> i32;
        // `n` como en `audio.rs` (una sola declaración de `poll` en el crate).
        fn poll(fds: *mut PollFd, n: u32, timeout_ms: i32) -> i32;
        fn clock_gettime(clock: i32, ts: *mut [i64; 2]) -> i32;
        #[link_name = "fcntl"]
        fn fcntl_raw(fd: i32, cmd: i32, ...) -> i32;
    }
    const RTLD_NOW: i32 = 2;
    const AF_UNIX: i32 = 1;
    const SOCK_STREAM: i32 = 1;
    const SOCK_SEQPACKET: i32 = 5;
    const SOL_SOCKET: i32 = 1;
    const SO_SNDBUF: i32 = 7;
    const SO_RCVBUF: i32 = 8;
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    const O_NONBLOCK: i32 = 0o4000;
    const MSG_DONTWAIT: i32 = 0x40;
    const MSG_NOSIGNAL: i32 = 0x4000;
    const CLOCK_MONOTONIC: i32 = 1;
    const POLLIN: i16 = 0x1;
    const POLLERR: i16 = 0x8;
    const POLLHUP: i16 = 0x10;
    const EAGAIN: i32 = 11;
    const ENOSPC: i32 = 28;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }

    type Seq = *mut c_void;

    /// `snd_seq_event_t` (28 octetos): tipo, banderas, etiqueta, cola, marca de tiempo, origen
    /// y destino (cliente, puerto) y 12 octetos de datos.
    #[repr(C)]
    struct Event {
        kind: u8,
        flags: u8,
        tag: u8,
        queue: u8,
        time: [u32; 2],
        source: [u8; 2],
        dest: [u8; 2],
        data: [u32; 3],
    }

    const OPEN_OUTPUT: i32 = 1;
    const OPEN_DUPLEX: i32 = 3;
    const NONBLOCK: i32 = 1;
    const CAP_READ: u32 = 1;
    const CAP_WRITE: u32 = 1 << 1;
    const CAP_SUBS_READ: u32 = 1 << 5;
    const CAP_SUBS_WRITE: u32 = 1 << 6;
    const CAP_NO_EXPORT: u32 = 1 << 7;
    const TYPE_MIDI_GENERIC: u32 = 1 << 1;
    const TYPE_APPLICATION: u32 = 1 << 20;
    const EVENT_NONE: u8 = 255;
    /// Los avisos del sistema (cliente 0, puerto 1): del arranque de un cliente (60) al cambio
    /// de un puerto (65).
    const ANNOUNCE_EVENTS: std::ops::RangeInclusive<u8> = 60..=65;
    const SYSTEM_CLIENT: i32 = 0;
    const ANNOUNCE_PORT: i32 = 1;
    const ADDRESS_SUBSCRIBERS: u8 = 254;
    const ADDRESS_UNKNOWN: u8 = 253;
    const QUEUE_DIRECT: u8 = 253;

    /// Las funciones de libasound que se usan, resueltas una vez.
    struct Lib {
        open: unsafe extern "C" fn(*mut Seq, *const u8, i32, i32) -> i32,
        close: unsafe extern "C" fn(Seq) -> i32,
        set_client_name: unsafe extern "C" fn(Seq, *const u8) -> i32,
        create_simple_port: unsafe extern "C" fn(Seq, *const u8, u32, u32) -> i32,
        connect_from: unsafe extern "C" fn(Seq, i32, i32, i32) -> i32,
        connect_to: unsafe extern "C" fn(Seq, i32, i32, i32) -> i32,
        event_input: unsafe extern "C" fn(Seq, *mut *mut Event) -> i32,
        event_output_direct: unsafe extern "C" fn(Seq, *mut Event) -> i32,
        poll_descriptors_count: unsafe extern "C" fn(Seq, i16) -> i32,
        poll_descriptors: unsafe extern "C" fn(Seq, *mut PollFd, u32, i16) -> i32,
        client_info_malloc: unsafe extern "C" fn(*mut *mut c_void) -> i32,
        client_info_free: unsafe extern "C" fn(*mut c_void),
        client_info_set_client: unsafe extern "C" fn(*mut c_void, i32),
        client_info_get_client: unsafe extern "C" fn(*const c_void) -> i32,
        client_info_get_name: unsafe extern "C" fn(*const c_void) -> *const u8,
        query_next_client: unsafe extern "C" fn(Seq, *mut c_void) -> i32,
        port_info_malloc: unsafe extern "C" fn(*mut *mut c_void) -> i32,
        port_info_free: unsafe extern "C" fn(*mut c_void),
        port_info_set_client: unsafe extern "C" fn(*mut c_void, i32),
        port_info_set_port: unsafe extern "C" fn(*mut c_void, i32),
        port_info_get_port: unsafe extern "C" fn(*const c_void) -> i32,
        port_info_get_name: unsafe extern "C" fn(*const c_void) -> *const u8,
        port_info_get_capability: unsafe extern "C" fn(*const c_void) -> u32,
        query_next_port: unsafe extern "C" fn(Seq, *mut c_void) -> i32,
        midi_event_new: unsafe extern "C" fn(usize, *mut *mut c_void) -> i32,
        midi_event_free: unsafe extern "C" fn(*mut c_void),
        midi_event_no_status: unsafe extern "C" fn(*mut c_void, i32),
        midi_event_reset_encode: unsafe extern "C" fn(*mut c_void),
        midi_event_decode: unsafe extern "C" fn(*mut c_void, *mut u8, c_long, *const Event) -> c_long,
        midi_event_encode: unsafe extern "C" fn(*mut c_void, *const u8, c_long, *mut Event) -> c_long,
    }

    /// El manejador de errores de alsa-lib: no imprime nada.
    unsafe extern "C" fn quiet_alsa(_file: *const u8, _line: i32, _function: *const u8, _err: i32, _fmt: *const u8) {}

    fn lib() -> Result<&'static Lib, String> {
        static LIB: OnceLock<Result<Lib, String>> = OnceLock::new();
        LIB.get_or_init(|| {
            // SAFETY: literal NUL-terminado; dlopen es seguro de llamar.
            let handle = unsafe { dlopen(b"libasound.so.2\0".as_ptr(), RTLD_NOW) };
            if handle.is_null() {
                return Err("midi: libasound.so.2 not found (install ALSA, e.g. libasound2)".to_string());
            }
            // alsa-lib escribe sus errores en stderr por su cuenta («open /dev/snd/seq failed»);
            // aquí cada fallo ya vuelve como `Err`, así que se le pone un manejador mudo.
            // SAFETY: el símbolo, si está, es `snd_lib_error_set_handler(handler)`; el manejador
            // es variádico en C y este ignora todos sus argumentos (los fijos y los de más).
            unsafe {
                let set = dlsym(handle, b"snd_lib_error_set_handler\0".as_ptr());
                if !set.is_null() {
                    let set: unsafe extern "C" fn(unsafe extern "C" fn(*const u8, i32, *const u8, i32, *const u8)) -> i32 = std::mem::transmute(set);
                    set(quiet_alsa);
                }
            }
            macro_rules! sym {
                ($name:literal) => {{
                    // SAFETY: el nombre termina en NUL; la firma del campo replica el header de
                    // ALSA (API estable), y un puntero a función tiene el tamaño de `*mut c_void`.
                    let p = unsafe { dlsym(handle, concat!($name, "\0").as_ptr()) };
                    if p.is_null() {
                        return Err(format!("midi: libasound without {}", $name));
                    }
                    unsafe { std::mem::transmute::<*mut c_void, _>(p) }
                }};
            }
            Ok(Lib {
                open: sym!("snd_seq_open"),
                close: sym!("snd_seq_close"),
                set_client_name: sym!("snd_seq_set_client_name"),
                create_simple_port: sym!("snd_seq_create_simple_port"),
                connect_from: sym!("snd_seq_connect_from"),
                connect_to: sym!("snd_seq_connect_to"),
                event_input: sym!("snd_seq_event_input"),
                event_output_direct: sym!("snd_seq_event_output_direct"),
                poll_descriptors_count: sym!("snd_seq_poll_descriptors_count"),
                poll_descriptors: sym!("snd_seq_poll_descriptors"),
                client_info_malloc: sym!("snd_seq_client_info_malloc"),
                client_info_free: sym!("snd_seq_client_info_free"),
                client_info_set_client: sym!("snd_seq_client_info_set_client"),
                client_info_get_client: sym!("snd_seq_client_info_get_client"),
                client_info_get_name: sym!("snd_seq_client_info_get_name"),
                query_next_client: sym!("snd_seq_query_next_client"),
                port_info_malloc: sym!("snd_seq_port_info_malloc"),
                port_info_free: sym!("snd_seq_port_info_free"),
                port_info_set_client: sym!("snd_seq_port_info_set_client"),
                port_info_set_port: sym!("snd_seq_port_info_set_port"),
                port_info_get_port: sym!("snd_seq_port_info_get_port"),
                port_info_get_name: sym!("snd_seq_port_info_get_name"),
                port_info_get_capability: sym!("snd_seq_port_info_get_capability"),
                query_next_port: sym!("snd_seq_query_next_port"),
                midi_event_new: sym!("snd_midi_event_new"),
                midi_event_free: sym!("snd_midi_event_free"),
                midi_event_no_status: sym!("snd_midi_event_no_status"),
                midi_event_reset_encode: sym!("snd_midi_event_reset_encode"),
                midi_event_decode: sym!("snd_midi_event_decode"),
                midi_event_encode: sym!("snd_midi_event_encode"),
            })
        })
        .as_ref()
        .map_err(|e| e.clone())
    }

    fn cstring(s: &str) -> Vec<u8> {
        s.bytes().filter(|&b| b != 0).chain(std::iter::once(0)).collect()
    }

    fn text(p: *const u8) -> String {
        if p.is_null() {
            return String::new();
        }
        // SAFETY: alsa-lib devuelve una cadena NUL-terminada que vive mientras viva el info.
        unsafe { std::ffi::CStr::from_ptr(p as *const core::ffi::c_char) }.to_string_lossy().into_owned()
    }

    /// Abre un cliente del secuenciador llamado `name`.
    fn open_client(lib: &Lib, streams: i32, mode: i32, name: &str) -> Result<Seq, String> {
        let mut seq: Seq = std::ptr::null_mut();
        // SAFETY: salida y literal válidos.
        let st = unsafe { (lib.open)(&mut seq, b"default\0".as_ptr(), streams, mode) };
        if st < 0 || seq.is_null() {
            return Err(format!("midi: could not open the ALSA sequencer (snd_seq_open, {st}); is /dev/snd/seq there and readable?"));
        }
        let name = cstring(name);
        // SAFETY: `seq` recién abierto; `name` termina en NUL.
        unsafe { (lib.set_client_name)(seq, name.as_ptr()) };
        Ok(seq)
    }

    /// El cliente que solo consulta (listar puertos), compartido tras un cerrojo.
    fn lister(lib: &Lib) -> Result<&'static Mutex<usize>, String> {
        static LISTER: OnceLock<Result<Mutex<usize>, String>> = OnceLock::new();
        LISTER.get_or_init(|| open_client(lib, OPEN_OUTPUT, 0, "raylang").map(|seq| Mutex::new(seq as usize))).as_ref().map_err(|e| e.clone())
    }

    /// Los hilos lectores vivos: el extremo que vigilan y la marca de «ya solté el cliente».
    fn readers() -> &'static Mutex<Vec<(i32, std::sync::Arc<std::sync::atomic::AtomicBool>)>> {
        static R: OnceLock<Mutex<Vec<(i32, std::sync::Arc<std::sync::atomic::AtomicBool>)>>> = OnceLock::new();
        R.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// Espera (un momento, acotado) a que los hilos cuya entrada ya cerró el programa suelten su
    /// cliente: así un `close(h)` seguido de `outputs()` no ve todavía el puerto recién cerrado.
    fn settle() {
        let snapshot: Vec<_> = readers().lock().unwrap_or_else(|e| e.into_inner()).clone();
        for (fd, done) in snapshot {
            let mut pfd = PollFd { fd, events: 0, revents: 0 };
            // SAFETY: `pfd` es nuestro; un fd ya cerrado da POLLNVAL y se ignora.
            unsafe { poll(&mut pfd, 1, 0) };
            if pfd.revents & (POLLHUP | POLLERR) == 0 {
                continue;
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
            while !done.load(std::sync::atomic::Ordering::SeqCst) && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
        }
    }

    pub fn list(inputs: bool) -> Result<Vec<Port>, String> {
        let lib = lib()?;
        settle();
        let guard = lister(lib)?.lock().unwrap_or_else(|e| e.into_inner());
        let seq = *guard as Seq;
        let wanted = if inputs { CAP_READ | CAP_SUBS_READ } else { CAP_WRITE | CAP_SUBS_WRITE };
        let mut out = Vec::new();
        // SAFETY: los dos info son nuestros (malloc/free de alsa-lib) y `seq` vive tras el cerrojo.
        unsafe {
            let mut cinfo: *mut c_void = std::ptr::null_mut();
            let mut pinfo: *mut c_void = std::ptr::null_mut();
            if (lib.client_info_malloc)(&mut cinfo) < 0 || (lib.port_info_malloc)(&mut pinfo) < 0 {
                if !cinfo.is_null() {
                    (lib.client_info_free)(cinfo);
                }
                return Err("midi: out of memory".to_string());
            }
            (lib.client_info_set_client)(cinfo, -1);
            while (lib.query_next_client)(seq, cinfo) >= 0 {
                let client = (lib.client_info_get_client)(cinfo);
                if client == SYSTEM_CLIENT {
                    continue; // el reloj y los avisos del sistema no son puertos MIDI
                }
                let client_name = text((lib.client_info_get_name)(cinfo));
                (lib.port_info_set_client)(pinfo, client);
                (lib.port_info_set_port)(pinfo, -1);
                while (lib.query_next_port)(seq, pinfo) >= 0 {
                    let caps = (lib.port_info_get_capability)(pinfo);
                    if caps & CAP_NO_EXPORT != 0 || caps & wanted != wanted {
                        continue;
                    }
                    let port = (lib.port_info_get_port)(pinfo);
                    let port_name = text((lib.port_info_get_name)(pinfo));
                    let name = if port_name.is_empty() {
                        client_name.clone()
                    } else if client_name.is_empty() || port_name.contains(&client_name) {
                        port_name
                    } else {
                        format!("{client_name} {port_name}")
                    };
                    out.push((((client as i64) << 8) | port as i64, name));
                }
            }
            (lib.port_info_free)(pinfo);
            (lib.client_info_free)(cinfo);
        }
        Ok(out)
    }

    /// ¿Existe el puerto `id` con la capacidad pedida?
    fn check(inputs: bool, id: i64) -> Result<(i32, i32), String> {
        if list(inputs)?.iter().any(|(p, _)| *p == id) {
            Ok(((id >> 8) as i32, (id & 0xff) as i32))
        } else {
            Err(format!("midi: no {} port with id {id} (was it disconnected?)", if inputs { "input" } else { "output" }))
        }
    }

    fn pair(kind: i32, reader_nonblocking: bool, buffer: i32) -> Result<(i32, i32), String> {
        let mut fds = [0i32; 2];
        // SAFETY: socketpair escribe dos fds válidos.
        if unsafe { socketpair(AF_UNIX, kind, 0, fds.as_mut_ptr()) } != 0 {
            return Err(format!("midi: could not create the pipe: {}", std::io::Error::last_os_error()));
        }
        let (fd_r, fd_w) = (fds[0], fds[1]);
        // SAFETY: fds recién creados y nuestros; los valores viven durante cada llamada.
        unsafe {
            let v = &buffer as *const i32 as *const c_void;
            for fd in [fd_r, fd_w] {
                setsockopt(fd, SOL_SOCKET, SO_SNDBUF, v, 4);
                setsockopt(fd, SOL_SOCKET, SO_RCVBUF, v, 4);
            }
            for (fd, nonblocking) in [(fd_r, reader_nonblocking), (fd_w, true)] {
                if nonblocking {
                    let fl = fcntl_raw(fd, F_GETFL);
                    fcntl_raw(fd, F_SETFL, fl | O_NONBLOCK);
                }
            }
        }
        Ok((fd_r, fd_w))
    }

    fn file(fd: i32) -> std::fs::File {
        // SAFETY: `fd` es nuestro; File toma la propiedad (su Drop lo cierra).
        unsafe { std::os::unix::io::FromRawFd::from_raw_fd(fd) }
    }

    fn now_ns() -> u64 {
        let mut ts = [0i64; 2];
        // SAFETY: `ts` es un `timespec` de 64 bits nuestro.
        unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
        ts[0] as u64 * 1_000_000_000 + ts[1] as u64
    }

    /// Qué entrega el hilo lector de un cliente.
    #[derive(Clone, Copy, PartialEq)]
    enum Feed {
        /// Los mensajes MIDI que llegan al puerto: `[ns: 8 LE][octetos]`.
        Messages,
        /// Un octeto por cada aviso del sistema (un cliente o un puerto aparece o desaparece).
        Changes,
    }

    /// El hilo de una entrada o de un `watch`: espera eventos del secuenciador y los entrega por
    /// `fd_w`; termina cuando el programa cierra su extremo (POLLHUP) y entonces suelta el
    /// cliente, con lo que el puerto desaparece para los demás.
    fn reader_thread(lib: &'static Lib, seq: usize, fd_w: i32, feed: Feed) {
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        readers().lock().unwrap_or_else(|e| e.into_inner()).push((fd_w, done.clone()));
        std::thread::spawn(move || {
            let seq = seq as Seq;
            let mut decoder: *mut c_void = std::ptr::null_mut();
            // SAFETY: el decodificador es de este hilo; sin estado en curso (cada mensaje sale
            // con su octeto de estado).
            unsafe {
                if (lib.midi_event_new)(256, &mut decoder) >= 0 && !decoder.is_null() {
                    (lib.midi_event_no_status)(decoder, 1);
                }
            }
            // SAFETY: consulta sobre el cliente de este hilo.
            let n = unsafe { (lib.poll_descriptors_count)(seq, POLLIN) }.max(0) as usize;
            let mut fds = vec![PollFd { fd: -1, events: 0, revents: 0 }; n + 1];
            // SAFETY: hay sitio para `n` descriptores.
            unsafe { (lib.poll_descriptors)(seq, fds.as_mut_ptr(), n as u32, POLLIN) };
            fds[n] = PollFd { fd: fd_w, events: 0, revents: 0 }; // solo para POLLHUP/POLLERR
            let mut buf = vec![0u8; 64 * 1024];
            let mut record = Vec::with_capacity(256);
            'serve: loop {
                // SAFETY: `fds` es nuestro y vive durante la llamada.
                let ready = unsafe { poll(fds.as_mut_ptr(), fds.len() as u32, -1) };
                if ready < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                if fds[n].revents & (POLLHUP | POLLERR) != 0 {
                    break; // el programa cerró la entrada
                }
                loop {
                    let mut ev: *mut Event = std::ptr::null_mut();
                    // SAFETY: `ev` recibe un evento que vive hasta la siguiente llamada.
                    let st = unsafe { (lib.event_input)(seq, &mut ev) };
                    if st == -ENOSPC {
                        continue; // el búfer del cliente se desbordó: se perdieron eventos
                    }
                    if st == -EAGAIN {
                        break;
                    }
                    if st < 0 || ev.is_null() {
                        break 'serve;
                    }
                    record.clear();
                    match feed {
                        Feed::Messages => {
                            if decoder.is_null() {
                                continue;
                            }
                            // SAFETY: `buf` es nuestro; el evento vive hasta el próximo input.
                            let len = unsafe { (lib.midi_event_decode)(decoder, buf.as_mut_ptr(), buf.len() as c_long, ev) };
                            if len <= 0 {
                                continue; // no es un mensaje MIDI (una suscripción, p. ej.)
                            }
                            record.extend_from_slice(&now_ns().to_le_bytes());
                            record.extend_from_slice(&buf[..len as usize]);
                        }
                        Feed::Changes => {
                            // SAFETY: lee el tipo del evento recién entregado.
                            if !ANNOUNCE_EVENTS.contains(&unsafe { (*ev).kind }) {
                                continue;
                            }
                            record.push(1);
                        }
                    }
                    // SAFETY: `record` vive durante la llamada; `fd_w` es de este hilo.
                    let sent = unsafe { send(fd_w, record.as_ptr(), record.len(), MSG_DONTWAIT | MSG_NOSIGNAL) };
                    if sent < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::WouldBlock {
                        break 'serve; // el programa cerró (EPIPE); con la cola llena el mensaje se pierde
                    }
                }
            }
            // SAFETY: el decodificador, el cliente y el fd son de este hilo.
            unsafe {
                if !decoder.is_null() {
                    (lib.midi_event_free)(decoder);
                }
                (lib.close)(seq);
            }
            // Fuera del registro ANTES de cerrar el fd (el SO lo reutiliza enseguida).
            readers().lock().unwrap_or_else(|e| e.into_inner()).retain(|(fd, _)| *fd != fd_w);
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            // SAFETY: el fd es de este hilo.
            unsafe { close(fd_w) };
        });
    }

    /// Abre la entrada con id `id`, o crea una virtual llamada `name` si `id` es `None`.
    pub fn open_input(id: Option<i64>, name: &str) -> Result<std::fs::File, String> {
        let lib = lib()?;
        let source = match id {
            Some(id) => Some(check(true, id)?),
            None => None,
        };
        let label = if name.is_empty() { "raylang input" } else { name };
        let seq = open_client(lib, OPEN_DUPLEX, NONBLOCK, if source.is_some() { "raylang" } else { label })?;
        let port_name = cstring(label);
        // Una entrada virtual se anuncia (los demás la ven y se suscriben); la de un puerto real
        // es un extremo privado que se conecta a su fuente.
        let caps = if source.is_some() { CAP_WRITE | CAP_NO_EXPORT } else { CAP_WRITE | CAP_SUBS_WRITE };
        // SAFETY: cliente recién abierto y nuestro; el nombre termina en NUL.
        let port = unsafe { (lib.create_simple_port)(seq, port_name.as_ptr(), caps, TYPE_MIDI_GENERIC | TYPE_APPLICATION) };
        let connected = if port < 0 {
            Err(format!("midi: could not open the input (snd_seq_create_simple_port, {port})"))
        } else if let Some((client, source_port)) = source {
            // SAFETY: el puerto es nuestro.
            match unsafe { (lib.connect_from)(seq, port, client, source_port) } {
                st if st < 0 => Err(format!("midi: could not connect to the input (snd_seq_connect_from, {st})")),
                _ => Ok(()),
            }
        } else {
            Ok(())
        };
        let fds = connected.and_then(|()| pair(SOCK_SEQPACKET, true, 256 * 1024));
        match fds {
            Ok((fd_r, fd_w)) => {
                reader_thread(lib, seq as usize, fd_w, Feed::Messages);
                Ok(file(fd_r))
            }
            Err(e) => {
                // SAFETY: el cliente es nuestro y nadie más lo conoce.
                unsafe { (lib.close)(seq) };
                Err(e)
            }
        }
    }

    pub fn watch() -> Result<std::fs::File, String> {
        let lib = lib()?;
        let seq = open_client(lib, OPEN_DUPLEX, NONBLOCK, "raylang")?;
        // SAFETY: cliente recién abierto y nuestro; literal NUL-terminado.
        let port = unsafe { (lib.create_simple_port)(seq, b"raylang watch\0".as_ptr(), CAP_WRITE | CAP_NO_EXPORT, TYPE_APPLICATION) };
        let connected = if port < 0 {
            Err(format!("midi: could not watch the ports (snd_seq_create_simple_port, {port})"))
        } else {
            // SAFETY: el puerto es nuestro; el de avisos del sistema existe siempre.
            match unsafe { (lib.connect_from)(seq, port, SYSTEM_CLIENT, ANNOUNCE_PORT) } {
                st if st < 0 => Err(format!("midi: could not watch the ports (snd_seq_connect_from, {st})")),
                _ => Ok(()),
            }
        };
        match connected.and_then(|()| pair(SOCK_SEQPACKET, true, 4096)) {
            Ok((fd_r, fd_w)) => {
                reader_thread(lib, seq as usize, fd_w, Feed::Changes);
                Ok(file(fd_r))
            }
            Err(e) => {
                // SAFETY: el cliente es nuestro y nadie más lo conoce.
                unsafe { (lib.close)(seq) };
                Err(e)
            }
        }
    }

    /// Lee exactamente `buf.len()` octetos del extremo bloqueante; `false` en EOF o error.
    fn read_exact(fd: i32, buf: &mut [u8]) -> bool {
        let mut got = 0;
        while got < buf.len() {
            // SAFETY: el tramo es nuestro y vive durante la llamada.
            let n = unsafe { read(fd, buf[got..].as_mut_ptr(), buf.len() - got) };
            if n > 0 {
                got += n as usize;
            } else if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            } else {
                return false;
            }
        }
        true
    }

    /// Abre la salida con id `id`, o crea una virtual llamada `name` si `id` es `None`. Un hilo
    /// lee las tramas `[longitud u16 LE][octetos]` del programa y las entrega a los suscriptores
    /// del puerto.
    pub fn open_output(id: Option<i64>, name: &str) -> Result<std::fs::File, String> {
        let lib = lib()?;
        let destination = match id {
            Some(id) => Some(check(false, id)?),
            None => None,
        };
        let label = if name.is_empty() { "raylang output" } else { name };
        let seq = open_client(lib, OPEN_OUTPUT, 0, if destination.is_some() { "raylang" } else { label })?;
        let port_name = cstring(label);
        let caps = if destination.is_some() { CAP_READ | CAP_NO_EXPORT } else { CAP_READ | CAP_SUBS_READ };
        // SAFETY: cliente recién abierto y nuestro; el nombre termina en NUL.
        let port = unsafe { (lib.create_simple_port)(seq, port_name.as_ptr(), caps, TYPE_MIDI_GENERIC | TYPE_APPLICATION) };
        let connected = if port < 0 {
            Err(format!("midi: could not open the output (snd_seq_create_simple_port, {port})"))
        } else if let Some((client, dest_port)) = destination {
            // SAFETY: el puerto es nuestro.
            match unsafe { (lib.connect_to)(seq, port, client, dest_port) } {
                st if st < 0 => Err(format!("midi: could not connect to the output (snd_seq_connect_to, {st})")),
                _ => Ok(()),
            }
        } else {
            Ok(())
        };
        // Flujo: el programa escribe tramas (con contrapresión) y el hilo las lee bloqueando.
        let (fd_r, fd_w) = match connected.and_then(|()| pair(SOCK_STREAM, false, 64 * 1024)) {
            Ok(p) => p,
            Err(e) => {
                // SAFETY: el cliente es nuestro y nadie más lo conoce.
                unsafe { (lib.close)(seq) };
                return Err(e);
            }
        };
        let seq_addr = seq as usize;
        std::thread::spawn(move || {
            let seq = seq_addr as Seq;
            let mut encoder: *mut c_void = std::ptr::null_mut();
            // SAFETY: el codificador es de este hilo; su búfer acota el SysEx más largo.
            unsafe { (lib.midi_event_new)(65536, &mut encoder) };
            let mut header = [0u8; 2];
            let mut frame = Vec::new();
            while read_exact(fd_r, &mut header) {
                frame.resize(u16::from_le_bytes(header) as usize, 0);
                if !read_exact(fd_r, &mut frame) {
                    break;
                }
                if encoder.is_null() {
                    continue;
                }
                // SAFETY: `frame` vive durante el bucle; el evento es nuestro y el codificador lo
                // rellena (un SysEx apunta al búfer del codificador, válido hasta la próxima
                // llamada, y se envía antes).
                unsafe {
                    (lib.midi_event_reset_encode)(encoder);
                    let mut at = 0usize;
                    while at < frame.len() {
                        let mut ev: Event = std::mem::zeroed();
                        let used = (lib.midi_event_encode)(encoder, frame[at..].as_ptr(), (frame.len() - at) as c_long, &mut ev);
                        if used <= 0 {
                            break;
                        }
                        at += used as usize;
                        if ev.kind == EVENT_NONE {
                            continue; // el mensaje aún no está completo
                        }
                        ev.source[1] = port as u8;
                        ev.dest = [ADDRESS_SUBSCRIBERS, ADDRESS_UNKNOWN];
                        ev.queue = QUEUE_DIRECT;
                        (lib.event_output_direct)(seq, &mut ev);
                    }
                }
            }
            // SAFETY: el codificador, el cliente y el fd son de este hilo desde que arrancó.
            unsafe {
                if !encoder.is_null() {
                    (lib.midi_event_free)(encoder);
                }
                (lib.close)(seq);
                close(fd_r);
            }
        });
        Ok(file(fd_w))
    }
}

// ── Windows: WinMM (M466) ───────────────────────────────────────────────────────────────────
//
// La API MIDI clásica de Windows (`winmm.dll`, siempre presente): `midiIn*` entrega cada mensaje
// en un callback y `midiOut*` los envía. Los dispositivos se numeran por posición (0..n): ese
// índice es el id del puerto, y cambia si se conecta o desconecta otro. No hay puertos
// virtuales (hace falta un driver de bucle, como loopMIDI) ni aviso de cambios sin una ventana:
// `watch` compara la lista de dispositivos una vez por segundo.
//
// La entrada escribe en el pipe con nombre en modo mensaje de `audio::win_pipe` (un mensaje MIDI
// por lectura, y el callback nunca espera); la salida lee tramas de un pipe anónimo.
#[cfg(windows)]
mod winmm {
    use super::Port;
    use crate::audio::win_pipe;
    use crate::audio::{SourceEnd, Wrote};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    type Handle = isize;

    #[repr(C)]
    struct MidiInCaps {
        mid: u16,
        pid: u16,
        driver_version: u32,
        name: [u16; 32],
        support: u32,
    }
    #[repr(C)]
    struct MidiOutCaps {
        mid: u16,
        pid: u16,
        driver_version: u32,
        name: [u16; 32],
        technology: u16,
        voices: u16,
        notes: u16,
        channel_mask: u16,
        support: u32,
    }
    /// `MIDIHDR`: el búfer de un mensaje largo (SysEx).
    #[repr(C)]
    struct MidiHdr {
        data: *mut u8,
        buffer_length: u32,
        bytes_recorded: u32,
        user: usize,
        flags: u32,
        next: *mut MidiHdr,
        reserved: usize,
        offset: u32,
        reserved2: [usize; 8],
    }
    const HDR_SIZE: u32 = std::mem::size_of::<MidiHdr>() as u32;

    type InProc = unsafe extern "system" fn(Handle, u32, usize, usize, usize);

    #[link(name = "winmm")]
    unsafe extern "system" {
        fn midiInGetNumDevs() -> u32;
        fn midiInGetDevCapsW(id: usize, caps: *mut MidiInCaps, size: u32) -> u32;
        fn midiInOpen(handle: *mut Handle, id: u32, callback: InProc, instance: usize, flags: u32) -> u32;
        fn midiInPrepareHeader(handle: Handle, hdr: *mut MidiHdr, size: u32) -> u32;
        fn midiInUnprepareHeader(handle: Handle, hdr: *mut MidiHdr, size: u32) -> u32;
        fn midiInAddBuffer(handle: Handle, hdr: *mut MidiHdr, size: u32) -> u32;
        fn midiInStart(handle: Handle) -> u32;
        fn midiInReset(handle: Handle) -> u32;
        fn midiInClose(handle: Handle) -> u32;
        fn midiOutGetNumDevs() -> u32;
        fn midiOutGetDevCapsW(id: usize, caps: *mut MidiOutCaps, size: u32) -> u32;
        fn midiOutOpen(handle: *mut Handle, id: u32, callback: usize, instance: usize, flags: u32) -> u32;
        fn midiOutShortMsg(handle: Handle, msg: u32) -> u32;
        fn midiOutPrepareHeader(handle: Handle, hdr: *mut MidiHdr, size: u32) -> u32;
        fn midiOutUnprepareHeader(handle: Handle, hdr: *mut MidiHdr, size: u32) -> u32;
        fn midiOutLongMsg(handle: Handle, hdr: *mut MidiHdr, size: u32) -> u32;
        fn midiOutReset(handle: Handle) -> u32;
        fn midiOutClose(handle: Handle) -> u32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn QueryPerformanceCounter(count: *mut i64) -> i32;
        fn QueryPerformanceFrequency(freq: *mut i64) -> i32;
    }
    const CALLBACK_FUNCTION: u32 = 0x0003_0000;
    const MIM_DATA: u32 = 0x3C3;
    const MIM_LONGDATA: u32 = 0x3C4;
    const MIDIERR_STILLPLAYING: u32 = 65;
    /// El trozo en que llega un SysEx; uno más largo llega en varios mensajes.
    const SYSEX_CHUNK: usize = 4096;

    fn now_ns() -> u64 {
        let (mut count, mut freq) = (0i64, 0i64);
        // SAFETY: las dos llamadas solo escriben un i64 propio.
        unsafe {
            QueryPerformanceCounter(&mut count);
            QueryPerformanceFrequency(&mut freq);
        }
        (count as u128 * 1_000_000_000 / freq.max(1) as u128) as u64
    }

    fn wide_name(name: &[u16; 32]) -> String {
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        String::from_utf16_lossy(&name[..end])
    }

    pub fn list(inputs: bool) -> Result<Vec<Port>, String> {
        let mut out = Vec::new();
        // SAFETY: cada consulta rellena una estructura nuestra del tamaño declarado.
        unsafe {
            if inputs {
                for i in 0..midiInGetNumDevs() {
                    let mut caps: MidiInCaps = std::mem::zeroed();
                    if midiInGetDevCapsW(i as usize, &mut caps, std::mem::size_of::<MidiInCaps>() as u32) == 0 {
                        out.push((i as i64, wide_name(&caps.name)));
                    }
                }
            } else {
                for i in 0..midiOutGetNumDevs() {
                    let mut caps: MidiOutCaps = std::mem::zeroed();
                    if midiOutGetDevCapsW(i as usize, &mut caps, std::mem::size_of::<MidiOutCaps>() as u32) == 0 {
                        out.push((i as i64, wide_name(&caps.name)));
                    }
                }
            }
        }
        Ok(out)
    }

    const NO_VIRTUAL: &str = "midi: Windows has no virtual MIDI ports (install a loopback driver such as loopMIDI and open its port)";

    /// El estado de una entrada: lo que usa el callback.
    struct Input {
        end: SourceEnd,
        closed: AtomicBool,
    }

    /// Cuántos octetos ocupa el mensaje corto que empieza por `status` (0 = no es un estado).
    fn short_len(status: u8) -> usize {
        match status {
            0x80..=0xBF | 0xE0..=0xEF | 0xF2 => 3,
            0xC0..=0xDF | 0xF1 | 0xF3 => 2,
            0xF6 | 0xF8..=0xFF => 1,
            _ => 0,
        }
    }

    fn deliver(input: &Input, data: &[u8]) {
        let mut record = Vec::with_capacity(8 + data.len());
        record.extend_from_slice(&now_ns().to_le_bytes());
        record.extend_from_slice(data);
        if let Wrote::Closed = win_pipe::write_block(input.end, &record) {
            input.closed.store(true, Ordering::SeqCst);
        }
    }

    /// El callback de WinMM, en un hilo del sistema: un mensaje por llamada. Nunca espera.
    unsafe extern "system" fn on_input(handle: Handle, msg: u32, instance: usize, p1: usize, _p2: usize) {
        // SAFETY: `instance` es el `Arc<Input>` filtrado al abrir; vive hasta que el vigilante
        // cierra el dispositivo (ya sin callbacks).
        let input = unsafe { &*(instance as *const Input) };
        if input.closed.load(Ordering::SeqCst) {
            return;
        }
        match msg {
            MIM_DATA => {
                let bytes = (p1 as u32).to_le_bytes();
                let len = short_len(bytes[0]);
                if len > 0 {
                    deliver(input, &bytes[..len]);
                }
            }
            MIM_LONGDATA => {
                let hdr = p1 as *mut MidiHdr;
                // SAFETY: WinMM devuelve el búfer que le prestamos, con lo grabado en él.
                unsafe {
                    let recorded = (*hdr).bytes_recorded as usize;
                    if recorded > 0 {
                        deliver(input, std::slice::from_raw_parts((*hdr).data, recorded));
                        // Se devuelve al dispositivo para el siguiente trozo. (Con `midiInReset`
                        // llega vacío, y entonces no se vuelve a prestar.)
                        midiInAddBuffer(handle, hdr, HDR_SIZE);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn open_input(id: Option<i64>, _name: &str) -> Result<std::fs::File, String> {
        let Some(id) = id else {
            return Err(NO_VIRTUAL.to_string());
        };
        // SAFETY: consulta sin punteros.
        if id < 0 || id >= unsafe { midiInGetNumDevs() } as i64 {
            return Err(format!("midi: no input port with id {id} (was it disconnected?)"));
        }
        let (reader, end) = win_pipe::create_named("midi", 256 * 1024).map_err(|e| format!("midi: {e}"))?;
        let input = Arc::new(Input { end, closed: AtomicBool::new(false) });
        let instance = Arc::into_raw(input.clone()) as usize;
        let mut handle: Handle = 0;
        // SAFETY: salida válida; el callback y `instance` viven hasta `midiInClose`.
        let st = unsafe { midiInOpen(&mut handle, id as u32, on_input, instance, CALLBACK_FUNCTION) };
        if st != 0 {
            // SAFETY: nada más referencia el `Arc` filtrado ni el extremo.
            unsafe { drop(Arc::from_raw(instance as *const Input)) };
            win_pipe::close_end(end);
            return Err(format!("midi: could not open the input (midiInOpen, {st}); is another program using it?"));
        }
        // Un búfer para los SysEx, prestado al dispositivo; vive en el montón hasta el cierre.
        let buffer = Box::into_raw(vec![0u8; SYSEX_CHUNK].into_boxed_slice()) as *mut u8;
        // SAFETY: el header y su búfer son nuestros y viven hasta que el vigilante los suelta.
        let hdr = unsafe {
            let hdr = Box::into_raw(Box::new(MidiHdr { data: buffer, buffer_length: SYSEX_CHUNK as u32, ..std::mem::zeroed() }));
            midiInPrepareHeader(handle, hdr, HDR_SIZE);
            midiInAddBuffer(handle, hdr, HDR_SIZE);
            midiInStart(handle);
            hdr as usize
        };
        let buffer = buffer as usize;
        // El vigilante: cuando el programa cierra la entrada, para y cierra el dispositivo (no
        // se puede desde el callback).
        std::thread::spawn(move || {
            while !input.closed.load(Ordering::SeqCst) && win_pipe::peer_alive(input.end) {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            input.closed.store(true, Ordering::SeqCst);
            // SAFETY: `midiInReset` devuelve el búfer prestado y `midiInClose` espera a los
            // callbacks en curso: después nadie más usa el header, el búfer ni `instance`.
            unsafe {
                midiInReset(handle);
                midiInUnprepareHeader(handle, hdr as *mut MidiHdr, HDR_SIZE);
                midiInClose(handle);
                drop(Box::from_raw(hdr as *mut MidiHdr));
                drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(buffer as *mut u8, SYSEX_CHUNK)));
                drop(Arc::from_raw(instance as *const Input));
            }
            win_pipe::close_end(input.end);
        });
        Ok(reader)
    }

    /// Envía los mensajes de una trama: cada SysEx (F0…F7) como mensaje largo, el resto como
    /// mensajes cortos. Los octetos sueltos sin estado se ignoran (no hay *running status*).
    fn send_frame(handle: Handle, frame: &mut [u8]) {
        let mut at = 0;
        while at < frame.len() {
            let status = frame[at];
            if status == 0xF0 {
                let end = frame[at..].iter().position(|&b| b == 0xF7).map_or(frame.len(), |i| at + i + 1);
                // SAFETY: el header apunta a un tramo de `frame`, que vive hasta que WinMM lo
                // suelta (se espera a `MIDIERR_STILLPLAYING` antes de seguir).
                unsafe {
                    let mut hdr = MidiHdr { data: frame[at..].as_mut_ptr(), buffer_length: (end - at) as u32, ..std::mem::zeroed() };
                    if midiOutPrepareHeader(handle, &mut hdr, HDR_SIZE) == 0 {
                        midiOutLongMsg(handle, &mut hdr, HDR_SIZE);
                        while midiOutUnprepareHeader(handle, &mut hdr, HDR_SIZE) == MIDIERR_STILLPLAYING {
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                    }
                }
                at = end;
                continue;
            }
            let len = short_len(status);
            if len == 0 || at + len > frame.len() {
                at += 1;
                continue;
            }
            let mut packed = [0u8; 4];
            packed[..len].copy_from_slice(&frame[at..at + len]);
            // SAFETY: dispositivo abierto por este hilo.
            unsafe { midiOutShortMsg(handle, u32::from_le_bytes(packed)) };
            at += len;
        }
    }

    pub fn open_output(id: Option<i64>, _name: &str) -> Result<std::fs::File, String> {
        use std::io::Read;
        use std::os::windows::io::OwnedHandle;
        let Some(id) = id else {
            return Err(NO_VIRTUAL.to_string());
        };
        // SAFETY: consulta sin punteros.
        if id < 0 || id >= unsafe { midiOutGetNumDevs() } as i64 {
            return Err(format!("midi: no output port with id {id} (was it disconnected?)"));
        }
        let mut handle: Handle = 0;
        // SAFETY: salida válida; sin callback.
        let st = unsafe { midiOutOpen(&mut handle, id as u32, 0, 0, 0) };
        if st != 0 {
            return Err(format!("midi: could not open the output (midiOutOpen, {st}); is another program using it?"));
        }
        let (r, w) = match std::io::pipe() {
            Ok(p) => p,
            Err(e) => {
                // SAFETY: el dispositivo es nuestro y nadie más lo conoce.
                unsafe { midiOutClose(handle) };
                return Err(format!("midi: could not create the pipe: {e}"));
            }
        };
        let mut reader = std::fs::File::from(OwnedHandle::from(r));
        std::thread::spawn(move || {
            let mut header = [0u8; 2];
            let mut frame = Vec::new();
            while reader.read_exact(&mut header).is_ok() {
                frame.resize(u16::from_le_bytes(header) as usize, 0);
                if reader.read_exact(&mut frame).is_err() {
                    break;
                }
                send_frame(handle, &mut frame);
            }
            // SAFETY: el dispositivo es de este hilo desde que arrancó.
            unsafe {
                midiOutReset(handle);
                midiOutClose(handle);
            }
        });
        Ok(std::fs::File::from(OwnedHandle::from(w)))
    }

    /// WinMM no avisa de los cambios sin una ventana: un hilo compara la lista de dispositivos
    /// una vez por segundo y escribe un octeto cuando cambia.
    pub fn watch() -> Result<std::fs::File, String> {
        let (reader, end) = win_pipe::create_named("midi", 4096).map_err(|e| format!("midi: {e}"))?;
        std::thread::spawn(move || {
            let snapshot = || (list(true).unwrap_or_default(), list(false).unwrap_or_default());
            let mut last = snapshot();
            let mut ticks = 0u32;
            while win_pipe::peer_alive(end) {
                std::thread::sleep(std::time::Duration::from_millis(100));
                ticks += 1;
                if ticks % 10 != 0 {
                    continue;
                }
                let now = snapshot();
                if now != last {
                    last = now;
                    if let Wrote::Closed = win_pipe::write_block(end, &[1]) {
                        break;
                    }
                }
            }
            win_pipe::close_end(end);
        });
        Ok(reader)
    }
}

// ── Android: `android.media.midi` a través del shell (M467) ─────────────────────────────────
//
// La API nativa (AMidi) no sabe listar ni abrir dispositivos: eso es de `MidiManager`, en Java,
// y un `AMidiDevice` se obtiene de un objeto Java. En vez de cruzar objetos por JNI, todo el
// MIDI vive en `RayMidi` (el shell) y aquí se le habla por `ui::shell_request`, que ya existe:
//
//   - `midi_list in|out` → una línea `id\tnombre` por puerto.
//   - `midi_open_in id` / `midi_open_out id` / `midi_watch` → un testigo del puerto abierto.
//   - `midi_read testigo` → sondeo largo: una línea `ns hex` por mensaje, o vacío a los 300 ms.
//   - `midi_send testigo hex`, `midi_close testigo`.
//
// Hacia el programa, lo de siempre: un par SOCK_SEQPACKET para las entradas (Android es el
// kernel de Linux: avisa con POLLHUP cuando el programa cierra) y uno de flujo para las salidas.
// No hay puertos virtuales: en Android exigen declarar un `MidiDeviceService` en la app.
#[cfg(target_os = "android")]
mod android {
    use super::Port;
    use std::ffi::c_void;

    unsafe extern "C" {
        fn socketpair(domain: i32, ty: i32, protocol: i32, fds: *mut i32) -> i32;
        fn setsockopt(fd: i32, level: i32, name: i32, value: *const c_void, len: u32) -> i32;
        fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
        fn send(fd: i32, buf: *const u8, n: usize, flags: i32) -> isize;
        fn close(fd: i32) -> i32;
        // `n` como en `audio.rs` (una sola declaración de `poll` en el crate).
        fn poll(fds: *mut PollFd, n: u32, timeout_ms: i32) -> i32;
        #[link_name = "fcntl"]
        fn fcntl_raw(fd: i32, cmd: i32, ...) -> i32;
    }
    const AF_UNIX: i32 = 1;
    const SOCK_STREAM: i32 = 1;
    const SOCK_SEQPACKET: i32 = 5;
    const SOL_SOCKET: i32 = 1;
    const SO_SNDBUF: i32 = 7;
    const SO_RCVBUF: i32 = 8;
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    const O_NONBLOCK: i32 = 0o4000;
    const MSG_DONTWAIT: i32 = 0x40;
    const MSG_NOSIGNAL: i32 = 0x4000;
    const POLLERR: i16 = 0x8;
    const POLLHUP: i16 = 0x10;

    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }

    const NO_VIRTUAL: &str = "midi: Android has no virtual MIDI ports for an app without a MidiDeviceService";

    /// Una petición a `RayMidi`. Sin shell (un binario suelto), `Err`.
    fn shell(op: &str, arg: &str) -> Result<String, String> {
        #[cfg(feature = "ui")]
        {
            crate::ui::shell_request(op, arg).map_err(|e| if e.starts_with("midi:") { e } else { format!("midi: {e}") })
        }
        #[cfg(not(feature = "ui"))]
        {
            let _ = (op, arg);
            Err("midi: on Android, MIDI needs the app shell (ray bundle --android)".to_string())
        }
    }

    pub fn list(inputs: bool) -> Result<Vec<Port>, String> {
        let text = shell("midi_list", if inputs { "in" } else { "out" })?;
        Ok(text
            .lines()
            .filter_map(|line| {
                let (id, name) = line.split_once('\t')?;
                Some((id.parse().ok()?, name.to_string()))
            })
            .collect())
    }

    fn pair(kind: i32, reader_nonblocking: bool, buffer: i32) -> Result<(i32, i32), String> {
        let mut fds = [0i32; 2];
        // SAFETY: socketpair escribe dos fds válidos.
        if unsafe { socketpair(AF_UNIX, kind, 0, fds.as_mut_ptr()) } != 0 {
            return Err(format!("midi: could not create the pipe: {}", std::io::Error::last_os_error()));
        }
        let (fd_r, fd_w) = (fds[0], fds[1]);
        // SAFETY: fds recién creados y nuestros; los valores viven durante cada llamada.
        unsafe {
            let v = &buffer as *const i32 as *const c_void;
            for fd in [fd_r, fd_w] {
                setsockopt(fd, SOL_SOCKET, SO_SNDBUF, v, 4);
                setsockopt(fd, SOL_SOCKET, SO_RCVBUF, v, 4);
            }
            for (fd, nonblocking) in [(fd_r, reader_nonblocking), (fd_w, true)] {
                if nonblocking {
                    let fl = fcntl_raw(fd, F_GETFL);
                    fcntl_raw(fd, F_SETFL, fl | O_NONBLOCK);
                }
            }
        }
        Ok((fd_r, fd_w))
    }

    fn file(fd: i32) -> std::fs::File {
        // SAFETY: `fd` es nuestro; File toma la propiedad (su Drop lo cierra).
        unsafe { std::os::unix::io::FromRawFd::from_raw_fd(fd) }
    }

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len() / 2).filter_map(|i| u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()).collect()
    }

    /// El hilo de una entrada o de un `watch`: sondea a `RayMidi` y entrega lo que llega; para
    /// cuando el programa cierra su extremo, y entonces suelta el puerto en el shell.
    fn reader_thread(token: String, fd_w: i32, messages: bool) {
        std::thread::spawn(move || {
            let mut record = Vec::with_capacity(256);
            'serve: loop {
                let mut pfd = PollFd { fd: fd_w, events: 0, revents: 0 };
                // SAFETY: `pfd` es nuestro y vive durante la llamada.
                unsafe { poll(&mut pfd, 1, 0) };
                if pfd.revents & (POLLHUP | POLLERR) != 0 {
                    break; // el programa cerró la entrada
                }
                let Ok(batch) = shell("midi_read", &token) else {
                    break; // el shell soltó el puerto (o ya no contesta)
                };
                for line in batch.lines() {
                    record.clear();
                    if messages {
                        let Some((ns, hex)) = line.split_once(' ') else { continue };
                        record.extend_from_slice(&ns.parse::<u64>().unwrap_or(0).to_le_bytes());
                        record.extend_from_slice(&unhex(hex));
                    } else {
                        record.push(1);
                    }
                    // SAFETY: `record` vive durante la llamada; `fd_w` es de este hilo.
                    let sent = unsafe { send(fd_w, record.as_ptr(), record.len(), MSG_DONTWAIT | MSG_NOSIGNAL) };
                    if sent < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::WouldBlock {
                        break 'serve;
                    }
                }
            }
            let _ = shell("midi_close", &token);
            // SAFETY: el fd es de este hilo.
            unsafe { close(fd_w) };
        });
    }

    pub fn open_input(id: Option<i64>, _name: &str) -> Result<std::fs::File, String> {
        let Some(id) = id else {
            return Err(NO_VIRTUAL.to_string());
        };
        let token = shell("midi_open_in", &id.to_string())?;
        match pair(SOCK_SEQPACKET, true, 256 * 1024) {
            Ok((fd_r, fd_w)) => {
                reader_thread(token, fd_w, true);
                Ok(file(fd_r))
            }
            Err(e) => {
                let _ = shell("midi_close", &token);
                Err(e)
            }
        }
    }

    pub fn watch() -> Result<std::fs::File, String> {
        let token = shell("midi_watch", "")?;
        match pair(SOCK_SEQPACKET, true, 4096) {
            Ok((fd_r, fd_w)) => {
                reader_thread(token, fd_w, false);
                Ok(file(fd_r))
            }
            Err(e) => {
                let _ = shell("midi_close", &token);
                Err(e)
            }
        }
    }

    /// Lee exactamente `buf.len()` octetos del extremo bloqueante; `false` en EOF o error.
    fn read_exact(fd: i32, buf: &mut [u8]) -> bool {
        let mut got = 0;
        while got < buf.len() {
            // SAFETY: el tramo es nuestro y vive durante la llamada.
            let n = unsafe { read(fd, buf[got..].as_mut_ptr(), buf.len() - got) };
            if n > 0 {
                got += n as usize;
            } else if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            } else {
                return false;
            }
        }
        true
    }

    pub fn open_output(id: Option<i64>, _name: &str) -> Result<std::fs::File, String> {
        let Some(id) = id else {
            return Err(NO_VIRTUAL.to_string());
        };
        let token = shell("midi_open_out", &id.to_string())?;
        let (fd_r, fd_w) = match pair(SOCK_STREAM, false, 64 * 1024) {
            Ok(p) => p,
            Err(e) => {
                let _ = shell("midi_close", &token);
                return Err(e);
            }
        };
        std::thread::spawn(move || {
            let mut header = [0u8; 2];
            let mut frame = Vec::new();
            let mut arg = String::new();
            while read_exact(fd_r, &mut header) {
                frame.resize(u16::from_le_bytes(header) as usize, 0);
                if !read_exact(fd_r, &mut frame) {
                    break;
                }
                arg.clear();
                arg.push_str(&token);
                arg.push(' ');
                for b in &frame {
                    arg.push_str(&format!("{b:02x}"));
                }
                if shell("midi_send", &arg).is_err() {
                    break; // el dispositivo se desconectó
                }
            }
            let _ = shell("midi_close", &token);
            // SAFETY: el fd es de este hilo desde que arrancó.
            unsafe { close(fd_r) };
        });
        Ok(file(fd_w))
    }
}

#[cfg(test)]
mod split_tests {
    use super::split_messages;

    fn split(packets: &[&[u8]]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut in_sysex = false;
        for p in packets {
            split_messages(p, &mut in_sysex, |m| out.push(m.to_vec()));
        }
        out
    }

    /// ray-daw #446: dos mensajes en un paquete son dos lecturas.
    #[test]
    fn a_packet_with_two_messages_gives_two() {
        assert_eq!(split(&[&[0x90, 60, 100, 0x80, 60, 0]]), vec![vec![0x90, 60, 100], vec![0x80, 60, 0]]);
        assert_eq!(split(&[&[0xC0, 5, 0xD0, 9, 0xE0, 1, 2]]), vec![vec![0xC0, 5], vec![0xD0, 9], vec![0xE0, 1, 2]]);
    }

    #[test]
    fn running_status_and_realtime_are_split_out() {
        assert_eq!(split(&[&[0x90, 60, 100, 62, 100]]), vec![vec![0x90, 60, 100], vec![0x90, 62, 100]]);
        assert_eq!(split(&[&[0x90, 60, 0xF8, 100, 0xFE]]), vec![vec![0xF8], vec![0x90, 60, 100], vec![0xFE]]);
    }

    #[test]
    fn a_sysex_stays_whole_or_comes_in_pieces() {
        assert_eq!(split(&[&[0xF0, 1, 2, 0xF7, 0x90, 60, 1]]), vec![vec![0xF0, 1, 2, 0xF7], vec![0x90, 60, 1]]);
        assert_eq!(split(&[&[0xF0, 1, 2], &[3, 4, 0xF7], &[0x80, 60, 0]]), vec![vec![0xF0, 1, 2], vec![3, 4, 0xF7], vec![0x80, 60, 0]]);
        // Un SysEx cortado por otro estado no se traga el mensaje siguiente.
        assert_eq!(split(&[&[0xF0, 1, 2, 0x90, 60, 1]]), vec![vec![0xF0, 1, 2], vec![0x90, 60, 1]]);
    }
}
