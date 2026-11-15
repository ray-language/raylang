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
//! Backend A MANO, cero crates, como el audio: CoreMIDI en macOS e iOS (el framework está
//! siempre presente). Linux (ALSA seq), Windows (WinMM) y Android (AMidi) no tienen backend
//! todavía: toda operación da `Err`.
//!
//! Va tras la feature `audio` (un build sin audio tampoco tiene MIDI).
#![cfg(all(feature = "audio", any(unix, windows)))]

/// Un puerto MIDI del sistema: su id estable (el *unique ID* de CoreMIDI) y su nombre.
pub type Port = (i64, String);

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
const UNSUPPORTED: &str = "midi: not supported on this platform yet (macOS and iOS only)";

/// Los puertos de entrada (`inputs = true`: fuentes de las que se puede leer) o de salida
/// (destinos a los que se puede enviar) presentes ahora.
pub fn list(inputs: bool) -> Result<Vec<Port>, String> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        coremidi::list(inputs)
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        let _ = inputs;
        Err(UNSUPPORTED.to_string())
    }
}

/// Abre la entrada `id` (de `list(true)`) y devuelve el extremo de LECTURA, no bloqueante: un
/// datagrama por mensaje.
pub fn open_input(id: i64) -> Result<std::fs::File, String> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        coremidi::open_input(Some(id), "")
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        let _ = id;
        Err(UNSUPPORTED.to_string())
    }
}

/// Crea una entrada VIRTUAL llamada `name`: otros programas la ven como un destino MIDI y lo
/// que le envían se lee del extremo devuelto.
pub fn create_input(name: &str) -> Result<std::fs::File, String> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        coremidi::open_input(None, name)
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        let _ = name;
        Err(UNSUPPORTED.to_string())
    }
}

/// Abre la salida `id` (de `list(false)`) y devuelve el extremo de ESCRITURA: tramas
/// `[longitud u16 LE][octetos MIDI]`.
pub fn open_output(id: i64) -> Result<std::fs::File, String> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        coremidi::open_output(Some(id), "")
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        let _ = id;
        Err(UNSUPPORTED.to_string())
    }
}

/// Crea una salida VIRTUAL llamada `name`: otros programas la ven como una fuente MIDI y
/// reciben lo que se escribe en el extremo devuelto.
pub fn create_output(name: &str) -> Result<std::fs::File, String> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        coremidi::open_output(None, name)
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        let _ = name;
        Err(UNSUPPORTED.to_string())
    }
}

/// Un extremo de lectura que recibe un datagrama (un octeto) cada vez que cambia el conjunto de
/// puertos: se conectó o se desconectó un dispositivo.
pub fn watch() -> Result<std::fs::File, String> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        coremidi::watch()
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        Err(UNSUPPORTED.to_string())
    }
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
            let alive = n == 1 || std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock;
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
        /// El puerto de entrada (entrada real) o el destino virtual: lo que se desmonta.
        object: Mutex<(MidiRef, bool)>,
    }

    /// Las entradas vivas, para desmontar las que el programa cerró (`sweep`).
    fn inputs_alive() -> &'static Mutex<Vec<usize>> {
        static I: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
        I.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// El callback de lectura de CoreMIDI: un datagrama por paquete, `[ns: 8 LE][octetos]`.
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
            for _ in 0..count {
                let stamp = std::ptr::read_unaligned(p as *const u64);
                let len = std::ptr::read_unaligned(p.add(8) as *const u16) as usize;
                let data = std::slice::from_raw_parts(p.add(10), len);
                let ticks = if stamp == 0 { mach_absolute_time() } else { stamp };
                record.clear();
                record.extend_from_slice(&ticks_to_ns(ticks).to_le_bytes());
                record.extend_from_slice(data);
                let n = write(input.fd_w, record.as_ptr(), record.len());
                if n < 0 {
                    let kind = std::io::Error::last_os_error().kind();
                    let full = kind == std::io::ErrorKind::WouldBlock || std::io::Error::last_os_error().raw_os_error() == Some(55); // ENOBUFS
                    if !full {
                        input.closed.store(true, Ordering::SeqCst); // el programa cerró la entrada
                        return;
                    }
                }
                // `MIDIPacketNext`: en arm64 los paquetes van alineados a 4 octetos.
                let next = p.add(10 + len);
                p = if cfg!(target_arch = "aarch64") { ((next as usize + 3) & !3) as *const u8 } else { next };
            }
        }
    }

    /// Desmonta las entradas cuyo lector ya cerró. Se llama al empezar cada operación: una
    /// entrada cerrada sin tráfico queda montada hasta la siguiente, lo que no cuesta nada.
    fn sweep() {
        let mut alive = inputs_alive().lock().unwrap_or_else(|e| e.into_inner());
        alive.retain(|&addr| {
            // SAFETY: cada dirección es un `Arc<Input>` filtrado y aún no soltado.
            let input = unsafe { &*(addr as *const Input) };
            if !input.closed.load(Ordering::SeqCst) {
                // Sin tráfico el callback no se entera del cierre: se sondea con un datagrama
                // VACÍO. Un lector vivo lo recibe y `std/midi` lo descarta (un mensaje lleva al
                // menos los 8 octetos de su marca de tiempo); aquí solo interesa si el otro
                // extremo sigue ahí.
                let probe: [u8; 0] = [];
                // SAFETY: longitud cero; el fd es nuestro.
                let n = unsafe { write(input.fd_w, probe.as_ptr(), 0) };
                let gone = n < 0 && {
                    let err = std::io::Error::last_os_error();
                    err.kind() != std::io::ErrorKind::WouldBlock && err.raw_os_error() != Some(55)
                };
                if !gone {
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
        let input = Arc::new(Input { fd_w, closed: AtomicBool::new(false), object: Mutex::new((0, source.is_none())) });
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
