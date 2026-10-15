//! «Now Playing» y controles multimedia del sistema (M403, IDEAS §112, rayplay #170).
//!
//! Un programa que suena con `std/audio` no aparece en el Centro de control ni en la pantalla de
//! bloqueo, y los botones de reproducir/pausa (auriculares, teclado, reloj) van a la última app que
//! sí lo hizo. `std/media` lo arregla con dos piezas: lo que suena (`now_playing`: título, artista,
//! álbum, duración, posición, estado, carátula) y las órdenes del sistema, que llegan al programa
//! como eventos `("media", 0, "play" | "pause" | "toggle" | "next" | "previous" | "stop" |
//! "seek:<ms>")` por la cola de `std/ui`.
//!
//! Backends: `MPNowPlayingInfoCenter` + `MPRemoteCommandCenter` (MediaPlayer.framework) en macOS e
//! iOS — el MISMO código Rust vía `objc_msgSend`, sin Objective-C intermedio (patrón de `ui.rs`);
//! en iOS la app necesita la sesión de audio `playback` activa (`[ios] background_audio = true`).
//! Android (`MediaSession`) y Linux (MPRIS) quedan para una entrega posterior: `Err` claro.
//!
//! Hilos: todo se despacha a la cola principal con `dispatch_async_f` (MediaPlayer lo exige para el
//! centro de órdenes); las órdenes llegan en el hilo principal y se encolan con `ui::push_event`,
//! que es seguro desde cualquier hilo. Sin asignaciones por evento salvo el tag.

#[cfg(all(feature = "ui", any(target_os = "macos", target_os = "ios")))]
mod apple {
    use std::ffi::{c_char, c_void};
    use std::sync::Mutex;

    type Id = *mut c_void;
    type Sel = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    struct DispatchQueue {
        _opaque: [u8; 0],
    }

    #[repr(C)]
    struct BlockLiteral {
        isa: *const c_void,
        flags: i32,
        reserved: i32,
        invoke: *const c_void,
        descriptor: *const BlockDescriptor,
    }
    #[repr(C)]
    struct BlockDescriptor {
        reserved: u64,
        size: u64,
    }
    unsafe impl Sync for BlockLiteral {}
    const BLOCK_IS_GLOBAL: i32 = 1 << 28;

    // Las clases llegan por el enlace del framework; los símbolos C de objc y libdispatch son la
    // única superficie declarada (patrón ui.rs/audio.rs: sin build.rs). Las claves del diccionario
    // de Now Playing son `NSString * const` exportadas por MediaPlayer.
    #[allow(clippy::duplicated_attributes)]
    #[link(name = "objc")]
    #[link(name = "Foundation", kind = "framework")]
    #[link(name = "MediaPlayer", kind = "framework")]
    unsafe extern "C" {
        fn objc_getClass(name: *const u8) -> Id;
        fn sel_registerName(name: *const u8) -> Sel;
        fn objc_msgSend();
        fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
        fn objc_registerClassPair(cls: Id);
        fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const c_char) -> u8;
        static _dispatch_main_q: DispatchQueue;
        fn dispatch_async_f(queue: *const DispatchQueue, ctx: *mut c_void, work: extern "C" fn(*mut c_void));
        static _NSConcreteGlobalBlock: c_void;
        static MPMediaItemPropertyTitle: Id;
        static MPMediaItemPropertyArtist: Id;
        static MPMediaItemPropertyAlbumTitle: Id;
        static MPMediaItemPropertyPlaybackDuration: Id;
        static MPMediaItemPropertyArtwork: Id;
        static MPNowPlayingInfoPropertyElapsedPlaybackTime: Id;
        static MPNowPlayingInfoPropertyPlaybackRate: Id;
    }

    type MsgId = unsafe extern "C" fn(Id, Sel) -> Id;
    type MsgVoid = unsafe extern "C" fn(Id, Sel);
    type MsgVoidId = unsafe extern "C" fn(Id, Sel, Id);
    type MsgVoidBool = unsafe extern "C" fn(Id, Sel, u8);
    #[cfg(target_os = "macos")]
    type MsgVoidInt = unsafe extern "C" fn(Id, Sel, i64);
    type MsgIdId = unsafe extern "C" fn(Id, Sel, Id) -> Id;
    type MsgIdDouble = unsafe extern "C" fn(Id, Sel, f64) -> Id;
    type MsgDouble = unsafe extern "C" fn(Id, Sel) -> f64;
    type MsgSize = unsafe extern "C" fn(Id, Sel) -> CGSize;
    type MsgVoidIdId = unsafe extern "C" fn(Id, Sel, Id, Id);
    type MsgIdIdSel = unsafe extern "C" fn(Id, Sel, Id, Sel) -> Id;
    type MsgInitBytes = unsafe extern "C" fn(Id, Sel, *const u8, usize, u64) -> Id;
    type MsgDataBytes = unsafe extern "C" fn(Id, Sel, *const u8, usize) -> Id;
    type MsgInitArtwork = unsafe extern "C" fn(Id, Sel, CGSize, *const BlockLiteral) -> Id;

    fn msg_send() -> *const c_void {
        objc_msgSend as unsafe extern "C" fn() as *const c_void
    }
    fn cls(name: &[u8]) -> Id {
        // SAFETY: literal NUL-terminado.
        unsafe { objc_getClass(name.as_ptr()) }
    }
    fn sel(name: &[u8]) -> Sel {
        // SAFETY: literal NUL-terminado.
        unsafe { sel_registerName(name.as_ptr()) }
    }
    unsafe fn nsstring(s: &str) -> Id {
        const NS_UTF8: u64 = 4;
        unsafe {
            let alloc: MsgId = std::mem::transmute(msg_send());
            let init: MsgInitBytes = std::mem::transmute(msg_send());
            init(alloc(cls(b"NSString\0"), sel(b"alloc\0")), sel(b"initWithBytes:length:encoding:\0"), s.as_ptr(), s.len(), NS_UTF8)
        }
    }
    unsafe fn nsnumber(v: f64) -> Id {
        unsafe {
            let make: MsgIdDouble = std::mem::transmute(msg_send());
            make(cls(b"NSNumber\0"), sel(b"numberWithDouble:\0"), v)
        }
    }
    unsafe fn release(obj: Id) {
        if !obj.is_null() {
            unsafe {
                let rel: MsgVoid = std::mem::transmute(msg_send());
                rel(obj, sel(b"release\0"));
            }
        }
    }

    fn on_main(f: impl FnOnce() + Send + 'static) {
        extern "C" fn trampoline(ctx: *mut c_void) {
            // SAFETY: `ctx` es el Box de abajo, entregado una sola vez por libdispatch.
            let f = unsafe { Box::from_raw(ctx as *mut Box<dyn FnOnce() + Send>) };
            f();
        }
        let boxed: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(f));
        // SAFETY: la cola principal es un static del sistema; el Box viaja al trampoline.
        unsafe { dispatch_async_f(&_dispatch_main_q, Box::into_raw(boxed) as *mut c_void, trampoline) };
    }

    /// La carátula vigente (NSImage/UIImage retenida) que devuelve el bloque de `MPMediaItemArtwork`.
    static ARTWORK: Mutex<usize> = Mutex::new(0);

    extern "C" fn artwork_invoke(_block: *const BlockLiteral, _size: CGSize) -> Id {
        *ARTWORK.lock().unwrap_or_else(|e| e.into_inner()) as Id
    }
    static ARTWORK_DESC: BlockDescriptor = BlockDescriptor { reserved: 0, size: std::mem::size_of::<BlockLiteral>() as u64 };
    static ARTWORK_BLOCK: BlockLiteral = BlockLiteral {
        // SAFETY: dirección de un static del sistema, solo se toma su dirección.
        isa: unsafe { &_NSConcreteGlobalBlock as *const c_void },
        flags: BLOCK_IS_GLOBAL,
        reserved: 0,
        invoke: artwork_invoke as *const c_void,
        descriptor: &ARTWORK_DESC,
    };

    fn center() -> Result<Id, String> {
        let c = cls(b"MPNowPlayingInfoCenter\0");
        if c.is_null() {
            return Err("media: MediaPlayer framework not available".to_string());
        }
        // SAFETY: mensaje de clase documentado.
        Ok(unsafe {
            let get: MsgId = std::mem::transmute(msg_send());
            get(c, sel(b"defaultCenter\0"))
        })
    }

    #[cfg(target_os = "macos")]
    const IMAGE_CLASS: &[u8] = b"NSImage\0";
    #[cfg(target_os = "ios")]
    const IMAGE_CLASS: &[u8] = b"UIImage\0";

    /// Una imagen retenida (+1) desde los octetos de un PNG/JPEG, o null si no se decodifica.
    unsafe fn image_from(data: &[u8]) -> Id {
        unsafe {
            let bytes: MsgDataBytes = std::mem::transmute(msg_send());
            let nsdata = bytes(cls(b"NSData\0"), sel(b"dataWithBytes:length:\0"), data.as_ptr(), data.len());
            let alloc: MsgId = std::mem::transmute(msg_send());
            let init: MsgIdId = std::mem::transmute(msg_send());
            init(alloc(cls(IMAGE_CLASS), sel(b"alloc\0")), sel(b"initWithData:\0"), nsdata)
        }
    }

    pub(super) struct Info {
        pub title: String,
        pub artist: String,
        pub album: String,
        pub duration_ms: i64,
        pub position_ms: i64,
        pub playing: bool,
        pub artwork: Vec<u8>,
    }

    pub(super) fn now_playing(info: Info) -> Result<(), String> {
        center()?;
        on_main(move || {
            // SAFETY: mensajes documentados de Foundation/MediaPlayer en el hilo principal; los
            // objetos autoreleased los retiene el diccionario y el centro al asignarlos.
            unsafe {
                let Ok(center) = center() else { return };
                let get: MsgId = std::mem::transmute(msg_send());
                let set: MsgVoidIdId = std::mem::transmute(msg_send());
                let set_id: MsgVoidId = std::mem::transmute(msg_send());
                let dict = get(cls(b"NSMutableDictionary\0"), sel(b"dictionary\0"));
                let put = |value: Id, key: Id| set(dict, sel(b"setObject:forKey:\0"), value, key);
                put(nsstring(&info.title), MPMediaItemPropertyTitle);
                if !info.artist.is_empty() {
                    put(nsstring(&info.artist), MPMediaItemPropertyArtist);
                }
                if !info.album.is_empty() {
                    put(nsstring(&info.album), MPMediaItemPropertyAlbumTitle);
                }
                if info.duration_ms > 0 {
                    put(nsnumber(info.duration_ms as f64 / 1000.0), MPMediaItemPropertyPlaybackDuration);
                }
                put(nsnumber(info.position_ms.max(0) as f64 / 1000.0), MPNowPlayingInfoPropertyElapsedPlaybackTime);
                put(nsnumber(if info.playing { 1.0 } else { 0.0 }), MPNowPlayingInfoPropertyPlaybackRate);
                if !info.artwork.is_empty() {
                    let image = image_from(&info.artwork);
                    if !image.is_null() {
                        let size: MsgSize = std::mem::transmute(msg_send());
                        let bounds = size(image, sel(b"size\0"));
                        let old = std::mem::replace(&mut *ARTWORK.lock().unwrap_or_else(|e| e.into_inner()), image as usize);
                        release(old as Id);
                        let alloc: MsgId = std::mem::transmute(msg_send());
                        let init: MsgInitArtwork = std::mem::transmute(msg_send());
                        let art = init(alloc(cls(b"MPMediaItemArtwork\0"), sel(b"alloc\0")), sel(b"initWithBoundsSize:requestHandler:\0"), bounds, &ARTWORK_BLOCK);
                        if !art.is_null() {
                            put(art, MPMediaItemPropertyArtwork);
                            release(art);
                        }
                    }
                }
                set_id(center, sel(b"setNowPlayingInfo:\0"), dict);
                #[cfg(target_os = "macos")]
                {
                    // macOS: el estado lo da la app (MPNowPlayingPlaybackStatePlaying = 1, Paused = 2).
                    let set_state: MsgVoidInt = std::mem::transmute(msg_send());
                    set_state(center, sel(b"setPlaybackState:\0"), if info.playing { 1 } else { 2 });
                }
                if crate::ui::ui_trace() {
                    // Relee lo publicado: confirma que el centro aceptó el diccionario.
                    type MsgUsize = unsafe extern "C" fn(Id, Sel) -> usize;
                    let count: MsgUsize = std::mem::transmute(msg_send());
                    let published = get(center, sel(b"nowPlayingInfo\0"));
                    let n = if published.is_null() { 0 } else { count(published, sel(b"count\0")) };
                    eprintln!("[ui] media now playing published: {n} keys, playing={}", info.playing);
                }
            }
        });
        Ok(())
    }

    pub(super) fn clear() -> Result<(), String> {
        center()?;
        on_main(|| {
            // SAFETY: ídem now_playing.
            unsafe {
                let Ok(center) = center() else { return };
                let set_id: MsgVoidId = std::mem::transmute(msg_send());
                set_id(center, sel(b"setNowPlayingInfo:\0"), std::ptr::null_mut());
                #[cfg(target_os = "macos")]
                {
                    let set_state: MsgVoidInt = std::mem::transmute(msg_send());
                    set_state(center, sel(b"setPlaybackState:\0"), 3); // Stopped
                }
                let old = std::mem::replace(&mut *ARTWORK.lock().unwrap_or_else(|e| e.into_inner()), 0);
                release(old as Id);
            }
        });
        Ok(())
    }

    /// Las órdenes que entendemos: (nombre del evento, propiedad del centro, selector del target).
    const COMMANDS: [(&str, &[u8], &[u8]); 7] = [
        ("play", b"playCommand\0", b"rayMediaPlay:\0"),
        ("pause", b"pauseCommand\0", b"rayMediaPause:\0"),
        ("toggle", b"togglePlayPauseCommand\0", b"rayMediaToggle:\0"),
        ("next", b"nextTrackCommand\0", b"rayMediaNext:\0"),
        ("previous", b"previousTrackCommand\0", b"rayMediaPrevious:\0"),
        ("stop", b"stopCommand\0", b"rayMediaStop:\0"),
        ("seek", b"changePlaybackPositionCommand\0", b"rayMediaSeek:\0"),
    ];

    pub(super) fn known_command(name: &str) -> bool {
        COMMANDS.iter().any(|(n, _, _)| *n == name)
    }

    // Los handlers: `MPRemoteCommandHandlerStatusSuccess` es 0. Cada uno encola su evento.
    extern "C" fn on_play(_this: Id, _sel: Sel, _event: Id) -> i64 {
        crate::ui::push_media_event("play");
        0
    }
    extern "C" fn on_pause(_this: Id, _sel: Sel, _event: Id) -> i64 {
        crate::ui::push_media_event("pause");
        0
    }
    extern "C" fn on_toggle(_this: Id, _sel: Sel, _event: Id) -> i64 {
        crate::ui::push_media_event("toggle");
        0
    }
    extern "C" fn on_next(_this: Id, _sel: Sel, _event: Id) -> i64 {
        crate::ui::push_media_event("next");
        0
    }
    extern "C" fn on_previous(_this: Id, _sel: Sel, _event: Id) -> i64 {
        crate::ui::push_media_event("previous");
        0
    }
    extern "C" fn on_stop(_this: Id, _sel: Sel, _event: Id) -> i64 {
        crate::ui::push_media_event("stop");
        0
    }
    extern "C" fn on_seek(_this: Id, _sel: Sel, event: Id) -> i64 {
        // SAFETY: `event` es un MPChangePlaybackPositionCommandEvent válido durante la llamada.
        let secs = unsafe {
            let pos: MsgDouble = std::mem::transmute(msg_send());
            pos(event, sel(b"positionTime\0"))
        };
        crate::ui::push_media_event(&format!("seek:{}", (secs * 1000.0).round().max(0.0) as i64));
        0
    }

    /// El objeto target (clase creada en caliente con un método por orden), uno por proceso.
    unsafe fn target() -> Id {
        static TARGET: Mutex<usize> = Mutex::new(0);
        let mut t = TARGET.lock().unwrap_or_else(|e| e.into_inner());
        if *t == 0 {
            // SAFETY: creación de clase ObjC estándar en el hilo principal; firma `q@:@`
            // (NSInteger, self, _cmd, event) de cada método.
            unsafe {
                let c = objc_allocateClassPair(cls(b"NSObject\0"), c"RayMediaTarget".as_ptr(), 0);
                let imps: [(&[u8], *const c_void); 7] = [
                    (b"rayMediaPlay:\0", on_play as *const c_void),
                    (b"rayMediaPause:\0", on_pause as *const c_void),
                    (b"rayMediaToggle:\0", on_toggle as *const c_void),
                    (b"rayMediaNext:\0", on_next as *const c_void),
                    (b"rayMediaPrevious:\0", on_previous as *const c_void),
                    (b"rayMediaStop:\0", on_stop as *const c_void),
                    (b"rayMediaSeek:\0", on_seek as *const c_void),
                ];
                for (name, imp) in imps {
                    class_addMethod(c, sel(name), imp, c"q@:@".as_ptr());
                }
                objc_registerClassPair(c);
                let get: MsgId = std::mem::transmute(msg_send());
                *t = get(get(c, sel(b"alloc\0")), sel(b"init\0")) as usize;
            }
        }
        *t as Id
    }

    pub(super) fn controls(enabled: Vec<String>) -> Result<(), String> {
        if cls(b"MPRemoteCommandCenter\0").is_null() {
            return Err("media: MediaPlayer framework not available".to_string());
        }
        on_main(move || {
            // SAFETY: mensajes documentados de MPRemoteCommandCenter en el hilo principal.
            unsafe {
                let get: MsgId = std::mem::transmute(msg_send());
                let center = get(cls(b"MPRemoteCommandCenter\0"), sel(b"sharedCommandCenter\0"));
                let add: MsgIdIdSel = std::mem::transmute(msg_send());
                let remove: MsgVoidId = std::mem::transmute(msg_send());
                let set_enabled: MsgVoidBool = std::mem::transmute(msg_send());
                let target = target();
                for (name, prop, action) in COMMANDS {
                    let cmd = get(center, sel(prop));
                    if cmd.is_null() {
                        continue;
                    }
                    // Idempotente: quitar antes de añadir (un target no se registra dos veces).
                    remove(cmd, sel(b"removeTarget:\0"), target);
                    let on = enabled.iter().any(|e| e == name);
                    if on {
                        add(cmd, sel(b"addTarget:action:\0"), target, sel(action));
                    }
                    set_enabled(cmd, sel(b"setEnabled:\0"), on as u8);
                }
            }
        });
        Ok(())
    }
}

/// Lo que suena: los campos de `media.now_playing` ya parseados.
#[derive(Debug)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: i64,
    pub position_ms: i64,
    pub playing: bool,
    /// PNG/JPEG de la carátula (vacío = sin carátula).
    pub artwork: Vec<u8>,
}

/// Las órdenes que acepta `controls`.
pub const COMMAND_NAMES: [&str; 7] = ["play", "pause", "toggle", "next", "previous", "stop", "seek"];

/// Publica lo que suena en el «Now Playing» del sistema (reemplaza lo anterior).
#[cfg(all(feature = "ui", any(target_os = "macos", target_os = "ios")))]
pub fn now_playing(info: NowPlaying) -> Result<(), String> {
    apple::now_playing(apple::Info {
        title: info.title,
        artist: info.artist,
        album: info.album,
        duration_ms: info.duration_ms,
        position_ms: info.position_ms,
        playing: info.playing,
        artwork: info.artwork,
    })
}

/// Retira lo publicado (la app deja de aparecer como reproductor).
#[cfg(all(feature = "ui", any(target_os = "macos", target_os = "ios")))]
pub fn clear() -> Result<(), String> {
    apple::clear()
}

/// Declara qué órdenes atiende el programa (y recibe como eventos `"media"`); las demás quedan
/// deshabilitadas en los controles del sistema. Vacío = ninguna.
#[cfg(all(feature = "ui", any(target_os = "macos", target_os = "ios")))]
pub fn controls(enabled: Vec<String>) -> Result<(), String> {
    if let Some(bad) = enabled.iter().find(|e| !apple::known_command(e)) {
        return Err(format!("media: unknown command '{bad}' (play, pause, toggle, next, previous, stop, seek)"));
    }
    apple::controls(enabled)
}

#[cfg(not(all(feature = "ui", any(target_os = "macos", target_os = "ios"))))]
const UNAVAILABLE: &str = "media: Now Playing is not available on this platform yet (macOS and iOS only)";

#[cfg(not(all(feature = "ui", any(target_os = "macos", target_os = "ios"))))]
pub fn now_playing(_info: NowPlaying) -> Result<(), String> {
    Err(UNAVAILABLE.to_string())
}
#[cfg(not(all(feature = "ui", any(target_os = "macos", target_os = "ios"))))]
pub fn clear() -> Result<(), String> {
    Err(UNAVAILABLE.to_string())
}
#[cfg(not(all(feature = "ui", any(target_os = "macos", target_os = "ios"))))]
pub fn controls(enabled: Vec<String>) -> Result<(), String> {
    if let Some(bad) = enabled.iter().find(|e| !COMMAND_NAMES.contains(&e.as_str())) {
        return Err(format!("media: unknown command '{bad}' (play, pause, toggle, next, previous, stop, seek)"));
    }
    Err(UNAVAILABLE.to_string())
}
