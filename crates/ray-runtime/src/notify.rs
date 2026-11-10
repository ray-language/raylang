//! Notificaciones del sistema: locales y programadas (M442, IDEAS §121).
//!
//! `std/notify` habla con este módulo por el túnel `__ui_window(0, "notify_<op>", arg)` (el mismo
//! camino que `std/media`): una notificación viaja como campos separados por `\x1f` (ver
//! [`parse_note`]), las acciones por `\x1e`/`\x1d` y el disparador como `now`, `after:<s>:<0|1>`,
//! `at:<unix>` o `daily:<h>:<m>`. Lo que vuelve es un string (`"granted"`, el id, la lista de
//! pendientes separada por `\n`) o un `Err` con el motivo.
//!
//! Backends: `UNUserNotificationCenter` en macOS (dentro de un `.app`) e iOS — el MISMO código Rust
//! vía `objc_msgSend`, sin Objective-C intermedio (patrón de `media.rs`; el framework se carga con
//! `dlopen` para no exigirlo en el enlace); en macOS sin bundle y en Linux `show` cae al `notify`
//! clásico de `std/ui` (M371) y el resto no existe (`Err`); Android cruza al shell Java por la
//! petición genérica `ui::shell_request` (`RayNotify.java` hace todo, también sin el proceso
//! raylang vivo: las programadas las dispara `AlarmManager` + un `BroadcastReceiver`). Windows:
//! `Err` (como `ui.notify`). Headless: `Ok` + traza (`RAY_UI_TRACE=1`), una lista de pendientes en
//! memoria, `RAY_UI_NOTIFY_PERMISSION=denied` simula la negativa y `RAY_UI_NOTIFY_CLICK=1` (o `=<id
//! de acción>`) «pulsa» cada notificación al mostrarla (tests).
//!
//! Una pulsación llega como evento `("notification", 0, tag)` de `std/ui`: `tag` es el id a secas
//! cuando no hay acción, texto ni datos (compatible con M371) y `id\taction\ttext\tdata` si los hay
//! ([`tap_tag`]); `notify.event_tap` lo deshace.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

/// Separador de campos del protocolo (`\x1f`, *unit separator*).
pub const SEP: char = '\u{1f}';
/// Separador entre acciones (`\x1e`) y entre el id y el título de una acción (`\x1d`).
pub const ACTION_SEP: char = '\u{1e}';
pub const ACTION_FIELD: char = '\u{1d}';

/// Lo que el programa quiere mostrar.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub body: String,
    pub subtitle: String,
    pub channel: String,
    pub group: String,
    pub data: String,
    pub sound: bool,
    /// M447 (pepa #331): el nombre de un sonido propio (`alarm` o `alarm.caf`), de los archivos de
    /// `[ios] sounds`; vacío = el sonido por defecto. En Android el sonido es del CANAL.
    pub sound_name: String,
    /// −1 = no tocar el badge.
    pub badge: i64,
    /// `passive` | `active` | `time_sensitive` | `critical`.
    pub interruption: String,
    /// `(id, título)`.
    pub actions: Vec<(String, String)>,
}

/// Cuándo mostrarla.
#[derive(Debug, Clone, PartialEq)]
pub enum Trigger {
    Now,
    After { secs: i64, repeat: bool },
    At(i64),
    Daily { hour: i64, minute: i64 },
}

pub const INTERRUPTIONS: [&str; 4] = ["passive", "active", "time_sensitive", "critical"];
pub const IMPORTANCES: [&str; 4] = ["min", "low", "default", "high"];

static SEQ: AtomicI64 = AtomicI64::new(0);

/// Un id para una notificación que no trae ninguno.
fn next_id() -> String {
    format!("ray-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst) + 1)
}

/// `after:<s>:<0|1>` | `at:<unix>` | `daily:<h>:<m>` | `now` | vacío.
pub fn parse_trigger(s: &str) -> Result<Trigger, String> {
    let mut it = s.split(':');
    let kind = it.next().unwrap_or("");
    let num = |v: Option<&str>, what: &str| -> Result<i64, String> {
        v.unwrap_or("").trim().parse::<i64>().map_err(|_| format!("notify: bad trigger '{s}' ({what} must be an integer)"))
    };
    match kind {
        "" | "now" => Ok(Trigger::Now),
        "after" => {
            let secs = num(it.next(), "the delay")?;
            let repeat = it.next().unwrap_or("0") == "1";
            if secs <= 0 {
                return Err("notify: After needs a positive number of seconds".to_string());
            }
            if repeat && secs < 60 {
                return Err("notify: a repeating After needs at least 60 seconds".to_string());
            }
            Ok(Trigger::After { secs, repeat })
        }
        "at" => {
            let unix = num(it.next(), "the time")?;
            // Revisión 1.27.91: validado aquí, no solo en Apple — Android armaba y disparaba al instante.
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
            if unix <= now {
                return Err("notify: At(…) is in the past".to_string());
            }
            Ok(Trigger::At(unix))
        }
        "daily" => {
            let hour = num(it.next(), "the hour")?;
            let minute = num(it.next(), "the minute")?;
            if !(0..24).contains(&hour) || !(0..60).contains(&minute) {
                return Err(format!("notify: Daily({hour}, {minute}) is not a valid time of day"));
            }
            Ok(Trigger::Daily { hour, minute })
        }
        other => Err(format!("notify: unknown trigger '{other}'")),
    }
}

/// La forma en que viaja un `Trigger` (la inversa de [`parse_trigger`]).
pub fn trigger_text(t: &Trigger) -> String {
    match t {
        Trigger::Now => "now".to_string(),
        Trigger::After { secs, repeat } => format!("after:{secs}:{}", u8::from(*repeat)),
        Trigger::At(unix) => format!("at:{unix}"),
        Trigger::Daily { hour, minute } => format!("daily:{hour}:{minute}"),
    }
}

/// `trigger ␟ id ␟ title ␟ body ␟ subtitle ␟ channel ␟ group ␟ data ␟ sound ␟ badge ␟ interruption ␟
/// actions [␟ sound_name]` (acciones: `id␝título` separadas por `␞`; el campo 13 es opcional, M447).
/// Valida y rellena el id si falta.
pub fn parse_note(arg: &str) -> Result<(Trigger, Note), String> {
    let f: Vec<&str> = arg.split(SEP).collect();
    if f.len() < 12 {
        return Err("notify: malformed notification (regenerate std/notify callers)".to_string());
    }
    let trigger = parse_trigger(f[0])?;
    let mut n = Note {
        id: f[1].trim().to_string(),
        title: f[2].to_string(),
        body: f[3].to_string(),
        subtitle: f[4].to_string(),
        channel: f[5].trim().to_string(),
        group: f[6].trim().to_string(),
        data: f[7].to_string(),
        sound: f[8] == "1",
        badge: f[9].trim().parse().unwrap_or(-1),
        interruption: f[10].trim().to_string(),
        actions: Vec::new(),
        sound_name: f.get(12).map(|s| s.trim().to_string()).unwrap_or_default(),
    };
    if n.sound_name.contains(['/', '\\']) {
        return Err("notify: sound_name is a file name (from [ios] sounds / [android] sounds), not a path".to_string());
    }
    for a in f[11].split(ACTION_SEP).filter(|a| !a.is_empty()) {
        let (id, title) = a.split_once(ACTION_FIELD).unwrap_or((a, a));
        if id.trim().is_empty() {
            return Err("notify: an action needs an id".to_string());
        }
        n.actions.push((id.trim().to_string(), title.to_string()));
    }
    if n.title.trim().is_empty() && n.body.trim().is_empty() {
        return Err("notify: a notification needs a title or a body".to_string());
    }
    if n.id.is_empty() {
        n.id = next_id();
    }
    if n.id.contains(['\t', '\n']) {
        return Err("notify: the id cannot contain tabs or newlines".to_string());
    }
    if n.channel.is_empty() {
        n.channel = "default".to_string();
    }
    if n.interruption.is_empty() {
        n.interruption = "active".to_string();
    }
    if !INTERRUPTIONS.contains(&n.interruption.as_str()) {
        return Err(format!("notify: unknown interruption level '{}' ({})", n.interruption, INTERRUPTIONS.join(" | ")));
    }
    Ok((trigger, n))
}

/// Un canal de Android: `id ␟ name ␟ description ␟ importance`.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub importance: String,
    /// M447 (pepa #331): el sonido del canal (Android: un recurso de `[android] sounds`, sin
    /// extensión); vacío = el del sistema. Fijo una vez creado el canal.
    pub sound: String,
}

pub fn parse_channel(arg: &str) -> Result<Channel, String> {
    let f: Vec<&str> = arg.split(SEP).collect();
    if f.len() < 4 {
        return Err("notify: malformed channel".to_string());
    }
    let c = Channel {
        id: f[0].trim().to_string(),
        name: f[1].trim().to_string(),
        description: f[2].to_string(),
        importance: if f[3].trim().is_empty() { "default".to_string() } else { f[3].trim().to_string() },
        sound: f.get(4).map(|s| s.trim().to_string()).unwrap_or_default(),
    };
    if c.id.is_empty() || c.name.is_empty() {
        return Err("notify: a channel needs an id and a name".to_string());
    }
    if !IMPORTANCES.contains(&c.importance.as_str()) {
        return Err(format!("notify: unknown channel importance '{}' ({})", c.importance, IMPORTANCES.join(" | ")));
    }
    Ok(c)
}

/// El `tag` del evento `"notification"`: el id a secas cuando no hay nada más (M371), si no
/// `id\taction\ttext\tdata`.
pub fn tap_tag(id: &str, action: &str, text: &str, data: &str) -> String {
    if action.is_empty() && text.is_empty() && data.is_empty() {
        id.to_string()
    } else {
        format!("{id}\t{action}\t{text}\t{data}")
    }
}

/// Una pulsación → la cola de `std/ui`.
pub fn push_tap(id: &str, action: &str, text: &str, data: &str) {
    crate::ui::push_notification_event(&tap_tag(id, action, text, data));
}

/// `arg` de `notify_permission`: `provisional,time_sensitive` como `0|1`.
fn parse_permission_flags(arg: &str) -> (bool, bool) {
    let mut it = arg.split(',');
    let p = it.next().unwrap_or("0") == "1";
    let t = it.next().unwrap_or("0") == "1";
    (p, t)
}

/// La entrada del túnel: `op` es `notify_permission` | `notify_status` | `notify_channel` |
/// `notify_show` | `notify_cancel` | `notify_cancel_all` | `notify_pending` | `notify_clear` |
/// `notify_badge`.
pub fn op(op: &str, arg: &str) -> Result<String, String> {
    if crate::ui::is_headless() {
        return headless::op(op, arg);
    }
    match op {
        "notify_permission" => {
            let (provisional, time_sensitive) = parse_permission_flags(arg);
            backend::permission(provisional, time_sensitive)
        }
        "notify_status" => backend::status(),
        "notify_channel" => backend::channel(parse_channel(arg)?).map(|_| String::new()),
        "notify_show" => {
            let (trigger, note) = parse_note(arg)?;
            backend::show(trigger, note)
        }
        "notify_cancel" => backend::cancel(arg.trim()).map(|_| String::new()),
        "notify_cancel_all" => backend::cancel_all().map(|_| String::new()),
        "notify_pending" => backend::pending().map(|ids| ids.join("\n")),
        "notify_clear" => backend::clear().map(|_| String::new()),
        "notify_badge" => {
            let n: i64 = arg.trim().parse().map_err(|_| "notify: set_badge takes an integer".to_string())?;
            backend::badge(n).map(|_| String::new())
        }
        other => Err(format!("notify: unknown op '{other}'")),
    }
}

/// `ui.notify_with(title, body, tag, sound)` (M371) expresado como una `Note` ahora, para que el
/// camino antiguo y el nuevo compartan backend donde los dos existen.
pub fn legacy_note(title: &str, body: &str, tag: &str, sound: bool) -> Note {
    Note {
        id: if tag.is_empty() { next_id() } else { tag.to_string() },
        title: title.to_string(),
        body: body.to_string(),
        sound,
        badge: -1,
        channel: "default".to_string(),
        interruption: "active".to_string(),
        ..Default::default()
    }
}

/// Lo mismo que `show` por el túnel, desde Rust (el `app_op("notify")` de `ui.rs` en móvil y en
/// un `.app` de macOS).
pub fn show_now(note: Note) -> Result<(), String> {
    backend::show(Trigger::Now, note).map(|_| ())
}

// ── Headless: todo en memoria, con traza. ──
mod headless {
    use super::*;

    fn pending() -> &'static Mutex<Vec<(String, String)>> {
        static P: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
        &P
    }

    fn trace(line: &str) {
        if crate::ui::ui_trace() {
            eprintln!("[ui] notify {line}");
        }
    }

    pub(super) fn op(op: &str, arg: &str) -> Result<String, String> {
        match op {
            "notify_permission" => {
                let (provisional, time_sensitive) = parse_permission_flags(arg);
                trace(&format!("permission provisional={provisional} time_sensitive={time_sensitive}"));
                Ok(status_text(provisional))
            }
            "notify_status" => Ok(status_text(false)),
            "notify_channel" => {
                let c = parse_channel(arg)?;
                trace(&format!("channel {:?} {:?} importance={}{}", c.id, c.name, c.importance, if c.sound.is_empty() { String::new() } else { format!(" sound={}", c.sound) }));
                Ok(String::new())
            }
            "notify_show" => {
                let (trigger, n) = parse_note(arg)?;
                let acts: Vec<String> = n.actions.iter().map(|(i, t)| format!("{i}={t:?}")).collect();
                trace(&format!(
                    "show {} {:?} {:?} channel={} group={:?} interruption={} badge={} sound={}{} actions=[{}] trigger={}",
                    n.id,
                    n.title,
                    n.body,
                    n.channel,
                    n.group,
                    n.interruption,
                    n.badge,
                    n.sound,
                    if n.sound_name.is_empty() { String::new() } else { format!(" sound_name={}", n.sound_name) },
                    acts.join(","),
                    trigger_text(&trigger)
                ));
                if status_text(false) == "denied" {
                    return Err("notify: notifications are not authorized for this app".to_string());
                }
                if trigger != Trigger::Now {
                    let mut p = pending().lock().unwrap();
                    p.retain(|(id, _)| *id != n.id);
                    p.push((n.id.clone(), trigger_text(&trigger)));
                }
                // El inyector de pruebas: `1` pulsa la notificación; otro valor = el id de la acción.
                if let Ok(click) = std::env::var("RAY_UI_NOTIFY_CLICK")
                    && !click.is_empty()
                {
                    let action = if click == "1" { "" } else { click.as_str() };
                    push_tap(&n.id, action, "", &n.data);
                }
                Ok(n.id)
            }
            "notify_cancel" => {
                trace(&format!("cancel {arg}"));
                pending().lock().unwrap().retain(|(id, _)| id != arg.trim());
                Ok(String::new())
            }
            "notify_cancel_all" => {
                trace("cancel all");
                pending().lock().unwrap().clear();
                Ok(String::new())
            }
            "notify_pending" => Ok(pending().lock().unwrap().iter().map(|(id, _)| id.clone()).collect::<Vec<_>>().join("\n")),
            "notify_clear" => {
                trace("clear delivered");
                Ok(String::new())
            }
            "notify_badge" => {
                let n: i64 = arg.trim().parse().map_err(|_| "notify: set_badge takes an integer".to_string())?;
                trace(&format!("badge {n}"));
                Ok(String::new())
            }
            other => Err(format!("notify: unknown op '{other}'")),
        }
    }

    /// `RAY_UI_NOTIFY_PERMISSION` = `granted` (por defecto) | `denied` | `not_determined`.
    fn status_text(provisional: bool) -> String {
        match std::env::var("RAY_UI_NOTIFY_PERMISSION").as_deref() {
            Ok("denied") => "denied".to_string(),
            Ok("not_determined") => "not_determined".to_string(),
            _ if provisional => "provisional".to_string(),
            _ => "granted".to_string(),
        }
    }
}

// ── macOS e iOS: UNUserNotificationCenter por objc_msgSend. ──
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple {
    use super::{Note, Trigger};
    use std::ffi::{c_char, c_void};
    use std::sync::atomic::{AtomicI8, Ordering};
    use std::sync::{Condvar, Mutex};

    type Id = *mut c_void;
    type Sel = *mut c_void;

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
    static BLOCK_DESCRIPTOR: BlockDescriptor = BlockDescriptor { reserved: 0, size: std::mem::size_of::<BlockLiteral>() as u64 };

    #[link(name = "objc")]
    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {
        fn objc_getClass(name: *const u8) -> Id;
        fn sel_registerName(name: *const u8) -> Sel;
        fn objc_msgSend();
        static _NSConcreteGlobalBlock: c_void;
        fn dlopen(path: *const c_char, flags: i32) -> *mut c_void;
    }

    type MsgId = unsafe extern "C" fn(Id, Sel) -> Id;
    type MsgVoid = unsafe extern "C" fn(Id, Sel);
    type MsgVoidId = unsafe extern "C" fn(Id, Sel, Id);
    type MsgVoidI64 = unsafe extern "C" fn(Id, Sel, i64);
    type MsgIdIdId = unsafe extern "C" fn(Id, Sel, Id, Id) -> Id;
    type MsgIdId = unsafe extern "C" fn(Id, Sel, Id) -> Id; // M447: soundNamed:
    type MsgIdI64 = unsafe extern "C" fn(Id, Sel, i64) -> Id;
    type MsgI64 = unsafe extern "C" fn(Id, Sel) -> i64;
    type MsgU64 = unsafe extern "C" fn(Id, Sel) -> u64;
    type MsgBoolSel = unsafe extern "C" fn(Id, Sel, Sel) -> u8;
    type MsgCStr = unsafe extern "C" fn(Id, Sel) -> *const c_char;
    type MsgInitBytes = unsafe extern "C" fn(Id, Sel, *const u8, usize, u64) -> Id;
    type MsgVoidIdPtr = unsafe extern "C" fn(Id, Sel, Id, *const BlockLiteral);
    type MsgVoidPtr = unsafe extern "C" fn(Id, Sel, *const BlockLiteral);
    type MsgVoidU64Ptr = unsafe extern "C" fn(Id, Sel, u64, *const BlockLiteral);
    type MsgVoidI64Ptr = unsafe extern "C" fn(Id, Sel, i64, *const BlockLiteral);
    type MsgTrigger = unsafe extern "C" fn(Id, Sel, f64, u8) -> Id;
    type MsgCalTrigger = unsafe extern "C" fn(Id, Sel, Id, u8) -> Id;
    type MsgRequest = unsafe extern "C" fn(Id, Sel, Id, Id, Id) -> Id;
    type MsgAction = unsafe extern "C" fn(Id, Sel, Id, Id, u64) -> Id;
    type MsgCategory = unsafe extern "C" fn(Id, Sel, Id, Id, Id, u64) -> Id;

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
            let s = init(alloc(cls(b"NSString\0"), sel(b"alloc\0")), sel(b"initWithBytes:length:encoding:\0"), s.as_ptr(), s.len(), NS_UTF8);
            autorelease(s)
        }
    }
    unsafe fn autorelease(obj: Id) -> Id {
        unsafe {
            let a: MsgId = std::mem::transmute(msg_send());
            a(obj, sel(b"autorelease\0"))
        }
    }
    unsafe fn text_of(obj: Id) -> String {
        unsafe {
            if obj.is_null() {
                return String::new();
            }
            let utf8: MsgCStr = std::mem::transmute(msg_send());
            let c = utf8(obj, sel(b"UTF8String\0"));
            if c.is_null() { String::new() } else { std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned() }
        }
    }
    unsafe fn nsnumber(v: i64) -> Id {
        unsafe {
            let make: MsgIdI64 = std::mem::transmute(msg_send());
            make(cls(b"NSNumber\0"), sel(b"numberWithLongLong:\0"), v)
        }
    }
    unsafe fn responds(obj: Id, s: Sel) -> bool {
        unsafe {
            let r: MsgBoolSel = std::mem::transmute(msg_send());
            r(obj, sel(b"respondsToSelector:\0"), s) != 0
        }
    }

    fn global_block(invoke: *const c_void) -> &'static BlockLiteral {
        static BLOCKS: Mutex<Vec<&'static BlockLiteral>> = Mutex::new(Vec::new());
        let mut v = BLOCKS.lock().unwrap();
        if let Some(b) = v.iter().find(|b| b.invoke == invoke) {
            return b;
        }
        let b: &'static BlockLiteral = Box::leak(Box::new(BlockLiteral {
            isa: &raw const _NSConcreteGlobalBlock,
            flags: BLOCK_IS_GLOBAL,
            reserved: 0,
            invoke,
            descriptor: &BLOCK_DESCRIPTOR,
        }));
        v.push(b);
        b
    }

    /// El centro de notificaciones (el framework se carga en caliente la primera vez).
    fn center() -> Result<Id, String> {
        // SAFETY: carga idempotente del framework + mensaje de clase; nil si no existe.
        unsafe {
            dlopen(c"/System/Library/Frameworks/UserNotifications.framework/UserNotifications".as_ptr(), 2 /* RTLD_NOW */);
            let c = cls(b"UNUserNotificationCenter\0");
            if c.is_null() {
                return Err("notify: UserNotifications framework missing".to_string());
            }
            let get: MsgId = std::mem::transmute(msg_send());
            let center = get(c, sel(b"currentNotificationCenter\0"));
            if center.is_null() {
                return Err("notify: no notification center for this process (needs an app bundle)".to_string());
            }
            #[cfg(target_os = "macos")]
            {
                // En macOS el delegate (pulsaciones, presentación en primer plano) vive en ui.rs; en
                // iOS lo pone el AppDelegate del shell.
                let set: MsgVoidId = std::mem::transmute(msg_send());
                set(center, sel(b"setDelegate:\0"), crate::ui::mac_un_delegate() as Id);
            }
            Ok(center)
        }
    }

    // ── Llamadas con bloque de finalización, serializadas y esperadas con un condvar. ──
    struct Reply {
        value: Mutex<Option<Result<String, String>>>,
        done: Condvar,
    }
    fn reply() -> &'static Reply {
        static R: std::sync::OnceLock<Reply> = std::sync::OnceLock::new();
        R.get_or_init(|| Reply { value: Mutex::new(None), done: Condvar::new() })
    }
    fn calls() -> &'static Mutex<()> {
        static C: Mutex<()> = Mutex::new(());
        &C
    }
    fn deliver(r: Result<String, String>) {
        let rp = reply();
        *rp.value.lock().unwrap() = Some(r);
        rp.done.notify_all();
    }
    /// Ejecuta `start` (que dispara la llamada asíncrona) y espera su respuesta.
    fn blocking(start: impl FnOnce() -> Result<(), String>) -> Result<String, String> {
        let _serial = calls().lock().unwrap_or_else(|e| e.into_inner());
        let rp = reply();
        *rp.value.lock().unwrap() = None;
        start()?;
        let mut v = rp.value.lock().unwrap();
        loop {
            if let Some(r) = v.take() {
                return r;
            }
            v = rp.done.wait(v).unwrap();
        }
    }

    /// −1 sin saber, 0 denegada, 1 concedida, 2 provisional.
    static AUTH: AtomicI8 = AtomicI8::new(-1);
    /// Peticiones retenidas a la espera de la autorización (se añaden al concederla): (id, puntero).
    static WAITING: Mutex<Vec<(String, usize)>> = Mutex::new(Vec::new());

    /// Suelta una petición retenida que ya no se va a añadir.
    fn release_request(request: usize) {
        // SAFETY: el puntero vino de `retain` en `show`; un `release` por retención.
        unsafe {
            let rel: MsgVoid = std::mem::transmute(msg_send());
            rel(request as Id, sel(b"release\0"));
        }
    }
    /// ¿Hay una petición de autorización en vuelo (lanzada por `show`, sin esperar)?
    static ASKING: Mutex<bool> = Mutex::new(false);
    /// Revisión 1.27.91: ¿la autorización en vuelo la espera `permission()` en el slot de respuestas?
    /// La que lanza `show` (sin esperar) NO debe entregar ahí: pisaría la respuesta de otra llamada
    /// (`pending()` recibía "granted" como lista de ids).
    static AUTH_AWAITED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    const OPT_BADGE: u64 = 1;
    const OPT_SOUND: u64 = 2;
    const OPT_ALERT: u64 = 4;
    const OPT_PROVISIONAL: u64 = 1 << 6;
    const OPT_TIME_SENSITIVE: u64 = 1 << 8;

    extern "C" fn auth_done(_block: *const BlockLiteral, granted: u8, _error: Id) {
        let was_provisional = *ASKING.lock().unwrap();
        // `granted` es YES también con `provisional`; el estado fino lo da `status`.
        AUTH.store(if granted != 0 { 1 } else { 0 }, Ordering::SeqCst);
        *ASKING.lock().unwrap() = false;
        let waiting: Vec<(String, usize)> = std::mem::take(&mut *WAITING.lock().unwrap());
        if granted != 0 {
            for (_, req) in waiting {
                add_request(req as Id);
            }
        } else {
            if crate::ui::ui_trace() {
                eprintln!("[ui] notify: the user did not authorize notifications ({} dropped)", waiting.len());
            }
            for (_, req) in waiting {
                release_request(req);
            }
        }
        let _ = was_provisional;
        if AUTH_AWAITED.swap(false, Ordering::SeqCst) {
            deliver(Ok(if granted != 0 { "granted".to_string() } else { "denied".to_string() }));
        }
    }

    fn request_authorization(opts: u64) -> Result<(), String> {
        let center = center()?;
        // SAFETY: mensaje documentado; el bloque es global (vive siempre).
        unsafe {
            let ask: MsgVoidU64Ptr = std::mem::transmute(msg_send());
            let block = global_block(auth_done as extern "C" fn(*const BlockLiteral, u8, Id) as *const c_void);
            ask(center, sel(b"requestAuthorizationWithOptions:completionHandler:\0"), opts, block);
        }
        Ok(())
    }

    pub(super) fn permission(provisional: bool, time_sensitive: bool) -> Result<String, String> {
        let mut opts = OPT_ALERT | OPT_SOUND | OPT_BADGE;
        if provisional {
            opts |= OPT_PROVISIONAL;
        }
        if time_sensitive {
            opts |= OPT_TIME_SENSITIVE;
        }
        let r = blocking(|| {
            AUTH_AWAITED.store(true, Ordering::SeqCst);
            request_authorization(opts).inspect_err(|_| AUTH_AWAITED.store(false, Ordering::SeqCst))
        })?;
        // Con `provisional` el sistema concede sin diálogo: el estado real lo cuenta `status`.
        if provisional && r == "granted" { status() } else { Ok(r) }
    }

    extern "C" fn settings_done(_block: *const BlockLiteral, settings: Id) {
        // SAFETY: `settings` es un UNNotificationSettings válido durante el bloque.
        let st = unsafe {
            let get: MsgI64 = std::mem::transmute(msg_send());
            get(settings, sel(b"authorizationStatus\0"))
        };
        let text = match st {
            0 => "not_determined",
            1 => "denied",
            2 => "granted",
            3 => "provisional",
            4 => "ephemeral",
            _ => "not_determined",
        };
        AUTH.store(
            match text {
                "granted" | "ephemeral" => 1,
                "provisional" => 2,
                "denied" => 0,
                _ => -1,
            },
            Ordering::SeqCst,
        );
        deliver(Ok(text.to_string()));
    }

    pub(super) fn status() -> Result<String, String> {
        blocking(|| {
            let center = center()?;
            // SAFETY: mensaje documentado con bloque global.
            unsafe {
                let get: MsgVoidPtr = std::mem::transmute(msg_send());
                let block = global_block(settings_done as extern "C" fn(*const BlockLiteral, Id) as *const c_void);
                get(center, sel(b"getNotificationSettingsWithCompletionHandler:\0"), block);
            }
            Ok(())
        })
    }

    /// Los canales son de Android; aquí no hay nada que crear.
    pub(super) fn channel(_c: super::Channel) -> Result<(), String> {
        Ok(())
    }

    fn add_request(request: Id) {
        // SAFETY: `addNotificationRequest:withCompletionHandler:` acepta handler nil; la petición
        // viene retenida y el centro la copia.
        unsafe {
            if let Ok(center) = center() {
                let add: MsgVoidIdPtr = std::mem::transmute(msg_send());
                add(center, sel(b"addNotificationRequest:withCompletionHandler:\0"), request, std::ptr::null());
            }
            let rel: MsgVoid = std::mem::transmute(msg_send());
            rel(request, sel(b"release\0"));
        }
    }

    /// Una categoría registrada: su id y sus acciones `(id, título)`.
    type Category = (String, Vec<(String, String)>);

    /// Las categorías registradas, para re-registrarlas todas juntas (`setNotificationCategories:`
    /// sustituye el conjunto entero).
    fn categories() -> &'static Mutex<Vec<Category>> {
        static C: Mutex<Vec<Category>> = Mutex::new(Vec::new());
        &C
    }

    unsafe fn register_category(center: Id, actions: &[(String, String)]) -> Id {
        unsafe {
            // Revisión 1.27.91: la clave lleva también los títulos — dos notificaciones con los mismos ids
            // y botones distintos tenían una sola categoría (la segunda mostraba los títulos de la primera).
            let cat_id = format!("ray:{}", actions.iter().map(|(i, t)| format!("{i}={t}")).collect::<Vec<_>>().join(","));
            let mut all = categories().lock().unwrap();
            if !all.iter().any(|(id, _)| *id == cat_id) {
                all.push((cat_id.clone(), actions.to_vec()));
            }
            // NSMutableSet con una UNNotificationCategory por entrada.
            let get: MsgId = std::mem::transmute(msg_send());
            let add: MsgVoidId = std::mem::transmute(msg_send());
            let set = get(cls(b"NSMutableSet\0"), sel(b"set\0"));
            const ACTION_FOREGROUND: u64 = 4;
            for (id, acts) in all.iter() {
                let arr = get(cls(b"NSMutableArray\0"), sel(b"array\0"));
                for (aid, title) in acts {
                    let make: MsgAction = std::mem::transmute(msg_send());
                    let a = make(cls(b"UNNotificationAction\0"), sel(b"actionWithIdentifier:title:options:\0"), nsstring(aid), nsstring(title), ACTION_FOREGROUND);
                    add(arr, sel(b"addObject:\0"), a);
                }
                let make_cat: MsgCategory = std::mem::transmute(msg_send());
                let empty = get(cls(b"NSArray\0"), sel(b"array\0"));
                let cat = make_cat(cls(b"UNNotificationCategory\0"), sel(b"categoryWithIdentifier:actions:intentIdentifiers:options:\0"), nsstring(id), arr, empty, 0);
                add(set, sel(b"addObject:\0"), cat);
            }
            add(center, sel(b"setNotificationCategories:\0"), set);
            nsstring(&cat_id)
        }
    }

    /// M447: `nombre[.ext]` → `nombre.ext` si el archivo está en el bundle principal (probando las
    /// extensiones que UserNotifications acepta cuando no viene ninguna).
    fn bundled_sound(name: &str) -> Option<String> {
        let (stem, exts): (&str, Vec<&str>) = match name.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s, vec![e]),
            _ => (name, vec!["caf", "aiff", "aif", "wav", "m4a", "mp3"]),
        };
        // SAFETY: mensajes documentados de Foundation; los NSString son autoreleased.
        unsafe {
            let get: MsgId = std::mem::transmute(msg_send());
            let path_for: MsgIdIdId = std::mem::transmute(msg_send());
            let bundle = get(cls(b"NSBundle\0"), sel(b"mainBundle\0"));
            for ext in exts {
                let p = path_for(bundle, sel(b"pathForResource:ofType:\0"), nsstring(stem), nsstring(ext));
                if !p.is_null() {
                    return Some(format!("{stem}.{ext}"));
                }
            }
        }
        None
    }

    unsafe fn make_trigger(trigger: &Trigger) -> Result<Id, String> {
        unsafe {
            Ok(match trigger {
                Trigger::Now => std::ptr::null_mut(),
                Trigger::After { secs, repeat } => {
                    let make: MsgTrigger = std::mem::transmute(msg_send());
                    make(cls(b"UNTimeIntervalNotificationTrigger\0"), sel(b"triggerWithTimeInterval:repeats:\0"), *secs as f64, u8::from(*repeat))
                }
                Trigger::At(unix) => {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
                    let secs = unix - now;
                    if secs <= 0 {
                        return Err("notify: At(…) is in the past".to_string());
                    }
                    let make: MsgTrigger = std::mem::transmute(msg_send());
                    make(cls(b"UNTimeIntervalNotificationTrigger\0"), sel(b"triggerWithTimeInterval:repeats:\0"), secs as f64, 0)
                }
                Trigger::Daily { hour, minute } => {
                    let get: MsgId = std::mem::transmute(msg_send());
                    let set_i: MsgVoidI64 = std::mem::transmute(msg_send());
                    let comps = get(get(cls(b"NSDateComponents\0"), sel(b"alloc\0")), sel(b"init\0"));
                    set_i(comps, sel(b"setHour:\0"), *hour);
                    set_i(comps, sel(b"setMinute:\0"), *minute);
                    let make: MsgCalTrigger = std::mem::transmute(msg_send());
                    make(cls(b"UNCalendarNotificationTrigger\0"), sel(b"triggerWithDateMatchingComponents:repeats:\0"), autorelease(comps), 1)
                }
            })
        }
    }

    pub(super) fn show(trigger: Trigger, n: Note) -> Result<String, String> {
        let center = center()?;
        if AUTH.load(Ordering::SeqCst) == 0 {
            return Err("notify: notifications are not authorized for this app (Settings → Notifications)".to_string());
        }
        // SAFETY: mensajes documentados de UserNotifications; la petición se retiene hasta que el
        // centro la copia (`add_request` la suelta).
        let request = unsafe {
            let get: MsgId = std::mem::transmute(msg_send());
            let set_id: MsgVoidId = std::mem::transmute(msg_send());
            let set_i64: MsgVoidI64 = std::mem::transmute(msg_send());
            let content = autorelease(get(get(cls(b"UNMutableNotificationContent\0"), sel(b"alloc\0")), sel(b"init\0")));
            set_id(content, sel(b"setTitle:\0"), nsstring(&n.title));
            set_id(content, sel(b"setBody:\0"), nsstring(&n.body));
            if !n.subtitle.is_empty() {
                set_id(content, sel(b"setSubtitle:\0"), nsstring(&n.subtitle));
            }
            if !n.group.is_empty() {
                set_id(content, sel(b"setThreadIdentifier:\0"), nsstring(&n.group));
            }
            if n.sound {
                let snd = if !n.sound_name.is_empty() {
                    // M447 (pepa #331): un archivo del bundle (`[ios] sounds`): caf/aiff/wav de ≤ 30 s.
                    let file = bundled_sound(&n.sound_name).ok_or_else(|| format!("notify: sound '{}' is not in the app bundle (list the file in [ios] sounds of ray.toml and regenerate the shell)", n.sound_name))?;
                    let named: MsgIdId = std::mem::transmute(msg_send());
                    named(cls(b"UNNotificationSound\0"), sel(b"soundNamed:\0"), nsstring(&file))
                } else if n.interruption == "critical" && responds(cls(b"UNNotificationSound\0"), sel(b"defaultCriticalSound\0")) {
                    get(cls(b"UNNotificationSound\0"), sel(b"defaultCriticalSound\0"))
                } else {
                    get(cls(b"UNNotificationSound\0"), sel(b"defaultSound\0"))
                };
                set_id(content, sel(b"setSound:\0"), snd);
            }
            if n.badge >= 0 {
                set_id(content, sel(b"setBadge:\0"), nsnumber(n.badge));
            }
            if responds(content, sel(b"setInterruptionLevel:\0")) {
                let level = match n.interruption.as_str() {
                    "passive" => 0,
                    "time_sensitive" => 2,
                    "critical" => 3,
                    _ => 1,
                };
                set_i64(content, sel(b"setInterruptionLevel:\0"), level);
            }
            // userInfo: el id (`tag`, como M371) y los datos del programa.
            let dict: MsgIdIdId = std::mem::transmute(msg_send());
            let keys = get(cls(b"NSMutableArray\0"), sel(b"array\0"));
            let vals = get(cls(b"NSMutableArray\0"), sel(b"array\0"));
            let add: MsgVoidId = std::mem::transmute(msg_send());
            add(keys, sel(b"addObject:\0"), nsstring("tag"));
            add(vals, sel(b"addObject:\0"), nsstring(&n.id));
            add(keys, sel(b"addObject:\0"), nsstring("data"));
            add(vals, sel(b"addObject:\0"), nsstring(&n.data));
            let info = dict(cls(b"NSDictionary\0"), sel(b"dictionaryWithObjects:forKeys:\0"), vals, keys);
            set_id(content, sel(b"setUserInfo:\0"), info);
            if !n.actions.is_empty() {
                let cat = register_category(center, &n.actions);
                set_id(content, sel(b"setCategoryIdentifier:\0"), cat);
            }
            let make: MsgRequest = std::mem::transmute(msg_send());
            let t = make_trigger(&trigger)?;
            let request = make(cls(b"UNNotificationRequest\0"), sel(b"requestWithIdentifier:content:trigger:\0"), nsstring(&n.id), content, t);
            let retain: MsgId = std::mem::transmute(msg_send());
            retain(request, sel(b"retain\0"));
            request
        };
        match AUTH.load(Ordering::SeqCst) {
            1 | 2 => add_request(request),
            _ => {
                // Sin saber aún: se pide (sin esperar, como M371) y la petición espera al permiso.
                let first = {
                    let mut w = WAITING.lock().unwrap();
                    w.push((n.id.clone(), request as usize));
                    w.len() == 1
                };
                if first {
                    let mut asking = ASKING.lock().unwrap();
                    if !*asking {
                        *asking = true;
                        drop(asking);
                        request_authorization(OPT_ALERT | OPT_SOUND | OPT_BADGE)?;
                    }
                }
            }
        }
        Ok(n.id)
    }

    pub(super) fn cancel(id: &str) -> Result<(), String> {
        let center = center()?;
        // SAFETY: mensajes documentados; el array es autoreleased.
        unsafe {
            let get: MsgId = std::mem::transmute(msg_send());
            let add: MsgVoidId = std::mem::transmute(msg_send());
            let arr = get(cls(b"NSMutableArray\0"), sel(b"array\0"));
            add(arr, sel(b"addObject:\0"), nsstring(id));
            add(center, sel(b"removePendingNotificationRequestsWithIdentifiers:\0"), arr);
            add(center, sel(b"removeDeliveredNotificationsWithIdentifiers:\0"), arr);
        }
        // Revisión 1.27.91: solo la retenida con ESE id (antes vaciaba toda la cola de espera).
        let dropped: Vec<usize> = {
            let mut w = WAITING.lock().unwrap();
            let (gone, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut *w).into_iter().partition(|(i, _)| i == id);
            *w = keep;
            gone.into_iter().map(|(_, p)| p).collect()
        };
        for p in dropped {
            release_request(p);
        }
        Ok(())
    }

    pub(super) fn cancel_all() -> Result<(), String> {
        let center = center()?;
        // SAFETY: mensaje documentado sin argumentos.
        unsafe {
            let call: MsgVoid = std::mem::transmute(msg_send());
            call(center, sel(b"removeAllPendingNotificationRequests\0"));
        }
        for (_, p) in std::mem::take(&mut *WAITING.lock().unwrap()) {
            release_request(p);
        }
        Ok(())
    }

    extern "C" fn pending_done(_block: *const BlockLiteral, requests: Id) {
        // SAFETY: NSArray<UNNotificationRequest> válido durante el bloque.
        let ids = unsafe {
            let count: MsgU64 = std::mem::transmute(msg_send());
            let at: MsgIdI64 = std::mem::transmute(msg_send());
            let get: MsgId = std::mem::transmute(msg_send());
            let n = if requests.is_null() { 0 } else { count(requests, sel(b"count\0")) };
            (0..n as i64).map(|i| text_of(get(at(requests, sel(b"objectAtIndex:\0"), i), sel(b"identifier\0")))).collect::<Vec<_>>()
        };
        deliver(Ok(ids.join("\n")));
    }

    pub(super) fn pending() -> Result<Vec<String>, String> {
        let text = blocking(|| {
            let center = center()?;
            // SAFETY: mensaje documentado con bloque global.
            unsafe {
                let get: MsgVoidPtr = std::mem::transmute(msg_send());
                let block = global_block(pending_done as extern "C" fn(*const BlockLiteral, Id) as *const c_void);
                get(center, sel(b"getPendingNotificationRequestsWithCompletionHandler:\0"), block);
            }
            Ok(())
        })?;
        Ok(text.split('\n').filter(|s| !s.is_empty()).map(String::from).collect())
    }

    pub(super) fn clear() -> Result<(), String> {
        let center = center()?;
        // SAFETY: mensaje documentado sin argumentos.
        unsafe {
            let call: MsgVoid = std::mem::transmute(msg_send());
            call(center, sel(b"removeAllDeliveredNotifications\0"));
        }
        Ok(())
    }

    extern "C" fn badge_done(_block: *const BlockLiteral, error: Id) {
        deliver(if error.is_null() { Ok(String::new()) } else { Err("notify: the system refused the badge".to_string()) });
    }

    pub(super) fn badge(n: i64) -> Result<(), String> {
        let center = center()?;
        // iOS 16+ / macOS 13+: `setBadgeCount:withCompletionHandler:`; antes, el Dock (macOS) o
        // `applicationIconBadgeNumber` (iOS, en el hilo principal).
        // SAFETY: `respondsToSelector:` decide la vía; mensajes documentados.
        if unsafe { responds(center, sel(b"setBadgeCount:withCompletionHandler:\0")) } {
            return blocking(|| {
                unsafe {
                    let set: MsgVoidI64Ptr = std::mem::transmute(msg_send());
                    let block = global_block(badge_done as extern "C" fn(*const BlockLiteral, Id) as *const c_void);
                    set(center, sel(b"setBadgeCount:withCompletionHandler:\0"), n.max(0), block);
                }
                Ok(())
            })
            .map(|_| ());
        }
        #[cfg(target_os = "macos")]
        {
            crate::ui::window_op(0, "badge", &if n <= 0 { String::new() } else { n.to_string() })
        }
        #[cfg(target_os = "ios")]
        {
            let _ = n;
            Err("notify: set_badge needs iOS 16 or newer".to_string())
        }
    }
}

// ── La selección por plataforma. ──
#[cfg(target_os = "ios")]
mod backend {
    pub(super) use super::apple::{badge, cancel, cancel_all, channel, clear, pending, permission, show, status};
}

#[cfg(target_os = "macos")]
mod backend {
    use super::{Note, Trigger};

    fn bundled() -> bool {
        crate::ui::apple_bundled()
    }
    fn needs_bundle() -> String {
        "notify: this needs an app bundle (`ray bundle`); a bare binary only has `show` for immediate notifications".to_string()
    }

    pub(super) fn permission(provisional: bool, time_sensitive: bool) -> Result<String, String> {
        if bundled() { super::apple::permission(provisional, time_sensitive) } else { Ok("granted".to_string()) }
    }
    pub(super) fn status() -> Result<String, String> {
        if bundled() { super::apple::status() } else { Ok("granted".to_string()) }
    }
    pub(super) fn channel(c: super::Channel) -> Result<(), String> {
        if bundled() { super::apple::channel(c) } else { Ok(()) }
    }
    pub(super) fn show(trigger: Trigger, n: Note) -> Result<String, String> {
        if bundled() {
            return super::apple::show(trigger, n);
        }
        if trigger != Trigger::Now {
            return Err(needs_bundle());
        }
        let id = n.id.clone();
        crate::ui::legacy_notify(&n.title, &n.body, &n.id, n.sound)?;
        Ok(id)
    }
    pub(super) fn cancel(id: &str) -> Result<(), String> {
        if bundled() { super::apple::cancel(id) } else { Ok(()) }
    }
    pub(super) fn cancel_all() -> Result<(), String> {
        if bundled() { super::apple::cancel_all() } else { Ok(()) }
    }
    pub(super) fn pending() -> Result<Vec<String>, String> {
        if bundled() { super::apple::pending() } else { Ok(Vec::new()) }
    }
    pub(super) fn clear() -> Result<(), String> {
        if bundled() { super::apple::clear() } else { Ok(()) }
    }
    pub(super) fn badge(n: i64) -> Result<(), String> {
        if bundled() { super::apple::badge(n) } else { crate::ui::window_op(0, "badge", &if n <= 0 { String::new() } else { n.to_string() }) }
    }
}

#[cfg(target_os = "android")]
mod backend {
    use super::{Note, Trigger, SEP};

    fn ask(op: &str, arg: &str) -> Result<String, String> {
        crate::ui::shell_request(op, arg)
    }
    pub(super) fn permission(provisional: bool, time_sensitive: bool) -> Result<String, String> {
        ask("notify_permission", &format!("{},{}", u8::from(provisional), u8::from(time_sensitive)))
    }
    pub(super) fn status() -> Result<String, String> {
        ask("notify_status", "")
    }
    pub(super) fn channel(c: super::Channel) -> Result<(), String> {
        ask("notify_channel", &format!("{}{SEP}{}{SEP}{}{SEP}{}{SEP}{}", c.id, c.name, c.description, c.importance, c.sound)).map(|_| ())
    }
    pub(super) fn show(trigger: Trigger, n: Note) -> Result<String, String> {
        let actions: Vec<String> = n.actions.iter().map(|(i, t)| format!("{i}{}{t}", super::ACTION_FIELD)).collect();
        let arg = [
            super::trigger_text(&trigger),
            n.id.clone(),
            n.title,
            n.body,
            n.subtitle,
            n.channel,
            n.group,
            n.data,
            u8::from(n.sound).to_string(),
            n.badge.to_string(),
            n.interruption,
            actions.join(&super::ACTION_SEP.to_string()),
            n.sound_name,
        ]
        .join(&SEP.to_string());
        ask("notify_show", &arg).map(|_| n.id)
    }
    pub(super) fn cancel(id: &str) -> Result<(), String> {
        ask("notify_cancel", id).map(|_| ())
    }
    pub(super) fn cancel_all() -> Result<(), String> {
        ask("notify_cancel_all", "").map(|_| ())
    }
    pub(super) fn pending() -> Result<Vec<String>, String> {
        ask("notify_pending", "").map(|t| t.split('\n').filter(|s| !s.is_empty()).map(String::from).collect())
    }
    pub(super) fn clear() -> Result<(), String> {
        ask("notify_clear", "").map(|_| ())
    }
    pub(super) fn badge(n: i64) -> Result<(), String> {
        ask("notify_badge", &n.to_string()).map(|_| ())
    }
}

#[cfg(target_os = "linux")]
mod backend {
    use super::{Note, Trigger};

    fn unavailable(what: &str) -> String {
        format!("notify: {what} is not available on Linux desktops (only immediate `show`, via libnotify)")
    }
    pub(super) fn permission(_provisional: bool, _time_sensitive: bool) -> Result<String, String> {
        Ok("granted".to_string())
    }
    pub(super) fn status() -> Result<String, String> {
        Ok("granted".to_string())
    }
    pub(super) fn channel(_c: super::Channel) -> Result<(), String> {
        Ok(())
    }
    pub(super) fn show(trigger: Trigger, n: Note) -> Result<String, String> {
        if trigger != Trigger::Now {
            return Err(unavailable("schedule"));
        }
        let id = n.id.clone();
        crate::ui::legacy_notify(&n.title, &n.body, &n.id, n.sound)?;
        Ok(id)
    }
    pub(super) fn cancel(_id: &str) -> Result<(), String> {
        Ok(())
    }
    pub(super) fn cancel_all() -> Result<(), String> {
        Ok(())
    }
    pub(super) fn pending() -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
    pub(super) fn clear() -> Result<(), String> {
        Ok(())
    }
    pub(super) fn badge(_n: i64) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android", target_os = "linux")))]
mod backend {
    use super::{Note, Trigger};

    fn unavailable() -> String {
        "notify: notifications are not available on this platform yet".to_string()
    }
    pub(super) fn permission(_provisional: bool, _time_sensitive: bool) -> Result<String, String> {
        Err(unavailable())
    }
    pub(super) fn status() -> Result<String, String> {
        Err(unavailable())
    }
    pub(super) fn channel(_c: super::Channel) -> Result<(), String> {
        Err(unavailable())
    }
    pub(super) fn show(_trigger: Trigger, _n: Note) -> Result<String, String> {
        Err(unavailable())
    }
    pub(super) fn cancel(_id: &str) -> Result<(), String> {
        Err(unavailable())
    }
    pub(super) fn cancel_all() -> Result<(), String> {
        Err(unavailable())
    }
    pub(super) fn pending() -> Result<Vec<String>, String> {
        Err(unavailable())
    }
    pub(super) fn clear() -> Result<(), String> {
        Err(unavailable())
    }
    pub(super) fn badge(_n: i64) -> Result<(), String> {
        Err(unavailable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(fields: &[&str]) -> String {
        fields.join(&SEP.to_string())
    }

    #[test]
    fn a_note_round_trips_through_the_wire_format() {
        let actions = format!("track{ACTION_FIELD}Seguir{ACTION_SEP}snooze{ACTION_FIELD}Luego");
        let arg = enc(&["after:3600:0", "order-42", "Pedido", "Llega el viernes", "Sub", "orders", "orders", "{\"o\":42}", "1", "3", "time_sensitive", &actions]);
        let (t, n) = parse_note(&arg).unwrap();
        assert_eq!(t, Trigger::After { secs: 3600, repeat: false });
        assert_eq!(n.id, "order-42");
        assert_eq!(n.badge, 3);
        assert_eq!(n.actions, vec![("track".to_string(), "Seguir".to_string()), ("snooze".to_string(), "Luego".to_string())]);
        assert_eq!(trigger_text(&t), "after:3600:0");
        // Sin id: se genera uno; sin canal ni nivel: los valores por defecto.
        let (t, n) = parse_note(&enc(&["", "", "Hola", "", "", "", "", "", "0", "-1", "", ""])).unwrap();
        assert_eq!(t, Trigger::Now);
        assert!(n.id.starts_with("ray-"), "{}", n.id);
        assert_eq!((n.channel.as_str(), n.interruption.as_str()), ("default", "active"));
    }

    #[test]
    fn invalid_notes_and_triggers_are_rejected_with_a_reason() {
        let bad = |arg: &str| parse_note(arg).unwrap_err();
        assert!(bad(&enc(&["now", "", "", "", "", "", "", "", "0", "-1", "", ""])).contains("needs a title or a body"));
        assert!(bad(&enc(&["now", "", "T", "", "", "", "", "", "0", "-1", "loud", ""])).contains("unknown interruption level"));
        assert!(bad(&enc(&["after:0:0", "", "T", "", "", "", "", "", "0", "-1", "", ""])).contains("positive number of seconds"));
        assert!(bad(&enc(&["after:30:1", "", "T", "", "", "", "", "", "0", "-1", "", ""])).contains("at least 60 seconds"));
        assert!(bad(&enc(&["daily:24:0", "", "T", "", "", "", "", "", "0", "-1", "", ""])).contains("not a valid time of day"));
        assert!(bad(&enc(&["weekly:1", "", "T", "", "", "", "", "", "0", "-1", "", ""])).contains("unknown trigger"));
        assert!(bad(&enc(&["now", "a\tb", "T", "", "", "", "", "", "0", "-1", "", ""])).contains("tabs or newlines"));
        assert!(bad("x").contains("malformed"));
        assert_eq!(parse_trigger("daily:9:30").unwrap(), Trigger::Daily { hour: 9, minute: 30 });
        assert_eq!(parse_trigger("at:1800000000").unwrap(), Trigger::At(1_800_000_000));
    }

    #[test]
    fn channels_and_tap_tags() {
        let c = parse_channel(&enc(&["orders", "Pedidos", "Estado de los pedidos", "high"])).unwrap();
        assert_eq!((c.id.as_str(), c.importance.as_str()), ("orders", "high"));
        assert!(parse_channel(&enc(&["", "x", "", ""])).unwrap_err().contains("needs an id and a name"));
        assert!(parse_channel(&enc(&["a", "b", "", "loud"])).unwrap_err().contains("unknown channel importance"));
        assert_eq!(tap_tag("n1", "", "", ""), "n1");
        assert_eq!(tap_tag("n1", "track", "", "{\"o\":1}"), "n1\ttrack\t\t{\"o\":1}");
    }
}
