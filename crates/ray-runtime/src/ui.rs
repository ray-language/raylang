//! M146 — `std/ui` (IDEAS §80 F1): la PRIMITIVA ventana + webview. Una ventana nativa del SO
//! con el webview del sistema cargando una URL (el patrón: el webserver embebido del programa);
//! el IPC JS↔raylang es el propio framework web — este módulo no inventa mensajería.
//!
//! **El contrato del hilo principal** (la pieza estructural): AppKit exige poseer el hilo 1 del
//! proceso y su run loop. Aquí no hay `ui.run()` de cara al usuario — el host (el binario `ray`
//! o el `main` emitido por el transpilador) registra un *waker* con [`set_main_thread_waker`] y
//! deja el hilo 1 esperando; la PRIMERA operación de UI dispara el waker, el hilo 1 entra en
//! [`run_main_loop`] (NSApplication + `[NSApp run]`, para el resto de la vida del proceso) y
//! toda operación posterior se despacha ahí vía `dispatch_async_f` (libdispatch en C plano, sin
//! blocks). Sin waker registrado (tests de cargo, embedding) la operación falla con `Err` —
//! nunca espera.
//!
//! **Eventos**: cola global + self-pipe (el patrón de `watch.rs`/`signals()`): el delegate de
//! AppKit encola y escribe un octeto; el lector (VM/nativo) aparca la fibra por readiness del
//! extremo de lectura y drena al despertar. Ni la VM ni el scheduler saben de AppKit.
//!
//! **Backends**: AppKit/WKWebView en macOS (frameworks siempre presentes, se enlazan al build —
//! `#[link(kind = "framework")]`, sin build.rs); `RAY_UI_BACKEND=headless` = ventanas de mesa
//! (tabla en memoria, `close` sintetiza el evento `closed`) en cualquier OS — la vía de los
//! tests/CI, como `RAY_AUDIO_SINK=null` en audio. Objective-C A MANO (sin crates objc/wry: la
//! lección cpal de M145 — los crates de webview exigen toolchains GTK/WebKit en build).

#![cfg(all(feature = "ui", unix))]

use std::collections::{HashMap, VecDeque};
use std::sync::{Condvar, Mutex, OnceLock};

unsafe extern "C" {
    fn pipe(fds: *mut i32) -> i32;
    fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
    fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    // Variádica a propósito (lección de arm64, como en watch.rs/audio.rs).
    #[link_name = "fcntl"]
    fn fcntl_raw(fd: i32, cmd: i32, ...) -> i32;
}
const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0o4000;
#[cfg(not(target_os = "linux"))]
const O_NONBLOCK: i32 = 0x0004;

// ── La cola global de eventos + self-pipe ────────────────────────────────────

/// Un evento de UI: `(kind, ventana)`. v1 emite solo `"closed"` (la ventana se fue de la
/// pantalla, sea por el botón rojo o por `close(h)` — exactamente una vez por ventana).
type Event = (String, i64);

struct Events {
    queue: Mutex<VecDeque<Event>>,
    /// Para la espera BLOQUEANTE (intérprete): `next_blocking` duerme aquí, no sondea.
    ready: Condvar,
    pipe_rd: i32,
    pipe_wr: i32,
}

fn events() -> &'static Events {
    static EVENTS: OnceLock<Events> = OnceLock::new();
    EVENTS.get_or_init(|| {
        let mut fds = [0i32; 2];
        // SAFETY: pipe escribe dos fds válidos. Si fallara (sin fds), el self-pipe queda en -1:
        // `event_fd` devolvería un fd inválido y el aparcado fallaría con error, no colgado.
        if unsafe { pipe(fds.as_mut_ptr()) } != 0 {
            fds = [-1, -1];
        }
        // Ambos extremos NO-bloqueantes: el lector drena sin colgarse y el escritor (el hilo de
        // AppKit) jamás se bloquea — con el pipe lleno se omite el octeto (la cola manda; el
        // lector drena huérfanos al vaciarla, patrón watch.rs).
        for fd in fds {
            if fd >= 0 {
                unsafe {
                    let fl = fcntl_raw(fd, F_GETFL);
                    fcntl_raw(fd, F_SETFL, fl | O_NONBLOCK);
                }
            }
        }
        Events {
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            pipe_rd: fds[0],
            pipe_wr: fds[1],
        }
    })
}

fn push_event(kind: &str, window: i64) {
    let ev = events();
    ev.queue.lock().unwrap().push_back((kind.to_string(), window));
    ev.ready.notify_all();
    if ev.pipe_wr >= 0 {
        // SAFETY: un octeto a un fd nuestro; con el pipe lleno se omite (ver arriba).
        unsafe { write(ev.pipe_wr, [1u8].as_ptr(), 1) };
    }
}

/// El fd por el que aparca la fibra (extremo de lectura del self-pipe, no-bloqueante).
pub fn event_fd() -> i32 {
    events().pipe_rd
}

/// El siguiente evento si ya hay uno, SIN bloquear. Consume un octeto del pipe por evento
/// entregado; con la cola vacía drena los huérfanos (para no despertar en falso).
pub fn try_next_event() -> Option<(String, i64)> {
    let ev = events();
    let mut q = ev.queue.lock().unwrap();
    match q.pop_front() {
        Some(e) => {
            drain_pipe(ev.pipe_rd, 1);
            Some(e)
        }
        None => {
            drain_pipe(ev.pipe_rd, usize::MAX);
            None
        }
    }
}

/// El siguiente evento BLOQUEANDO el hilo (el intérprete, oráculo secuencial): espera en la
/// condvar de la cola — sin sondeo. `ms <= 0` = sin plazo; `Ok(None)` = plazo vencido.
pub fn next_event_blocking(ms: i64) -> Option<(String, i64)> {
    let ev = events();
    let deadline = if ms > 0 {
        Some(std::time::Instant::now() + std::time::Duration::from_millis(ms as u64))
    } else {
        None
    };
    let mut q = ev.queue.lock().unwrap();
    loop {
        if let Some(e) = q.pop_front() {
            drain_pipe(ev.pipe_rd, 1);
            return Some(e);
        }
        match deadline {
            None => q = ev.ready.wait(q).unwrap(),
            Some(d) => {
                let rem = d.saturating_duration_since(std::time::Instant::now());
                if rem.is_zero() {
                    return None;
                }
                q = ev.ready.wait_timeout(q, rem).unwrap().0;
            }
        }
    }
}

// Lee hasta `max` octetos del pipe (no-bloqueante) y los descarta.
fn drain_pipe(fd: i32, max: usize) {
    if fd < 0 {
        return;
    }
    let mut buf = [0u8; 64];
    let mut left = max;
    while left > 0 {
        let want = buf.len().min(left);
        // SAFETY: buf vive durante la llamada; el fd es nuestro.
        let n = unsafe { read(fd, buf.as_mut_ptr(), want) };
        if n <= 0 {
            break;
        }
        left -= n as usize;
    }
}

// ── El registro de ventanas ──────────────────────────────────────────────────

/// Una ventana viva. Los punteros objc se guardan como `usize` y SOLO se dereferencian en el
/// hilo principal (toda operación se despacha ahí) — el mapa en sí es datos ordinarios.
enum Win {
    /// Backend de mesa: la ventana solo existe como fila (tests/CI).
    Headless,
    #[cfg(target_os = "macos")]
    Mac {
        window: usize,
        webview: usize,
        delegate: usize,
    },
}

struct WinState {
    win: Win,
    /// ¿Ya se emitió su evento `closed`? (El botón rojo y `close(h)` convergen aquí: una vez.)
    closed: bool,
}

/// Clave del mapa: el ID DEL LLAMADOR (el handle del registro del host) — así los eventos
/// llevan directamente el handle que el programa conoce, sin tabla de traducción en el borde.
fn windows() -> &'static Mutex<HashMap<i64, WinState>> {
    static WINDOWS: OnceLock<Mutex<HashMap<i64, WinState>>> = OnceLock::new();
    WINDOWS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Marca la ventana como cerrada y emite su evento `closed` si aún no se emitió. Punto único:
/// lo llaman el delegate (botón rojo) y `close_window` (cierre programático).
fn mark_closed(id: i64) {
    let mut map = windows().lock().unwrap();
    if let Some(w) = map.get_mut(&id)
        && !w.closed
    {
        w.closed = true;
        drop(map);
        push_event("closed", id);
    }
}

// ── El gate del hilo principal ───────────────────────────────────────────────

enum AppState {
    NotStarted,
    Ready,
    Failed(String),
}

struct Gate {
    waker: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    state: Mutex<AppState>,
    changed: Condvar,
}

fn gate() -> &'static Gate {
    static GATE: OnceLock<Gate> = OnceLock::new();
    GATE.get_or_init(|| Gate {
        waker: Mutex::new(None),
        state: Mutex::new(AppState::NotStarted),
        changed: Condvar::new(),
    })
}

/// Registra el aviso "una operación de UI necesita el hilo principal". Lo llama el HOST (el
/// binario `ray` o el `main` emitido) desde el arranque; el closure debe despertar al hilo 1,
/// que entonces llama [`run_main_loop`]. Sin registro, la primera operación devuelve `Err`.
pub fn set_main_thread_waker(waker: Box<dyn Fn() + Send + Sync>) {
    *gate().waker.lock().unwrap() = Some(waker);
}

/// Corre el loop de AppKit EN EL HILO QUE LLAMA — que debe ser el hilo 1 del proceso. Marca la
/// app como lista (despierta a la operación que la pidió) y no retorna salvo fallo de
/// inicialización (`Err`: sin sesión gráfica), en cuyo caso los que esperan reciben el error.
#[cfg(target_os = "macos")]
pub fn run_main_loop() -> Result<(), String> {
    let g = gate();
    match mac::init_app() {
        Ok(()) => {
            *g.state.lock().unwrap() = AppState::Ready;
            g.changed.notify_all();
            mac::run_app(); // no retorna: [NSApp run] para el resto del proceso
            unreachable!("NSApp run returned");
        }
        Err(e) => {
            *g.state.lock().unwrap() = AppState::Failed(e.clone());
            g.changed.notify_all();
            Err(e)
        }
    }
}

/// Pide el hilo principal (una vez) y espera a que la app esté lista, con plazo — sin sesión
/// gráfica o sin host el error es limpio, nunca un cuelgue.
#[cfg(target_os = "macos")]
fn ensure_app() -> Result<(), String> {
    let g = gate();
    let mut st = g.state.lock().unwrap();
    if let AppState::Ready = *st {
        return Ok(());
    }
    // Dispara el waker (idempotente: el host ignora avisos repetidos; el estado manda).
    match g.waker.lock().unwrap().as_ref() {
        Some(w) => w(),
        None => {
            return Err(
                "ui: no main-thread host (run through the ray binary or a native build)".to_string()
            )
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match &*st {
            AppState::Ready => return Ok(()),
            AppState::Failed(e) => return Err(e.clone()),
            AppState::NotStarted => {
                let rem = deadline.saturating_duration_since(std::time::Instant::now());
                if rem.is_zero() {
                    return Err("ui: could not initialize AppKit (no GUI session?)".to_string());
                }
                st = g.changed.wait_timeout(st, rem).unwrap().0;
            }
        }
    }
}

// ── La superficie ────────────────────────────────────────────────────────────

/// Fuerza el backend headless salvo que `RAY_UI_BACKEND` diga otra cosa (el runner de `ray
/// test` lo llama: una suite no debe abrir ventanas reales por defecto; el env del usuario manda).
pub fn default_headless() {
    FORCED_HEADLESS.store(true, std::sync::atomic::Ordering::SeqCst);
}

static FORCED_HEADLESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn headless() -> bool {
    match std::env::var("RAY_UI_BACKEND").as_deref() {
        Ok("headless") => true,
        Ok(_) => false,
        Err(_) => FORCED_HEADLESS.load(std::sync::atomic::Ordering::SeqCst),
    }
}

/// Avisa a `ray dev` (una vez por proceso) de que este programa es una APP CON VENTANA: el hub
/// del supervisor vive en el puerto de `RAY_DEV_RELOAD` (el mismo canal del live-reload,
/// precedente `dev_notify_ready` del webserver). Con eso, "la ventana se cerró y el programa
/// salió limpio" = el usuario cerró la app → `ray dev` sale con ella (el contrato de las TUI
/// de M139, sin tecla extra). Best-effort: sin la variable o sin hub, silencio y se sigue.
fn notify_dev_windowed() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Ok(port) = std::env::var("RAY_DEV_RELOAD") else {
            return;
        };
        let Ok(port) = port.parse::<u16>() else {
            return;
        };
        // Aparte del hilo llamador: un connect que se cuelgue no debe retrasar la ventana.
        std::thread::spawn(move || {
            use std::io::Write;
            if let Ok(mut s) = std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
                std::time::Duration::from_millis(500),
            ) {
                let _ = s.write_all(b"GET /ui HTTP/1.0\r\n\r\n");
            }
        });
    });
}

/// Abre una ventana con el webview del sistema cargando `url`, registrada con el `id` DEL
/// LLAMADOR (su handle): los eventos la nombran con ese id. `close(h)` → [`close_window`].
pub fn open_window(id: i64, title: &str, url: &str, width: i64, height: i64) -> Result<(), String> {
    if !(1..=16384).contains(&width) || !(1..=16384).contains(&height) {
        return Err(format!("ui: unsupported window size {width}x{height}"));
    }
    notify_dev_windowed();
    if headless() {
        windows().lock().unwrap().insert(id, WinState { win: Win::Headless, closed: false });
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        ensure_app()?;
        let mw = mac::open_window(title, url, width, height)?;
        windows().lock().unwrap().insert(id, WinState { win: mw, closed: false });
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (title, url);
        Err("ui: no backend for this platform (macOS; RAY_UI_BACKEND=headless works anywhere)"
            .to_string())
    }
}

/// Ejecuta JavaScript en la página de la ventana, SIN esperar el resultado (v1: el
/// completionHandler es nil — el eval con retorno exige el ABI de blocks y queda para v2).
pub fn eval_js(id: i64, js: &str) -> Result<(), String> {
    let map = windows().lock().unwrap();
    match map.get(&id) {
        None => Err("ui: not an open window".to_string()),
        Some(w) => match &w.win {
            Win::Headless => Ok(()),
            #[cfg(target_os = "macos")]
            Win::Mac { webview, .. } => {
                let wv = *webview;
                drop(map);
                mac::eval_js(wv, js);
                Ok(())
            }
        },
    }
}

/// Cierra la ventana `id` (idempotente; el evento `closed` se emite si no se había emitido).
/// SIEMPRE asíncrono hacia el hilo principal: lo llama el `Drop` del handle, que puede correr
/// en cualquier hilo — y con la app sin arrancar un despacho síncrono jamás volvería.
pub fn close_window(id: i64) {
    mark_closed(id);
    let removed = windows().lock().unwrap().remove(&id);
    match removed {
        None | Some(WinState { win: Win::Headless, .. }) => {}
        #[cfg(target_os = "macos")]
        Some(WinState { win: Win::Mac { window, webview, delegate }, .. }) => {
            mac::close_window_async(window, webview, delegate);
        }
    }
}

// ── macOS: AppKit + WKWebView por mensajes objc a mano ───────────────────────
#[cfg(target_os = "macos")]
mod mac {
    use super::Win;
    use std::ffi::c_void;

    // Los objetos y selectores de objc son punteros opacos.
    type Id = *mut c_void;
    type Sel = *mut c_void;

    /// NSRect en 64-bit: cuatro f64 planos (origin + size).
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    // La cola principal de libdispatch es un STATIC exportado (el header la envuelve en un
    // macro); su tipo real es opaco — solo se necesita su dirección.
    #[repr(C)]
    struct DispatchQueue {
        _opaque: [u8; 0],
    }

    // Un solo bloque extern: los `#[link]` enlazan libobjc y los tres frameworks (siempre
    // presentes en macOS) — las CLASES (NSWindow, WKWebView…) llegan por estos enlaces; los
    // símbolos C de objc son la única superficie declarada. Sin build.rs (patrón audio.rs).
    #[allow(clippy::duplicated_attributes)] // un #[link] por framework: no es una repetición
    #[link(name = "objc")]
    #[link(name = "Foundation", kind = "framework")]
    #[link(name = "AppKit", kind = "framework")]
    #[link(name = "WebKit", kind = "framework")]
    unsafe extern "C" {
        fn objc_getClass(name: *const u8) -> Id;
        fn sel_registerName(name: *const u8) -> Sel;
        // Se declara SIN tipo y se castea por sitio a la firma exacta (abajo): en arm64 el ABI
        // de msgSend exige el prototipo real, jamás una llamada variádica.
        fn objc_msgSend();
        fn objc_allocateClassPair(
            superclass: Id,
            name: *const std::ffi::c_char,
            extra: usize,
        ) -> Id;
        fn objc_registerClassPair(cls: Id);
        fn class_addMethod(
            cls: Id,
            sel: Sel,
            imp: extern "C" fn(Id, Sel, Id),
            types: *const std::ffi::c_char,
        ) -> u8;
    }
    unsafe extern "C" {
        static _dispatch_main_q: DispatchQueue;
        fn dispatch_async_f(
            queue: *const DispatchQueue,
            context: *mut c_void,
            work: extern "C" fn(*mut c_void),
        );
    }

    // msgSend casteado a la firma exacta de cada mensaje (los alias nombran la forma).
    type MsgId = unsafe extern "C" fn(Id, Sel) -> Id;
    type MsgVoid = unsafe extern "C" fn(Id, Sel);
    type MsgVoidId = unsafe extern "C" fn(Id, Sel, Id);
    type MsgVoidBool = unsafe extern "C" fn(Id, Sel, u8);
    type MsgBoolInt = unsafe extern "C" fn(Id, Sel, i64) -> u8;
    type MsgIdId = unsafe extern "C" fn(Id, Sel, Id) -> Id;
    type MsgIdIdId = unsafe extern "C" fn(Id, Sel, Id, Id);
    type MsgInitWindow = unsafe extern "C" fn(Id, Sel, CGRect, u64, u64, u8) -> Id;
    type MsgInitFrame = unsafe extern "C" fn(Id, Sel, CGRect) -> Id;
    type MsgInitBytes = unsafe extern "C" fn(Id, Sel, *const u8, usize, u64) -> Id;

    fn msg_send() -> *const c_void {
        objc_msgSend as unsafe extern "C" fn() as *const c_void
    }

    fn cls(name: &[u8]) -> Id {
        // SAFETY: literal NUL-terminado (todos los llamadores usan b"...\0").
        unsafe { objc_getClass(name.as_ptr()) }
    }

    fn sel(name: &[u8]) -> Sel {
        // SAFETY: literal NUL-terminado.
        unsafe { sel_registerName(name.as_ptr()) }
    }

    // Un NSString desde un &str (initWithBytes: acepta interior NULs, a diferencia de UTF8String).
    unsafe fn nsstring(s: &str) -> Id {
        const NS_UTF8: u64 = 4;
        unsafe {
            let alloc: MsgId = std::mem::transmute(msg_send());
            let init: MsgInitBytes = std::mem::transmute(msg_send());
            let obj = alloc(cls(b"NSString\0"), sel(b"alloc\0"));
            init(obj, sel(b"initWithBytes:length:encoding:\0"), s.as_ptr(), s.len(), NS_UTF8)
        }
    }

    /// Despacha `f` a la cola principal (la drena el run loop de NSApp). Nunca síncrono.
    fn on_main(f: impl FnOnce() + Send + 'static) {
        extern "C" fn trampoline(ctx: *mut c_void) {
            // SAFETY: `ctx` es el Box de abajo, entregado una sola vez por libdispatch.
            let f = unsafe { Box::from_raw(ctx as *mut Box<dyn FnOnce() + Send>) };
            f();
        }
        let boxed: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(f));
        // SAFETY: la cola principal es un static del sistema; el Box viaja al trampoline.
        unsafe {
            dispatch_async_f(&_dispatch_main_q, Box::into_raw(boxed) as *mut c_void, trampoline);
        }
    }

    /// Despacha `f` al hilo principal y ESPERA su resultado (con plazo: si el run loop no
    /// drena — app muerta a mitad — el error es limpio).
    fn on_main_sync<T: Send + 'static>(
        f: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let slot = std::sync::Arc::new((std::sync::Mutex::new(None), std::sync::Condvar::new()));
        let slot2 = slot.clone();
        on_main(move || {
            *slot2.0.lock().unwrap() = Some(f());
            slot2.1.notify_all();
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut got = slot.0.lock().unwrap();
        loop {
            if let Some(r) = got.take() {
                return r;
            }
            let rem = deadline.saturating_duration_since(std::time::Instant::now());
            if rem.is_zero() {
                return Err("ui: the main thread did not respond".to_string());
            }
            got = slot.1.wait_timeout(got, rem).unwrap().0;
        }
    }

    /// Inicializa NSApplication en el hilo que llama (el 1). Falla limpio sin sesión gráfica.
    pub(super) fn init_app() -> Result<(), String> {
        const POLICY_REGULAR: i64 = 0;
        unsafe {
            let shared: MsgId = std::mem::transmute(msg_send());
            let app = shared(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
            if app.is_null() {
                return Err("ui: could not initialize AppKit (no GUI session?)".to_string());
            }
            let set_policy: MsgBoolInt = std::mem::transmute(msg_send());
            set_policy(app, sel(b"setActivationPolicy:\0"), POLICY_REGULAR);
        }
        Ok(())
    }

    /// `[NSApp run]` — no retorna.
    pub(super) fn run_app() {
        unsafe {
            let shared: MsgId = std::mem::transmute(msg_send());
            let app = shared(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
            let run: MsgVoid = std::mem::transmute(msg_send());
            run(app, sel(b"run\0"));
        }
    }

    /// La clase delegate (una por proceso): NSObject + `windowWillClose:` → evento `closed`.
    fn delegate_class() -> Id {
        static CLASS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *CLASS.get_or_init(|| {
            extern "C" fn window_will_close(_this: Id, _sel: Sel, notification: Id) {
                // SAFETY: el run loop entrega una NSNotification válida; `object` es la ventana.
                let win = unsafe {
                    let object: MsgId = std::mem::transmute(msg_send());
                    object(notification, sel(b"object\0"))
                };
                // La ventana → nuestro id: se busca en el mapa (solo entradas Mac vivas; un
                // cierre programático ya la quitó y no re-emite).
                let map = super::windows().lock().unwrap();
                let found = map.iter().find_map(|(id, w)| match &w.win {
                    Win::Mac { window, .. } if *window == win as usize => Some(*id),
                    _ => None,
                });
                drop(map);
                if let Some(id) = found {
                    super::mark_closed(id);
                }
            }
            unsafe {
                let cls_new =
                    objc_allocateClassPair(cls(b"NSObject\0"), c"RayWindowDelegate".as_ptr(), 0);
                class_addMethod(
                    cls_new,
                    sel(b"windowWillClose:\0"),
                    window_will_close,
                    c"v@:@".as_ptr(),
                );
                objc_registerClassPair(cls_new);
                cls_new as usize
            }
        }) as Id
    }

    /// Crea la ventana + webview EN EL HILO PRINCIPAL y devuelve sus punteros retenidos.
    pub(super) fn open_window(title: &str, url: &str, width: i64, height: i64) -> Result<Win, String> {
        let title = title.to_string();
        let url = url.to_string();
        on_main_sync(move || {
            const STYLE_TITLED_CLOSABLE_MINIATURIZABLE_RESIZABLE: u64 = 1 | 2 | 4 | 8;
            const BACKING_BUFFERED: u64 = 2;
            let rect = CGRect { x: 0.0, y: 0.0, w: width as f64, h: height as f64 };
            unsafe {
                let alloc: MsgId = std::mem::transmute(msg_send());
                let init_window: MsgInitWindow = std::mem::transmute(msg_send());
                let init_frame: MsgInitFrame = std::mem::transmute(msg_send());
                let set_id: MsgVoidId = std::mem::transmute(msg_send());
                let set_bool: MsgVoidBool = std::mem::transmute(msg_send());
                let plain: MsgVoid = std::mem::transmute(msg_send());
                let id_id: MsgIdId = std::mem::transmute(msg_send());
                let shared: MsgId = std::mem::transmute(msg_send());

                let window = init_window(
                    alloc(cls(b"NSWindow\0"), sel(b"alloc\0")),
                    sel(b"initWithContentRect:styleMask:backing:defer:\0"),
                    rect,
                    STYLE_TITLED_CLOSABLE_MINIATURIZABLE_RESIZABLE,
                    BACKING_BUFFERED,
                    0,
                );
                if window.is_null() {
                    return Err("ui: could not create the window".to_string());
                }
                // ¡La trampa nº1!: los NSWindow programáticos se AUTOLIBERAN al cerrarse; el
                // registro guarda el puntero → use-after-free en el siguiente close(h). El
                // ciclo de vida es nuestro: retención del registro + release explícito.
                set_bool(window, sel(b"setReleasedWhenClosed:\0"), 0);
                set_id(window, sel(b"setTitle:\0"), nsstring(&title));

                let webview = init_frame(
                    alloc(cls(b"WKWebView\0"), sel(b"alloc\0")),
                    sel(b"initWithFrame:\0"),
                    rect,
                );
                if webview.is_null() {
                    return Err("ui: could not create the webview".to_string());
                }
                set_id(window, sel(b"setContentView:\0"), webview);

                let ns_url = id_id(cls(b"NSURL\0"), sel(b"URLWithString:\0"), nsstring(&url));
                if ns_url.is_null() {
                    return Err(format!("ui: invalid url: {url}"));
                }
                let request = id_id(cls(b"NSURLRequest\0"), sel(b"requestWithURL:\0"), ns_url);
                let _navigation = id_id(webview, sel(b"loadRequest:\0"), request);

                // El delegate NO es retenido por la ventana (referencia débil de AppKit): se
                // retiene en el registro y se libera junto a la ventana.
                let delegate = alloc(delegate_class(), sel(b"alloc\0"));
                let delegate = {
                    let init: MsgId = std::mem::transmute(msg_send());
                    init(delegate, sel(b"init\0"))
                };
                set_id(window, sel(b"setDelegate:\0"), delegate);

                plain(window, sel(b"center\0"));
                set_id(window, sel(b"makeKeyAndOrderFront:\0"), std::ptr::null_mut());
                // Un binario sin bundle abre DETRÁS de las demás apps si no se activa.
                let app = shared(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
                set_bool(app, sel(b"activateIgnoringOtherApps:\0"), 1);

                Ok(Win::Mac {
                    window: window as usize,
                    webview: webview as usize,
                    delegate: delegate as usize,
                })
            }
        })
    }

    /// Ejecuta JS en el webview, fire-and-forget (completionHandler nil), en el hilo principal.
    pub(super) fn eval_js(webview: usize, js: &str) {
        let js = js.to_string();
        on_main(move || unsafe {
            let eval: MsgIdIdId = std::mem::transmute(msg_send());
            eval(
                webview as Id,
                sel(b"evaluateJavaScript:completionHandler:\0"),
                nsstring(&js),
                std::ptr::null_mut(),
            );
        });
    }

    /// Cierra y LIBERA la ventana en el hilo principal, asíncrono (llamable desde un Drop).
    pub(super) fn close_window_async(window: usize, webview: usize, delegate: usize) {
        on_main(move || unsafe {
            let set_id: MsgVoidId = std::mem::transmute(msg_send());
            let plain: MsgVoid = std::mem::transmute(msg_send());
            // El delegate se desengancha ANTES del close: el registro ya no tiene la entrada y
            // el evento `closed` ya se emitió — el callback no debe correr sobre un mapa vacío.
            set_id(window as Id, sel(b"setDelegate:\0"), std::ptr::null_mut());
            plain(window as Id, sel(b"close\0"));
            // Nuestras retenciones (alloc/init): la ventana, el webview y el delegate.
            plain(webview as Id, sel(b"release\0"));
            plain(delegate as Id, sel(b"release\0"));
            plain(window as Id, sel(b"release\0"));
        });
    }
}
