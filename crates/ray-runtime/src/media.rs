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


#[cfg(all(feature = "ui", target_os = "linux"))]
mod linux {
    //! MPRIS (org.mpris.MediaPlayer2) por D-Bus con GIO cargado por `dlopen` (patrón GTK de `ui.rs`:
    //! sin headers ni dependencias de build; sin libglib → `Err` claro). Un hilo propio con su
    //! `GMainContext` conduce la conexión: no hace falta GTK ni ventana. Solo se usan funciones NO
    //! variádicas de GVariant (`g_variant_new_*`, `_new_dict_entry`, `_new_array`, `_new_tuple`).
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    type P = *mut c_void;
    const RTLD_NOW: i32 = 2;
    const RTLD_GLOBAL: i32 = 0x100;
    const G_BUS_TYPE_SESSION: i32 = 2;
    const G_BUS_NAME_OWNER_FLAGS_NONE: i32 = 0;

    unsafe extern "C" {
        fn dlopen(path: *const c_char, flags: i32) -> P;
        fn dlsym(lib: P, name: *const c_char) -> P;
    }

    type FnMainContextNew = unsafe extern "C" fn() -> P;
    type FnCtxOp = unsafe extern "C" fn(P);
    type FnMainLoopNew = unsafe extern "C" fn(P, i32) -> P;
    type FnInvoke = unsafe extern "C" fn(P, i32, extern "C" fn(P) -> i32, P, Option<extern "C" fn(P)>);
    type FnBusOwnName = unsafe extern "C" fn(i32, *const c_char, i32, extern "C" fn(P, *const c_char, P), extern "C" fn(P, *const c_char, P), extern "C" fn(P, *const c_char, P), P, Option<extern "C" fn(P)>) -> u32;
    type FnNodeInfo = unsafe extern "C" fn(*const c_char, *mut P) -> P;
    type FnLookupIface = unsafe extern "C" fn(P, *const c_char) -> P;
    type FnRegisterObject = unsafe extern "C" fn(P, *const c_char, P, *const VTable, P, Option<extern "C" fn(P)>, *mut P) -> u32;
    type FnReturnValue = unsafe extern "C" fn(P, P);
    type FnEmitSignal = unsafe extern "C" fn(P, *const c_char, *const c_char, *const c_char, *const c_char, P, *mut P) -> i32;
    type FnVNewStr = unsafe extern "C" fn(*const c_char) -> P;
    type FnVNewI64 = unsafe extern "C" fn(i64) -> P;
    type FnVNewBool = unsafe extern "C" fn(i32) -> P;
    type FnVNewF64 = unsafe extern "C" fn(f64) -> P;
    type FnVNewStrv = unsafe extern "C" fn(*const *const c_char, isize) -> P;
    type FnVNewDictEntry = unsafe extern "C" fn(P, P) -> P;
    type FnVNewArray = unsafe extern "C" fn(P, *const P, usize) -> P;
    type FnVNewTuple = unsafe extern "C" fn(*const P, usize) -> P;
    type FnVNewVariant = unsafe extern "C" fn(P) -> P;
    type FnVTypeNew = unsafe extern "C" fn(*const c_char) -> P;
    type FnVChild = unsafe extern "C" fn(P, usize) -> P;
    type FnVGetI64 = unsafe extern "C" fn(P) -> i64;
    type FnVUnref = unsafe extern "C" fn(P);
    type FnErrorFree = unsafe extern "C" fn(P);

    /// GDBusInterfaceVTable (gio/gdbusconnection.h): tres callbacks + relleno.
    #[repr(C)]
    struct VTable {
        method_call: extern "C" fn(P, *const c_char, *const c_char, *const c_char, *const c_char, P, P, P),
        get_property: extern "C" fn(P, *const c_char, *const c_char, *const c_char, *const c_char, *mut P, P) -> P,
        set_property: extern "C" fn(P, *const c_char, *const c_char, *const c_char, *const c_char, P, *mut P, P) -> i32,
        padding: [P; 8],
    }
    unsafe impl Sync for VTable {}

    struct Api {
        main_context_new: FnMainContextNew,
        push_thread_default: FnCtxOp,
        main_loop_new: FnMainLoopNew,
        main_loop_run: FnCtxOp,
        invoke: FnInvoke,
        bus_own_name: FnBusOwnName,
        node_info_new: FnNodeInfo,
        lookup_iface: FnLookupIface,
        register_object: FnRegisterObject,
        return_value: FnReturnValue,
        emit_signal: FnEmitSignal,
        v_str: FnVNewStr,
        v_i64: FnVNewI64,
        v_bool: FnVNewBool,
        v_f64: FnVNewF64,
        v_path: FnVNewStr,
        v_strv: FnVNewStrv,
        v_dict_entry: FnVNewDictEntry,
        v_array: FnVNewArray,
        v_tuple: FnVNewTuple,
        v_variant: FnVNewVariant,
        v_type_new: FnVTypeNew,
        v_child: FnVChild,
        v_get_i64: FnVGetI64,
        v_unref: FnVUnref,
        error_free: FnErrorFree,
    }
    unsafe impl Sync for Api {}
    unsafe impl Send for Api {}

    fn sym(lib: P, name: &CStr) -> Result<P, String> {
        // SAFETY: name es un CStr; dlsym no retiene el puntero.
        let p = unsafe { dlsym(lib, name.as_ptr()) };
        if p.is_null() { Err(format!("media: libgio without {}", name.to_string_lossy())) } else { Ok(p) }
    }

    fn api() -> Result<&'static Api, String> {
        static API: OnceLock<Result<Api, String>> = OnceLock::new();
        API.get_or_init(|| {
            // SAFETY: literal NUL-terminado; dlopen es seguro de llamar.
            let gio = unsafe { dlopen(c"libgio-2.0.so.0".as_ptr(), RTLD_NOW | RTLD_GLOBAL) };
            if gio.is_null() {
                return Err("media: libgio-2.0.so.0 not found (install libglib2.0-0) — MPRIS needs GLib".to_string());
            }
            // SAFETY: las firmas replican los headers de GLib/GIO (API C estable); glib/gobject se
            // resuelven por la clausura de dependencias del handle de gio.
            unsafe {
                Ok(Api {
                    main_context_new: std::mem::transmute::<P, FnMainContextNew>(sym(gio, c"g_main_context_new")?),
                    push_thread_default: std::mem::transmute::<P, FnCtxOp>(sym(gio, c"g_main_context_push_thread_default")?),
                    main_loop_new: std::mem::transmute::<P, FnMainLoopNew>(sym(gio, c"g_main_loop_new")?),
                    main_loop_run: std::mem::transmute::<P, FnCtxOp>(sym(gio, c"g_main_loop_run")?),
                    invoke: std::mem::transmute::<P, FnInvoke>(sym(gio, c"g_main_context_invoke_full")?),
                    bus_own_name: std::mem::transmute::<P, FnBusOwnName>(sym(gio, c"g_bus_own_name")?),
                    node_info_new: std::mem::transmute::<P, FnNodeInfo>(sym(gio, c"g_dbus_node_info_new_for_xml")?),
                    lookup_iface: std::mem::transmute::<P, FnLookupIface>(sym(gio, c"g_dbus_node_info_lookup_interface")?),
                    register_object: std::mem::transmute::<P, FnRegisterObject>(sym(gio, c"g_dbus_connection_register_object")?),
                    return_value: std::mem::transmute::<P, FnReturnValue>(sym(gio, c"g_dbus_method_invocation_return_value")?),
                    emit_signal: std::mem::transmute::<P, FnEmitSignal>(sym(gio, c"g_dbus_connection_emit_signal")?),
                    v_str: std::mem::transmute::<P, FnVNewStr>(sym(gio, c"g_variant_new_string")?),
                    v_i64: std::mem::transmute::<P, FnVNewI64>(sym(gio, c"g_variant_new_int64")?),
                    v_bool: std::mem::transmute::<P, FnVNewBool>(sym(gio, c"g_variant_new_boolean")?),
                    v_f64: std::mem::transmute::<P, FnVNewF64>(sym(gio, c"g_variant_new_double")?),
                    v_path: std::mem::transmute::<P, FnVNewStr>(sym(gio, c"g_variant_new_object_path")?),
                    v_strv: std::mem::transmute::<P, FnVNewStrv>(sym(gio, c"g_variant_new_strv")?),
                    v_dict_entry: std::mem::transmute::<P, FnVNewDictEntry>(sym(gio, c"g_variant_new_dict_entry")?),
                    v_array: std::mem::transmute::<P, FnVNewArray>(sym(gio, c"g_variant_new_array")?),
                    v_tuple: std::mem::transmute::<P, FnVNewTuple>(sym(gio, c"g_variant_new_tuple")?),
                    v_variant: std::mem::transmute::<P, FnVNewVariant>(sym(gio, c"g_variant_new_variant")?),
                    v_type_new: std::mem::transmute::<P, FnVTypeNew>(sym(gio, c"g_variant_type_new")?),
                    v_child: std::mem::transmute::<P, FnVChild>(sym(gio, c"g_variant_get_child_value")?),
                    v_get_i64: std::mem::transmute::<P, FnVGetI64>(sym(gio, c"g_variant_get_int64")?),
                    v_unref: std::mem::transmute::<P, FnVUnref>(sym(gio, c"g_variant_unref")?),
                    error_free: std::mem::transmute::<P, FnErrorFree>(sym(gio, c"g_error_free")?),
                })
            }
        })
        .as_ref()
        .map_err(|e| e.clone())
    }

    /// El estado publicado (lo escribe el programa, lo lee el hilo del bus bajo el mismo lock).
    #[derive(Default)]
    struct State {
        title: String,
        artist: String,
        album: String,
        duration_ms: i64,
        position_ms: i64,
        playing: bool,
        /// Instante de la última publicación: `Position` extrapola si `playing`.
        since: Option<Instant>,
        published: bool,
        art_url: String,
        commands: Vec<String>,
        /// Nº de pista: `mpris:trackid` cambia con cada `now_playing` (los clientes lo usan de clave).
        track: u64,
    }
    static STATE: Mutex<State> = Mutex::new(State {
        title: String::new(),
        artist: String::new(),
        album: String::new(),
        duration_ms: 0,
        position_ms: 0,
        playing: false,
        since: None,
        published: false,
        art_url: String::new(),
        commands: Vec::new(),
        track: 0,
    });
    /// La conexión al bus de sesión (0 hasta `bus_acquired`) y el GMainContext del hilo del bus.
    static CONN: Mutex<usize> = Mutex::new(0);
    static CTX: OnceLock<usize> = OnceLock::new();

    const PATH: &CStr = c"/org/mpris/MediaPlayer2";
    const IFACE_ROOT: &CStr = c"org.mpris.MediaPlayer2";
    const IFACE_PLAYER: &CStr = c"org.mpris.MediaPlayer2.Player";
    const INTROSPECTION: &CStr = c"<node>
  <interface name='org.mpris.MediaPlayer2'>
    <method name='Raise'/><method name='Quit'/>
    <property name='CanQuit' type='b' access='read'/>
    <property name='CanRaise' type='b' access='read'/>
    <property name='HasTrackList' type='b' access='read'/>
    <property name='Identity' type='s' access='read'/>
    <property name='SupportedUriSchemes' type='as' access='read'/>
    <property name='SupportedMimeTypes' type='as' access='read'/>
  </interface>
  <interface name='org.mpris.MediaPlayer2.Player'>
    <method name='Next'/><method name='Previous'/><method name='Pause'/><method name='PlayPause'/>
    <method name='Stop'/><method name='Play'/>
    <method name='Seek'><arg direction='in' name='Offset' type='x'/></method>
    <method name='SetPosition'><arg direction='in' name='TrackId' type='o'/><arg direction='in' name='Position' type='x'/></method>
    <method name='OpenUri'><arg direction='in' name='Uri' type='s'/></method>
    <signal name='Seeked'><arg name='Position' type='x'/></signal>
    <property name='PlaybackStatus' type='s' access='read'/>
    <property name='Rate' type='d' access='readwrite'/>
    <property name='Metadata' type='a{sv}' access='read'/>
    <property name='Volume' type='d' access='readwrite'/>
    <property name='Position' type='x' access='read'/>
    <property name='MinimumRate' type='d' access='read'/>
    <property name='MaximumRate' type='d' access='read'/>
    <property name='CanGoNext' type='b' access='read'/>
    <property name='CanGoPrevious' type='b' access='read'/>
    <property name='CanPlay' type='b' access='read'/>
    <property name='CanPause' type='b' access='read'/>
    <property name='CanSeek' type='b' access='read'/>
    <property name='CanControl' type='b' access='read'/>
  </interface>
</node>";

    fn cstr(s: &str) -> CString {
        CString::new(s.replace('\0', "")).unwrap_or_default()
    }

    fn identity() -> String {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "raylang".to_string())
    }

    fn can(st: &State, cmd: &str) -> bool {
        st.commands.iter().any(|c| c == cmd)
    }

    /// Un GVariant `a{sv}` con los metadatos MPRIS de lo que suena (vacío si no hay nada).
    unsafe fn metadata_variant(a: &Api, st: &State) -> P {
        unsafe {
            let sv = (a.v_type_new)(c"{sv}".as_ptr());
            let mut entries: Vec<P> = Vec::new();
            let mut entry = |key: &str, value: P| {
                let k = cstr(key);
                entries.push((a.v_dict_entry)((a.v_str)(k.as_ptr()), (a.v_variant)(value)));
            };
            if st.published {
                let track = cstr(&format!("/org/raylang/track/{}", st.track));
                entry("mpris:trackid", (a.v_path)(track.as_ptr()));
                if st.duration_ms > 0 {
                    entry("mpris:length", (a.v_i64)(st.duration_ms * 1000));
                }
                let title = cstr(&st.title);
                entry("xesam:title", (a.v_str)(title.as_ptr()));
                if !st.artist.is_empty() {
                    let artist = cstr(&st.artist);
                    let arr = [artist.as_ptr()];
                    entry("xesam:artist", (a.v_strv)(arr.as_ptr(), 1));
                }
                if !st.album.is_empty() {
                    let album = cstr(&st.album);
                    entry("xesam:album", (a.v_str)(album.as_ptr()));
                }
                if !st.art_url.is_empty() {
                    let url = cstr(&st.art_url);
                    entry("mpris:artUrl", (a.v_str)(url.as_ptr()));
                }
            }
            (a.v_array)(sv, entries.as_ptr(), entries.len())
        }
    }

    fn status(st: &State) -> &'static CStr {
        if !st.published { c"Stopped" } else if st.playing { c"Playing" } else { c"Paused" }
    }

    fn position_us(st: &State) -> i64 {
        let extra = match (st.playing, st.since) {
            (true, Some(t)) => t.elapsed().as_millis() as i64,
            _ => 0,
        };
        (st.position_ms + extra).max(0) * 1000
    }

    extern "C" fn get_property(_conn: P, _sender: *const c_char, _path: *const c_char, iface: *const c_char, prop: *const c_char, _err: *mut P, _ud: P) -> P {
        let Ok(a) = api() else { return std::ptr::null_mut() };
        // SAFETY: strings NUL-terminados de GDBus, vivos durante la llamada.
        let (iface, prop) = unsafe { (CStr::from_ptr(iface).to_string_lossy(), CStr::from_ptr(prop).to_string_lossy()) };
        let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: constructores de GVariant con argumentos válidos; GDBus se queda la referencia.
        unsafe {
            if iface == IFACE_ROOT.to_string_lossy() {
                return match prop.as_ref() {
                    "Identity" => {
                        let id = cstr(&identity());
                        (a.v_str)(id.as_ptr())
                    }
                    "SupportedUriSchemes" | "SupportedMimeTypes" => (a.v_strv)(std::ptr::null(), 0),
                    _ => (a.v_bool)(0), // CanQuit, CanRaise, HasTrackList
                };
            }
            match prop.as_ref() {
                "PlaybackStatus" => (a.v_str)(status(&st).as_ptr()),
                "Rate" | "MinimumRate" | "MaximumRate" | "Volume" => (a.v_f64)(1.0),
                "Metadata" => metadata_variant(a, &st),
                "Position" => (a.v_i64)(position_us(&st)),
                "CanGoNext" => (a.v_bool)(can(&st, "next") as i32),
                "CanGoPrevious" => (a.v_bool)(can(&st, "previous") as i32),
                "CanPlay" => (a.v_bool)((can(&st, "play") || can(&st, "toggle")) as i32),
                "CanPause" => (a.v_bool)((can(&st, "pause") || can(&st, "toggle")) as i32),
                "CanSeek" => (a.v_bool)(can(&st, "seek") as i32),
                "CanControl" => (a.v_bool)(!st.commands.is_empty() as i32),
                _ => (a.v_bool)(0),
            }
        }
    }

    extern "C" fn set_property(_conn: P, _sender: *const c_char, _path: *const c_char, _iface: *const c_char, _prop: *const c_char, _value: P, _err: *mut P, _ud: P) -> i32 {
        1 // Rate/Volume: aceptados y sin efecto (el programa manda)
    }

    extern "C" fn method_call(_conn: P, _sender: *const c_char, _path: *const c_char, _iface: *const c_char, method: *const c_char, params: P, invocation: P, _ud: P) {
        let Ok(a) = api() else { return };
        // SAFETY: `method` NUL-terminado y vivo durante la llamada; `params` es la tupla de argumentos.
        let method = unsafe { CStr::from_ptr(method) }.to_string_lossy().into_owned();
        let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let tag: Option<String> = match method.as_str() {
            "Play" => Some(if can(&st, "play") { "play" } else { "toggle" }.to_string()),
            "Pause" => Some(if can(&st, "pause") { "pause" } else { "toggle" }.to_string()),
            "PlayPause" => Some(if can(&st, "toggle") { "toggle" } else if st.playing { "pause" } else { "play" }.to_string()),
            "Next" => Some("next".to_string()),
            "Previous" => Some("previous".to_string()),
            "Stop" => Some("stop".to_string()),
            "Seek" => {
                // SAFETY: Seek(x): un hijo int64 (microsegundos de desplazamiento).
                let offset_us = unsafe {
                    let child = (a.v_child)(params, 0);
                    let v = (a.v_get_i64)(child);
                    (a.v_unref)(child);
                    v
                };
                Some(format!("seek:{}", ((position_us(&st) + offset_us) / 1000).max(0)))
            }
            "SetPosition" => {
                // SAFETY: SetPosition(o, x): el segundo hijo es la posición en microsegundos.
                let pos_us = unsafe {
                    let child = (a.v_child)(params, 1);
                    let v = (a.v_get_i64)(child);
                    (a.v_unref)(child);
                    v
                };
                Some(format!("seek:{}", (pos_us / 1000).max(0)))
            }
            _ => None, // Raise, Quit, OpenUri: sin efecto
        };
        drop(st);
        if let Some(tag) = tag {
            crate::ui::push_media_event(&tag);
        }
        // SAFETY: la invocación es válida hasta que se responde; NULL = sin valores de retorno.
        unsafe { (a.return_value)(invocation, std::ptr::null_mut()) };
    }

    static VTABLE: VTable = VTable { method_call, get_property, set_property, padding: [std::ptr::null_mut(); 8] };

    extern "C" fn bus_acquired(conn: P, _name: *const c_char, _ud: P) {
        let Ok(a) = api() else { return };
        // SAFETY: XML literal válido; los punteros de GDBus viven mientras la conexión.
        unsafe {
            let mut err: P = std::ptr::null_mut();
            let info = (a.node_info_new)(INTROSPECTION.as_ptr(), &mut err);
            if info.is_null() {
                if !err.is_null() { (a.error_free)(err); }
                return;
            }
            for iface in [IFACE_ROOT, IFACE_PLAYER] {
                let ii = (a.lookup_iface)(info, iface.as_ptr());
                let mut err: P = std::ptr::null_mut();
                let id = (a.register_object)(conn, PATH.as_ptr(), ii, &VTABLE, std::ptr::null_mut(), None, &mut err);
                if id == 0 && !err.is_null() {
                    (a.error_free)(err);
                }
            }
        }
        *CONN.lock().unwrap_or_else(|e| e.into_inner()) = conn as usize;
    }
    extern "C" fn name_acquired(_conn: P, _name: *const c_char, _ud: P) {}
    extern "C" fn name_lost(_conn: P, _name: *const c_char, _ud: P) {
        if crate::ui::ui_trace() {
            eprintln!("[ui] media: could not own the MPRIS name on the session bus (no D-Bus session?)");
        }
    }

    /// Arranca (una vez) el hilo del bus: contexto propio, nombre `org.mpris.MediaPlayer2.<id>.instance<pid>`.
    fn ensure_bus() -> Result<(), String> {
        let a = api()?;
        if CTX.get().is_some() {
            return Ok(());
        }
        let name = cstr(&format!(
            "org.mpris.MediaPlayer2.{}.instance{}",
            identity().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect::<String>(),
            std::process::id()
        ));
        let (tx, rx) = std::sync::mpsc::channel::<usize>();
        std::thread::Builder::new()
            .name("ray-media-bus".into())
            .spawn(move || {
                // SAFETY: bucle GLib estándar en un hilo propio con su contexto por defecto.
                unsafe {
                    let ctx = (a.main_context_new)();
                    (a.push_thread_default)(ctx);
                    let _ = tx.send(ctx as usize);
                    (a.bus_own_name)(G_BUS_TYPE_SESSION, name.as_ptr(), G_BUS_NAME_OWNER_FLAGS_NONE, bus_acquired, name_acquired, name_lost, std::ptr::null_mut(), None);
                    let lp = (a.main_loop_new)(ctx, 0);
                    (a.main_loop_run)(lp);
                }
            })
            .map_err(|e| format!("media: could not start the D-Bus thread: {e}"))?;
        let ctx = rx.recv().map_err(|_| "media: the D-Bus thread died".to_string())?;
        let _ = CTX.set(ctx);
        Ok(())
    }

    /// En el hilo del bus: `PropertiesChanged` del Player con lo que cambia al publicar.
    extern "C" fn emit_changed(_ud: P) -> i32 {
        let Ok(a) = api() else { return 0 };
        let conn = *CONN.lock().unwrap_or_else(|e| e.into_inner()) as P;
        if conn.is_null() {
            return 0;
        }
        let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: construcción de `(sa{sv}as)` con constructores no variádicos; GDBus toma la referencia.
        unsafe {
            let sv = (a.v_type_new)(c"{sv}".as_ptr());
            let mut entries: Vec<P> = Vec::new();
            let mut entry = |key: &CStr, value: P| entries.push((a.v_dict_entry)((a.v_str)(key.as_ptr()), (a.v_variant)(value)));
            entry(c"PlaybackStatus", (a.v_str)(status(&st).as_ptr()));
            entry(c"Metadata", metadata_variant(a, &st));
            entry(c"CanGoNext", (a.v_bool)(can(&st, "next") as i32));
            entry(c"CanGoPrevious", (a.v_bool)(can(&st, "previous") as i32));
            entry(c"CanPlay", (a.v_bool)((can(&st, "play") || can(&st, "toggle")) as i32));
            entry(c"CanPause", (a.v_bool)((can(&st, "pause") || can(&st, "toggle")) as i32));
            entry(c"CanSeek", (a.v_bool)(can(&st, "seek") as i32));
            entry(c"CanControl", (a.v_bool)(!st.commands.is_empty() as i32));
            let changed = (a.v_array)(sv, entries.as_ptr(), entries.len());
            let children = [(a.v_str)(IFACE_PLAYER.as_ptr()), changed, (a.v_strv)(std::ptr::null(), 0)];
            let params = (a.v_tuple)(children.as_ptr(), 3);
            let mut err: P = std::ptr::null_mut();
            (a.emit_signal)(conn, std::ptr::null(), PATH.as_ptr(), c"org.freedesktop.DBus.Properties".as_ptr(), c"PropertiesChanged".as_ptr(), params, &mut err);
            if !err.is_null() {
                (a.error_free)(err);
            }
        }
        0 // G_SOURCE_REMOVE
    }

    fn publish_changed() -> Result<(), String> {
        let a = api()?;
        ensure_bus()?;
        let ctx = *CTX.get().unwrap_or(&0) as P;
        // SAFETY: el contexto es del hilo del bus (vivo mientras el proceso); el callback no toma datos.
        unsafe { (a.invoke)(ctx, 0, emit_changed, std::ptr::null_mut(), None) };
        Ok(())
    }

    /// La carátula como `file://` en el directorio de ejecución del usuario (MPRIS pide una URL).
    fn art_url(bytes: &[u8]) -> String {
        if bytes.is_empty() {
            return String::new();
        }
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let ext = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { "png" } else { "jpg" };
        let path = dir.join(format!("raylang-media-{}.{ext}", std::process::id()));
        match std::fs::write(&path, bytes) {
            Ok(()) => format!("file://{}", path.display()),
            Err(_) => String::new(),
        }
    }

    pub(super) fn now_playing(info: super::NowPlaying) -> Result<(), String> {
        {
            let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
            st.title = info.title;
            st.artist = info.artist;
            st.album = info.album;
            st.duration_ms = info.duration_ms;
            st.position_ms = info.position_ms;
            st.playing = info.playing;
            st.since = Some(Instant::now());
            st.published = true;
            st.track += 1;
            st.art_url = art_url(&info.artwork);
        }
        publish_changed()
    }

    pub(super) fn clear() -> Result<(), String> {
        {
            let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
            st.published = false;
            st.playing = false;
            st.title.clear();
            st.art_url.clear();
        }
        publish_changed()
    }

    pub(super) fn controls(enabled: Vec<String>) -> Result<(), String> {
        STATE.lock().unwrap_or_else(|e| e.into_inner()).commands = enabled;
        publish_changed()
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

/// Linux (M404): MPRIS por D-Bus (GIO por dlopen; sin GLib → `Err`).
#[cfg(all(feature = "ui", target_os = "linux"))]
pub fn now_playing(info: NowPlaying) -> Result<(), String> {
    linux::now_playing(info)
}
#[cfg(all(feature = "ui", target_os = "linux"))]
pub fn clear() -> Result<(), String> {
    linux::clear()
}
#[cfg(all(feature = "ui", target_os = "linux"))]
pub fn controls(enabled: Vec<String>) -> Result<(), String> {
    if let Some(bad) = enabled.iter().find(|e| !COMMAND_NAMES.contains(&e.as_str())) {
        return Err(format!("media: unknown command '{bad}' (play, pause, toggle, next, previous, stop, seek)"));
    }
    linux::controls(enabled)
}

#[cfg(not(all(feature = "ui", any(target_os = "macos", target_os = "ios", target_os = "linux"))))]
const UNAVAILABLE: &str = "media: Now Playing is not available on this platform yet (macOS, iOS, Android and Linux only)";

#[cfg(not(all(feature = "ui", any(target_os = "macos", target_os = "ios", target_os = "linux"))))]
pub fn now_playing(_info: NowPlaying) -> Result<(), String> {
    Err(UNAVAILABLE.to_string())
}
#[cfg(not(all(feature = "ui", any(target_os = "macos", target_os = "ios", target_os = "linux"))))]
pub fn clear() -> Result<(), String> {
    Err(UNAVAILABLE.to_string())
}
#[cfg(not(all(feature = "ui", any(target_os = "macos", target_os = "ios", target_os = "linux"))))]
pub fn controls(enabled: Vec<String>) -> Result<(), String> {
    if let Some(bad) = enabled.iter().find(|e| !COMMAND_NAMES.contains(&e.as_str())) {
        return Err(format!("media: unknown command '{bad}' (play, pause, toggle, next, previous, stop, seek)"));
    }
    Err(UNAVAILABLE.to_string())
}
