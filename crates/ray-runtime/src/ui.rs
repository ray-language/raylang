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
//! `#[link(kind = "framework")]`, sin build.rs); GTK3 + WebKitGTK en Linux (M147d, por `dlopen`
//! EN RUNTIME — sin headers de build; sin las libs → `Err` claro, patrón ALSA de audio);
//! `RAY_UI_BACKEND=headless` = ventanas de mesa (tabla en memoria, `close` sintetiza el evento
//! `closed`) en cualquier OS — la vía de los tests/CI, como `RAY_AUDIO_SINK=null` en audio.
//! Objective-C A MANO y GTK A MANO (sin crates objc/wry: la lección cpal de M145 — los crates
//! de webview exigen toolchains GTK/WebKit en build).

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
#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NONBLOCK: i32 = 0o4000; // M156: bionic también es 0o4000 (android es unix, no "linux")
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const O_NONBLOCK: i32 = 0x0004;

// ── La cola global de eventos + self-pipe ────────────────────────────────────

/// Un evento de UI: `(kind, ventana, tag)`. `"closed"` (la ventana se fue de la pantalla, sea
/// por el botón rojo o por `close(h)` — exactamente una vez por ventana; tag vacío) y `"menu"`
/// (M148: un item de menú custom; ventana 0 — el menú es de la app — y tag del item).
type Event = (String, i64, String);

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

/// M152/M157: el shim del puente IPC, inyectado como user script en el webview (mac/GTK; las
/// plantillas de los shells iOS/Android llevan la MISMA lógica adaptada a su transporte).
/// Superficie: `window.ray.send(v)` (v no-string viaja como JSON — M157) y
/// `window.ray.request(v) -> Promise` (M157: sobre con id `\u0001q\u0001<id>\u0001<payload>`;
/// el programa responde con `ui.reply(w, id, valor)`, que resuelve la Promise vía
/// `window.ray._deliver` — TODO sobre el eval_js fire-and-forget existente: cero cambios
/// nativos). Los NULs se eliminan; el lado nativo siempre ve un C-string completo.
#[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))] // el shell móvil lleva el shim copiado en su plantilla
pub(crate) const RAY_JS_SHIM: &str = r#"(function(){var p={},n=0;function e(t){return typeof t==="string"?t:JSON.stringify(t)}function q(s){window.webkit.messageHandlers.ray.postMessage(String(s).replace(/\u0000/g,""))}window.ray={send:function(t){q(e(t))},request:function(t){n=n+1;var i=n;return new Promise(function(r){p[i]=r;q("\u0001q\u0001"+i+"\u0001"+e(t))})},_deliver:function(i,v){var r=p[i];if(r){delete p[i];r(v)}}}})();"#;

fn push_event(kind: &str, window: i64, tag: &str) {
    let ev = events();
    ev.queue.lock().unwrap().push_back((kind.to_string(), window, tag.to_string()));
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
pub fn try_next_event() -> Option<(String, i64, String)> {
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
pub fn next_event_blocking(ms: i64) -> Option<(String, i64, String)> {
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
    /// §80b: una "ventana" del SHELL móvil (iOS; o el modo de prueba `ui-shell`): el shell
    /// posee el webview — aquí solo queda la fila (los handlers C hacen el trabajo).
    #[cfg(any(target_os = "ios", target_os = "android", feature = "ui-shell"))]
    Shell,
    /// M147d: GTK3 + WebKitGTK (Linux, por dlopen). `alive` lo apaga el handler de `destroy` —
    /// TODA closure despachada al hilo gtk lo re-chequea antes de tocar los punteros (el botón
    /// de cerrar del WM destruye la ventana por debajo nuestro: anti use-after-destroy).
    #[cfg(target_os = "linux")]
    Gtk {
        window: usize,
        webview: usize,
        alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
        push_event("closed", id, "");
    }
}

// ── El gate del hilo principal ───────────────────────────────────────────────

// En iOS el gate entero es letra muerta (el shell posee el hilo 1 y los brazos ios lo
// esquivan) — se conserva compilando por coherencia de los 6 sitios.
#[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
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

/// Corre el loop de UI del backend EN EL HILO QUE LLAMA — que debe ser el hilo 1 del proceso
/// (AppKit en macOS; GTK en Linux, M147d). Marca la app como lista (despierta a la operación
/// que la pidió) y no retorna salvo fallo de inicialización (`Err`: sin sesión gráfica o sin
/// backend), en cuyo caso los que esperan reciben el error. Existe en TODO unix: el host la
/// llama sin cfg por plataforma (los 6 gates deben moverse juntos — la lección del plan).
pub fn run_main_loop() -> Result<(), String> {
    let g = gate();
    #[cfg(target_os = "macos")]
    let init = mac::init_app();
    #[cfg(target_os = "linux")]
    let init = gtk::init_app();
    // iOS/Android: el hilo 1 es del shell (UIApplicationMain / ART) — este loop jamás corre.
    #[cfg(target_os = "ios")]
    let init: Result<(), String> =
        Err("ui: the shell owns the main loop on iOS (run inside the generated app shell)".to_string());
    #[cfg(target_os = "android")]
    let init: Result<(), String> =
        Err("ui: the shell owns the main loop on Android (run inside the generated app shell)".to_string());
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "ios", target_os = "android")))]
    let init: Result<(), String> =
        Err("ui: no backend for this platform (macOS/Linux; RAY_UI_BACKEND=headless works anywhere)"
            .to_string());
    match init {
        Ok(()) => {
            *g.state.lock().unwrap() = AppState::Ready;
            g.changed.notify_all();
            // No retorna: [NSApp run] / gtk_main() para el resto del proceso.
            #[cfg(target_os = "macos")]
            mac::run_app();
            #[cfg(target_os = "linux")]
            gtk::run_app();
            unreachable!("the UI main loop returned");
        }
        Err(e) => {
            *g.state.lock().unwrap() = AppState::Failed(e.clone());
            g.changed.notify_all();
            Err(e)
        }
    }
}

/// El mensaje del plazo vencido de `ensure_app`, por backend.
#[cfg(target_os = "macos")]
const APP_TIMEOUT_MSG: &str = "ui: could not initialize AppKit (no GUI session?)";
#[cfg(target_os = "linux")]
const APP_TIMEOUT_MSG: &str = "ui: could not initialize GTK (no DISPLAY/WAYLAND_DISPLAY?)";
#[cfg(not(any(target_os = "macos", target_os = "linux")))] // ios/android: dead_code permitido abajo
#[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
const APP_TIMEOUT_MSG: &str = "ui: could not initialize the display backend (no GUI session?)";

/// Pide el hilo principal (una vez) y espera a que la app esté lista, con plazo — sin sesión
/// gráfica o sin host el error es limpio, nunca un cuelgue. Compartida por todos los backends.
#[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
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
                    return Err(APP_TIMEOUT_MSG.to_string());
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
        // M152: el inyector de MENSAJES para pruebas (precedente RAY_UI_PICK): con la
        // variable seteada y no vacía, cada ventana headless "recibe" ese window.ray.send
        // al abrir — la batería de 3 motores asevera el kind "message" byte-idéntico.
        if let Ok(msg) = std::env::var("RAY_UI_MSG")
            && !msg.is_empty()
        {
            push_event("message", id, &msg);
        }
        return Ok(());
    }
    // §80b: modo SHELL (iOS; o `ui-shell` en pruebas) — el shell registró sus handlers ANTES
    // de ray_start: la "ventana" es su webview; aquí solo viaja (title, url). Sin gate: el
    // hilo principal es del shell (UIApplicationMain), no nuestro.
    #[cfg(any(target_os = "ios", target_os = "android", feature = "ui-shell"))]
    if shell::active() {
        shell::open(title, url);
        windows().lock().unwrap().insert(id, WinState { win: Win::Shell, closed: false });
        return Ok(());
    }
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        Err("ui: no shell handler (run inside the generated app shell)".to_string())
    }
    #[cfg(target_os = "macos")]
    {
        ensure_app()?;
        let mw = mac::open_window(title, url, width, height)?;
        windows().lock().unwrap().insert(id, WinState { win: mw, closed: false });
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        ensure_app()?;
        let gw = gtk::open_window(id, title, url, width, height)?;
        windows().lock().unwrap().insert(id, WinState { win: gw, closed: false });
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "ios", target_os = "android")))]
    {
        let _ = (title, url);
        Err("ui: no backend for this platform (macOS/Linux; RAY_UI_BACKEND=headless works anywhere)"
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
            #[cfg(any(target_os = "ios", target_os = "android", feature = "ui-shell"))]
            Win::Shell => {
                drop(map);
                shell::eval(js);
                Ok(())
            }
            #[cfg(target_os = "macos")]
            Win::Mac { webview, .. } => {
                let wv = *webview;
                drop(map);
                mac::eval_js(wv, js);
                Ok(())
            }
            #[cfg(target_os = "linux")]
            Win::Gtk { webview, alive, .. } => {
                let wv = *webview;
                let alive = alive.clone();
                drop(map);
                gtk::eval_js(wv, alive, js);
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
        // §80b: la fila del shell — el evento `closed` ya lo emitió el mark_closed genérico;
        // avisar al shell (esconder su webview) queda para cuando un dogfood lo pida.
        #[cfg(any(target_os = "ios", target_os = "android", feature = "ui-shell"))]
        Some(WinState { win: Win::Shell, .. }) => {}
        #[cfg(target_os = "macos")]
        Some(WinState { win: Win::Mac { window, webview, delegate }, .. }) => {
            mac::close_window_async(window, webview, delegate);
        }
        #[cfg(target_os = "linux")]
        Some(WinState { win: Win::Gtk { window, alive, .. }, .. }) => {
            gtk::close_window_async(window, alive);
        }
    }
}


/// M148: añade UN menú de nivel superior con items custom. `items` llegan CODIFICADOS
/// "tag\ttitle\tshortcut" (el borde de los builtins es [string]); la decodificación vive AQUÍ
/// — compartida por los tres motores (el binario transpilado llama directo a este crate). Un
/// click emite el evento ("menu", 0, tag). Headless: no-op Ok (los tests no montan menús).
/// macOS: el menú es GLOBAL (la barra de la app); Linux/GTK: el menubar es POR VENTANA — los
/// menús aplican a las ventanas abiertas DESPUÉS de esta llamada (documentado).
pub fn menu(title: &str, items: &[String]) -> Result<(), String> {
    let decoded: Vec<(String, String, String)> = items
        .iter()
        .map(|it| {
            let mut parts = it.splitn(3, '\t');
            (
                parts.next().unwrap_or("").to_string(),
                parts.next().unwrap_or("").to_string(),
                parts.next().unwrap_or("").to_string(),
            )
        })
        .collect();
    if decoded.iter().any(|(tag, _, _)| tag.is_empty()) {
        return Err("ui: a menu item needs a non-empty tag".to_string());
    }
    if headless() {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        ensure_app()?;
        mac::add_menu(title, &decoded)
    }
    #[cfg(target_os = "linux")]
    {
        ensure_app()?;
        gtk::add_menu(title, &decoded)
    }
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        let _ = (title, decoded);
        Err("ui: menus are not available on mobile (v1)".to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "ios", target_os = "android")))]
    {
        let _ = title;
        Err("ui: no backend for this platform (macOS/Linux; RAY_UI_BACKEND=headless works anywhere)"
            .to_string())
    }
}

/// M155: los valores del panel About declarados por el programa (name, version, description,
/// copyright; "" = omitir el campo). Los lee el action del item "role:about" en macOS AL
/// CLICK (así set_about vale antes o después de app_menu); en Linux/iOS se guardan pero el
/// panel nativo no existe ahí (Linux entrega "role:about" como evento — about propio).
type AboutInfo = (String, String, String, String);

fn about_info() -> &'static std::sync::Mutex<Option<AboutInfo>> {
    static ABOUT: std::sync::OnceLock<std::sync::Mutex<Option<AboutInfo>>> =
        std::sync::OnceLock::new();
    ABOUT.get_or_init(|| std::sync::Mutex::new(None))
}

/// M155: declara el contenido del panel About nativo (macOS). Cada campo con "" se omite y
/// el panel usa lo del bundle. Headless y plataformas sin panel: se guarda y Ok.
pub fn set_about(name: &str, version: &str, description: &str, copyright: &str) -> Result<(), String> {
    *about_info().lock().unwrap() = Some((
        name.to_string(),
        version.to_string(),
        description.to_string(),
        copyright.to_string(),
    ));
    Ok(())
}

/// M151 (raydesk #10): items en el menú de APLICACIÓN + su título opcional. macOS: item 0 de
/// la barra global (encima de Hide/Quit; tag "role:about" = About nativo sin evento; `name`
/// re-titula el menú — bajo `ray run` salía el nombre del proceso). Linux: no existe menú de
/// app global — los items van como un menú normal titulado `name` (o "App") y TODOS emiten el
/// evento ("menu", 0, tag), "role:about" incluido (el programa muestra su propio about).
/// Headless: no-op Ok. iOS: sin barra de menús.
pub fn app_menu(name: &str, items: &[String]) -> Result<(), String> {
    let decoded: Vec<(String, String, String)> = items
        .iter()
        .map(|it| {
            let mut parts = it.splitn(3, '\t');
            (
                parts.next().unwrap_or("").to_string(),
                parts.next().unwrap_or("").to_string(),
                parts.next().unwrap_or("").to_string(),
            )
        })
        .collect();
    if decoded.iter().any(|(tag, _, _)| tag.is_empty()) {
        return Err("ui: a menu item needs a non-empty tag".to_string());
    }
    if headless() {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        ensure_app()?;
        mac::set_app_menu(name, &decoded)
    }
    #[cfg(target_os = "linux")]
    {
        ensure_app()?;
        let title = if name.is_empty() { "App" } else { name };
        gtk::add_menu(title, &decoded)
    }
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        let _ = (name, decoded);
        Err("ui: menus are not available on mobile (v1)".to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "ios", target_os = "android")))]
    {
        let _ = name;
        Err("ui: no backend for this platform (macOS/Linux; RAY_UI_BACKEND=headless works anywhere)"
            .to_string())
    }
}

/// M148: un diálogo de archivo nativo, MODAL (bloquea al llamador lo que el usuario tarde —
/// por eso va por la espera SIN plazo, alcanzable solo tras `ensure_app`). `kind`:
/// "open_file" | "open_folder" | "save_file" (`arg` = nombre sugerido del save). `Ok(None)` =
/// canceló. Headless: el resultado se inyecta con `RAY_UI_PICK` (ausente/vacío = None) — la
/// vía de la batería de 3 motores. Contrato v1: UN modal a la vez (en macOS la main queue es
/// serial: otra op de UI concurrente espera al panel; GTK no lo sufre — su loop recursivo
/// sigue drenando idles).
pub fn dialog(kind: &str, arg: &str) -> Result<Option<String>, String> {
    if !matches!(kind, "open_file" | "open_folder" | "save_file") {
        return Err(format!("ui: unknown dialog kind '{kind}'"));
    }
    if headless() {
        return Ok(std::env::var("RAY_UI_PICK").ok().filter(|s| !s.is_empty()));
    }
    #[cfg(target_os = "macos")]
    {
        ensure_app()?;
        mac::dialog(kind, arg)
    }
    #[cfg(target_os = "linux")]
    {
        ensure_app()?;
        gtk::dialog(kind, arg)
    }
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        let _ = (kind, arg);
        Err("ui: file dialogs are not available on mobile (v1)".to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "ios", target_os = "android")))]
    {
        let _ = arg;
        Err("ui: no backend for this platform (macOS/Linux; RAY_UI_BACKEND=headless works anywhere)"
            .to_string())
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
        // M152: `imp` va SIN tipo (como objc_msgSend) y se castea por sitio — el método del
        // puente (userContentController:didReceiveScriptMessage:) es de 4 args y una segunda
        // declaración del mismo símbolo con otra firma dispararía clashing_extern_declarations.
        fn class_addMethod(
            cls: Id,
            sel: Sel,
            imp: *const c_void,
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
    type MsgInitBytes = unsafe extern "C" fn(Id, Sel, *const u8, usize, u64) -> Id;
    // M148 (menús + diálogos):
    type MsgMenuItemInit = unsafe extern "C" fn(Id, Sel, Id, Sel, Id) -> Id;
    type MsgVoidI64 = unsafe extern "C" fn(Id, Sel, i64);
    type MsgI64 = unsafe extern "C" fn(Id, Sel) -> i64;
    // M151: itemAtIndex: / insertItem:atIndex: (menú de aplicación).
    type MsgIdI64 = unsafe extern "C" fn(Id, Sel, i64) -> Id;
    type MsgVoidIdI64 = unsafe extern "C" fn(Id, Sel, Id, i64);
    type MsgCStr = unsafe extern "C" fn(Id, Sel) -> *const std::ffi::c_char;
    // M152 (puente IPC): initWithFrame:configuration: / addScriptMessageHandler:name: /
    // isKindOfClass: / initWithSource:injectionTime:forMainFrameOnly:.
    type MsgInitFrameCfg = unsafe extern "C" fn(Id, Sel, CGRect, Id) -> Id;
    type MsgVoidIdId = unsafe extern "C" fn(Id, Sel, Id, Id);
    type MsgBoolId = unsafe extern "C" fn(Id, Sel, Id) -> u8;
    type MsgInitUserScript = unsafe extern "C" fn(Id, Sel, Id, i64, u8) -> Id;

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

    /// Como `on_main_sync` pero SIN plazo: para los diálogos MODALES (el usuario tarda lo que
    /// tarda). Solo alcanzable tras `ensure_app` (que sí acota a 5 s) — con la app Ready el
    /// loop jamás sale, así que la espera no puede quedar huérfana.
    fn on_main_sync_wait<T: Send + 'static>(
        f: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let slot = std::sync::Arc::new((std::sync::Mutex::new(None), std::sync::Condvar::new()));
        let slot2 = slot.clone();
        on_main(move || {
            *slot2.0.lock().unwrap() = Some(f());
            slot2.1.notify_all();
        });
        let mut got = slot.0.lock().unwrap();
        loop {
            if let Some(r) = got.take() {
                return r;
            }
            got = slot.1.wait(got).unwrap();
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
            // M148: el menú ESTÁNDAR, automático. Sin menú principal, los key equivalents no
            // viajan (⌘C/⌘V/⌘X muertos en los campos de texto del webview — el bug real que
            // esto arregla). Item 0 = menú de la app POR POSICIÓN (Hide ⌘H, Quit ⌘Q); Edit
            // completo con targets NIL (responder chain → el webview los atiende) + ⌘W que
            // desemboca en el windowWillClose: existente (evento closed).
            let main_menu = build_standard_menu(app);
            MAIN_MENU.store(main_menu as usize, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }

    /// La barra de menús viva del proceso (para que `add_menu` appendee), como usize opaco.
    static MAIN_MENU: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    // Un NSMenuItem `title` con action/keyEquivalent (target nil = responder chain), añadido a
    // `menu`. `key` en minúscula = ⌘+tecla; en MAYÚSCULA AppKit añade ⇧ solo (no tocar el mask).
    unsafe fn add_std_item(menu: Id, title: &str, action: &[u8], key: &str) {
        unsafe {
            let alloc: MsgId = std::mem::transmute(msg_send());
            let init: MsgMenuItemInit = std::mem::transmute(msg_send());
            let add: MsgVoidId = std::mem::transmute(msg_send());
            let item = init(
                alloc(cls(b"NSMenuItem\0"), sel(b"alloc\0")),
                sel(b"initWithTitle:action:keyEquivalent:\0"),
                nsstring(title),
                sel(action),
                nsstring(key),
            );
            add(menu, sel(b"addItem:\0"), item);
        }
    }

    // Construye [NSApp setMainMenu:] con el menú de la app + Edit. Corre en el hilo 1, con la
    // app viva, antes de [NSApp run].
    unsafe fn build_standard_menu(app: Id) -> Id {
        unsafe {
            let alloc: MsgId = std::mem::transmute(msg_send());
            let init: MsgId = std::mem::transmute(msg_send());
            let init_title: MsgIdId = std::mem::transmute(msg_send());
            let add: MsgVoidId = std::mem::transmute(msg_send());
            let set_submenu: MsgVoidId = std::mem::transmute(msg_send());
            let set_main: MsgVoidId = std::mem::transmute(msg_send());

            let main_menu = init(alloc(cls(b"NSMenu\0"), sel(b"alloc\0")), sel(b"init\0"));
            // Menú de la app (item 0; el título lo pone el sistema).
            let app_item = init(alloc(cls(b"NSMenuItem\0"), sel(b"alloc\0")), sel(b"init\0"));
            add(main_menu, sel(b"addItem:\0"), app_item);
            let app_menu = init(alloc(cls(b"NSMenu\0"), sel(b"alloc\0")), sel(b"init\0"));
            add_std_item(app_menu, "Hide", b"hide:\0", "h");
            add_std_item(app_menu, "Quit", b"terminate:\0", "q");
            set_submenu(app_item, sel(b"setSubmenu:\0"), app_menu);
            // Edit: el portapapeles/undo del webview viven aquí. El item de la barra lleva su
            // propio título (action nil: solo cuelga el submenú).
            let item_init: MsgMenuItemInit = std::mem::transmute(msg_send());
            let edit_item = item_init(
                alloc(cls(b"NSMenuItem\0"), sel(b"alloc\0")),
                sel(b"initWithTitle:action:keyEquivalent:\0"),
                nsstring("Edit"),
                std::ptr::null_mut(),
                nsstring(""),
            );
            add(main_menu, sel(b"addItem:\0"), edit_item);
            let edit_menu = init_title(
                alloc(cls(b"NSMenu\0"), sel(b"alloc\0")),
                sel(b"initWithTitle:\0"),
                nsstring("Edit"),
            );
            add_std_item(edit_menu, "Undo", b"undo:\0", "z");
            add_std_item(edit_menu, "Redo", b"redo:\0", "Z");
            add_std_item(edit_menu, "Cut", b"cut:\0", "x");
            add_std_item(edit_menu, "Copy", b"copy:\0", "c");
            add_std_item(edit_menu, "Paste", b"paste:\0", "v");
            add_std_item(edit_menu, "Select All", b"selectAll:\0", "a");
            add_std_item(edit_menu, "Close Window", b"performClose:\0", "w");
            set_submenu(edit_item, sel(b"setSubmenu:\0"), edit_menu);

            set_main(app, sel(b"setMainMenu:\0"), main_menu);
            main_menu
        }
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
            // M148: la acción de los menús custom — el sender es el NSMenuItem; su `tag`
            // (i64) mapea al tag String del programa (objc no carga Strings de Rust).
            extern "C" fn menu_action(_this: Id, _sel: Sel, sender: Id) {
                // SAFETY: el run loop entrega un NSMenuItem válido.
                let n = unsafe {
                    let get_tag: MsgI64 = std::mem::transmute(msg_send());
                    get_tag(sender, sel(b"tag\0"))
                };
                if let Some(tag) = menu_tags().lock().unwrap().get(&n) {
                    super::push_event("menu", 0, tag);
                }
            }
            // M152: el puente IPC — userContentController:didReceiveScriptMessage: (4 args).
            // Corre en el hilo principal; el body se COPIA dentro del bloque (autorelease
            // pool, patrón dialog) y el id de ventana sale del scan por puntero del webview
            // (precedente window_will_close). push_event va con el lock ya SOLTADO
            // (disciplina de mark_closed).
            extern "C" fn script_message(_this: Id, _sel: Sel, _controller: Id, message: Id) {
                // SAFETY: el run loop entrega un WKScriptMessage válido.
                let (body, wv) = unsafe {
                    let get_id: MsgId = std::mem::transmute(msg_send());
                    let is_kind: MsgBoolId = std::mem::transmute(msg_send());
                    let utf8: MsgCStr = std::mem::transmute(msg_send());
                    let body = get_id(message, sel(b"body\0"));
                    // Solo strings v1 (paridad con GTK, que guarda con jsc_value_is_string).
                    if body.is_null()
                        || is_kind(body, sel(b"isKindOfClass:\0"), cls(b"NSString\0")) == 0
                    {
                        return;
                    }
                    let c = utf8(body, sel(b"UTF8String\0"));
                    if c.is_null() {
                        return;
                    }
                    let owned = std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned();
                    (owned, get_id(message, sel(b"webView\0")))
                };
                let map = super::windows().lock().unwrap();
                let found = map.iter().find_map(|(id, w)| match &w.win {
                    Win::Mac { webview, .. } if *webview == wv as usize => Some(*id),
                    _ => None,
                });
                drop(map);
                if let Some(id) = found {
                    super::push_event("message", id, &body);
                }
            }
            fn about_info_snapshot() -> Option<super::AboutInfo> {
                super::about_info().lock().unwrap().clone()
            }
            // M155: la action del item "role:about" — abre el panel estándar CON el
            // diccionario de opciones si el programa declaró contenido (ui.set_about), o el
            // panel a secas si no. Corre en el hilo principal (el run loop la entrega).
            extern "C" fn about_action(_this: Id, _sel: Sel, _sender: Id) {
                let declared = about_info_snapshot();
                // SAFETY: objc en el hilo principal; los NSString/dict son autoreleased.
                unsafe {
                    let shared: MsgId = std::mem::transmute(msg_send());
                    let set_id: MsgVoidId = std::mem::transmute(msg_send());
                    let set_bool: MsgVoidBool = std::mem::transmute(msg_send());
                    let app = shared(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
                    // El panel puede abrir DETRÁS si la app no está activa (patrón dialog).
                    set_bool(app, sel(b"activateIgnoringOtherApps:\0"), 1);
                    match declared {
                        None => {
                            set_id(app, sel(b"orderFrontStandardAboutPanel:\0"), std::ptr::null_mut());
                        }
                        Some((name, version, description, copyright)) => {
                            let plain_id: MsgId = std::mem::transmute(msg_send());
                            let set_kv: MsgVoidIdId = std::mem::transmute(msg_send());
                            let id_id: MsgIdId = std::mem::transmute(msg_send());
                            let alloc: MsgId = std::mem::transmute(msg_send());
                            let dict =
                                plain_id(cls(b"NSMutableDictionary\0"), sel(b"dictionary\0"));
                            if !name.is_empty() {
                                set_kv(dict, sel(b"setObject:forKey:\0"), nsstring(&name), nsstring("ApplicationName"));
                            }
                            if !version.is_empty() {
                                set_kv(dict, sel(b"setObject:forKey:\0"), nsstring(&version), nsstring("ApplicationVersion"));
                            }
                            if !copyright.is_empty() {
                                set_kv(dict, sel(b"setObject:forKey:\0"), nsstring(&copyright), nsstring("Copyright"));
                            }
                            if !description.is_empty() {
                                // La descripción viaja como Credits (NSAttributedString):
                                // es la zona de texto bajo la versión, como en el Finder.
                                let attr = {
                                    let a = alloc(cls(b"NSAttributedString\0"), sel(b"alloc\0"));
                                    id_id(a, sel(b"initWithString:\0"), nsstring(&description))
                                };
                                set_kv(dict, sel(b"setObject:forKey:\0"), attr, nsstring("Credits"));
                            }
                            set_id(app, sel(b"orderFrontStandardAboutPanelWithOptions:\0"), dict);
                        }
                    }
                }
            }
            unsafe {
                let cls_new =
                    objc_allocateClassPair(cls(b"NSObject\0"), c"RayWindowDelegate".as_ptr(), 0);
                // Los imp van SIN tipo en la extern (ver arriba): castear por sitio.
                class_addMethod(
                    cls_new,
                    sel(b"windowWillClose:\0"),
                    window_will_close as extern "C" fn(Id, Sel, Id) as *const c_void,
                    c"v@:@".as_ptr(),
                );
                class_addMethod(
                    cls_new,
                    sel(b"rayMenuAction:\0"),
                    menu_action as extern "C" fn(Id, Sel, Id) as *const c_void,
                    c"v@:@".as_ptr(),
                );
                class_addMethod(
                    cls_new,
                    sel(b"rayAboutAction:\0"),
                    about_action as extern "C" fn(Id, Sel, Id) as *const c_void,
                    c"v@:@".as_ptr(),
                );
                class_addMethod(
                    cls_new,
                    sel(b"userContentController:didReceiveScriptMessage:\0"),
                    script_message as extern "C" fn(Id, Sel, Id, Id) as *const c_void,
                    c"v@:@@".as_ptr(),
                );
                objc_registerClassPair(cls_new);
                cls_new as usize
            }
        }) as Id
    }

    /// tag numérico del NSMenuItem → tag String del programa.
    fn menu_tags() -> &'static std::sync::Mutex<std::collections::HashMap<i64, String>> {
        static TAGS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<i64, String>>> =
            std::sync::OnceLock::new();
        TAGS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
    }

    /// M148: appendea un menú de nivel superior con items custom (en el hilo principal). El
    /// target es un singleton del delegate (autoenablesItems resuelve a favor: target que
    /// responde al action sin validateMenuItem: = item HABILITADO — cero plomería extra).
    pub(super) fn add_menu(title: &str, items: &[(String, String, String)]) -> Result<(), String> {
        let title = title.to_string();
        let items = items.to_vec();
        on_main_sync(move || {
            let main_menu = MAIN_MENU.load(std::sync::atomic::Ordering::SeqCst);
            if main_menu == 0 {
                return Err("ui: the menu bar is not ready".to_string());
            }
            unsafe {
                let alloc: MsgId = std::mem::transmute(msg_send());
                let init: MsgId = std::mem::transmute(msg_send());
                let init_title: MsgIdId = std::mem::transmute(msg_send());
                let item_init: MsgMenuItemInit = std::mem::transmute(msg_send());
                let add: MsgVoidId = std::mem::transmute(msg_send());
                let set_submenu: MsgVoidId = std::mem::transmute(msg_send());
                let set_target: MsgVoidId = std::mem::transmute(msg_send());
                let set_tag: MsgVoidI64 = std::mem::transmute(msg_send());

                // El target singleton (una instancia del delegate), creado aquí en main.
                static TARGET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                let mut target = TARGET.load(std::sync::atomic::Ordering::SeqCst) as Id;
                if target.is_null() {
                    target = init(alloc(delegate_class(), sel(b"alloc\0")), sel(b"init\0"));
                    TARGET.store(target as usize, std::sync::atomic::Ordering::SeqCst);
                }

                let bar_item = item_init(
                    alloc(cls(b"NSMenuItem\0"), sel(b"alloc\0")),
                    sel(b"initWithTitle:action:keyEquivalent:\0"),
                    nsstring(&title),
                    std::ptr::null_mut(),
                    nsstring(""),
                );
                let menu = init_title(
                    alloc(cls(b"NSMenu\0"), sel(b"alloc\0")),
                    sel(b"initWithTitle:\0"),
                    nsstring(&title),
                );
                for (tag, label, shortcut) in &items {
                    let item = item_init(
                        alloc(cls(b"NSMenuItem\0"), sel(b"alloc\0")),
                        sel(b"initWithTitle:action:keyEquivalent:\0"),
                        nsstring(label),
                        sel(b"rayMenuAction:\0"),
                        nsstring(shortcut),
                    );
                    set_target(item, sel(b"setTarget:\0"), target);
                    let n = {
                        let mut tags = menu_tags().lock().unwrap();
                        let n = tags.len() as i64 + 1;
                        tags.insert(n, tag.clone());
                        n
                    };
                    set_tag(item, sel(b"setTag:\0"), n);
                    add(menu, sel(b"addItem:\0"), item);
                }
                set_submenu(bar_item, sel(b"setSubmenu:\0"), menu);
                add(main_menu as Id, sel(b"addItem:\0"), bar_item);
            }
            Ok(())
        })
    }

    /// M151 (raydesk #10): items en el MENÚ DE APLICACIÓN (item 0 de la barra, el que el
    /// sistema pone en negrita), insertados ENCIMA de Hide/Quit + separador; `name` no vacío
    /// re-titula ese menú (bajo `ray run` sale el nombre del proceso — "ray"; ponerle título
    /// al submenu ANTES de que la barra se realice funciona en procesos sin bundle, el truco
    /// de glfw/SDL). Un item con tag "role:about" instala el "About" NATIVO
    /// (orderFrontStandardAboutPanel: por la responder chain — target nil = NSApp lo valida y
    /// habilita; NO emite evento); el resto emite ("menu", 0, tag) como los menús custom.
    pub(super) fn set_app_menu(name: &str, items: &[(String, String, String)]) -> Result<(), String> {
        let name = name.to_string();
        let items = items.to_vec();
        on_main_sync(move || {
            let main_menu = MAIN_MENU.load(std::sync::atomic::Ordering::SeqCst);
            if main_menu == 0 {
                return Err("ui: the menu bar is not ready".to_string());
            }
            unsafe {
                let alloc: MsgId = std::mem::transmute(msg_send());
                let init: MsgId = std::mem::transmute(msg_send());
                let item_init: MsgMenuItemInit = std::mem::transmute(msg_send());
                let item_at: MsgIdI64 = std::mem::transmute(msg_send());
                let submenu_of: MsgId = std::mem::transmute(msg_send());
                let insert_at: MsgVoidIdI64 = std::mem::transmute(msg_send());
                let set_id: MsgVoidId = std::mem::transmute(msg_send());
                let set_tag: MsgVoidI64 = std::mem::transmute(msg_send());
                let class_item: MsgId = std::mem::transmute(msg_send());

                let app_item = item_at(main_menu as Id, sel(b"itemAtIndex:\0"), 0);
                let app_menu = submenu_of(app_item, sel(b"submenu\0"));
                if app_menu.is_null() {
                    return Err("ui: the application menu is not ready".to_string());
                }
                if !name.is_empty() {
                    set_id(app_menu, sel(b"setTitle:\0"), nsstring(&name));
                }
                // El target singleton del delegate (mismo patrón que add_menu).
                static TARGET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                let mut target = TARGET.load(std::sync::atomic::Ordering::SeqCst) as Id;
                if target.is_null() {
                    target = init(alloc(delegate_class(), sel(b"alloc\0")), sel(b"init\0"));
                    TARGET.store(target as usize, std::sync::atomic::Ordering::SeqCst);
                }
                let mut idx: i64 = 0;
                for (tag, label, shortcut) in &items {
                    let item = if tag == "role:about" {
                        let title = if label.is_empty() { "About".to_string() } else { label.clone() };
                        // M155: action propia con target (el singleton del delegate) — al
                        // click decide: panel con opciones (ui.set_about) o el estándar.
                        let item = item_init(
                            alloc(cls(b"NSMenuItem\0"), sel(b"alloc\0")),
                            sel(b"initWithTitle:action:keyEquivalent:\0"),
                            nsstring(&title),
                            sel(b"rayAboutAction:\0"),
                            nsstring(""),
                        );
                        set_id(item, sel(b"setTarget:\0"), target);
                        item
                    } else {
                        let item = item_init(
                            alloc(cls(b"NSMenuItem\0"), sel(b"alloc\0")),
                            sel(b"initWithTitle:action:keyEquivalent:\0"),
                            nsstring(label),
                            sel(b"rayMenuAction:\0"),
                            nsstring(shortcut),
                        );
                        set_id(item, sel(b"setTarget:\0"), target);
                        let n = {
                            let mut tags = menu_tags().lock().unwrap();
                            let n = tags.len() as i64 + 1;
                            tags.insert(n, tag.clone());
                            n
                        };
                        set_tag(item, sel(b"setTag:\0"), n);
                        item
                    };
                    insert_at(app_menu, sel(b"insertItem:atIndex:\0"), item, idx);
                    idx += 1;
                }
                if idx > 0 {
                    let separator = class_item(cls(b"NSMenuItem\0"), sel(b"separatorItem\0"));
                    insert_at(app_menu, sel(b"insertItem:atIndex:\0"), separator, idx);
                }
            }
            Ok(())
        })
    }

    /// M148: diálogo de archivo modal (NSOpenPanel/NSSavePanel), por la espera SIN plazo. La
    /// main queue es SERIAL: mientras el panel está abierto, otras ops de UI esperan (un modal
    /// a la vez — contrato v1; GTK no lo sufre).
    pub(super) fn dialog(kind: &str, arg: &str) -> Result<Option<String>, String> {
        let kind = kind.to_string();
        let arg = arg.to_string();
        on_main_sync_wait(move || {
            const MODAL_OK: i64 = 1;
            unsafe {
                let shared: MsgId = std::mem::transmute(msg_send());
                let plain_id: MsgId = std::mem::transmute(msg_send());
                let set_bool: MsgVoidBool = std::mem::transmute(msg_send());
                let set_id: MsgVoidId = std::mem::transmute(msg_send());
                let run_modal: MsgI64 = std::mem::transmute(msg_send());
                let utf8: MsgCStr = std::mem::transmute(msg_send());

                // El panel puede abrir DETRÁS de otras apps si la nuestra no está activa.
                let app = shared(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
                set_bool(app, sel(b"activateIgnoringOtherApps:\0"), 1);

                let panel = match kind.as_str() {
                    "save_file" => {
                        let p = plain_id(cls(b"NSSavePanel\0"), sel(b"savePanel\0"));
                        if !arg.is_empty() {
                            set_id(p, sel(b"setNameFieldStringValue:\0"), nsstring(&arg));
                        }
                        p
                    }
                    other => {
                        let p = plain_id(cls(b"NSOpenPanel\0"), sel(b"openPanel\0"));
                        if other == "open_folder" {
                            set_bool(p, sel(b"setCanChooseFiles:\0"), 0);
                            set_bool(p, sel(b"setCanChooseDirectories:\0"), 1);
                        }
                        p
                    }
                };
                if run_modal(panel, sel(b"runModal\0")) != MODAL_OK {
                    return Ok(None);
                }
                let url = plain_id(panel, sel(b"URL\0"));
                if url.is_null() {
                    return Ok(None);
                }
                let path = plain_id(url, sel(b"path\0"));
                let c = utf8(path, sel(b"UTF8String\0"));
                if c.is_null() {
                    return Ok(None);
                }
                // Copia DENTRO del bloque (la main queue drena con autorelease pool).
                Ok(Some(std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned()))
            }
        })
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

                // M152: el delegate nace ANTES del webview — el puente IPC lo registra como
                // script message handler en la configuration con la que el webview se crea.
                // (Antes nacía después; el reorden es deliberado.)
                let delegate = alloc(delegate_class(), sel(b"alloc\0"));
                let delegate = {
                    let init: MsgId = std::mem::transmute(msg_send());
                    init(delegate, sel(b"init\0"))
                };

                // M152 — puente IPC JS→raylang: WKWebViewConfiguration con nuestro user
                // content controller. El handler "ray" recibe los postMessage (el delegate
                // implementa userContentController:didReceiveScriptMessage:) y el user
                // script inyecta window.ray.send en el MAIN frame al arrancar el documento.
                // OJO ciclo de vida: el controller retiene FUERTE al delegate — se
                // desregistra en close_window_async antes de los release.
                let init_plain: MsgId = std::mem::transmute(msg_send());
                let cfg = init_plain(
                    alloc(cls(b"WKWebViewConfiguration\0"), sel(b"alloc\0")),
                    sel(b"init\0"),
                );
                let get_ucc: MsgId = std::mem::transmute(msg_send());
                let ucc = get_ucc(cfg, sel(b"userContentController\0"));
                let add_handler: MsgVoidIdId = std::mem::transmute(msg_send());
                add_handler(
                    ucc,
                    sel(b"addScriptMessageHandler:name:\0"),
                    delegate,
                    nsstring("ray"),
                );
                let init_script: MsgInitUserScript = std::mem::transmute(msg_send());
                // injectionTime 0 = WKUserScriptInjectionTimeAtDocumentStart; 1 = solo main frame.
                let script = init_script(
                    alloc(cls(b"WKUserScript\0"), sel(b"alloc\0")),
                    sel(b"initWithSource:injectionTime:forMainFrameOnly:\0"),
                    nsstring(super::RAY_JS_SHIM),
                    0,
                    1,
                );
                set_id(ucc, sel(b"addUserScript:\0"), script);
                plain(script, sel(b"release\0")); // el controller lo retiene

                let init_frame_cfg: MsgInitFrameCfg = std::mem::transmute(msg_send());
                let webview = init_frame_cfg(
                    alloc(cls(b"WKWebView\0"), sel(b"alloc\0")),
                    sel(b"initWithFrame:configuration:\0"),
                    rect,
                    cfg,
                );
                plain(cfg, sel(b"release\0")); // el webview posee su copia
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
                // retiene en el registro y se libera junto a la ventana (el controller, en
                // cambio, sí lo retiene fuerte hasta el desregistro del close).
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
            // M152: el user content controller retiene FUERTE al delegate (asimetría con el
            // delegate de ventana, que es weak) — desregistrar el handler ANTES de los
            // release o el delegate sobrevive con la ventana muerta. El controller se pide
            // VÍA el webview: WKWebView COPIA su configuration en el init (el puntero
            // pre-init sería otro objeto).
            let get_id: MsgId = std::mem::transmute(msg_send());
            let cfg = get_id(webview as Id, sel(b"configuration\0"));
            if !cfg.is_null() {
                let ucc = get_id(cfg, sel(b"userContentController\0"));
                if !ucc.is_null() {
                    set_id(ucc, sel(b"removeScriptMessageHandlerForName:\0"), nsstring("ray"));
                }
            }
            plain(window as Id, sel(b"close\0"));
            // Nuestras retenciones (alloc/init): la ventana, el webview y el delegate (la
            // configuration/controller mueren con el webview: el init la copió y la posee).
            plain(webview as Id, sel(b"release\0"));
            plain(delegate as Id, sel(b"release\0"));
            plain(window as Id, sel(b"release\0"));
        });
    }
}

// ── Linux: GTK3 + WebKitGTK por dlopen (sin headers de build; sin las libs → Err claro) ──────
#[cfg(target_os = "linux")]
mod gtk {
    use super::Win;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    unsafe extern "C" {
        fn dlopen(path: *const std::ffi::c_char, flags: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
    }
    const RTLD_NOW: i32 = 2;
    // GLOBAL: los símbolos de webkit resuelven contra los de gtk/glib ya cargados (una sola
    // instancia de cada biblioteca en el proceso).
    const RTLD_GLOBAL: i32 = 0x100;

    type Widget = *mut c_void;
    type FnInitCheck = unsafe extern "C" fn(*mut i32, *mut c_void) -> i32;
    type FnMain = unsafe extern "C" fn();
    type FnWindowNew = unsafe extern "C" fn(i32) -> Widget;
    type FnSetTitle = unsafe extern "C" fn(Widget, *const std::ffi::c_char);
    type FnSetDefaultSize = unsafe extern "C" fn(Widget, i32, i32);
    type FnContainerAdd = unsafe extern "C" fn(Widget, Widget);
    type FnWidgetOp = unsafe extern "C" fn(Widget);
    type FnIdleAdd = unsafe extern "C" fn(extern "C" fn(*mut c_void) -> i32, *mut c_void) -> u32;
    type FnSignalConnect = unsafe extern "C" fn(
        Widget,
        *const std::ffi::c_char,
        extern "C" fn(Widget, *mut c_void),
        *mut c_void,
        extern "C" fn(*mut c_void, *mut c_void),
        i32,
    ) -> u64;
    // M152 — puente IPC: la señal "script-message-received::ray" lleva handler de TRES args
    // (manager, WebKitJavascriptResult*, user_data) — MISMO símbolo g_signal_connect_data,
    // otro alias (el precedente "dos aliases, jamás uno flexible" de FnEvalJs/FnRunJs).
    type FnSignalConnect3 = unsafe extern "C" fn(
        Widget,
        *const std::ffi::c_char,
        extern "C" fn(Widget, *mut c_void, *mut c_void),
        *mut c_void,
        extern "C" fn(*mut c_void, *mut c_void),
        i32,
    ) -> u64;
    type FnWebViewNew = unsafe extern "C" fn() -> Widget;
    type FnLoadUri = unsafe extern "C" fn(Widget, *const std::ffi::c_char);
    // M152 — user content manager + lectura del payload (JSC). user_script_new:
    // (source, injected_frames, injection_time, allow_list, block_list).
    type FnUcmNew = unsafe extern "C" fn() -> Widget;
    type FnWebViewNewWithUcm = unsafe extern "C" fn(Widget) -> Widget;
    type FnUcmRegister = unsafe extern "C" fn(Widget, *const std::ffi::c_char) -> i32;
    type FnUserScriptNew = unsafe extern "C" fn(
        *const std::ffi::c_char,
        i32,
        i32,
        *const *const std::ffi::c_char,
        *const *const std::ffi::c_char,
    ) -> *mut c_void;
    type FnUcmAddScript = unsafe extern "C" fn(Widget, *mut c_void);
    type FnJsResultGetValue = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
    type FnJscIsString = unsafe extern "C" fn(*mut c_void) -> i32;
    type FnJscToString = unsafe extern "C" fn(*mut c_void) -> *mut std::ffi::c_char;
    // Las DOS generaciones del eval (aridades distintas — dos aliases, jamás uno "flexible"):
    // 2.40+ `evaluate_javascript(view, script, len, world, source_uri, cancellable, cb, data)`;
    // el clásico `run_javascript(view, script, cancellable, cb, data)`. Fire-and-forget: cb nulo.
    type FnEvalJs = unsafe extern "C" fn(
        Widget,
        *const std::ffi::c_char,
        i64,
        *const c_void,
        *const c_void,
        *mut c_void,
        *mut c_void,
        *mut c_void,
    );
    type FnRunJs =
        unsafe extern "C" fn(Widget, *const std::ffi::c_char, *mut c_void, *mut c_void, *mut c_void);
    // M148 (menús + diálogos):
    type FnWidgetNew0 = unsafe extern "C" fn() -> Widget;
    type FnItemNewLabel = unsafe extern "C" fn(*const std::ffi::c_char) -> Widget;
    type FnWidgetPair = unsafe extern "C" fn(Widget, Widget);
    type FnBoxNew = unsafe extern "C" fn(i32, i32) -> Widget;
    type FnBoxPack = unsafe extern "C" fn(Widget, Widget, i32, i32, u32);
    type FnChooserNew = unsafe extern "C" fn(
        *const std::ffi::c_char,
        Widget,
        i32,
        *const std::ffi::c_char,
        *const std::ffi::c_char,
    ) -> Widget;
    type FnDialogRun = unsafe extern "C" fn(Widget) -> i32;
    type FnGetFilename = unsafe extern "C" fn(Widget) -> *mut std::ffi::c_char;
    type FnSetCurrentName = unsafe extern "C" fn(Widget, *const std::ffi::c_char);
    type FnGFree = unsafe extern "C" fn(*mut c_void);

    const GTK_WINDOW_TOPLEVEL: i32 = 0;
    const G_SOURCE_REMOVE: i32 = 0;

    /// Los punteros de función resueltos una vez (dlopen + dlsym al primer uso, en el hilo 1).
    struct Api {
        init_check: FnInitCheck,
        main: FnMain,
        window_new: FnWindowNew,
        set_title: FnSetTitle,
        set_default_size: FnSetDefaultSize,
        container_add: FnContainerAdd,
        show_all: FnWidgetOp,
        destroy: FnWidgetOp,
        idle_add: FnIdleAdd,
        signal_connect: FnSignalConnect,
        webview_new: FnWebViewNew,
        load_uri: FnLoadUri,
        /// 2.40+ (aridad 8) o, si no está, el clásico (aridad 5): exactamente uno queda `Some`.
        evaluate_js: Option<FnEvalJs>,
        run_js: Option<FnRunJs>,
        // M148 — menús (core GTK3, requeridos) + diálogos (chooser NATIVO, 3.20+: opcionales
        // con Err limpio; jamás la variádica gtk_file_chooser_dialog_new).
        menu_bar_new: FnWidgetNew0,
        menu_new: FnWidgetNew0,
        menu_item_new_with_label: FnItemNewLabel,
        menu_item_set_submenu: FnWidgetPair,
        menu_shell_append: FnWidgetPair,
        box_new: FnBoxNew,
        box_pack_start: FnBoxPack,
        g_free: FnGFree,
        g_object_unref: FnGFree,
        chooser_new: Option<FnChooserNew>,
        dialog_run: Option<FnDialogRun>,
        get_filename: Option<FnGetFilename>,
        set_current_name: Option<FnSetCurrentName>,
        // M152 — puente IPC (todos opcionales: si alguno falta, la ventana nace SIN puente —
        // no romper ui.open en distros viejas por una feature nueva; webkit2gtk ≥2.22 los trae).
        signal_connect3: FnSignalConnect3,
        ucm_new: Option<FnUcmNew>,
        webview_new_with_ucm: Option<FnWebViewNewWithUcm>,
        ucm_register: Option<FnUcmRegister>,
        user_script_new: Option<FnUserScriptNew>,
        ucm_add_script: Option<FnUcmAddScript>,
        js_result_get_value: Option<FnJsResultGetValue>,
        jsc_is_string: Option<FnJscIsString>,
        jsc_to_string: Option<FnJscToString>,
    }
    // SAFETY: los punteros de función son inmutables tras la resolución; toda llamada que toca
    // objetos GTK viaja al hilo del loop (idle_add) — aquí solo se COMPARTEN los fn pointers.
    unsafe impl Send for Api {}
    unsafe impl Sync for Api {}

    fn api() -> &'static Result<Api, String> {
        static API: std::sync::OnceLock<Result<Api, String>> = std::sync::OnceLock::new();
        API.get_or_init(load_api)
    }

    /// M152: resuelve un símbolo del puente — por la clausura del handle webkit y, si el
    /// linker no lo expone así, por dlopen explícito de libjavascriptcoregtk (una vez).
    fn bridge_sym(webkit: *mut c_void, name: &std::ffi::CStr) -> *mut c_void {
        // SAFETY: name es un CStr; dlsym/dlopen no retienen los punteros.
        unsafe {
            let p = dlsym(webkit, name.as_ptr());
            if !p.is_null() {
                return p;
            }
            static JSC: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
            let jsc = *JSC.get_or_init(|| {
                let l = dlopen(c"libjavascriptcoregtk-4.1.so.0".as_ptr(), RTLD_NOW | RTLD_GLOBAL);
                let l = if l.is_null() {
                    dlopen(c"libjavascriptcoregtk-4.0.so.18".as_ptr(), RTLD_NOW | RTLD_GLOBAL)
                } else {
                    l
                };
                l as usize
            }) as *mut c_void;
            if jsc.is_null() { std::ptr::null_mut() } else { dlsym(jsc, name.as_ptr()) }
        }
    }

    fn sym(lib: *mut c_void, name: &std::ffi::CStr) -> Result<*mut c_void, String> {
        // SAFETY: name es un CStr (NUL garantizado); dlsym no retiene el puntero.
        let p = unsafe { dlsym(lib, name.as_ptr()) };
        if p.is_null() {
            Err(format!("ui: libgtk without {}", name.to_string_lossy()))
        } else {
            Ok(p)
        }
    }

    fn load_api() -> Result<Api, String> {
        // SAFETY: literales NUL-terminados; dlopen es seguro de llamar.
        let gtk = unsafe { dlopen(c"libgtk-3.so.0".as_ptr(), RTLD_NOW | RTLD_GLOBAL) };
        if gtk.is_null() {
            return Err("ui: libgtk-3.so.0 not found (install GTK 3, e.g. libgtk-3-0)".to_string());
        }
        // El soname de WebKitGTK cambió con la transición de libsoup: 4.1 (Ubuntu 22.04+,
        // Debian 12+, Fedora) y el 4.0 clásico (OJO: su soname es .37). Jamás el 6.0 (GTK4).
        let webkit = unsafe {
            let w = dlopen(c"libwebkit2gtk-4.1.so.0".as_ptr(), RTLD_NOW | RTLD_GLOBAL);
            if w.is_null() {
                dlopen(c"libwebkit2gtk-4.0.so.37".as_ptr(), RTLD_NOW | RTLD_GLOBAL)
            } else {
                w
            }
        };
        if webkit.is_null() {
            return Err(
                "ui: WebKitGTK not found (install libwebkit2gtk-4.1-0 or libwebkit2gtk-4.0-37)"
                    .to_string(),
            );
        }
        // SAFETY de los transmutes: las firmas replican los headers de GTK3/GLib/WebKitGTK
        // (API C estable). `g_idle_add`/`g_signal_connect_data` viven en glib/gobject, que el
        // dlsym de glibc resuelve por la clausura de dependencias del handle de gtk.
        unsafe {
            let evaluate_js = dlsym(webkit, c"webkit_web_view_evaluate_javascript".as_ptr());
            let run_js = dlsym(webkit, c"webkit_web_view_run_javascript".as_ptr());
            if evaluate_js.is_null() && run_js.is_null() {
                return Err("ui: WebKitGTK without a JavaScript entry point".to_string());
            }
            Ok(Api {
                init_check: std::mem::transmute::<*mut c_void, FnInitCheck>(sym(gtk, c"gtk_init_check")?),
                main: std::mem::transmute::<*mut c_void, FnMain>(sym(gtk, c"gtk_main")?),
                window_new: std::mem::transmute::<*mut c_void, FnWindowNew>(sym(gtk, c"gtk_window_new")?),
                set_title: std::mem::transmute::<*mut c_void, FnSetTitle>(sym(gtk, c"gtk_window_set_title")?),
                set_default_size: std::mem::transmute::<*mut c_void, FnSetDefaultSize>(sym(gtk, c"gtk_window_set_default_size")?),
                container_add: std::mem::transmute::<*mut c_void, FnContainerAdd>(sym(gtk, c"gtk_container_add")?),
                show_all: std::mem::transmute::<*mut c_void, FnWidgetOp>(sym(gtk, c"gtk_widget_show_all")?),
                destroy: std::mem::transmute::<*mut c_void, FnWidgetOp>(sym(gtk, c"gtk_widget_destroy")?),
                idle_add: std::mem::transmute::<*mut c_void, FnIdleAdd>(sym(gtk, c"g_idle_add")?),
                signal_connect: std::mem::transmute::<*mut c_void, FnSignalConnect>(sym(gtk, c"g_signal_connect_data")?),
                webview_new: std::mem::transmute::<*mut c_void, FnWebViewNew>(sym(webkit, c"webkit_web_view_new")?),
                load_uri: std::mem::transmute::<*mut c_void, FnLoadUri>(sym(webkit, c"webkit_web_view_load_uri")?),
                evaluate_js: (!evaluate_js.is_null())
                    .then(|| std::mem::transmute::<*mut c_void, FnEvalJs>(evaluate_js)),
                run_js: (evaluate_js.is_null() && !run_js.is_null())
                    .then(|| std::mem::transmute::<*mut c_void, FnRunJs>(run_js)),
                menu_bar_new: std::mem::transmute::<*mut c_void, FnWidgetNew0>(sym(gtk, c"gtk_menu_bar_new")?),
                menu_new: std::mem::transmute::<*mut c_void, FnWidgetNew0>(sym(gtk, c"gtk_menu_new")?),
                menu_item_new_with_label: std::mem::transmute::<*mut c_void, FnItemNewLabel>(sym(gtk, c"gtk_menu_item_new_with_label")?),
                menu_item_set_submenu: std::mem::transmute::<*mut c_void, FnWidgetPair>(sym(gtk, c"gtk_menu_item_set_submenu")?),
                menu_shell_append: std::mem::transmute::<*mut c_void, FnWidgetPair>(sym(gtk, c"gtk_menu_shell_append")?),
                box_new: std::mem::transmute::<*mut c_void, FnBoxNew>(sym(gtk, c"gtk_box_new")?),
                box_pack_start: std::mem::transmute::<*mut c_void, FnBoxPack>(sym(gtk, c"gtk_box_pack_start")?),
                g_free: std::mem::transmute::<*mut c_void, FnGFree>(sym(gtk, c"g_free")?),
                g_object_unref: std::mem::transmute::<*mut c_void, FnGFree>(sym(gtk, c"g_object_unref")?),
                chooser_new: {
                    let p = dlsym(gtk, c"gtk_file_chooser_native_new".as_ptr());
                    (!p.is_null()).then(|| std::mem::transmute::<*mut c_void, FnChooserNew>(p))
                },
                dialog_run: {
                    let p = dlsym(gtk, c"gtk_native_dialog_run".as_ptr());
                    (!p.is_null()).then(|| std::mem::transmute::<*mut c_void, FnDialogRun>(p))
                },
                get_filename: {
                    let p = dlsym(gtk, c"gtk_file_chooser_get_filename".as_ptr());
                    (!p.is_null()).then(|| std::mem::transmute::<*mut c_void, FnGetFilename>(p))
                },
                set_current_name: {
                    let p = dlsym(gtk, c"gtk_file_chooser_set_current_name".as_ptr());
                    (!p.is_null()).then(|| std::mem::transmute::<*mut c_void, FnSetCurrentName>(p))
                },
                signal_connect3: std::mem::transmute::<*mut c_void, FnSignalConnect3>(sym(gtk, c"g_signal_connect_data")?),
                ucm_new: {
                    let p = dlsym(webkit, c"webkit_user_content_manager_new".as_ptr());
                    (!p.is_null()).then(|| std::mem::transmute::<*mut c_void, FnUcmNew>(p))
                },
                webview_new_with_ucm: {
                    let p = dlsym(webkit, c"webkit_web_view_new_with_user_content_manager".as_ptr());
                    (!p.is_null()).then(|| std::mem::transmute::<*mut c_void, FnWebViewNewWithUcm>(p))
                },
                ucm_register: {
                    let p = dlsym(webkit, c"webkit_user_content_manager_register_script_message_handler".as_ptr());
                    (!p.is_null()).then(|| std::mem::transmute::<*mut c_void, FnUcmRegister>(p))
                },
                user_script_new: {
                    let p = dlsym(webkit, c"webkit_user_script_new".as_ptr());
                    (!p.is_null()).then(|| std::mem::transmute::<*mut c_void, FnUserScriptNew>(p))
                },
                ucm_add_script: {
                    let p = dlsym(webkit, c"webkit_user_content_manager_add_script".as_ptr());
                    (!p.is_null()).then(|| std::mem::transmute::<*mut c_void, FnUcmAddScript>(p))
                },
                // M152: los jsc_* viven en libjavascriptcoregtk — normalmente resolubles por
                // la clausura del handle webkit (como g_idle_add por gtk); si no, dlopen
                // explícito de la lib (soname .18 en la serie 4.0).
                js_result_get_value: {
                    let p = bridge_sym(webkit, c"webkit_javascript_result_get_js_value");
                    (!p.is_null()).then(|| std::mem::transmute::<*mut c_void, FnJsResultGetValue>(p))
                },
                jsc_is_string: {
                    let p = bridge_sym(webkit, c"jsc_value_is_string");
                    (!p.is_null()).then(|| std::mem::transmute::<*mut c_void, FnJscIsString>(p))
                },
                jsc_to_string: {
                    let p = bridge_sym(webkit, c"jsc_value_to_string");
                    (!p.is_null()).then(|| std::mem::transmute::<*mut c_void, FnJscToString>(p))
                },
            })
        }
    }

    /// Inicializa GTK en el hilo que llama (el 1). `gtk_init_check` — NO `gtk_init`, que ABORTA
    /// el proceso sin display; aquí un headless (CI, ssh) debe dar `Err`, jamás morir.
    pub(super) fn init_app() -> Result<(), String> {
        let api = api().as_ref().map_err(|e| e.clone())?;
        // SAFETY: init_check acepta (NULL, NULL); corre en el hilo 1 antes de cualquier widget.
        let ok = unsafe { (api.init_check)(std::ptr::null_mut(), std::ptr::null_mut()) };
        if ok == 0 {
            return Err("ui: could not initialize GTK (no DISPLAY/WAYLAND_DISPLAY?)".to_string());
        }
        Ok(())
    }

    /// `gtk_main()` — no retorna.
    pub(super) fn run_app() {
        if let Ok(api) = api().as_ref() {
            // SAFETY: init_app ya corrió en este mismo hilo.
            unsafe { (api.main)() };
        }
    }

    /// Despacha `f` al hilo del loop de GTK (g_idle_add por closure; la fuente se auto-remueve).
    fn on_main(f: impl FnOnce() + Send + 'static) {
        extern "C" fn trampoline(ctx: *mut c_void) -> i32 {
            // SAFETY: `ctx` es el Box de abajo, entregado una sola vez por la fuente idle.
            let f = unsafe { Box::from_raw(ctx as *mut Box<dyn FnOnce() + Send>) };
            f();
            G_SOURCE_REMOVE
        }
        let Ok(api) = api().as_ref() else { return };
        let boxed: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(f));
        // SAFETY: el Box viaja al trampoline; idle_add es thread-safe (glib).
        unsafe { (api.idle_add)(trampoline, Box::into_raw(boxed) as *mut c_void) };
    }

    /// Despacha al hilo del loop y ESPERA el resultado, con plazo (espejo del on_main_sync de mac).
    fn on_main_sync<T: Send + 'static>(
        f: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let slot = Arc::new((std::sync::Mutex::new(None), std::sync::Condvar::new()));
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

    /// Como `on_main_sync` pero SIN plazo — los diálogos modales esperan al usuario. Solo
    /// alcanzable tras `ensure_app` (el loop de gtk jamás sale una vez Ready). A diferencia de
    /// macOS, el loop recursivo del diálogo SIGUE drenando idles: otras ops de UI no se atascan.
    fn on_main_sync_wait<T: Send + 'static>(
        f: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let slot = Arc::new((std::sync::Mutex::new(None), std::sync::Condvar::new()));
        let slot2 = slot.clone();
        on_main(move || {
            *slot2.0.lock().unwrap() = Some(f());
            slot2.1.notify_all();
        });
        let mut got = slot.0.lock().unwrap();
        loop {
            if let Some(r) = got.take() {
                return r;
            }
            got = slot.1.wait(got).unwrap();
        }
    }

    /// M148: los menús declarados hasta ahora — el menubar de GTK es POR VENTANA (a diferencia
    /// del global de macOS): cada `open_window` construye el suyo de estos specs; los menús
    /// aplican a las ventanas abiertas DESPUÉS de `ui.menu()` (documentado). v1 sin
    /// aceleradores de teclado en Linux (GtkAccelGroup diferido): click-only.
    /// Un menú declarado: (título, items (tag, label)).
    type MenuSpec = (String, Vec<(String, String)>);

    fn menu_specs() -> &'static std::sync::Mutex<Vec<MenuSpec>> {
        static SPECS: std::sync::OnceLock<std::sync::Mutex<Vec<MenuSpec>>> =
            std::sync::OnceLock::new();
        SPECS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
    }

    pub(super) fn add_menu(title: &str, items: &[(String, String, String)]) -> Result<(), String> {
        menu_specs().lock().unwrap().push((
            title.to_string(),
            items.iter().map(|(tag, label, _shortcut)| (tag.clone(), label.clone())).collect(),
        ));
        Ok(())
    }

    /// El contexto del handler `activate` de un item: su tag (liberado por el GClosureNotify).
    struct MenuCtx {
        tag: String,
    }

    extern "C" fn on_menu_activate(_w: Widget, data: *mut c_void) {
        // SAFETY: `data` es el MenuCtx de build_menubar; vive hasta el GClosureNotify.
        let ctx = unsafe { &*(data as *const MenuCtx) };
        super::push_event("menu", 0, &ctx.tag);
    }

    extern "C" fn drop_menu_ctx(data: *mut c_void, _closure: *mut c_void) {
        // SAFETY: reclamamos el Box exactamente una vez (GTK invoca el notify al destruir).
        drop(unsafe { Box::from_raw(data as *mut MenuCtx) });
    }

    // Construye el menubar de los specs vigentes (en el hilo del loop). None si no hay menús.
    unsafe fn build_menubar(api: &Api) -> Option<Widget> {
        let specs = menu_specs().lock().unwrap().clone();
        if specs.is_empty() {
            return None;
        }
        unsafe {
            let bar = (api.menu_bar_new)();
            for (title, items) in &specs {
                let title_c = std::ffi::CString::new(title.replace('\0', "")).unwrap();
                let top = (api.menu_item_new_with_label)(title_c.as_ptr());
                let menu = (api.menu_new)();
                for (tag, label) in items {
                    let label_c = std::ffi::CString::new(label.replace('\0', "")).unwrap();
                    let item = (api.menu_item_new_with_label)(label_c.as_ptr());
                    let ctx = Box::into_raw(Box::new(MenuCtx { tag: tag.clone() }));
                    (api.signal_connect)(
                        item,
                        c"activate".as_ptr(),
                        on_menu_activate,
                        ctx as *mut c_void,
                        drop_menu_ctx,
                        0,
                    );
                    (api.menu_shell_append)(menu, item);
                }
                (api.menu_item_set_submenu)(top, menu);
                (api.menu_shell_append)(bar, top);
            }
            Some(bar)
        }
    }

    /// M148: diálogo de archivo con el chooser NATIVO (3.20+; en libs más viejas, Err limpio).
    pub(super) fn dialog(kind: &str, arg: &str) -> Result<Option<String>, String> {
        let kind = kind.to_string();
        let arg = arg.to_string();
        on_main_sync_wait(move || {
            const ACTION_OPEN: i32 = 0;
            const ACTION_SAVE: i32 = 1;
            const ACTION_SELECT_FOLDER: i32 = 2;
            const RESPONSE_ACCEPT: i32 = -3; // NEGATIVO (GTK_RESPONSE_ACCEPT)
            let api = api().as_ref().map_err(|e| e.clone())?;
            let (Some(chooser_new), Some(dialog_run), Some(get_filename)) =
                (api.chooser_new, api.dialog_run, api.get_filename)
            else {
                return Err("ui: file dialogs need GTK >= 3.20 (gtk_file_chooser_native_new)"
                    .to_string());
            };
            let (action, title, accept) = match kind.as_str() {
                "open_folder" => (ACTION_SELECT_FOLDER, c"Select Folder", c"_Select"),
                "save_file" => (ACTION_SAVE, c"Save File", c"_Save"),
                _ => (ACTION_OPEN, c"Open File", c"_Open"),
            };
            // SAFETY: punteros válidos del loop; el chooser nativo pasa como GtkFileChooser*.
            unsafe {
                let chooser = chooser_new(
                    title.as_ptr(),
                    std::ptr::null_mut(),
                    action,
                    accept.as_ptr(),
                    c"_Cancel".as_ptr(),
                );
                if chooser.is_null() {
                    return Err("ui: could not create the file dialog".to_string());
                }
                if action == ACTION_SAVE
                    && !arg.is_empty()
                    && let Some(set_name) = api.set_current_name
                {
                    let c = std::ffi::CString::new(arg.replace('\0', "")).unwrap();
                    set_name(chooser, c.as_ptr());
                }
                let resp = dialog_run(chooser);
                let out = if resp == RESPONSE_ACCEPT {
                    let raw = get_filename(chooser);
                    if raw.is_null() {
                        None
                    } else {
                        // g_malloc'd: copiar y g_free.
                        let path = std::ffi::CStr::from_ptr(raw).to_string_lossy().into_owned();
                        (api.g_free)(raw as *mut c_void);
                        Some(path)
                    }
                } else {
                    None
                };
                (api.g_object_unref)(chooser);
                Ok(out)
            }
        })
    }

    /// El contexto del handler de `destroy`: nuestro id + el flag de vida. Se libera con el
    /// GClosureNotify que g_signal_connect_data invoca al destruirse el objeto (sin fugas).
    struct DestroyCtx {
        id: i64,
        alive: Arc<AtomicBool>,
    }

    extern "C" fn on_destroy(_w: Widget, data: *mut c_void) {
        // SAFETY: `data` es el DestroyCtx de open_window; vive hasta el GClosureNotify.
        let ctx = unsafe { &*(data as *const DestroyCtx) };
        ctx.alive.store(false, Ordering::SeqCst);
        super::mark_closed(ctx.id);
    }

    extern "C" fn drop_destroy_ctx(data: *mut c_void, _closure: *mut c_void) {
        // SAFETY: reclamamos el Box exactamente una vez (GTK invoca el notify al destruir).
        drop(unsafe { Box::from_raw(data as *mut DestroyCtx) });
    }

    /// M152 — el ctx del puente IPC: el id viaja aquí (sin scan, a diferencia de mac) y
    /// `alive` se re-chequea como en toda closure del hilo gtk.
    struct MsgCtx {
        id: i64,
        alive: Arc<AtomicBool>,
    }

    extern "C" fn on_script_message(_mgr: Widget, jsres: *mut c_void, data: *mut c_void) {
        // SAFETY: `data` es el MsgCtx de open_window; vive hasta el GClosureNotify.
        let ctx = unsafe { &*(data as *const MsgCtx) };
        if !ctx.alive.load(Ordering::SeqCst) {
            return;
        }
        let Ok(api) = api().as_ref() else { return };
        let (Some(get_value), Some(is_string), Some(to_string)) =
            (api.js_result_get_value, api.jsc_is_string, api.jsc_to_string)
        else {
            return; // sin JSC el puente no se registró: inalcanzable, pero jamás panic
        };
        // SAFETY: el loop entrega un WebKitJavascriptResult válido; el string de
        // jsc_value_to_string es g_malloc'd → copiar y g_free (patrón get_filename).
        let owned = unsafe {
            let value = get_value(jsres);
            // Solo strings v1 — SIN la coerción de jsc_value_to_string (divergiría de mac,
            // que descarta con isKindOfClass:NSString).
            if value.is_null() || is_string(value) == 0 {
                return;
            }
            let c = to_string(value);
            if c.is_null() {
                return;
            }
            let s = std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned();
            (api.g_free)(c as *mut c_void);
            s
        };
        super::push_event("message", ctx.id, &owned);
    }

    extern "C" fn drop_msg_ctx(data: *mut c_void, _closure: *mut c_void) {
        // SAFETY: reclamamos el Box exactamente una vez (GTK invoca el notify al destruir).
        drop(unsafe { Box::from_raw(data as *mut MsgCtx) });
    }

    /// Crea la ventana + webview EN EL HILO DEL LOOP. GTK posee los widgets (container_add
    /// sinkea el floating ref del webview; la toplevel es de GTK hasta gtk_widget_destroy):
    /// NO tomamos refs — el flag `alive` protege todo acceso posterior.
    pub(super) fn open_window(
        id: i64,
        title: &str,
        url: &str,
        width: i64,
        height: i64,
    ) -> Result<Win, String> {
        let title = std::ffi::CString::new(title.replace('\0', "")).unwrap();
        let url = std::ffi::CString::new(url.replace('\0', "")).unwrap();
        let alive = Arc::new(AtomicBool::new(true));
        let alive2 = alive.clone();
        let (window, webview) = on_main_sync(move || {
            let api = api().as_ref().map_err(|e| e.clone())?;
            unsafe {
                let window = (api.window_new)(GTK_WINDOW_TOPLEVEL);
                if window.is_null() {
                    return Err("ui: could not create the window".to_string());
                }
                (api.set_title)(window, title.as_ptr());
                (api.set_default_size)(window, width as i32, height as i32);
                // M152 — puente IPC: webview con user content manager (handler "ray" + el
                // shim window.ray.send inyectado en el MAIN frame al arrancar el documento).
                // Con CUALQUIER símbolo del puente ausente (webkit2gtk < 2.22): webview
                // clásico SIN puente — una feature nueva jamás rompe ui.open en distros
                // viejas (los mensajes simplemente no llegan; documentado).
                let webview = if let (
                    Some(ucm_new),
                    Some(webview_with_ucm),
                    Some(register),
                    Some(script_new),
                    Some(add_script),
                    Some(_),
                    Some(_),
                    Some(_),
                ) = (
                    api.ucm_new,
                    api.webview_new_with_ucm,
                    api.ucm_register,
                    api.user_script_new,
                    api.ucm_add_script,
                    api.js_result_get_value,
                    api.jsc_is_string,
                    api.jsc_to_string,
                ) {
                    let ucm = ucm_new();
                    if ucm.is_null() {
                        return Err("ui: could not create the content manager".to_string());
                    }
                    // WEBKIT_USER_CONTENT_INJECT_TOP_FRAME = 1; INJECTION_TIME_START = 0.
                    let shim = std::ffi::CString::new(super::RAY_JS_SHIM).unwrap();
                    let script = script_new(shim.as_ptr(), 1, 0, std::ptr::null(), std::ptr::null());
                    if !script.is_null() {
                        add_script(ucm, script);
                    }
                    let _ = register(ucm, c"ray".as_ptr());
                    let ctx = Box::into_raw(Box::new(MsgCtx { id, alive: alive2.clone() }));
                    (api.signal_connect3)(
                        ucm,
                        c"script-message-received::ray".as_ptr(),
                        on_script_message,
                        ctx as *mut c_void,
                        drop_msg_ctx,
                        0,
                    );
                    webview_with_ucm(ucm)
                } else {
                    (api.webview_new)()
                };
                if webview.is_null() {
                    return Err("ui: could not create the webview".to_string());
                }
                // M148: el hijo de la ventana es un GtkBox vertical — menubar (si hay menús
                // declarados) arriba, webview expandido debajo. GTK posee todo el árbol.
                const ORIENTATION_VERTICAL: i32 = 1;
                let content = (api.box_new)(ORIENTATION_VERTICAL, 0);
                if let Some(bar) = build_menubar(api) {
                    (api.box_pack_start)(content, bar, 0, 0, 0);
                }
                (api.box_pack_start)(content, webview, 1, 1, 0);
                (api.container_add)(window, content);
                (api.load_uri)(webview, url.as_ptr());
                let ctx = Box::into_raw(Box::new(DestroyCtx { id, alive: alive2 }));
                (api.signal_connect)(
                    window,
                    c"destroy".as_ptr(),
                    on_destroy,
                    ctx as *mut c_void,
                    drop_destroy_ctx,
                    0,
                );
                (api.show_all)(window);
                Ok((window as usize, webview as usize))
            }
        })?;
        Ok(Win::Gtk { window, webview, alive })
    }

    /// JS a la página, fire-and-forget (callback nulo), en el hilo del loop. `alive` se
    /// re-chequea DENTRO de la closure: el WM puede destruir la ventana antes de que corra.
    pub(super) fn eval_js(webview: usize, alive: Arc<AtomicBool>, js: &str) {
        let js = std::ffi::CString::new(js.replace('\0', "")).unwrap();
        on_main(move || {
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            let Ok(api) = api().as_ref() else { return };
            // SAFETY: webview vivo (alive, y estamos en el hilo del loop); ambas variantes
            // COPIAN el script antes de volver — el CString temporal alcanza.
            unsafe {
                if let Some(eval) = api.evaluate_js {
                    eval(
                        webview as Widget,
                        js.as_ptr(),
                        -1,
                        std::ptr::null(),
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    );
                } else if let Some(run) = api.run_js {
                    run(
                        webview as Widget,
                        js.as_ptr(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    );
                }
            }
        });
    }

    /// Destruye la ventana en el hilo del loop, asíncrono (llamable desde un Drop). El handler
    /// de `destroy` apaga `alive`; si el WM ya la destruyó, la closure no toca nada.
    pub(super) fn close_window_async(window: usize, alive: Arc<AtomicBool>) {
        on_main(move || {
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            let Ok(api) = api().as_ref() else { return };
            // SAFETY: ventana viva y estamos en el hilo del loop; destroy dispara el handler.
            unsafe { (api.destroy)(window as Widget) };
        });
    }
}

// ── §80b: el puente al SHELL móvil (iOS; `ui-shell` lo compila también en host para el test
// del driver C). El shell (la app UIKit generada por `ray bundle --ios`) registra sus handlers
// ANTES de llamar a ray_start; std/ui les entrega el trabajo — el shell posee el webview y el
// hilo principal, este módulo no despacha nada. ──
#[cfg(any(target_os = "ios", target_os = "android", feature = "ui-shell"))]
mod shell {
    use std::ffi::{c_char, CString};
    use std::sync::OnceLock;

    type OpenHandler = extern "C" fn(*const c_char, *const c_char);
    type EvalHandler = extern "C" fn(*const c_char);

    static HANDLERS: OnceLock<(OpenHandler, EvalHandler)> = OnceLock::new();

    /// El shell registra sus handlers (UNA vez, antes de `ray_start`). Contrato de strings en
    /// ambos handlers: NUL-terminated, VÁLIDOS SOLO DURANTE LA LLAMADA — el shell debe copiar
    /// antes de volver (y despachar a su hilo principal antes de tocar el webview: WebKit lo
    /// exige; la llamada llega desde el hilo del programa raylang).
    #[unsafe(no_mangle)]
    pub extern "C" fn ray_ui_set_handlers(open: OpenHandler, eval: EvalHandler) {
        let _ = HANDLERS.set((open, eval));
    }

    /// El shell empuja un evento a la cola de std/ui (ciclo de vida, botones del sistema…):
    /// llega al programa por `ui.next_event()` como (kind, window, tag). Strings como arriba.
    #[unsafe(no_mangle)]
    pub extern "C" fn ray_ui_push_event(kind: *const c_char, window: i64, tag: *const c_char) {
        if kind.is_null() {
            return;
        }
        // SAFETY: el contrato del export — NUL-terminated, vivos durante la llamada; se copia.
        let kind = unsafe { std::ffi::CStr::from_ptr(kind) }.to_string_lossy().into_owned();
        let tag = if tag.is_null() {
            String::new()
        } else {
            // SAFETY: ídem.
            unsafe { std::ffi::CStr::from_ptr(tag) }.to_string_lossy().into_owned()
        };
        super::push_event(&kind, window, &tag);
    }

    pub(super) fn active() -> bool {
        HANDLERS.get().is_some()
    }

    pub(super) fn open(title: &str, url: &str) {
        if let Some((open, _)) = HANDLERS.get() {
            let t = CString::new(title.replace('\0', "")).unwrap();
            let u = CString::new(url.replace('\0', "")).unwrap();
            open(t.as_ptr(), u.as_ptr());
        }
    }

    pub(super) fn eval(js: &str) {
        if let Some((_, eval)) = HANDLERS.get() {
            let j = CString::new(js.replace('\0', "")).unwrap();
            eval(j.as_ptr());
        }
    }
}

// ── M156: el puente JNI del shell ANDROID. La app Gradle generada (`ray bundle --android`)
// carga el programa como cdylib; el crate EMITIDO define los símbolos con nombre JNI
// (JNI_OnLoad / Java_org_raylang_shell_RayBridge_*) y delega aquí. Vtable de JNIEnv/JavaVM
// A MANO (precedente objc_msgSend: puntero sin tipo + cast por sitio a la firma exacta);
// los ÍNDICES están transcritos del jni.h del NDK r27 (ABI congelada desde JNI 1.6). ──
#[cfg(target_os = "android")]
mod android {
    use std::ffi::{c_char, c_void};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// `JNIEnv*` de C: puntero a puntero a la tabla de funciones.
    pub type JniEnv = *mut *const c_void;
    /// `JavaVM*` de C: misma forma.
    pub type JavaVm = *mut *const c_void;
    type JObject = *mut c_void;

    const JNI_VERSION_1_6: i32 = 0x0001_0006;
    const JNI_OK: i32 = 0;

    // Slots del vtable (jni.h del NDK r27; comentario = declaración C).
    const ENV_FIND_CLASS: usize = 6; // jclass FindClass(JNIEnv*, const char*)
    const ENV_EXCEPTION_CLEAR: usize = 17; // void ExceptionClear(JNIEnv*)
    const ENV_NEW_GLOBAL_REF: usize = 21; // jobject NewGlobalRef(JNIEnv*, jobject)
    const ENV_DELETE_LOCAL_REF: usize = 23; // void DeleteLocalRef(JNIEnv*, jobject)
    const ENV_GET_STATIC_METHOD_ID: usize = 113; // jmethodID GetStaticMethodID(JNIEnv*, jclass, name, sig)
    const ENV_CALL_STATIC_VOID_A: usize = 143; // void CallStaticVoidMethodA(JNIEnv*, jclass, jmethodID, const jvalue*)
    const ENV_NEW_STRING_UTF: usize = 167; // jstring NewStringUTF(JNIEnv*, const char*)
    const ENV_GET_STRING_UTF_CHARS: usize = 169; // const char* GetStringUTFChars(JNIEnv*, jstring, jboolean*)
    const ENV_RELEASE_STRING_UTF_CHARS: usize = 170; // void ReleaseStringUTFChars(JNIEnv*, jstring, const char*)
    const ENV_EXCEPTION_CHECK: usize = 228; // jboolean ExceptionCheck(JNIEnv*)
    const VM_GET_ENV: usize = 6; // jint GetEnv(JavaVM*, void**, jint)
    const VM_ATTACH_DAEMON: usize = 7; // jint AttachCurrentThreadAsDaemon(JavaVM*, JNIEnv**, void*)

    /// El puntero de función del slot `i` del vtable de `env` (se castea POR SITIO).
    unsafe fn env_slot(env: JniEnv, i: usize) -> *const c_void {
        // SAFETY: env es un JNIEnv* válido entregado por ART; el vtable es un array de fns.
        unsafe { *((*env) as *const *const c_void).add(i) }
    }
    unsafe fn vm_slot(vm: JavaVm, i: usize) -> *const c_void {
        // SAFETY: como env_slot.
        unsafe { *((*vm) as *const *const c_void).add(i) }
    }

    // El estado del puente: el VM del proceso, la clase RayBridge (GlobalRef) y los dos
    // methodIDs, cacheados en `init` con el env del hilo Java — el classloader correcto
    // (FindClass desde un hilo nativo attachado vería el classloader del sistema, el pitfall
    // JNI clásico; por eso aquí JAMÁS se llama FindClass fuera de init).
    static VM: AtomicUsize = AtomicUsize::new(0);
    static BRIDGE: AtomicUsize = AtomicUsize::new(0);
    static MID_OPEN: AtomicUsize = AtomicUsize::new(0);
    static MID_EVAL: AtomicUsize = AtomicUsize::new(0);

    /// JNI_OnLoad del cdylib emitido delega aquí: retiene el JavaVM y arma el relay
    /// stdout/stderr→logcat (en una app ambos van a /dev/null — sin esto, debugging a ciegas).
    pub fn on_load(vm: JavaVm) -> i32 {
        VM.store(vm as usize, Ordering::SeqCst);
        arm_logcat_relay();
        JNI_VERSION_1_6
    }

    /// `RayBridge.start()` delega aquí ANTES de ray_start: cachea la clase (GlobalRef) y los
    /// methodIDs con el env del hilo Java, y registra los handlers del shell.
    pub fn init(env: JniEnv, class: JObject) {
        // SAFETY: env/class válidos durante la llamada JNI; los casts replican jni.h.
        unsafe {
            let new_global: unsafe extern "C" fn(JniEnv, JObject) -> JObject =
                std::mem::transmute(env_slot(env, ENV_NEW_GLOBAL_REF));
            let get_static: unsafe extern "C" fn(JniEnv, JObject, *const c_char, *const c_char) -> *mut c_void =
                std::mem::transmute(env_slot(env, ENV_GET_STATIC_METHOD_ID));
            let bridge = new_global(env, class);
            BRIDGE.store(bridge as usize, Ordering::SeqCst);
            let open = get_static(
                env,
                bridge,
                c"onOpen".as_ptr(),
                c"(Ljava/lang/String;Ljava/lang/String;)V".as_ptr(),
            );
            let eval = get_static(env, bridge, c"onEval".as_ptr(), c"(Ljava/lang/String;)V".as_ptr());
            MID_OPEN.store(open as usize, Ordering::SeqCst);
            MID_EVAL.store(eval as usize, Ordering::SeqCst);
        }
        super::shell::ray_ui_set_handlers(open_handler, eval_handler);
    }

    /// `RayBridge.pushEvent(kind, window, tag)` delega aquí (jstrings → push_event).
    pub fn push_event(env: JniEnv, kind: JObject, window: i64, tag: JObject) {
        let kind = jstring_to_string(env, kind);
        let tag = jstring_to_string(env, tag);
        super::push_event(&kind, window, &tag);
    }

    // Los handlers del shell: llegan DESDE el hilo del programa raylang → attach como daemon
    // (no bloquea la salida del proceso) y llamada estática a RayBridge, que ya postea al
    // main thread de Android por su Handler.
    extern "C" fn open_handler(title: *const c_char, url: *const c_char) {
        // Copiar YA (contrato del shell: válidos solo durante la llamada).
        let title = cstr_owned(title);
        let url = cstr_owned(url);
        call_bridge(MID_OPEN.load(Ordering::SeqCst), &[&title, &url]);
    }

    extern "C" fn eval_handler(js: *const c_char) {
        let js = cstr_owned(js);
        call_bridge(MID_EVAL.load(Ordering::SeqCst), &[&js]);
    }

    fn cstr_owned(p: *const c_char) -> String {
        if p.is_null() {
            return String::new();
        }
        // SAFETY: NUL-terminated según el contrato del shell.
        unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }

    /// El env del hilo ACTUAL: GetEnv y, si el hilo no está attachado, AttachAsDaemon.
    fn current_env() -> Option<JniEnv> {
        let vm = VM.load(Ordering::SeqCst) as JavaVm;
        if vm.is_null() {
            return None;
        }
        let mut env: JniEnv = std::ptr::null_mut();
        // SAFETY: vtable de JavaVM según jni.h.
        unsafe {
            let get_env: unsafe extern "C" fn(JavaVm, *mut JniEnv, i32) -> i32 =
                std::mem::transmute(vm_slot(vm, VM_GET_ENV));
            if get_env(vm, &mut env, JNI_VERSION_1_6) == JNI_OK {
                return Some(env);
            }
            let attach: unsafe extern "C" fn(JavaVm, *mut JniEnv, *mut c_void) -> i32 =
                std::mem::transmute(vm_slot(vm, VM_ATTACH_DAEMON));
            if attach(vm, &mut env, std::ptr::null_mut()) == JNI_OK { Some(env) } else { None }
        }
    }

    /// Llama un método estático void de RayBridge con args String (MUTF-8, ver abajo).
    fn call_bridge(mid: usize, args: &[&str]) {
        let (Some(env), bridge) = (current_env(), BRIDGE.load(Ordering::SeqCst)) else { return };
        if mid == 0 || bridge == 0 {
            return;
        }
        // SAFETY: vtable según jni.h; los jstrings locales se liberan tras la llamada.
        unsafe {
            let new_string: unsafe extern "C" fn(JniEnv, *const c_char) -> JObject =
                std::mem::transmute(env_slot(env, ENV_NEW_STRING_UTF));
            let call: unsafe extern "C" fn(JniEnv, JObject, *mut c_void, *const u64) =
                std::mem::transmute(env_slot(env, ENV_CALL_STATIC_VOID_A));
            let check: unsafe extern "C" fn(JniEnv) -> u8 =
                std::mem::transmute(env_slot(env, ENV_EXCEPTION_CHECK));
            let clear: unsafe extern "C" fn(JniEnv) =
                std::mem::transmute(env_slot(env, ENV_EXCEPTION_CLEAR));
            let drop_ref: unsafe extern "C" fn(JniEnv, JObject) =
                std::mem::transmute(env_slot(env, ENV_DELETE_LOCAL_REF));
            let mut jvals: Vec<u64> = Vec::with_capacity(args.len());
            let mut locals: Vec<JObject> = Vec::with_capacity(args.len());
            for a in args {
                let bytes = super::utf8_to_mutf8(a);
                let js = new_string(env, bytes.as_ptr() as *const c_char);
                locals.push(js);
                jvals.push(js as u64); // jvalue = unión de 8 bytes; el miembro objeto es el puntero
            }
            call(env, bridge as JObject, mid as *mut c_void, jvals.as_ptr());
            if check(env) != 0 {
                clear(env); // una excepción Java pendiente NO puede cruzar de vuelta a Rust
            }
            for l in locals {
                drop_ref(env, l);
            }
        }
    }

    fn jstring_to_string(env: JniEnv, s: JObject) -> String {
        if s.is_null() {
            return String::new();
        }
        // SAFETY: vtable según jni.h; el buffer se copia antes del release.
        unsafe {
            let get: unsafe extern "C" fn(JniEnv, JObject, *mut u8) -> *const c_char =
                std::mem::transmute(env_slot(env, ENV_GET_STRING_UTF_CHARS));
            let release: unsafe extern "C" fn(JniEnv, JObject, *const c_char) =
                std::mem::transmute(env_slot(env, ENV_RELEASE_STRING_UTF_CHARS));
            let p = get(env, s, std::ptr::null_mut());
            if p.is_null() {
                return String::new();
            }
            let out = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
            release(env, s, p);
            out
        }
    }

    /// stdout/stderr → logcat (tag "ray"): pipe + dup2 + hilo lector por líneas. En una app
    /// Android ambos flujos van a /dev/null — el "listening on port" del programa se vería
    /// solo aquí. liblog está siempre presente en el proceso de una app.
    fn arm_logcat_relay() {
        #[link(name = "log")]
        unsafe extern "C" {
            fn __android_log_write(prio: i32, tag: *const c_char, text: *const c_char) -> i32;
        }
        unsafe extern "C" {
            fn pipe(fds: *mut i32) -> i32;
            fn dup2(old: i32, new: i32) -> i32;
            fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
        }
        const ANDROID_LOG_INFO: i32 = 4;
        let mut fds = [0i32; 2];
        // SAFETY: syscalls sobre fds propios; el hilo lector posee el extremo de lectura.
        unsafe {
            if pipe(fds.as_mut_ptr()) != 0 {
                return;
            }
            dup2(fds[1], 1);
            dup2(fds[1], 2);
        }
        let rd = fds[0];
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let mut line: Vec<u8> = Vec::new();
            loop {
                // SAFETY: read bloqueante sobre nuestro fd.
                let n = unsafe { read(rd, buf.as_mut_ptr(), buf.len()) };
                if n <= 0 {
                    break;
                }
                for &b in &buf[..n as usize] {
                    if b == b'\n' {
                        line.push(0);
                        // SAFETY: line es NUL-terminated; el tag es un literal.
                        unsafe {
                            __android_log_write(ANDROID_LOG_INFO, c"ray".as_ptr(), line.as_ptr() as *const c_char);
                        }
                        line.clear();
                    } else {
                        line.push(b);
                    }
                }
            }
        });
    }
}

// M156: la cara pública del puente Android (la llaman los símbolos JNI del crate EMITIDO —
// JNI_OnLoad / Java_org_raylang_shell_RayBridge_*; punteros opacos para no exportar tipos).
#[cfg(target_os = "android")]
pub fn android_on_load(vm: *mut std::ffi::c_void) -> i32 {
    android::on_load(vm as android::JavaVm)
}
#[cfg(target_os = "android")]
pub fn android_init(env: *mut std::ffi::c_void, class: *mut std::ffi::c_void) {
    android::init(env as android::JniEnv, class);
}
#[cfg(target_os = "android")]
pub fn android_push_event(env: *mut std::ffi::c_void, kind: *mut std::ffi::c_void, window: i64, tag: *mut std::ffi::c_void) {
    android::push_event(env as android::JniEnv, kind, window, tag);
}

/// M156 (C1): UTF-8 → Modified UTF-8 de la JVM, NUL-terminado — `NewStringUTF` exige MUTF-8
/// (un emoji en UTF-8 real aborta bajo CheckJNI): NUL → C0 80, y los suplementarios
/// (U+10000+) van como par de surrogates CESU-8 (dos secuencias de 3 octetos).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn utf8_to_mutf8(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 1);
    for ch in s.chars() {
        let cp = ch as u32;
        if cp == 0 {
            out.extend_from_slice(&[0xC0, 0x80]);
        } else if cp < 0x80 {
            out.push(cp as u8);
        } else if cp < 0x800 {
            out.push(0xC0 | (cp >> 6) as u8);
            out.push(0x80 | (cp & 0x3F) as u8);
        } else if cp < 0x10000 {
            out.push(0xE0 | (cp >> 12) as u8);
            out.push(0x80 | ((cp >> 6) & 0x3F) as u8);
            out.push(0x80 | (cp & 0x3F) as u8);
        } else {
            let v = cp - 0x10000;
            let hi = 0xD800 + (v >> 10);
            let lo = 0xDC00 + (v & 0x3FF);
            for surr in [hi, lo] {
                out.push(0xE0 | (surr >> 12) as u8);
                out.push(0x80 | ((surr >> 6) & 0x3F) as u8);
                out.push(0x80 | (surr & 0x3F) as u8);
            }
        }
    }
    out.push(0);
    out
}

#[cfg(test)]
mod mutf8_tests {
    use super::utf8_to_mutf8;

    #[test]
    fn mutf8_covers_ascii_nul_bmp_and_supplementary() {
        assert_eq!(utf8_to_mutf8("hi"), vec![b'h', b'i', 0]);
        // NUL interior → C0 80 (jamás un 0 crudo antes del terminador).
        assert_eq!(utf8_to_mutf8("a\0b"), vec![b'a', 0xC0, 0x80, b'b', 0]);
        // BMP (é = U+00E9) igual que UTF-8.
        assert_eq!(utf8_to_mutf8("é"), vec![0xC3, 0xA9, 0]);
        // Suplementario (U+1F600 😀) → par de surrogates CESU-8 (6 octetos), NO UTF-8 de 4.
        let grin = utf8_to_mutf8("😀");
        assert_eq!(grin.len(), 7);
        assert_eq!(&grin[..3], &[0xED, 0xA0, 0xBD]); // surrogate alto U+D83D
        assert_eq!(&grin[3..6], &[0xED, 0xB8, 0x80]); // surrogate bajo U+DE00
        assert_eq!(grin[6], 0);
    }
}
