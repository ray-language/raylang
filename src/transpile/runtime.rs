//! El runtime embebido que emite el backend nativo (movimiento puro; usar `git log --follow`).
//!
//! Texto Rust literal (`out.push_str`/`concat!`), no lógica: el preámbulo SIEMPRE presente
//! (manejo de errores/panic, la repr SEND universal, aritmética checked/fast, helpers de
//! string/Map, `RayShow`) y los bloques BAJO DEMANDA según lo que use el programa (handles,
//! red, TLS, SQLite, concurrencia/canales/Task/scope/select, señales, reloj/PRNG, cripto).

use super::*;

/// M173 (Windows, docs/windows.md W4 §3.4): el `__ray_stdin` del binario nativo en Windows —
/// espejo del `stdin_host` de la VM (`src/builtins.rs`): disponibilidad real por
/// `PeekConsoleInputW` (consola) / `PeekNamedPipe` (pipe), lectura cruda por `ReadConsoleW`
/// (UTF-16 → UTF-8, con resto) / `ReadFile`. Sin fibras en Windows, la espera es en el hilo.
const RT_WIN_STDIN: &str = r##"#[cfg(windows)]
mod __ray_stdin {
    use std::sync::Mutex;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(id: u32) -> usize;
        fn GetFileType(handle: usize) -> u32;
        fn GetConsoleMode(handle: usize, mode: *mut u32) -> i32;
        fn PeekConsoleInputW(handle: usize, buf: *mut InputRecord, len: u32, read: *mut u32) -> i32;
        fn ReadConsoleW(handle: usize, buf: *mut u16, len: u32, read: *mut u32, control: *const core::ffi::c_void) -> i32;
        fn PeekNamedPipe(handle: usize, buf: *mut core::ffi::c_void, len: u32, read: *mut u32, avail: *mut u32, left: *mut u32) -> i32;
        fn ReadFile(handle: usize, buf: *mut u8, len: u32, read: *mut u32, overlapped: *mut core::ffi::c_void) -> i32;
    }
    #[repr(C)]
    pub struct InputRecord { event_type: u16, _pad: u16, key_down: i32, repeat: u16, vk: u16, scan: u16, uchar: u16, ctrl: u32 }
    struct Pending { bytes: Vec<u8>, high_surrogate: Option<u16> }
    static PENDING: Mutex<Pending> = Mutex::new(Pending { bytes: Vec::new(), high_surrogate: None });
    fn stdin_handle() -> usize { unsafe { GetStdHandle(0xFFFF_FFF6) } }
    fn console_mode(h: usize) -> Option<u32> { let mut m = 0u32; if unsafe { GetConsoleMode(h, &mut m) } != 0 { Some(m) } else { None } }
    fn ready_now() -> bool {
        if !PENDING.lock().map(|p| p.bytes.is_empty()).unwrap_or(true) { return true; }
        let h = stdin_handle();
        if h == 0 || h == usize::MAX { return true; }
        if let Some(mode) = console_mode(h) {
            let line_mode = mode & 0x2 != 0;
            let mut recs: [InputRecord; 64] = std::array::from_fn(|_| InputRecord { event_type: 0, _pad: 0, key_down: 0, repeat: 0, vk: 0, scan: 0, uchar: 0, ctrl: 0 });
            let mut n = 0u32;
            if unsafe { PeekConsoleInputW(h, recs.as_mut_ptr(), 64, &mut n) } == 0 { return true; }
            let (mut has_char, mut has_enter) = (false, false);
            for r in recs.iter().take(n as usize) {
                if r.event_type == 1 && r.key_down != 0 && r.uchar != 0 { has_char = true; if r.uchar == 13 { has_enter = true; } }
            }
            return if line_mode { has_enter } else { has_char };
        }
        if unsafe { GetFileType(h) } == 3 {
            let mut avail = 0u32;
            let ok = unsafe { PeekNamedPipe(h, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut()) };
            return ok == 0 || avail > 0;
        }
        true
    }
    pub fn ready(timeout_ms: i32) -> bool {
        if ready_now() { return true; }
        if timeout_ms == 0 { return false; }
        let deadline = (timeout_ms > 0).then(|| std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64));
        loop {
            let quantum = match deadline {
                Some(d) => { let left = d.saturating_duration_since(std::time::Instant::now()); if left.is_zero() { return false; } left.min(std::time::Duration::from_millis(5)) }
                None => std::time::Duration::from_millis(5),
            };
            std::thread::sleep(quantum);
            if ready_now() { return true; }
        }
    }
    pub fn read_max(max: usize) -> Vec<u8> {
        {
            let mut p = match PENDING.lock() { Ok(p) => p, Err(_) => return Vec::new() };
            if !p.bytes.is_empty() { let n = p.bytes.len().min(max); return p.bytes.drain(..n).collect(); }
        }
        let h = stdin_handle();
        if h == 0 || h == usize::MAX { return Vec::new(); }
        if console_mode(h).is_some() { return read_console(h, max); }
        let mut buf = vec![0u8; max];
        let mut n = 0u32;
        if unsafe { ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut n, std::ptr::null_mut()) } == 0 { return Vec::new(); }
        buf.truncate(n as usize);
        buf
    }
    fn read_console(h: usize, max: usize) -> Vec<u8> {
        let mut wide = [0u16; 256];
        let mut n = 0u32;
        if unsafe { ReadConsoleW(h, wide.as_mut_ptr(), 256, &mut n, std::ptr::null()) } == 0 { return Vec::new(); }
        let mut p = match PENDING.lock() { Ok(p) => p, Err(_) => return Vec::new() };
        let mut units: Vec<u16> = Vec::with_capacity(n as usize + 1);
        if let Some(hs) = p.high_surrogate.take() { units.push(hs); }
        units.extend_from_slice(&wide[..n as usize]);
        if let Some(&last) = units.last() { if (0xD800..0xDC00).contains(&last) { p.high_surrogate = units.pop(); } }
        let mut out: Vec<u8> = Vec::with_capacity(units.len() * 3);
        for c in char::decode_utf16(units.iter().copied()) { let mut b = [0u8; 4]; out.extend_from_slice(c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b).as_bytes()); }
        if out.len() > max { p.bytes = out.split_off(max); }
        out
    }
}
"##;

/// M173 (Windows, docs/windows.md W4 §3.3): el `__ray_term` del binario nativo en Windows —
/// espejo del `term_host` de la VM: `IsTerminal`, `GetConsoleScreenBufferInfo` y el modo crudo
/// por `SetConsoleMode` (sin LINE/ECHO/PROCESSED_INPUT, con VT input; VT output sin auto-CR),
/// restaurado en `raw_off` y por `atexit`.
const RT_WIN_TERM: &str = r##"#[cfg(windows)]
mod __ray_term {
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(id: u32) -> usize;
        fn GetConsoleMode(handle: usize, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: usize, mode: u32) -> i32;
        fn GetConsoleScreenBufferInfo(handle: usize, info: *mut ScreenBufferInfo) -> i32;
    }
    unsafe extern "C" { fn atexit(f: extern "C" fn()) -> i32; }
    #[repr(C)]
    struct ScreenBufferInfo { size_x: i16, size_y: i16, cursor_x: i16, cursor_y: i16, attributes: u16, win_left: i16, win_top: i16, win_right: i16, win_bottom: i16, max_x: i16, max_y: i16 }
    static ORIGINAL_IN: AtomicU32 = AtomicU32::new(0);
    static ORIGINAL_OUT: AtomicU32 = AtomicU32::new(0);
    static SAVED: AtomicBool = AtomicBool::new(false);
    static ATEXIT_ARMED: AtomicBool = AtomicBool::new(false);
    static VT_OUTPUT_ARMED: AtomicBool = AtomicBool::new(false);
    static RAW_DEPTH: AtomicUsize = AtomicUsize::new(0);
    fn handle(fd: i32) -> usize { unsafe { GetStdHandle(match fd { 0 => 0xFFFF_FFF6, 1 => 0xFFFF_FFF5, _ => 0xFFFF_FFF4 }) } }
    fn mode_of(h: usize) -> Option<u32> { let mut m = 0u32; if unsafe { GetConsoleMode(h, &mut m) } != 0 { Some(m) } else { None } }
    pub fn ensure_vt_output() {
        if VT_OUTPUT_ARMED.swap(true, Ordering::AcqRel) { return; }
        for fd in [1, 2] { let h = handle(fd); if let Some(m) = mode_of(h) { if m & 0x4 == 0 { unsafe { SetConsoleMode(h, m | 0x4) }; } } }
    }
    pub fn is_tty(fd: i32) -> bool {
        use std::io::IsTerminal;
        let tty = match fd { 0 => std::io::stdin().is_terminal(), 1 => std::io::stdout().is_terminal(), 2 => std::io::stderr().is_terminal(), _ => false };
        if tty { ensure_vt_output(); }
        tty
    }
    pub fn size() -> Option<(i64, i64)> {
        for fd in [1, 0, 2] {
            let mut info = ScreenBufferInfo { size_x: 0, size_y: 0, cursor_x: 0, cursor_y: 0, attributes: 0, win_left: 0, win_top: 0, win_right: 0, win_bottom: 0, max_x: 0, max_y: 0 };
            if unsafe { GetConsoleScreenBufferInfo(handle(fd), &mut info) } != 0 {
                let cols = (info.win_right as i64 - info.win_left as i64) + 1;
                let rows = (info.win_bottom as i64 - info.win_top as i64) + 1;
                if cols > 0 && rows > 0 { ensure_vt_output(); return Some((cols, rows)); }
            }
        }
        None
    }
    pub fn size_px() -> Option<(i64, i64)> { None }
    extern "C" fn restore() {
        if SAVED.load(Ordering::Acquire) { unsafe { SetConsoleMode(handle(0), ORIGINAL_IN.load(Ordering::Acquire)); SetConsoleMode(handle(1), ORIGINAL_OUT.load(Ordering::Acquire)); } }
    }
    pub fn raw_on() -> Result<(), String> {
        if RAW_DEPTH.load(Ordering::Acquire) > 0 { RAW_DEPTH.fetch_add(1, Ordering::AcqRel); return Ok(()); }
        let hin = handle(0);
        let Some(in_mode) = mode_of(hin) else { return Err(format!("stdin is not a terminal: {}", std::io::Error::last_os_error())); };
        let hout = handle(1);
        let out_mode = mode_of(hout);
        if !SAVED.load(Ordering::Acquire) { ORIGINAL_IN.store(in_mode, Ordering::Release); ORIGINAL_OUT.store(out_mode.unwrap_or(0), Ordering::Release); SAVED.store(true, Ordering::Release); }
        if !ATEXIT_ARMED.swap(true, Ordering::AcqRel) { unsafe { atexit(restore) }; }
        let raw = (in_mode & !(0x2 | 0x4 | 0x1)) | 0x200;
        if unsafe { SetConsoleMode(hin, raw) } == 0 { return Err(format!("could not enter raw mode: {}", std::io::Error::last_os_error())); }
        if let Some(m) = out_mode { unsafe { SetConsoleMode(hout, m | 0x4 | 0x8) }; }
        RAW_DEPTH.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
    pub fn raw_off() -> Result<(), String> {
        if !SAVED.load(Ordering::Acquire) { return Ok(()); }
        if RAW_DEPTH.load(Ordering::Acquire) > 1 { RAW_DEPTH.fetch_sub(1, Ordering::AcqRel); return Ok(()); }
        if unsafe { SetConsoleMode(handle(0), ORIGINAL_IN.load(Ordering::Acquire)) } == 0 { return Err(format!("could not restore the terminal: {}", std::io::Error::last_os_error())); }
        let out = ORIGINAL_OUT.load(Ordering::Acquire);
        if out != 0 { unsafe { SetConsoleMode(handle(1), out) }; }
        RAW_DEPTH.store(0, Ordering::Release);
        Ok(())
    }
}
"##;

pub(super) fn emit_core_runtime(out: &mut String, fast: bool, ahash: bool, fibers: bool) {
    out.push_str("// Generado por el transpilador raylang→Rust (P2.b).\n");
    out.push_str("#![allow(unused_parens, unused_mut, dead_code, unused_variables, unreachable_patterns)]\n");
    out.push_str("use std::rc::Rc;\n");
    // H6 + H21-N1: errores de EJECUCIÓN como la VM — mensaje `runtime error: <msg>` (sin posición: el
    // nativo no lleva el AST) y exit 70 (EX_SOFTWARE, el de la VM). El error viaja como PANIC con
    // payload propio (`__RayErr`), no como exit directo: así el fallo de una TAREA lo captura su
    // `catch_unwind` (→ `TaskState::Failed`, como la VM guarda el fallo en la Task) y el proceso solo
    // muere cuando el fallo llega a `main` sin observarse. El hook de panic calla los `__RayErr` (el
    // mensaje limpio lo imprime quien lo observa); los panics ajenos (índice fuera de rango…) siguen
    // con el hook de Rust.
    out.push_str("struct __RayErr(String);\n");
    out.push_str("#[cold] fn __ray_rt_err(msg: &str) -> ! { std::panic::panic_any(__RayErr(msg.to_string())) }\n");
    // M130: exit(code) — termina el PROCESO, byte-idéntico a la VM. OJO (M132): el print nativo
    // va por el HILO ESCRITOR de M96f — flushear std::io::stdout() aquí era el buffer equivocado
    // (la salida pendiente se perdía; process::exit no corre destructores): hay que drenar el
    // canal con __ray_flush_prints(), como los otros tres sitios que llaman a process::exit.
    out.push_str("#[cold] fn __ray_exit(code: i64) -> ! { __ray_flush_prints(); use std::io::Write; let _ = std::io::stderr().flush(); std::process::exit(code as i32) }\n");
    // M97.2 (`try_call`): recuperación en el MISMO hilo. Devuelve `[]` si `f` volvió bien y `[msg]`
    // si falló, el mismo contrato que `__task_failed` — así el envoltorio `try_call` del prelude es
    // idéntico para los tres motores.
    //
    // Captura CUALQUIER panic, no solo los `__RayErr`, y esto es deliberado: no todos los fallos de
    // runtime del nativo pasan por `__ray_rt_err`. Un índice fuera de rango, por ejemplo, es el
    // bounds check de Rust (el indexado se emite sin comprobación propia, para no pagarla en el
    // camino caliente). Si aquí solo se capturasen los `__RayErr`, la VM recuperaría ese caso y el
    // nativo no → los dos motores DIVERGIRÍAN en el flujo de control, que es la línea que el
    // proyecto no cruza. Capturando todo, `try_call` recupera el mismo conjunto de fallos en los
    // tres motores; lo único que difiere es el TEXTO del mensaje en esa clase de errores, y esa
    // divergencia es preexistente (también se ve hoy en un fallo sin capturar: la VM dice
    // "index 7 out of range (length 2)" y el nativo el texto de Rust).
    // Profundidad de `try_call` en vuelo EN ESTE HILO. El hook de panic la consulta para callarse:
    // un fallo que se va a recuperar no debe escupir "thread panicked at …" ni la nota del
    // backtrace (la VM no imprime nada al recuperar, y los dos motores deben verse igual). Es
    // thread-local, no global: cada hilo lleva la suya, sin sincronización.
    // F2 (--fibers): la profundidad viaja EN LA FIBRA (ctx) — un try_call puede aparcar dentro
    // (E/S de socket) y reanudarse en OTRO worker; el contador por-hilo se quedaría en el anterior
    // (hook mal silenciado allí, silencio de más aquí). `__ray_ctx`/`__FiberCtx` se emiten en
    // emit_runtime_features (el orden de items no importa en Rust).
    if fibers {
        out.push_str("fn __ray_in_try() -> bool { __ray_ctx(|c| c.in_try > 0) }\n");
        out.push_str("fn __ray_try_call<F: FnOnce()>(f: F) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n");
        out.push_str("    __ray_ctx(|c| c.in_try += 1);\n");
        out.push_str("    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));\n");
        out.push_str("    __ray_ctx(|c| c.in_try -= 1);\n");
    } else {
        out.push_str("thread_local! { static __RAY_IN_TRY: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }\n");
        out.push_str("fn __ray_in_try() -> bool { __RAY_IN_TRY.with(|d| d.get() > 0) }\n");
        out.push_str("fn __ray_try_call<F: FnOnce()>(f: F) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n");
        out.push_str("    __RAY_IN_TRY.with(|d| d.set(d.get() + 1));\n");
        out.push_str("    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));\n");
        out.push_str("    __RAY_IN_TRY.with(|d| d.set(d.get() - 1));\n");
    }
    out.push_str("    Rc::new(std::cell::RefCell::new(match r {\n");
    out.push_str("        Ok(()) => Vec::new(),\n");
    out.push_str("        Err(e) => {\n");
    out.push_str("            let m = match e.downcast::<__RayErr>() {\n");
    out.push_str("                Ok(r) => r.0,\n");
    out.push_str("                Err(o) => match o.downcast::<String>() {\n");
    out.push_str("                    Ok(s) => *s,\n");
    out.push_str("                    Err(o) => match o.downcast::<&'static str>() {\n");
    out.push_str("                        Ok(s) => (*s).to_string(),\n");
    out.push_str("                        Err(_) => \"panic\".to_string(),\n");
    out.push_str("                    },\n");
    out.push_str("                },\n");
    out.push_str("            };\n");
    out.push_str("            vec![Rc::<str>::from(m.as_str())]\n");
    out.push_str("        }\n");
    out.push_str("    }))\n");
    out.push_str("}\n");
    out.push_str("fn __ray_panic_msg(e: &(dyn std::any::Any + Send)) -> String {\n");
    out.push_str("    if let Some(r) = e.downcast_ref::<__RayErr>() { r.0.clone() }\n");
    out.push_str("    else if let Some(s) = e.downcast_ref::<&str>() { s.to_string() }\n");
    out.push_str("    else if let Some(s) = e.downcast_ref::<String>() { s.clone() }\n");
    out.push_str("    else { \"panic\".to_string() }\n}\n");
    // H21-N5a: la repr SEND universal — un árbol de datos `Send` al que se CONVIERTE cualquier valor
    // de heap que cruce un hilo (capturas de spawn, elementos de canal, retorno de Task) y del que se
    // reconstruye al otro lado. Es la semántica de la VM (M38, actores de heap aislado: lo que cruza
    // se COPIA entre heaps — la mutación no se comparte; los canales/Tasks son el único conducto).
    // Los conversores por tipo (`__to_send_N`/`__from_send_N`) se generan bajo demanda.
    out.push_str(concat!(
        "#[derive(Clone)]\n",
        "enum __RaySend { I(i64), F(f64), B(bool), C(char), U, UI(u64), S(std::sync::Arc<str>), ",
        "By(std::sync::Arc<[u8]>), A(Vec<__RaySend>), M(Vec<(__RaySend, __RaySend)>), ",
        "T(Vec<__RaySend>), E(usize, Vec<__RaySend>), ",
        // Un canal/tarea dentro del árbol NO se copia: se COMPARTE (clone del Arc interno), como la VM
        // comparte el id de canal al cruzar (M12: el canal ES el conducto, no un dato). Va type-erased
        // (`__RayChan<T>`/`__RayTask<T>` son genéricos y el árbol es monomórfico); `from` downcastea.
        "Ch(std::sync::Arc<dyn std::any::Any + Send + Sync>) }\n",
    ));
    // Aritmética de `int` CHECKED por defecto, como la VM (overflow/div-cero → runtime error, no
    // wrapping silencioso). Mismos textos que interpreter.rs/vm.rs. Con `--fast` (opt-out medido:
    // ~2× en puro int-loop, ~20 % en fib, ~0 en código idiomático), wrapping — pero div/mod por
    // cero SIGUEN chequeados (Rust lo hace igual; gratis). Solo cambia este preámbulo: los sitios
    // de llamada emiten `__ray_add(...)` idéntico en ambos modos.
    if fast {
        out.push_str("#[inline(always)] fn __ray_add(a: i64, b: i64) -> i64 { a.wrapping_add(b) }\n");
        out.push_str("#[inline(always)] fn __ray_sub(a: i64, b: i64) -> i64 { a.wrapping_sub(b) }\n");
        out.push_str("#[inline(always)] fn __ray_mul(a: i64, b: i64) -> i64 { a.wrapping_mul(b) }\n");
        out.push_str("#[inline(always)] fn __ray_neg(a: i64) -> i64 { a.wrapping_neg() }\n");
        out.push_str("#[inline(always)] fn __ray_div(a: i64, b: i64) -> i64 { if b == 0 { __ray_rt_err(\"integer division by zero\") } else { a.wrapping_div(b) } }\n");
        out.push_str("#[inline(always)] fn __ray_mod(a: i64, b: i64) -> i64 { if b == 0 { __ray_rt_err(\"modulo by zero\") } else { a.wrapping_rem(b) } }\n");
    } else {
        out.push_str("#[inline(always)] fn __ray_add(a: i64, b: i64) -> i64 { a.checked_add(b).unwrap_or_else(|| __ray_rt_err(\"arithmetic overflow on int\")) }\n");
        out.push_str("#[inline(always)] fn __ray_sub(a: i64, b: i64) -> i64 { a.checked_sub(b).unwrap_or_else(|| __ray_rt_err(\"arithmetic overflow on int\")) }\n");
        out.push_str("#[inline(always)] fn __ray_mul(a: i64, b: i64) -> i64 { a.checked_mul(b).unwrap_or_else(|| __ray_rt_err(\"arithmetic overflow on int\")) }\n");
        out.push_str("#[inline(always)] fn __ray_neg(a: i64) -> i64 { a.checked_neg().unwrap_or_else(|| __ray_rt_err(\"arithmetic overflow on int\")) }\n");
        out.push_str("#[inline(always)] fn __ray_div(a: i64, b: i64) -> i64 { if b == 0 { __ray_rt_err(\"integer division by zero\") } else { a.checked_div(b).unwrap_or_else(|| __ray_rt_err(\"arithmetic overflow on int\")) } }\n");
        out.push_str("#[inline(always)] fn __ray_mod(a: i64, b: i64) -> i64 { if b == 0 { __ray_rt_err(\"modulo by zero\") } else { a.checked_rem(b).unwrap_or_else(|| __ray_rt_err(\"arithmetic overflow on int\")) } }\n");
    }
    // Preámbulo: helpers de runtime para operaciones de arreglo/string que no son 1:1 con Rust.
    out.push_str("fn __ray_split(s: &str, sep: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n");
    out.push_str("    Rc::new(std::cell::RefCell::new(s.split(sep).map(Rc::<str>::from).collect()))\n}\n");
    // N3 (bench políglota): join SIN recopia — construye el `Rc<str>` resultado UNA vez, escribiendo los
    // trozos directo en su buffer (antes: `Vec<&str>` + `join` a `String` + recopia entera a `Rc<str>` →
    // 3 allocs y el DOBLE del resultado vivo en el pico; en jsonserialize eso eran ~17 MB extra).
    // Soundness del unsafe: `total` se calcula del MISMO `v` (el borrow se retiene todo el cuerpo, nadie
    // muta), las copias cubren exactamente `total` bytes, `str` y `[u8]` comparten layout y la
    // concatenación de `str` válidos es UTF-8 válido → el cast `Rc<[u8]>`→`Rc<str>` preserva metadatos.
    out.push_str("fn __ray_join(a: &Rc<std::cell::RefCell<Vec<Rc<str>>>>, sep: &str) -> Rc<str> {\n");
    out.push_str("    let v = a.borrow();\n");
    out.push_str("    if v.is_empty() { return Rc::from(\"\"); }\n");
    out.push_str("    let total: usize = v.iter().map(|s| s.len()).sum::<usize>() + sep.len() * (v.len() - 1);\n");
    out.push_str("    let mut buf = Rc::<[u8]>::new_uninit_slice(total);\n");
    out.push_str("    let dst = Rc::get_mut(&mut buf).unwrap().as_mut_ptr() as *mut u8;\n");
    out.push_str("    let mut off = 0usize;\n");
    out.push_str("    for (i, s) in v.iter().enumerate() {\n");
    out.push_str("        if i > 0 { unsafe { std::ptr::copy_nonoverlapping(sep.as_ptr(), dst.add(off), sep.len()); } off += sep.len(); }\n");
    out.push_str("        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), dst.add(off), s.len()); } off += s.len();\n");
    out.push_str("    }\n");
    out.push_str("    let bytes: Rc<[u8]> = unsafe { buf.assume_init() };\n");
    out.push_str("    unsafe { Rc::from_raw(Rc::into_raw(bytes) as *const str) }\n}\n");
    // index_of(s, sub) -> Option<int>: índice por CARÁCTER de la primera aparición de sub (como la VM;
    // sub vacío → Some(0)). Rust `str::find` da índice de BYTE, así que se compara por char.
    out.push_str("fn __ray_index_of(s: &str, sub: &str) -> Option<i64> {\n");
    out.push_str("    if s.is_ascii() { return s.find(sub).map(|i| i as i64); }\n");
    out.push_str("    let chars: Vec<char> = s.chars().collect(); let sub: Vec<char> = sub.chars().collect();\n");
    out.push_str("    if sub.is_empty() { return Some(0); }\n");
    out.push_str("    if sub.len() > chars.len() { return None; }\n");
    out.push_str("    (0..=chars.len() - sub.len()).find(|&i| chars[i..i + sub.len()] == sub[..]).map(|i| i as i64)\n}\n");
    // N2: los `Map` del programa van tras el alias `__RayMap`. Con aHash (default), el mismo hasher que
    // el `MapStore` de la VM (P0.1) — SipHash es lento en claves string; con `--without ahash`, el
    // HashMap std puro. Todo el código generado construye con `__RayMap::default()`/`from_iter` (valen
    // para ambos hashers); los registros internos (sockets/TLS, clave i64) siguen en HashMap std.
    out.push_str("fn __ray_substring(s: &str, i: i64, j: i64) -> Rc<str> {\n");
    out.push_str("    if s.is_ascii() { let n = s.len() as i64; let lo = i.clamp(0, n); let hi = j.clamp(lo, n); return Rc::from(&s[lo as usize..hi as usize]); }\n");
    out.push_str("    let c: Vec<char> = s.chars().collect(); let n = c.len() as i64;\n");
    out.push_str("    let lo = i.clamp(0, n); let hi = j.clamp(lo, n);\n");
    out.push_str("    Rc::from(c[lo as usize..hi as usize].iter().collect::<String>())\n}\n");
    if ahash {
        out.push_str("type __RayMap<K, V> = std::collections::HashMap<K, V, ray_runtime::RandomState>;\n");
    } else {
        out.push_str("use std::collections::HashMap as __RayMap;\n");
    }
    out.push_str("fn __ray_sort<T: Ord + Clone>(a: &Rc<std::cell::RefCell<Vec<T>>>) -> Rc<std::cell::RefCell<Vec<T>>> {\n");
    out.push_str("    let mut v = a.borrow().clone(); v.sort(); Rc::new(std::cell::RefCell::new(v))\n}\n");
    // SN1 (bench sortnums): la forma FUSIONADA __sort_prim (solo primitivos: int/string/char, lo
    // garantiza el checker) ordena INESTABLE — para primitivos es observacionalmente idéntico y evita
    // el buffer n/2 del sort estable (4 MB en 1M de ints). Los tipos de usuario siguen en __ray_sort.
    out.push_str("fn __ray_sort_unstable<T: Ord + Clone>(a: &Rc<std::cell::RefCell<Vec<T>>>) -> Rc<std::cell::RefCell<Vec<T>>> {\n");
    out.push_str("    let mut v = a.borrow().clone(); v.sort_unstable(); Rc::new(std::cell::RefCell::new(v))\n}\n");
    // IDEAS §63: `sort([float])` — f64 no es `Ord` en Rust, así que __ray_sort no compila. La VM lo
    // enruta por el merge sort del prelude (NaN queda fuera de __sort_prim a propósito): aquí se
    // replica EXACTAMENTE ese merge bottom-up estable comparando con `<` — paridad byte-idéntica
    // incluso con NaN, cosa que `total_cmp`/`sort_by` con orden no-total no garantizarían.
    out.push_str("fn __ray_sort_float(a: &Rc<std::cell::RefCell<Vec<f64>>>) -> Rc<std::cell::RefCell<Vec<f64>>> {\n");
    out.push_str("    let mut src = a.borrow().clone(); let n = src.len(); let mut width = 1;\n");
    out.push_str("    while width < n {\n");
    out.push_str("        let mut dst = Vec::with_capacity(n); let mut lo = 0;\n");
    out.push_str("        while lo < n {\n");
    out.push_str("            let mid = (lo + width).min(n); let hi = (lo + 2 * width).min(n);\n");
    out.push_str("            let (mut p, mut q) = (lo, mid);\n");
    out.push_str("            while p < mid || q < hi {\n");
    out.push_str("                if p >= mid { dst.push(src[q]); q += 1; }\n");
    out.push_str("                else if q >= hi { dst.push(src[p]); p += 1; }\n");
    out.push_str("                else if src[q] < src[p] { dst.push(src[q]); q += 1; }\n");
    out.push_str("                else { dst.push(src[p]); p += 1; }\n");
    out.push_str("            }\n");
    out.push_str("            lo += 2 * width;\n");
    out.push_str("        }\n");
    out.push_str("        src = dst; width *= 2;\n");
    out.push_str("    }\n");
    out.push_str("    Rc::new(std::cell::RefCell::new(src))\n}\n");
    // keys()/values() ORDENADAS por clave (determinista, como la VM). values() en el orden de keys().
    out.push_str("fn __ray_keys<K: Ord + Clone, V>(m: &Rc<std::cell::RefCell<__RayMap<K, V>>>) -> Rc<std::cell::RefCell<Vec<K>>> {\n");
    out.push_str("    let b = m.borrow(); let mut ks: Vec<K> = b.keys().cloned().collect(); ks.sort();\n");
    out.push_str("    Rc::new(std::cell::RefCell::new(ks))\n}\n");
    out.push_str("fn __ray_values<K: Ord + Clone + std::hash::Hash + Eq, V: Clone>(m: &Rc<std::cell::RefCell<__RayMap<K, V>>>) -> Rc<std::cell::RefCell<Vec<V>>> {\n");
    out.push_str("    let b = m.borrow(); let mut ks: Vec<K> = b.keys().cloned().collect(); ks.sort();\n");
    out.push_str("    let vs: Vec<V> = ks.iter().map(|k| b[k].clone()).collect(); Rc::new(std::cell::RefCell::new(vs))\n}\n");
    // for (k, v) in Map: pares ORDENADOS por clave (como la VM). Materializa un Vec (suelta el borrow)
    // antes del cuerpo, que podría mutar el Map.
    out.push_str("fn __ray_pairs<K: Ord + Clone + std::hash::Hash + Eq, V: Clone>(m: &Rc<std::cell::RefCell<__RayMap<K, V>>>) -> Vec<(K, V)> {\n");
    out.push_str("    let b = m.borrow(); let mut ks: Vec<K> = b.keys().cloned().collect(); ks.sort();\n");
    out.push_str("    ks.into_iter().map(|k| { let v = b[&k].clone(); (k, v) }).collect()\n}\n");
    // RayShow: el `Show` de raylang como trait propio (Display no sirve: los structs son Rc<RefCell<..>>,
    // y RefCell no es Display; además un bound genérico `T: Display` fallaría). Impl para todo tipo; los
    // structs/enums de usuario reciben su impl generado (recursivo).
    out.push_str("trait RayShow { fn ray_show(&self) -> String; }\n");
    for (ty, body) in [
        ("i64", "self.to_string()"),
        ("f64", "self.to_string()"),
        ("bool", "self.to_string()"),
        ("char", "self.to_string()"),
        ("()", "\"()\".to_string()"),
        ("Rc<str>", "self.to_string()"),
        // M120: los enteros sin signo (u8/u32/u64) también se imprimen — el harness diferencial
        // cazó que `print(x: u8)` compilaba en la VM y moría con E0599 en el cargo del usuario.
        ("u8", "self.to_string()"),
        ("u32", "self.to_string()"),
        ("u64", "self.to_string()"),
    ] {
        writeln!(out, "impl RayShow for {} {{ fn ray_show(&self) -> String {{ {} }} }}", ty, body).unwrap();
    }
    out.push_str("impl<T: RayShow> RayShow for Rc<std::cell::RefCell<Vec<T>>> { fn ray_show(&self) -> String { format!(\"[{}]\", self.borrow().iter().map(|__e| __e.ray_show()).collect::<Vec<_>>().join(\", \")) } }\n");
    // Map: `Map{k: v, …}` con los pares (renderizados) ordenados como cadena, como el Display del
    // runtime (`Value::Map`): determinista pese al HashMap. `print(map)` directo lo veta el checker,
    // pero un struct/enum que CONTENGA un Map (p. ej. `Json.JObject`) sí se renderiza recursivamente.
    out.push_str("impl<K: RayShow + std::hash::Hash + Eq, V: RayShow> RayShow for Rc<std::cell::RefCell<__RayMap<K, V>>> { fn ray_show(&self) -> String { let __rt_m = self.borrow(); let mut __parts: Vec<String> = __rt_m.iter().map(|(__k, __rt_v)| format!(\"{}: {}\", __k.ray_show(), __rt_v.ray_show())).collect(); __parts.sort(); format!(\"Map{{{}}}\", __parts.join(\", \")) } }\n");
    out.push_str("impl<T: RayShow> RayShow for Option<T> { fn ray_show(&self) -> String { match self { Some(__rt_v) => format!(\"Option.Some({})\", __rt_v.ray_show()), None => \"Option.None\".to_string() } } }\n");
    out.push_str("impl<T: RayShow, E: RayShow> RayShow for Result<T, E> { fn ray_show(&self) -> String { match self { Ok(__rt_v) => format!(\"Result.Ok({})\", __rt_v.ray_show()), Err(__e) => format!(\"Result.Err({})\", __e.ray_show()) } } }\n");
    // Tuplas (2 y 3 elementos): `(a, b)`. El checker no deja `print`ar una tupla, así que esto rara vez
    // se llama; hace falta para satisfacer el bound `T: RayShow` de un `Iter<(k, v)>` (los adaptadores
    // `enumerate`/`zip` generados por el trait Iterator, aun cuando queden como stubs).
    out.push_str("impl<A: RayShow, B: RayShow> RayShow for (A, B) { fn ray_show(&self) -> String { format!(\"({}, {})\", self.0.ray_show(), self.1.ray_show()) } }\n");
    out.push_str("impl<A: RayShow, B: RayShow, C: RayShow> RayShow for (A, B, C) { fn ray_show(&self) -> String { format!(\"({}, {}, {})\", self.0.ray_show(), self.1.ray_show(), self.2.ray_show()) } }\n");
    // bytes → hex minúsculas sin separador ({:02x} por octeto), como la VM (bytes_to_hex).
    out.push_str("impl RayShow for Rc<[u8]> { fn ray_show(&self) -> String { let mut __rt_s = String::with_capacity(self.len() * 2); for __rt_b in self.iter() { __rt_s.push_str(&format!(\"{:02x}\", __rt_b)); } __rt_s } }\n\n");
}

pub(super) fn emit_runtime_features(out: &mut String, t: &mut Transpiler) {
    // TLS reusa el registro de handles + `TcpStream` (accept/upgrade parten de un handle TCP) →
    // implica net. La normalización va ANTES que nada: el ctx de fibras (justo debajo) elige sus
    // campos por estas flags, y con la implicación tardía un programa solo-TLS emitía un ctx sin
    // `socks`/`rd_to` que el close sí referenciaba (E0609, cazado por build_native_tls_connection).
    if t.needs_rt_tls {
        t.needs_net = true;
    }
    // M131: normalización Unicode — mismo código que la VM (ray_runtime::unicode). El Err (forma
    // desconocida; imposible desde los wrappers de std/text) aborta byte-idéntico a la VM.
    if t.needs_rt_unicode {
        out.push_str("fn __ray_unicode_normalize(s: &str, form: &str) -> Rc<str> { match ray_runtime::unicode::normalize(s, form) { Ok(o) => Rc::<str>::from(o), Err(e) => __ray_rt_err(&e) } }\n");
    }
    // F2 (--fibers): el CONTEXTO POR-TAREA que en el modelo de hilos eran thread-locals
    // (cancelación, pila de scopes, profundidad de try_call, caché de sockets y sus timeouts).
    // Con fibras que pueden reanudarse en otro worker, ese estado viaja EN LA FIBRA: vive en el
    // slot fiber-local de ray_runtime::fibers (Box<dyn Any> por Task); el hilo `main` — que no es
    // una fibra — cae al thread-local de respaldo. Los campos se recortan a lo que el programa usa.
    if t.fibers {
        let mut fields = String::from(" in_try: u32,");
        let mut init = String::from(" in_try: 0,");
        if t.needs_concurrency {
            fields.push_str(" cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>, scopes: Vec<Vec<std::boxed::Box<dyn __RayScopeChild>>>,");
            init.push_str(" cancel: None, scopes: Vec::new(),");
        }
        if t.needs_net {
            fields.push_str(" socks: std::collections::HashMap<i64, std::sync::Arc<std::net::TcpStream>>, rd_to: std::collections::HashMap<i64, i64>,");
            init.push_str(" socks: std::collections::HashMap::new(), rd_to: std::collections::HashMap::new(),");
        }
        if t.needs_rt_tls {
            // F4: la caché TLS también viaja con la fibra (mismo motivo que socks: una entrada en
            // el thread-local de un worker ajeno retendría la sesión viva tras el close), y el
            // búfer de lectura TLS es del ctx — se SACA con mem::take antes de leer (la lectura
            // TLS aparca dentro; el préstamo de __RAY_RDBUF no puede cruzar la cesión).
            fields.push_str(" tls: std::collections::HashMap<i64, Option<std::sync::Arc<std::sync::Mutex<ray_runtime::tls::TlsStream>>>>, tls_buf: Vec<u8>,");
            init.push_str(" tls: std::collections::HashMap::new(), tls_buf: Vec::new(),");
        }
        write!(out, "struct __FiberCtx {{{fields} }}\n").unwrap();
        write!(out, "fn __ray_ctx_new() -> __FiberCtx {{ __FiberCtx {{{init} }} }}\n").unwrap();
        out.push_str(concat!(
            "thread_local! { static __RAY_MAIN_CTX: std::cell::RefCell<__FiberCtx> = std::cell::RefCell::new(__ray_ctx_new()); }\n",
            // CONTRATO (de fibers::with_local): `f` no aparca la fibra ni anida otro __ray_ctx —
            // los accesos son hojas (leer un flag, push/pop, tocar un HashMap).
            "fn __ray_ctx<R>(f: impl FnOnce(&mut __FiberCtx) -> R) -> R {\n",
            "    let mut f = Some(f);\n",
            "    if let Some(r) = ray_runtime::fibers::with_local(|slot| {\n",
            "        if slot.is_none() { *slot = Some(std::boxed::Box::new(__ray_ctx_new())); }\n",
            "        (f.take().unwrap())(slot.as_mut().unwrap().downcast_mut::<__FiberCtx>().expect(\"fiber slot holds __FiberCtx\"))\n",
            "    }) { return r; }\n",
            "    __RAY_MAIN_CTX.with(|c| (f.take().unwrap())(&mut c.borrow_mut()))\n",
            "}\n",
        ));
    }
    // Registro global de handles de archivo (M11.8), solo si el programa los usa. Rust permite items
    // top-level en cualquier orden, así que va al final. Espejo del `FileRegistry` de la VM: un contador +
    // mapa handle→archivo tras un Mutex/OnceLock; los mensajes de error son byte-idénticos a la VM.
    // Registro de handles (M11.8): compartido por archivos y sockets. Se emite si el programa usa cualquiera.
    if t.needs_handles
        || t.needs_net
        || t.needs_rt_sqlite
        || t.needs_rt_process
        || t.needs_rt_watch
        || t.needs_rt_ui
    {
        // Variantes con-crate del registro, añadidas solo si el programa usa el subsistema: `Tls` (conexión
        // TLS bloqueante tras `Arc<Mutex>` propio → el I/O no retiene el lock global) y `Sqlite` (conexión
        // rusqlite; I/O local → se opera reteniendo el lock global, como la VM).
        let tls_variant = if t.needs_rt_tls {
            ", Tls(std::sync::Arc<std::sync::Mutex<ray_runtime::tls::TlsStream>>)"
        } else {
            ""
        };
        let sqlite_variant = if t.needs_rt_sqlite { ", Sqlite(ray_runtime::sqlite::Conn)" } else { "" };
        // M100 v2: los handles del streaming de procesos — el pipe de lectura (Arc: se clona para
        // leer FUERA del lock, como Tcp) y el Child (vive aquí, no un pid crudo: try_wait que
        // cosecha lo elimina bajo el lock → kill posterior es no-op, jamás a un pid reusado).
        let process_variant = if t.needs_rt_process {
            ", Pipe(std::sync::Arc<std::fs::File>), PipeW(std::sync::Arc<std::fs::File>), Child(std::process::Child)"
        } else {
            ""
        };
        // M115.4: watch de fs vivo (su Drop detiene los hilos de notify; close(h) basta).
        let watch_variant = if t.needs_rt_watch { ", Watch(ray_runtime::watch::FsWatcher)" } else { "" };
        // M146: una ventana de std/ui — el id ES el del registro (ray_runtime la mapea por él);
        // el cierre real lo hace __ray_close (despacho asíncrono al hilo principal).
        let ui_variant = if t.needs_rt_ui { ", Window(i64)" } else { "" };
        writeln!(
            out,
            "enum __RayHandle {{ Reader(std::io::BufReader<std::fs::File>), Writer(std::fs::File), Tcp(std::sync::Arc<std::net::TcpStream>), Listener(std::net::TcpListener), Udp(std::net::UdpSocket){tls_variant}{sqlite_variant}{process_variant}{watch_variant}{ui_variant} }}"
        )
        .unwrap();
        out.push_str(concat!(
            "struct __RayReg { next: i64, open: __RayMap<i64, __RayHandle> }\n",
            "fn __ray_reg() -> &'static std::sync::Mutex<__RayReg> {\n",
            "    static R: std::sync::OnceLock<std::sync::Mutex<__RayReg>> = std::sync::OnceLock::new();\n",
            "    R.get_or_init(|| std::sync::Mutex::new(__RayReg { next: 1, open: __RayMap::default() }))\n}\n",
            "fn __ray_reg_insert(h: __RayHandle) -> i64 { let mut reg = __ray_reg().lock().unwrap(); let id = reg.next; reg.next += 1; reg.open.insert(id, h); id }\n",
        ));
        // M96c/M96g: `close` corre en el mismo hilo dueño de la conexión (fin de `handle_http`) →
        // borra también la(s) entrada(s) de ESE hilo en las cachés de socket/TLS, para que un
        // worker del pool reusado miles de veces (M96) no acumule handles muertos indefinidamente.
        let sock_evict = if t.needs_net && t.fibers {
            // F2: evict del ctx de ESTA fibra (la dueña de la conexión) + poke al reactor — si otra
            // fibra estuviera aparcada en el fd (el caso real: el accept sobre el listener cerrado
            // en el apagado), el re-registro del siguiente ciclo lo devuelve como error/listo.
            "__ray_ctx(|c| { c.socks.remove(&h); c.rd_to.remove(&h); }); ray_runtime::fibers::poke(); "
        } else if t.needs_net {
            "__RAY_SOCK_CACHE.with(|c| { c.borrow_mut().remove(&h); }); "
        } else {
            ""
        };
        let tls_evict = if t.needs_rt_tls && t.fibers {
            "__ray_ctx(|c| { c.tls.remove(&h); }); "
        } else if t.needs_rt_tls {
            "__RAY_TLS_CACHE.with(|c| { c.borrow_mut().remove(&h); }); "
        } else {
            ""
        };
        // IDEAS §64: si el handle era un socket TCP, `shutdown(Both)` tras sacarlo del registro —
        // otra fibra puede retener su propio Arc del stream (clonado en su ctx o vivo dentro de un
        // read aparcado), y sin el shutdown el close era un no-op silencioso: ni FIN al peer ni
        // despertar del lector. Con él, el fd queda legible (EOF) → el reactor despierta al lector,
        // que re-verifica el registro y devuelve Err("invalid handle: h"), como la VM. Las entradas
        // viejas en cachés de OTRAS fibras quedan inertes (los ids nunca se reasignan).
        // M146: cerrar el handle de una ventana cierra la VENTANA (despacho asíncrono al hilo
        // principal dentro de close_window — este close puede correr en cualquier hilo).
        let ui_close = if t.needs_rt_ui {
            "if let Some(__RayHandle::Window(w)) = &__e { ray_runtime::ui::close_window(*w); } "
        } else {
            ""
        };
        write!(out, "fn __ray_close(h: i64) -> i64 {{ let __e = __ray_reg().lock().unwrap().open.remove(&h); if let Some(__RayHandle::Tcp(s)) = &__e {{ let _ = s.shutdown(std::net::Shutdown::Both); }} {ui_close}{sock_evict}{tls_evict}0 }}\n").unwrap();
    }
    // Ops de archivo (open/read_line/write) — solo si se usan handles de archivo.
    if t.needs_handles {
        out.push_str(concat!(
            "fn __ray_open(path: &str, mode: &str) -> Result<i64, Rc<str>> {\n",
            "    let h = match mode {\n",
            "        \"r\" => std::fs::File::open(path).map(|f| __RayHandle::Reader(std::io::BufReader::new(f))),\n",
            "        \"w\" => std::fs::File::create(path).map(__RayHandle::Writer),\n",
            "        \"a\" => std::fs::OpenOptions::new().create(true).append(true).open(path).map(__RayHandle::Writer),\n",
            "        _ => return Err(Rc::<str>::from(format!(\"invalid open mode: '{}' (use \\\"r\\\", \\\"w\\\" or \\\"a\\\")\", mode))),\n",
            "    }.map_err(|e| Rc::<str>::from(e.to_string()))?;\n",
            "    Ok(__ray_reg_insert(h))\n}\n",
            "fn __ray_read_line(h: i64) -> Option<Rc<str>> {\n",
            "    use std::io::BufRead; let mut reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get_mut(&h) {\n",
            "        Some(__RayHandle::Reader(r)) => { let mut line = String::new(); match r.read_line(&mut line) {\n",
            "            Ok(0) | Err(_) => None, Ok(_) => Some(Rc::<str>::from(line.trim_end_matches(['\\n', '\\r']))) } }\n",
            "        _ => None } }\n",
            // M113: lectura por trozos + seek (espejo de `builtins::read_bytes_handle`/`seek_handle`;
            // mismos mensajes de error que la VM). `take` + `read_to_end`: memoria = lo leído.
            "fn __ray_read_bytes(h: i64, max: i64) -> Result<Option<Rc<[u8]>>, Rc<str>> {\n",
            "    use std::io::Read; if max <= 0 { return Err(Rc::<str>::from(\"read_bytes expects max > 0\")); }\n",
            "    let mut reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get_mut(&h) {\n",
            "        Some(__RayHandle::Reader(r)) => { let mut buf = Vec::new(); match (&mut *r).take(max as u64).read_to_end(&mut buf) {\n",
            "            Ok(0) => Ok(None), Ok(_) => Ok(Some(Rc::<[u8]>::from(buf))), Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
            "        Some(_) => Err(Rc::<str>::from(\"the handle is not a file open for reading\")),\n",
            "        None => Err(Rc::<str>::from(format!(\"invalid file handle: {}\", h))) } }\n",
            "fn __ray_seek(h: i64, pos: i64) -> Result<i64, Rc<str>> {\n",
            "    use std::io::Seek; if pos < 0 { return Err(Rc::<str>::from(\"seek expects pos >= 0\")); }\n",
            "    let mut reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get_mut(&h) {\n",
            "        Some(__RayHandle::Reader(r)) => r.seek(std::io::SeekFrom::Start(pos as u64)).map(|p| p as i64).map_err(|e| Rc::<str>::from(e.to_string())),\n",
            "        Some(__RayHandle::Writer(f)) => f.seek(std::io::SeekFrom::Start(pos as u64)).map(|p| p as i64).map_err(|e| Rc::<str>::from(e.to_string())),\n",
            "        Some(_) => Err(Rc::<str>::from(\"the handle is not a file\")),\n",
            "        None => Err(Rc::<str>::from(format!(\"invalid file handle: {}\", h))) } }\n",
            "fn __ray_write(h: i64, s: &str) -> Result<i64, Rc<str>> {\n",
            "    use std::io::Write; let mut reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get_mut(&h) {\n",
            "        Some(__RayHandle::Writer(f)) => f.write_all(s.as_bytes()).map(|_| s.chars().count() as i64).map_err(|e| Rc::<str>::from(e.to_string())),\n",
            "        Some(__RayHandle::Reader(_)) => Err(Rc::<str>::from(\"the handle is open for reading, not writing\")),\n",
            "        _ => Err(Rc::<str>::from(format!(\"invalid file handle: {}\", h))) } }\n",
            // M115.1: gemelo binario de write + fsync (espejo de builtins::write_bytes_handle/
            // sync_handle — mismos mensajes que la VM, paridad byte-idéntica).
            "fn __ray_write_bytes(h: i64, data: &[u8]) -> Result<i64, Rc<str>> {\n",
            "    use std::io::Write; let mut reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get_mut(&h) {\n",
            "        Some(__RayHandle::Writer(f)) => f.write_all(data).map(|_| data.len() as i64).map_err(|e| Rc::<str>::from(e.to_string())),\n",
            "        Some(__RayHandle::Reader(_)) => Err(Rc::<str>::from(\"the handle is open for reading, not writing\")),\n",
            "        Some(_) => Err(Rc::<str>::from(\"the handle is not a file open for writing\")),\n",
            "        None => Err(Rc::<str>::from(format!(\"invalid file handle: {}\", h))) } }\n",
            "fn __ray_sync(h: i64) -> Result<i64, Rc<str>> {\n",
            "    let mut reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get_mut(&h) {\n",
            "        Some(__RayHandle::Writer(f)) => f.sync_all().map(|_| 0i64).map_err(|e| Rc::<str>::from(e.to_string())),\n",
            "        Some(__RayHandle::Reader(_)) => Err(Rc::<str>::from(\"the handle is open for reading, not writing\")),\n",
            "        Some(_) => Err(Rc::<str>::from(\"the handle is not a file open for writing\")),\n",
            "        None => Err(Rc::<str>::from(format!(\"invalid file handle: {}\", h))) } }\n",
            // M115.2: candado consultivo flock (espejo de builtins::try_lock_handle/unlock_handle).
            "fn __ray_try_lock_file(f: &std::fs::File) -> Result<bool, Rc<str>> {\n",
            "    match f.try_lock() { Ok(()) => Ok(true), Err(std::fs::TryLockError::WouldBlock) => Ok(false), Err(std::fs::TryLockError::Error(e)) => Err(Rc::<str>::from(e.to_string())) } }\n",
            "fn __ray_try_lock(h: i64) -> Result<bool, Rc<str>> {\n",
            "    let mut reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get_mut(&h) {\n",
            "        Some(__RayHandle::Writer(f)) => __ray_try_lock_file(f),\n",
            "        Some(__RayHandle::Reader(r)) => __ray_try_lock_file(r.get_ref()),\n",
            "        Some(_) => Err(Rc::<str>::from(\"the handle is not a file\")),\n",
            "        None => Err(Rc::<str>::from(format!(\"invalid file handle: {}\", h))) } }\n",
            "fn __ray_unlock(h: i64) -> Result<i64, Rc<str>> {\n",
            "    let mut reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get_mut(&h) {\n",
            "        Some(__RayHandle::Writer(f)) => f.unlock().map(|_| 0i64).map_err(|e| Rc::<str>::from(e.to_string())),\n",
            "        Some(__RayHandle::Reader(r)) => r.get_ref().unlock().map(|_| 0i64).map_err(|e| Rc::<str>::from(e.to_string())),\n",
            "        Some(_) => Err(Rc::<str>::from(\"the handle is not a file\")),\n",
            "        None => Err(Rc::<str>::from(format!(\"invalid file handle: {}\", h))) } }\n",
        ));
    }
    // M115.3: primitivos de metadatos (stat sin seguir symlinks + chmod). Arreglo etiquetado
    // byte-idéntico a builtins::fs_tagged(FsOp::Stat)/chmod_path — los wrappers fs.stat/fs.chmod
    // se EMITEN (el struct Stat vive en raylang) y llaman aquí.
    if t.needs_fs_meta {
        out.push_str(concat!(
            "fn __ray_tagged(parts: Vec<String>) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    Rc::new(std::cell::RefCell::new(parts.into_iter().map(Rc::<str>::from).collect()))\n}\n",
            "fn __ray_stat_prim(path: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    __ray_tagged(match std::fs::symlink_metadata(path) {\n",
            "        Ok(md) => {\n",
            "            let ft = md.file_type();\n",
            "            let kind = if ft.is_symlink() { \"symlink\" } else if ft.is_dir() { \"dir\" } else if ft.is_file() { \"file\" } else { \"other\" };\n",
            "            #[cfg(unix)]\n",
            "            let mode = { use std::os::unix::fs::PermissionsExt; (md.permissions().mode() & 0o7777) as u64 };\n",
            "            #[cfg(not(unix))]\n",
            "            let mode = 0u64;\n",
            "            let mtime = match md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()) {\n",
            "                Some(d) => d.as_millis().to_string(), None => \"0\".to_string() };\n",
            "            vec![\"ok\".to_string(), kind.to_string(), mode.to_string(), md.len().to_string(), mtime]\n",
            "        }\n",
            "        Err(e) => vec![\"err\".to_string(), e.to_string()],\n",
            "    })\n}\n",
            "fn __ray_chmod_prim(path: &str, mode: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    #[cfg(unix)]\n",
            "    let r = { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(path, std::fs::Permissions::from_mode((mode as u32) & 0o7777)).map_err(|e| e.to_string()) };\n",
            "    #[cfg(not(unix))]\n",
            "    let r: Result<(), String> = { let _ = (path, mode); Err(\"chmod is not supported on this platform\".to_string()) };\n",
            "    __ray_tagged(match r { Ok(()) => vec![\"ok\".to_string()], Err(e) => vec![\"err\".to_string(), e] })\n}\n",
        ));
    }
    // M115.4: watch de fs por eventos de kernel (ray_runtime::watch, crate notify). El aparcado
    // con fibras va por wait_readable_timeout sobre el fd del self-pipe del watcher; sin fibras,
    // sondeo de la cola + poll(2) por tramos (nunca reteniendo el lock del registro). Al
    // despertar SIEMPRE se re-verifica el registro: un close concurrente → Err, no cuelgue
    // (la lección de M115-close).
    // M145: salida de audio PCM (ray_runtime::audio). El handle es PipeW: `__audio_write`
    // reusa __ray_proc_write (despacho + espera de escribible) y `close(h)` es el EOF.
    if t.needs_rt_audio {
        out.push_str(concat!(
            "fn __ray_audio_open(rate: i64, ch: i64, latency_ms: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    Rc::new(std::cell::RefCell::new(match ray_runtime::audio::open(rate, ch, latency_ms) {\n",
            "        Ok(f) => { let id = __ray_reg_insert(__RayHandle::PipeW(std::sync::Arc::new(f))); vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(id.to_string().as_str())] }\n",
            "        Err(e) => vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e.as_str())],\n",
            "    }))\n}\n",
            "fn __ray_audio_played(h: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let r = match __ray_stdin_clone(h) {\n",
            "        Some(f) => ray_runtime::audio::played_ms(std::os::fd::AsRawFd::as_raw_fd(&*f)),\n",
            "        None => Err(\"audio: not an open audio output\".to_string()),\n",
            "    };\n",
            "    Rc::new(std::cell::RefCell::new(match r { Ok(ms) => vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(ms.to_string().as_str())], Err(e) => vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e.as_str())] }))\n}\n",
            "fn __ray_audio_drain(h: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let r = match __ray_stdin_clone(h) {\n",
            "        Some(f) => ray_runtime::audio::drain(std::os::fd::AsRawFd::as_raw_fd(&*f)),\n",
            "        None => Err(\"audio: not an open audio output\".to_string()),\n",
            "    };\n",
            "    Rc::new(std::cell::RefCell::new(match r { Ok(()) => vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(\"\")], Err(e) => vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e.as_str())] }))\n}\n",
        ));
    }
    // M147: std/embed — la tabla de assets HORNEADA con include_bytes! (rutas absolutas del
    // build; sin copia, un solo mecanismo para el tier cargo y el rustc pelado) + helpers. Las
    // claves llegan ya ORDENADAS del walker compartido (builtins::embed_walk) → el listado es
    // byte-idéntico al de la VM. Tabla vacía (built sin --embed ni [native] embed) → el MISMO
    // mensaje de sin-config que la VM.
    if t.needs_embed {
        out.push_str("static __RAY_EMBED: &[(&str, &[u8])] = &[\n");
        for (key, path) in &t.embed {
            writeln!(out, "    ({key:?}, include_bytes!({path:?})),").unwrap();
        }
        out.push_str("];\n");
        out.push_str(concat!(
            "const __RAY_EMBED_NO_CONFIG: &str = \"embed: no embedded assets configured (add [native] embed = [\\\"assets\\\"] to ray.toml)\";\n",
            "fn __ray_embed_read(path: &str) -> Rc<std::cell::RefCell<Vec<Rc<[u8]>>>> {\n",
            "    let arr: Vec<Rc<[u8]>> = if __RAY_EMBED.is_empty() {\n",
            "        vec![Rc::<[u8]>::from(&b\"err\"[..]), Rc::<[u8]>::from(__RAY_EMBED_NO_CONFIG.as_bytes())]\n",
            "    } else { match __RAY_EMBED.iter().find(|(k, _)| *k == path) {\n",
            "        Some((_, d)) => vec![Rc::<[u8]>::from(&b\"ok\"[..]), Rc::<[u8]>::from(*d)],\n",
            "        None => vec![Rc::<[u8]>::from(&b\"err\"[..]), Rc::<[u8]>::from(format!(\"embed: no embedded file '{path}'\").into_bytes().as_slice())],\n",
            "    } };\n",
            "    Rc::new(std::cell::RefCell::new(arr))\n}\n",
            "fn __ray_embed_list() -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let arr: Vec<Rc<str>> = if __RAY_EMBED.is_empty() {\n",
            "        vec![Rc::<str>::from(\"err\"), Rc::<str>::from(__RAY_EMBED_NO_CONFIG)]\n",
            "    } else {\n",
            "        let mut v = vec![Rc::<str>::from(\"ok\")];\n",
            "        v.extend(__RAY_EMBED.iter().map(|(k, _)| Rc::<str>::from(*k)));\n",
            "        v\n",
            "    };\n",
            "    Rc::new(std::cell::RefCell::new(arr))\n}\n",
        ));
    }
    // M146: ventana + webview (ray_runtime::ui). El id del registro se reserva ANTES de abrir y
    // se pasa al runtime: los eventos nombran a la ventana con el handle del programa. La espera
    // de eventos: con fibras, aparcar por el fd del self-pipe de la cola global (dual-mode: el
    // hilo del programa no es fibra y bloquea en poll); sin fibras, la espera condvar del runtime.
    if t.needs_rt_ui {
        out.push_str(concat!(
            "fn __ray_ui_open(title: &str, url: &str, w: i64, h: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let id = { let mut reg = __ray_reg().lock().unwrap(); let id = reg.next; reg.next += 1; id };\n",
            "    Rc::new(std::cell::RefCell::new(match ray_runtime::ui::open_window(id, title, url, w, h) {\n",
            "        Ok(()) => { __ray_reg().lock().unwrap().open.insert(id, __RayHandle::Window(id)); vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(id.to_string().as_str())] }\n",
            "        Err(e) => vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e.as_str())],\n",
            "    }))\n}\n",
            "fn __ray_ui_eval_js(h: i64, js: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let known = matches!(__ray_reg().lock().unwrap().open.get(&h), Some(__RayHandle::Window(_)));\n",
            "    let r = if known { ray_runtime::ui::eval_js(h, js) } else { Err(\"ui: not an open window\".to_string()) };\n",
            "    Rc::new(std::cell::RefCell::new(match r { Ok(()) => vec![Rc::<str>::from(\"ok\")], Err(e) => vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e.as_str())] }))\n}\n",
            "fn __ray_ui_menu(title: &str, items: &Rc<std::cell::RefCell<Vec<Rc<str>>>>) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let its: Vec<String> = items.borrow().iter().map(|s| s.to_string()).collect();\n",
            "    Rc::new(std::cell::RefCell::new(match ray_runtime::ui::menu(title, &its) {\n",
            "        Ok(()) => vec![Rc::<str>::from(\"ok\")],\n",
            "        Err(e) => vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e.as_str())],\n",
            "    }))\n}\n",
            "fn __ray_ui_app_menu(name: &str, items: &Rc<std::cell::RefCell<Vec<Rc<str>>>>) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let its: Vec<String> = items.borrow().iter().map(|s| s.to_string()).collect();\n",
            "    Rc::new(std::cell::RefCell::new(match ray_runtime::ui::app_menu(name, &its) {\n",
            "        Ok(()) => vec![Rc::<str>::from(\"ok\")],\n",
            "        Err(e) => vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e.as_str())],\n",
            "    }))\n}\n",
            "fn __ray_ui_set_about(name: &str, version: &str, description: &str, copyright: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    Rc::new(std::cell::RefCell::new(match ray_runtime::ui::set_about(name, version, description, copyright) {\n",
            "        Ok(()) => vec![Rc::<str>::from(\"ok\")],\n",
            "        Err(e) => vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e.as_str())],\n",
            "    }))\n}\n",
            "fn __ray_ui_dialog(kind: &str, arg: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    Rc::new(std::cell::RefCell::new(match ray_runtime::ui::dialog(kind, arg) {\n",
            "        Ok(Some(path)) => vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(path.as_str())],\n",
            "        Ok(None) => vec![Rc::<str>::from(\"none\")],\n",
            "        Err(e) => vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e.as_str())],\n",
            "    }))\n}\n",
                        "fn __ray_ui_next_event(ms: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let tag = |parts: Vec<String>| Rc::new(std::cell::RefCell::new(parts.into_iter().map(Rc::<str>::from).collect::<Vec<Rc<str>>>()));\n",
        ));
        if t.fibers {
            out.push_str(concat!(
                "    let dl = if ms > 0 { Some(std::time::Instant::now() + std::time::Duration::from_millis(ms as u64)) } else { None };\n",
                "    loop {\n",
                "        if let Some((kind, win, mtag)) = ray_runtime::ui::try_next_event() { return tag(vec![\"ok\".to_string(), kind, win.to_string(), mtag]); }\n",
                "        let fd = ray_runtime::ui::event_fd();\n",
                "        if fd < 0 { return tag(vec![\"err\".to_string(), \"ui: no event pipe\".to_string()]); }\n",
                "        let rem = match dl {\n",
                "            None => 0,\n",
                "            Some(d) => { let r = d.saturating_duration_since(std::time::Instant::now()).as_millis() as i64; if r <= 0 { return tag(vec![\"timeout\".to_string()]); } r }\n",
                "        };\n",
                "        if ray_runtime::fibers::wait_readable_timeout(fd, rem) && rem > 0 { return tag(vec![\"timeout\".to_string()]); }\n",
                "    }\n}\n",
            ));
        } else {
            out.push_str(concat!(
                "    match ray_runtime::ui::next_event_blocking(ms) {\n",
                "        Some((kind, win, mtag)) => tag(vec![\"ok\".to_string(), kind, win.to_string(), mtag]),\n",
                "        None => tag(vec![\"timeout\".to_string()]),\n",
                "    }\n}\n",
            ));
        }
    }
    if t.needs_rt_watch {
        out.push_str(concat!(
            "fn __ray_watch(path: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    Rc::new(std::cell::RefCell::new(match ray_runtime::watch::watch(path) {\n",
            "        Ok(w) => { let id = __ray_reg_insert(__RayHandle::Watch(w)); vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(id.to_string())] }\n",
            "        Err(e) => vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e)],\n",
            "    }))\n",
            "}\n",
            "fn __ray_watch_next(h: i64, ms: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let dl = if ms > 0 { Some(std::time::Instant::now() + std::time::Duration::from_millis(ms as u64)) } else { None };\n",
            "    let tag = |parts: Vec<String>| Rc::new(std::cell::RefCell::new(parts.into_iter().map(Rc::<str>::from).collect::<Vec<Rc<str>>>()));\n",
            "    loop {\n",
            "        let fd = { let mut reg = __ray_reg().lock().unwrap();\n",
            "            match reg.open.get_mut(&h) {\n",
            "                Some(__RayHandle::Watch(w)) => match w.try_next() {\n",
            "                    Some((kind, path)) => return tag(vec![\"ok\".to_string(), kind, path]),\n",
            "                    None => w.fd(),\n",
            "                },\n",
            "                Some(_) => return tag(vec![\"err\".to_string(), \"the handle is not a watch handle\".to_string()]),\n",
            "                None => return tag(vec![\"err\".to_string(), format!(\"invalid handle: {}\", h)]),\n",
            "            } };\n",
            "        let rem = match dl {\n",
            "            None => 0,\n",
            "            Some(d) => { let r = d.saturating_duration_since(std::time::Instant::now()).as_millis() as i64; if r <= 0 { return tag(vec![\"timeout\".to_string()]); } r }\n",
            "        };\n",
        ));
        if t.fibers {
            out.push_str(concat!(
                "        if ray_runtime::fibers::wait_readable_timeout(fd, rem) && rem > 0 { return tag(vec![\"timeout\".to_string()]); }\n",
            ));
        } else {
            out.push_str(concat!(
                "        let step = if rem == 0 { 200 } else { rem.min(200) as i32 };\n",
                "        ray_runtime::watch::fd_ready(fd, step);\n",
            ));
        }
        out.push_str("    }\n}\n");
    }
    // Ops de socket TCP — solo si se usa la red. Clonan el stream para no retener el lock en la I/O
    // bloqueante (como la VM). read lee ≤64KiB (lossy UTF-8; EOF → ""); write escribe todo (Ok(nº bytes)).
    if t.needs_net {
        out.push_str(concat!(
            // M96c: caché thread-local del Arc<TcpStream> por handle. Una conexión aceptada la
            // maneja SIEMPRE el mismo hilo durante toda su vida (`handle_http` corre en el hilo
            // dueño de la conexión, keep-alive incluido — el pool M96 solo reusa hilos ENTRE
            // conexiones distintas, nunca concurrentemente dentro de una); así que el primer
            // acceso paga el lock global (como antes) y los siguientes de ESA conexión, en ESE
            // hilo, no lo tocan más. Sonoro: el Arc cacheado nunca cruza de hilo (vive en un
            // `thread_local!`), así que clonarlo no es una carrera en su contador de referencias.
            // Los ids del registro NUNCA se reasignan (`reg.next` solo crece) → una entrada
            // vieja en la caché tras un `close` es inerte, no ambigua; igual se borra en
            // `__ray_close` para no crecer sin límite en un hilo del pool reusado miles de veces.
        ));
        // F2 (--fibers): la caché de sockets viaja en el CTX de la fibra, no en el hilo — una
        // conexión keep-alive puede migrar de worker entre peticiones, y una entrada retenida en
        // el thread-local de un worker ajeno mantendría el fd VIVO tras el close (el peer no vería
        // cerrar la conexión). El ctx muere con la fibra → los Arc caen con ella.
        if t.fibers {
            out.push_str(concat!(
                "fn __ray_sock_clone(h: i64) -> Result<std::sync::Arc<std::net::TcpStream>, Rc<str>> {\n",
                "    if let Some(s) = __ray_ctx(|c| c.socks.get(&h).cloned()) { return Ok(s); }\n",
                "    let reg = __ray_reg().lock().unwrap();\n",
                "    match reg.open.get(&h) { Some(__RayHandle::Tcp(s)) => { let s = std::sync::Arc::clone(s); drop(reg);\n",
                "            __ray_ctx(|c| { c.socks.insert(h, std::sync::Arc::clone(&s)); }); Ok(s) },\n",
                "        Some(_) => Err(Rc::<str>::from(format!(\"handle {} is not a socket\", h))), None => Err(Rc::<str>::from(format!(\"invalid handle: {}\", h))) } }\n",
                // connect sigue BLOQUEANTE (acotado por el SO; el connect no-bloqueante con
                // park_writable + SO_ERROR es de F4/cliente); el stream queda no-bloqueante para
                // que las lecturas/escrituras posteriores aparquen la fibra.
                "fn __ray_tcp_connect(host: &str, port: i64) -> Result<i64, Rc<str>> {\n",
                "    match std::net::TcpStream::connect((host, port as u16)) { Ok(s) => { let _ = s.set_nodelay(true); let _ = s.set_nonblocking(true); Ok(__ray_reg_insert(__RayHandle::Tcp(std::sync::Arc::new(s)))) }, Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
                // M122: connect con PLAZO — espera acotada pero bloqueante (connect_timeout del std);
                // el intento vencido devuelve el error estable "connect timeout".
                "fn __ray_tcp_connect_timeout(host: &str, port: i64, ms: i64) -> Result<i64, Rc<str>> {\n",
                "    if ms <= 0 { return __ray_tcp_connect(host, port); }\n",
                "    use std::net::ToSocketAddrs;\n",
                "    let addr = (host, port as u16).to_socket_addrs().map_err(|e| Rc::<str>::from(e.to_string()))?.next().ok_or_else(|| Rc::<str>::from(format!(\"could not resolve host '{}'\", host)))?;\n",
                "    match std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(ms as u64)) {\n",
                "        Ok(s) => { let _ = s.set_nodelay(true); let _ = s.set_nonblocking(true); Ok(__ray_reg_insert(__RayHandle::Tcp(std::sync::Arc::new(s)))) }\n",
                "        Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock => Err(Rc::<str>::from(\"connect timeout\")),\n",
                "        Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
                "fn __ray_tcp_listen(host: &str, port: i64) -> Result<i64, Rc<str>> {\n",
                "    match std::net::TcpListener::bind((host, port as u16)) { Ok(l) => { let _ = l.set_nonblocking(true); Ok(__ray_reg_insert(__RayHandle::Listener(l))) }, Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
                // El accept RE-RESUELVE el handle en cada vuelta (como la VM): así el `close` del
                // listener en el apagado ordenado lo despierta (poke) y la siguiente vuelta ve el
                // handle ausente → error, no un accept eterno sobre un dup vivo. Se aparca sobre
                // el fd del REGISTRO (estable hasta el close), no el del clon (muere con el drop).
                "fn __ray_tcp_accept(h: i64) -> Result<i64, Rc<str>> {\n",
                "    loop {\n",
                "        let (l, fd) = { let reg = __ray_reg().lock().unwrap(); match reg.open.get(&h) { Some(__RayHandle::Listener(l)) => (l.try_clone().map_err(|e| Rc::<str>::from(e.to_string()))?, std::os::fd::AsRawFd::as_raw_fd(l)), _ => return Err(Rc::<str>::from(format!(\"handle {} is not a listener\", h))) } };\n",
                "        match l.accept() {\n",
                "            Ok((s, _)) => { let _ = s.set_nonblocking(true); let _ = s.set_nodelay(true); return Ok(__ray_reg_insert(__RayHandle::Tcp(std::sync::Arc::new(s)))); }\n",
                "            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { drop(l); ray_runtime::fibers::wait_readable(fd); }\n",
                "            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}\n",
                "            Err(e) => return Err(Rc::<str>::from(e.to_string())),\n",
                "        }\n",
                "    }\n}\n",
            ));
        } else {
            out.push_str(concat!(
                "thread_local! { static __RAY_SOCK_CACHE: std::cell::RefCell<std::collections::HashMap<i64, std::sync::Arc<std::net::TcpStream>>> = std::cell::RefCell::new(std::collections::HashMap::new()); }\n",
                "fn __ray_sock_clone(h: i64) -> Result<std::sync::Arc<std::net::TcpStream>, Rc<str>> {\n",
                "    if let Some(s) = __RAY_SOCK_CACHE.with(|c| c.borrow().get(&h).cloned()) { return Ok(s); }\n",
                "    let reg = __ray_reg().lock().unwrap();\n",
                "    match reg.open.get(&h) { Some(__RayHandle::Tcp(s)) => { let s = std::sync::Arc::clone(s); drop(reg);\n",
                "            __RAY_SOCK_CACHE.with(|c| { c.borrow_mut().insert(h, std::sync::Arc::clone(&s)); }); Ok(s) },\n",
                "        Some(_) => Err(Rc::<str>::from(format!(\"handle {} is not a socket\", h))), None => Err(Rc::<str>::from(format!(\"invalid handle: {}\", h))) } }\n",
                "fn __ray_tcp_connect(host: &str, port: i64) -> Result<i64, Rc<str>> {\n",
                "    match std::net::TcpStream::connect((host, port as u16)) { Ok(s) => { let _ = s.set_nodelay(true); Ok(__ray_reg_insert(__RayHandle::Tcp(std::sync::Arc::new(s)))) }, Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
                // M122: connect con PLAZO (hilo-por-tarea: bloquea solo su hilo, acotado a ms).
                "fn __ray_tcp_connect_timeout(host: &str, port: i64, ms: i64) -> Result<i64, Rc<str>> {\n",
                "    if ms <= 0 { return __ray_tcp_connect(host, port); }\n",
                "    use std::net::ToSocketAddrs;\n",
                "    let addr = (host, port as u16).to_socket_addrs().map_err(|e| Rc::<str>::from(e.to_string()))?.next().ok_or_else(|| Rc::<str>::from(format!(\"could not resolve host '{}'\", host)))?;\n",
                "    match std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(ms as u64)) {\n",
                "        Ok(s) => { let _ = s.set_nodelay(true); Ok(__ray_reg_insert(__RayHandle::Tcp(std::sync::Arc::new(s)))) }\n",
                "        Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock => Err(Rc::<str>::from(\"connect timeout\")),\n",
                "        Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
                "fn __ray_tcp_listen(host: &str, port: i64) -> Result<i64, Rc<str>> {\n",
                "    match std::net::TcpListener::bind((host, port as u16)) { Ok(l) => Ok(__ray_reg_insert(__RayHandle::Listener(l))), Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
                "fn __ray_tcp_accept(h: i64) -> Result<i64, Rc<str>> {\n",
                "    let l = { let reg = __ray_reg().lock().unwrap(); match reg.open.get(&h) { Some(__RayHandle::Listener(l)) => l.try_clone().map_err(|e| Rc::<str>::from(e.to_string())), _ => return Err(Rc::<str>::from(format!(\"handle {} is not a listener\", h))) } }?;\n",
                "    match l.accept() { Ok((s, _)) => { let _ = s.set_nodelay(true); Ok(__ray_reg_insert(__RayHandle::Tcp(std::sync::Arc::new(s)))) }, Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
            ));
        }

        // socket_read/read_bytes/write DESPACHAN a TLS si el handle es una conexión TLS (solo si el
        // programa usa TLS): se clona el `Arc<Mutex<TlsStream>>` del registro y se hace I/O tras SU lock
        // (no el global) → conexiones concurrentes no se serializan. Si no, la vía TCP de siempre (clona el
        // stream para no retener el lock durante la I/O bloqueante).
        // Como la VM: SOLO las variantes `_bytes` despachan a TLS (socket_read/write string dan el error de
        // no-socket sobre un handle TLS). read tiene helper propio (matchea la VM: sin TLS); el `write`
        // compartido cubre write_bytes (el uso real de TLS) → lleva el despacho.
        let (tls_rdb, tls_wr) = if t.needs_rt_tls && t.fibers {
            // F4: la lectura TLS APARCA LA FIBRA dentro de read_wait (dirección por wants_write:
            // el handshake alterna). El búfer sale del ctx con mem::take (nada de __RAY_RDBUF: su
            // préstamo no puede cruzar la cesión — otra fibra del MISMO worker lo pediría y
            // reventaría el RefCell). El MutexGuard de la sesión SÍ cruza el park: es sólido
            // porque la fibra está FIJADA a su worker (el guard nunca cambia de hilo) y la sesión
            // es fiber-privada (solo su dueña la toca). Timeout M56.4 desde el ctx (rd_to);
            // vencido → "read timeout", byte-idéntico a la VM.
            (
                "if let Some(__t) = __ray_tls_get(h) { let to = __ray_ctx(|c| c.rd_to.get(&h).copied().unwrap_or(0)); let mut buf = __ray_ctx(|c| std::mem::take(&mut c.tls_buf)); if buf.len() < 65536 { buf.resize(65536, 0); } let mut __g = __t.lock().unwrap(); let r = __g.read_wait(&mut buf[..], to); drop(__g); let out = match r { Ok(n) => Ok(Rc::<[u8]>::from(&buf[..n])), Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Err(Rc::<str>::from(\"read timeout\")), Err(e) => Err(Rc::<str>::from(e.to_string())) }; __ray_ctx(|c| c.tls_buf = buf); return out; } ",
                "if let Some(__t) = __ray_tls_get(h) { let mut __g = __t.lock().unwrap(); return match __g.write_all_wait(bytes) { Ok(()) => Ok(bytes.len() as i64), Err(e) => Err(Rc::<str>::from(e.to_string())) }; } ",
            )
        } else if t.needs_rt_tls {
            (
                "if let Some(__t) = __ray_tls_get(h) { let mut __g = __t.lock().unwrap(); return __RAY_RDBUF.with(|__b| { let mut buf = __b.borrow_mut(); match __g.read(&mut buf[..]) { Ok(n) => Ok(Rc::<[u8]>::from(&buf[..n])), Err(e) => Err(Rc::<str>::from(e.to_string())) } }); } ",
                "if let Some(__t) = __ray_tls_get(h) { let mut __g = __t.lock().unwrap(); return match __g.write_all(bytes) { Ok(()) => Ok(bytes.len() as i64), Err(e) => Err(Rc::<str>::from(e.to_string())) }; } ",
            )
        } else {
            ("", "")
        };
        // Búfer de lectura por HILO, no por llamada. Antes era `let mut buf = [0u8; 65536]` en la
        // pila, y costaba por partida doble: (1) Rust lo inicializa a cero, así que cada hilo TOCABA
        // las 16 páginas enteras en su primer `read` → residentes para siempre (~45 KB/conexión
        // medidos: 59→50 MB de RSS a -c 200); (2) hundía la pila 64 KiB, lo que fijaba un suelo de
        // ~56 KiB de pila por conexión. Con el búfer fuera, el servidor sobrevive con la pila mínima
        // que macOS concede (pedida de 8 KiB, concedida de 28; la pila realmente tocada queda acotada
        // en 4-12 KiB por la bisección), que es el dato que faltaba para dimensionar corrutinas de
        // pila propia (docs/diseno-concurrencia-nativa.md §3c).
        // No es reentrante a propósito: no hay `socket_read` anidado dentro de otro en el mismo hilo.
        out.push_str("thread_local! { static __RAY_RDBUF: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(vec![0u8; 65536]); }\n");
        // IDEAS §64: ¿sigue el handle en el registro? Lo consultan las lecturas SOLO en EOF y al
        // despertar de un park (nunca en el camino caliente con datos): un close cross-fibra ya
        // hizo shutdown del fd (ver __ray_close) y este re-chequeo convierte ese despertar en
        // Err("invalid handle: h") — byte-idéntico a la VM — en vez de un Ok("") ambiguo o un
        // re-park eterno.
        out.push_str("fn __ray_handle_open(h: i64) -> bool { __ray_reg().lock().unwrap().open.contains_key(&h) }\n");
        if t.fibers {
            // F2: lectura no-bloqueante que APARCA LA FIBRA en WouldBlock. El intento va dentro del
            // préstamo de __RAY_RDBUF y el park FUERA (una fibra puede reanudar en otro worker; el
            // préstamo no debe cruzar la cesión). El timeout de lectura (M56.4) vive aquí: en un
            // socket no-bloqueante SO_RCVTIMEO es inerte → plazo total por lectura contra el ctx
            // (rd_to), vencimiento = Err("read timeout"), byte-idéntico a la VM (READ_TIMEOUT_MSG).
            // EINTR también aparca: si había datos, el readiness dispara de inmediato.
            // §64: el intento de lectura devuelve además `n` — en EOF (n == 0) y antes de aparcar
            // se re-verifica el registro (__ray_handle_open): un close cross-fibra → Err("invalid
            // handle"), como la VM. El camino caliente (n > 0) no toca el lock global.
            let read_loop = |ok_expr: &str| {
                format!(
                    "    let fd = std::os::fd::AsRawFd::as_raw_fd(&*s);\n    let to = __ray_ctx(|c| c.rd_to.get(&h).copied().unwrap_or(0));\n    let dl = if to > 0 {{ Some(std::time::Instant::now() + std::time::Duration::from_millis(to as u64)) }} else {{ None }};\n    loop {{\n        let res = __RAY_RDBUF.with(|__b| {{ let mut buf = __b.borrow_mut(); match r.read(&mut buf[..]) {{\n            Ok(n) => Some((n, Ok({ok_expr}))),\n            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::Interrupted => None,\n            Err(e) => Some((1, Err(Rc::<str>::from(e.to_string())))) }} }});\n        if let Some((n, v)) = res {{\n            if n == 0 && !__ray_handle_open(h) {{ return Err(Rc::<str>::from(format!(\"invalid handle: {{}}\", h))); }}\n            return v;\n        }}\n        if !__ray_handle_open(h) {{ return Err(Rc::<str>::from(format!(\"invalid handle: {{}}\", h))); }}\n        let ms = match dl {{ None => 0, Some(d) => {{ let rem = d.saturating_duration_since(std::time::Instant::now()).as_millis() as i64; if rem <= 0 {{ return Err(Rc::<str>::from(\"read timeout\")); }} rem }} }};\n        if ray_runtime::fibers::wait_readable_timeout(fd, ms) {{ return Err(Rc::<str>::from(\"read timeout\")); }}\n    }}\n}}\n"
                )
            };
            write!(out, "fn __ray_socket_read(h: i64) -> Result<Rc<str>, Rc<str>> {{\n    use std::io::Read; let s = __ray_sock_clone(h)?; let mut r = &*s;\n{}", read_loop("Rc::<str>::from(String::from_utf8_lossy(&buf[..n]).into_owned())")).unwrap();
            write!(out, "fn __ray_socket_read_bytes(h: i64) -> Result<Rc<[u8]>, Rc<str>> {{\n    {tls_rdb}use std::io::Read; let s = __ray_sock_clone(h)?; let mut r = &*s;\n{}", read_loop("Rc::<[u8]>::from(&buf[..n])")).unwrap();
            write!(out, "fn __ray_socket_write(h: i64, bytes: &[u8]) -> Result<i64, Rc<str>> {{\n    {tls_wr}use std::io::Write; let s = __ray_sock_clone(h)?; let mut w = &*s;\n    let fd = std::os::fd::AsRawFd::as_raw_fd(&*s); let mut off = 0;\n    while off < bytes.len() {{ match w.write(&bytes[off..]) {{\n        Ok(0) => return Err(Rc::<str>::from(\"the connection closed during the write\")),\n        Ok(n) => off += n,\n        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => ray_runtime::fibers::wait_writable(fd),\n        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {{}}\n        Err(e) => return Err(Rc::<str>::from(e.to_string())) }} }}\n    Ok(bytes.len() as i64)\n}}\n").unwrap();
        } else {
            // §64 (también en hilo-por-tarea): en EOF se re-verifica el registro — un close desde
            // otra tarea hizo shutdown del fd (el read bloqueado despierta con Ok(0)) y debe
            // reportarse como Err("invalid handle"), no como un fin de stream normal.
            write!(out, "fn __ray_socket_read(h: i64) -> Result<Rc<str>, Rc<str>> {{ use std::io::Read; let s = __ray_sock_clone(h)?; let mut r = &*s; __RAY_RDBUF.with(|__b| {{ let mut buf = __b.borrow_mut(); match r.read(&mut buf[..]) {{ Ok(0) if !__ray_handle_open(h) => Err(Rc::<str>::from(format!(\"invalid handle: {{}}\", h))), Ok(n) => Ok(Rc::<str>::from(String::from_utf8_lossy(&buf[..n]).into_owned())), Err(e) => Err(Rc::<str>::from(e.to_string())) }} }}) }}\n").unwrap();
            write!(out, "fn __ray_socket_read_bytes(h: i64) -> Result<Rc<[u8]>, Rc<str>> {{ {tls_rdb}use std::io::Read; let s = __ray_sock_clone(h)?; let mut r = &*s; __RAY_RDBUF.with(|__b| {{ let mut buf = __b.borrow_mut(); match r.read(&mut buf[..]) {{ Ok(0) if !__ray_handle_open(h) => Err(Rc::<str>::from(format!(\"invalid handle: {{}}\", h))), Ok(n) => Ok(Rc::<[u8]>::from(&buf[..n])), Err(e) => Err(Rc::<str>::from(e.to_string())) }} }}) }}\n").unwrap();
            write!(out, "fn __ray_socket_write(h: i64, bytes: &[u8]) -> Result<i64, Rc<str>> {{ {tls_wr}use std::io::Write; let s = __ray_sock_clone(h)?; let mut w = &*s; let mut off = 0; while off < bytes.len() {{ match w.write(&bytes[off..]) {{ Ok(0) => return Err(Rc::<str>::from(\"the connection closed during the write\")), Ok(n) => off += n, Err(e) => return Err(Rc::<str>::from(e.to_string())) }} }} Ok(bytes.len() as i64) }}\n").unwrap();
        }
        out.push_str(concat!(
            "fn __ray_local_port(h: i64) -> i64 {\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get(&h) { Some(__RayHandle::Tcp(s)) => s.local_addr().map(|a| a.port() as i64).unwrap_or(0), Some(__RayHandle::Listener(l)) => l.local_addr().map(|a| a.port() as i64).unwrap_or(0), Some(__RayHandle::Udp(s)) => s.local_addr().map(|a| a.port() as i64).unwrap_or(0), _ => 0 } }\n",
            ));
        // M123: la dirección del peer de una conexión TCP/TLS (intercept a nivel de wrapper →
        // Result nativo). El brazo TLS solo se splicea si el programa usa TLS (needs_rt_tls),
        // igual que en socket_read_bytes.
        let tls_peer = if t.needs_rt_tls {
            "Some(__RayHandle::Tls(a)) => a.lock().unwrap().peer_addr().map(|p| Rc::<str>::from(p.to_string())).map_err(|e| Rc::<str>::from(e.to_string())), "
        } else {
            ""
        };
        // M130: half-close — shutdown(SHUT_WR) de una conexión TCP (mismos errores que la VM).
        out.push_str(concat!(
            "fn __ray_shutdown_write(h: i64) -> Result<i64, Rc<str>> {\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get(&h) { Some(__RayHandle::Tcp(s)) => s.shutdown(std::net::Shutdown::Write).map(|_| 0).map_err(|e| Rc::<str>::from(e.to_string())),\n",
            "        Some(_) => Err(Rc::<str>::from(format!(\"handle {} is not a TCP socket\", h))), None => Err(Rc::<str>::from(format!(\"invalid handle: {}\", h))) } }\n",
        ));
        write!(out, "fn __ray_peer_addr(h: i64) -> Result<Rc<str>, Rc<str>> {{\n    let reg = __ray_reg().lock().unwrap();\n    match reg.open.get(&h) {{ Some(__RayHandle::Tcp(s)) => s.peer_addr().map(|p| Rc::<str>::from(p.to_string())).map_err(|e| Rc::<str>::from(e.to_string())), {tls_peer}Some(_) => Err(Rc::<str>::from(format!(\"handle {{}} is not a TCP/TLS socket\", h))), None => Err(Rc::<str>::from(format!(\"invalid handle: {{}}\", h))) }} }}\n").unwrap();
        if t.fibers {
            // F2: en no-bloqueante SO_RCVTIMEO es inerte — el plazo se guarda en el ctx (rd_to) y
            // lo aplica el park de la lectura (wait_readable_timeout). ms <= 0 lo quita, como hoy.
            out.push_str("fn __ray_set_read_timeout(h: i64, ms: i64) { __ray_ctx(|c| { if ms <= 0 { c.rd_to.remove(&h); } else { c.rd_to.insert(h, ms); } }); }\n");
        } else {
            out.push_str(concat!(
            "fn __ray_set_read_timeout(h: i64, ms: i64) {\n",
            "    let d = if ms <= 0 { None } else { Some(std::time::Duration::from_millis(ms as u64)) };\n",
            // M96c: mismo fast-path que __ray_sock_clone — si ya está en la caché de ESTE hilo, ni
            // toca el lock global (es el llamador más frecuente: una vez por ciclo de lectura).
            "    if let Some(s) = __RAY_SOCK_CACHE.with(|c| c.borrow().get(&h).cloned()) { let _ = s.set_read_timeout(d); return; }\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get(&h) {\n",
            "        Some(__RayHandle::Tcp(s)) => { let s2 = std::sync::Arc::clone(s); let _ = s2.set_read_timeout(d); drop(reg);\n",
            "            __RAY_SOCK_CACHE.with(|c| { c.borrow_mut().insert(h, s2); }); }\n",
            // M121: el timeout de lectura aplica también a UDP (SO_RCVTIMEO real: hilo-por-tarea
            // usa el recv bloqueante, y la espera vencida se mapea a "read timeout" en el recv).
            "        Some(__RayHandle::Udp(s)) => { let _ = s.set_read_timeout(d); }\n",
            "        _ => {}\n",
            "    } }\n",
            ));
        }
        out.push_str(concat!(
            // UDP: los primitivos devuelven ARREGLOS ETIQUETADOS (bind/send → [\"ok\"/\"err\", ...]; recv →
            // [b\"ok\"/b\"err\", host, port, data]) que los wrappers de raylang (udp.ray) parsean. recv es
            // BLOQUEANTE (con hilos de SO reales; la VM usa no-bloqueante + scheduler → mismo efecto).
        ));
        // F4 (--fibers): UDP no-bloqueante — recv_from aparca la FIBRA hasta que haya datagrama
        // (como la cesión M20.11 de la VM); send_to aparca en el raro buffer-lleno. Sin fibras,
        // el modelo bloqueante de siempre.
        if t.fibers {
            out.push_str(concat!(
                "fn __ray_udp_bind(host: &str, port: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
                "    match std::net::UdpSocket::bind((host, port as u16)) {\n",
                "        Ok(s) => { let _ = s.set_nonblocking(true); let id = __ray_reg_insert(__RayHandle::Udp(s)); Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(id.to_string())])) }\n",
                "        Err(e) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e.to_string())])) } }\n",
                "fn __ray_udp_clone(h: i64) -> Option<std::net::UdpSocket> { let reg = __ray_reg().lock().unwrap(); match reg.open.get(&h) { Some(__RayHandle::Udp(s)) => s.try_clone().ok(), _ => None } }\n",
                "fn __ray_udp_send_to(h: i64, host: &str, port: i64, data: &[u8]) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
                "    let r = match __ray_udp_clone(h) { Some(s) => { use std::os::fd::AsRawFd; let fd = s.as_raw_fd(); loop {\n",
                "        match s.send_to(data, (host, port as u16)) {\n",
                "            Ok(n) => break Ok(n),\n",
                "            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => ray_runtime::fibers::wait_writable(fd),\n",
                "            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}\n",
                "            Err(e) => break Err(e.to_string()) } } }, None => Err(format!(\"handle {} is not a UDP socket\", h)) };\n",
                "    match r { Ok(n) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(n.to_string())])), Err(e) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e)])) } }\n",
                // M121: el timeout de lectura por handle (rd_to del ctx, como TCP) — sin plazo,
                // aparcado indefinido (wait_readable); con plazo, wait_readable_timeout y al vencer
                // el error estable "read timeout" (byte-idéntico a VM/intérprete).
                "fn __ray_udp_recv_from(h: i64) -> Rc<std::cell::RefCell<Vec<Rc<[u8]>>>> {\n",
                "    match __ray_udp_clone(h) {\n",
                "        Some(s) => { use std::os::fd::AsRawFd; let fd = s.as_raw_fd(); let mut buf = vec![0u8; 65536];\n",
                "            let to = __ray_ctx(|c| c.rd_to.get(&h).copied().unwrap_or(0));\n",
                "            let dl = if to > 0 { Some(std::time::Instant::now() + std::time::Duration::from_millis(to as u64)) } else { None };\n",
                "            loop {\n",
                "            match s.recv_from(&mut buf) {\n",
                "                Ok((n, addr)) => { buf.truncate(n); break Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"ok\"[..]), Rc::<[u8]>::from(addr.ip().to_string().as_bytes()), Rc::<[u8]>::from(addr.port().to_string().as_bytes()), Rc::<[u8]>::from(&buf[..])])); }\n",
                "                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {\n",
                "                    match dl {\n",
                "                        None => ray_runtime::fibers::wait_readable(fd),\n",
                "                        Some(d) => {\n",
                "                            let rem = d.saturating_duration_since(std::time::Instant::now()).as_millis() as i64;\n",
                "                            if rem <= 0 || ray_runtime::fibers::wait_readable_timeout(fd, rem) { break Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"err\"[..]), Rc::<[u8]>::from(&b\"read timeout\"[..])])); }\n",
                "                        }\n",
                "                    }\n",
                "                }\n",
                "                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}\n",
                "                Err(e) => break Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"err\"[..]), Rc::<[u8]>::from(e.to_string().as_bytes())])) } } }\n",
                "        None => Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"err\"[..]), Rc::<[u8]>::from(format!(\"handle {} is not a UDP socket\", h).as_bytes())])) } }\n",
            ));
        } else {
            out.push_str(concat!(
                "fn __ray_udp_bind(host: &str, port: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
                "    match std::net::UdpSocket::bind((host, port as u16)) {\n",
                "        Ok(s) => { let id = __ray_reg_insert(__RayHandle::Udp(s)); Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(id.to_string())])) }\n",
                "        Err(e) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e.to_string())])) } }\n",
                "fn __ray_udp_clone(h: i64) -> Option<std::net::UdpSocket> { let reg = __ray_reg().lock().unwrap(); match reg.open.get(&h) { Some(__RayHandle::Udp(s)) => s.try_clone().ok(), _ => None } }\n",
                "fn __ray_udp_send_to(h: i64, host: &str, port: i64, data: &[u8]) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
                "    let r = match __ray_udp_clone(h) { Some(s) => s.send_to(data, (host, port as u16)).map_err(|e| e.to_string()), None => Err(format!(\"handle {} is not a UDP socket\", h)) };\n",
                "    match r { Ok(n) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(n.to_string())])), Err(e) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e)])) } }\n",
                // M121: con SO_RCVTIMEO (set_read_timeout), la espera vencida llega como
                // WouldBlock/TimedOut (según SO) → el error estable "read timeout".
                "fn __ray_udp_recv_from(h: i64) -> Rc<std::cell::RefCell<Vec<Rc<[u8]>>>> {\n",
                "    match __ray_udp_clone(h) {\n",
                "        Some(s) => { let mut buf = vec![0u8; 65536]; match s.recv_from(&mut buf) {\n",
                "            Ok((n, addr)) => { buf.truncate(n); Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"ok\"[..]), Rc::<[u8]>::from(addr.ip().to_string().as_bytes()), Rc::<[u8]>::from(addr.port().to_string().as_bytes()), Rc::<[u8]>::from(&buf[..])])) }\n",
                "            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"err\"[..]), Rc::<[u8]>::from(&b\"read timeout\"[..])])),\n",
                "            Err(e) => Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"err\"[..]), Rc::<[u8]>::from(e.to_string().as_bytes())])) } }\n",
                "        None => Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"err\"[..]), Rc::<[u8]>::from(format!(\"handle {} is not a UDP socket\", h).as_bytes())])) } }\n",
            ));
        }
    }
    // Helpers de TLS (P2.b Paso 1), solo si el programa usa TLS. El binario transpilado hace I/O TLS
    // BLOQUEANTE (hilos reales) vía `ray_runtime::tls` — a diferencia de la VM (no-bloqueante + fibras).
    // Los primitivos devuelven arreglos ETIQUETADOS (`["ok", handle]`/`["err", msg]`, como UDP); los
    // wrappers de `std/net.ray` los parsean a `Result`. accept/upgrade parten de un handle TCP: sacan su
    // `TcpStream` del registro y reinsertan la conexión TLS con el MISMO handle (como la VM).
    if t.needs_rt_tls {
        out.push_str(concat!(
            // M96g: mismo fast-path que M96c, aplicado al chequeo "¿es TLS este handle?" — se
            // consulta en CADA lectura/escritura de socket (incluso sobre una conexión plana,
            // para saber si despachar a TLS antes de la vía TCP), y era el mayor contribuyente
            // del profiling de la ronda anterior (339 apariciones — ver §13). Mismo argumento de
            // solidez que M96c: una conexión la sirve siempre el mismo hilo. La única diferencia
            // con M96c: un handle SÍ puede cambiar de tipo en vivo (STARTTLS, `tls_accept`/
            // `tls_upgrade` insertan `Tls` donde antes había `Tcp`, mismo id) — por eso, a
            // diferencia del registro puro, esta caché se ACTUALIZA explícitamente en el sitio
            // del upgrade (mismo hilo que hizo el upgrade → mismo thread_local), en vez de solo
            // rellenarse perezosa en el primer acceso; así nunca queda una entrada "no es TLS"
            // stale tras un upgrade. Se cachea el resultado POSITIVO y el NEGATIVO (None) — el
            // caso caliente de un programa sin TLS en absoluto es que TODA lectura dé None.
        ));
        // F4 (--fibers): la caché "¿es TLS este handle?" viaja en el ctx de la fibra, no en el
        // hilo (mismo motivo que la caché de sockets: una entrada retenida en un worker ajeno
        // mantendría la sesión viva tras el close al migrar... aquí no hay migración, pero sí
        // muerte de la fibra dueña: el ctx cae con ella y la sesión se libera).
        if t.fibers {
            out.push_str(concat!(
                "fn __ray_tls_get(h: i64) -> Option<std::sync::Arc<std::sync::Mutex<ray_runtime::tls::TlsStream>>> {\n",
                "    if let Some(v) = __ray_ctx(|c| c.tls.get(&h).cloned()) { return v; }\n",
                "    let reg = __ray_reg().lock().unwrap();\n",
                "    let v = match reg.open.get(&h) { Some(__RayHandle::Tls(a)) => Some(a.clone()), _ => None };\n",
                "    drop(reg);\n",
                "    __ray_ctx(|c| { c.tls.insert(h, v.clone()); });\n",
                "    v\n}\n",
            ));
        } else {
            out.push_str(concat!(
                "thread_local! { static __RAY_TLS_CACHE: std::cell::RefCell<std::collections::HashMap<i64, Option<std::sync::Arc<std::sync::Mutex<ray_runtime::tls::TlsStream>>>>> = std::cell::RefCell::new(std::collections::HashMap::new()); }\n",
                "fn __ray_tls_get(h: i64) -> Option<std::sync::Arc<std::sync::Mutex<ray_runtime::tls::TlsStream>>> {\n",
                "    if let Some(v) = __RAY_TLS_CACHE.with(|c| c.borrow().get(&h).cloned()) { return v; }\n",
                "    let reg = __ray_reg().lock().unwrap();\n",
                "    let v = match reg.open.get(&h) { Some(__RayHandle::Tls(a)) => Some(a.clone()), _ => None };\n",
                "    drop(reg);\n",
                "    __RAY_TLS_CACHE.with(|c| { c.borrow_mut().insert(h, v.clone()); });\n",
                "    v\n}\n",
            ));
        }
        out.push_str(concat!(
            "fn __ray_tls_tag_ok(id: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> { Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(id.to_string())])) }\n",
            "fn __ray_tls_tag_err(msg: String) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> { Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(msg)])) }\n",
            "fn __ray_tls_wrap(s: ray_runtime::tls::TlsStream) -> i64 { __ray_reg_insert(__RayHandle::Tls(std::sync::Arc::new(std::sync::Mutex::new(s)))) }\n",
        ));
        // F4 (--fibers): el socket de connect/connect_h2 nace aquí BLOQUEANTE (en connect_h2 el
        // handshake eager con complete_io bloquea el worker un instante, acotado) y pasa a
        // no-bloqueante al insertarse: toda I/O posterior aparca la fibra (read_wait/write_all_wait).
        if t.fibers {
            out.push_str(concat!(
                "fn __ray_tls_connect(host: &str, port: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
                "    match ray_runtime::tls::connect(host, port) { Ok(s) => { let _ = s.set_nonblocking(true); __ray_tls_tag_ok(__ray_tls_wrap(s)) }, Err(e) => __ray_tls_tag_err(e) } }\n",
                "fn __ray_tls_connect_h2(host: &str, port: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
                "    match ray_runtime::tls::connect_h2(host, port) { Ok(s) => { let _ = s.set_nonblocking(true); __ray_tls_tag_ok(__ray_tls_wrap(s)) }, Err(e) => __ray_tls_tag_err(e) } }\n",
            ));
        } else {
            out.push_str(concat!(
                "fn __ray_tls_connect(host: &str, port: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
                "    match ray_runtime::tls::connect(host, port) { Ok(s) => __ray_tls_tag_ok(__ray_tls_wrap(s)), Err(e) => __ray_tls_tag_err(e) } }\n",
                "fn __ray_tls_connect_h2(host: &str, port: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
                "    match ray_runtime::tls::connect_h2(host, port) { Ok(s) => __ray_tls_tag_ok(__ray_tls_wrap(s)), Err(e) => __ray_tls_tag_err(e) } }\n",
            ));
        }
        out.push_str(concat!(
            // Saca el TcpStream del handle `h` (debe ser TCP), lo deja fuera del registro y lo devuelve.
            // También lo saca de la caché M96c (M96g): dejó de ser Tcp, una lectura futura no debe
            // reusar el Arc<TcpStream> viejo (aunque hoy nunca se llegaría a consultar: __ray_tls_get,
            // ya actualizado, despacha antes — esto es higiene, no un fix de un bug observable).
            // El mensaje del handle-no-Tcp difiere entre `accept`/`upgrade` en la VM (dos funciones
            // separadas, cada una con su texto — src/builtins.rs `tls_accept`/`tls_upgrade`); nativo
            // comparte esta única función, así que el mensaje viene por parámetro para dar el mismo
            // texto byte-a-byte según quién llame (cazado por `starttls_upgrade_native`, M96g).
            "fn __ray_tls_take_tcp(h: i64, not_tcp_msg: &str) -> Result<std::net::TcpStream, String> {\n",
            "    let mut reg = __ray_reg().lock().unwrap(); match reg.open.remove(&h) {\n",
        ));
        // La higiene de la caché de sockets según el modelo: ctx de fibra (--fibers) o thread-local.
        if t.fibers {
            out.push_str("        Some(__RayHandle::Tcp(s)) => { drop(reg); __ray_ctx(|c| { c.socks.remove(&h); c.rd_to.remove(&h); });\n");
        } else {
            out.push_str("        Some(__RayHandle::Tcp(s)) => { drop(reg); __RAY_SOCK_CACHE.with(|c| { c.borrow_mut().remove(&h); });\n");
        }
        out.push_str(concat!(
            "            match std::sync::Arc::try_unwrap(s) { Ok(t) => Ok(t), Err(a) => a.try_clone().map_err(|e| e.to_string()) } }\n",
            "        Some(other) => { reg.open.insert(h, other); Err(format!(\"handle {} {}\", h, not_tcp_msg)) }\n",
            "        None => Err(format!(\"invalid handle: {}\", h)) } }\n",
            "fn __ray_tls_accept(h: i64, cert: &str, key: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let sock = match __ray_tls_take_tcp(h, \"is not an accepted TCP socket\") { Ok(s) => s, Err(e) => return __ray_tls_tag_err(e) };\n",
            "    match ray_runtime::tls::accept(sock, cert, key) {\n",
            "        Ok(s) => { let a = std::sync::Arc::new(std::sync::Mutex::new(s));\n",
            "            __ray_reg().lock().unwrap().open.insert(h, __RayHandle::Tls(a.clone()));\n",
        ));
        if t.fibers {
            out.push_str("            __ray_ctx(|c| { c.tls.insert(h, Some(a)); });\n");
        } else {
            out.push_str("            __RAY_TLS_CACHE.with(|c| { c.borrow_mut().insert(h, Some(a)); });\n");
        }
        out.push_str(concat!(
            "            __ray_tls_tag_ok(h) }\n",
            "        Err(e) => __ray_tls_tag_err(e) } }\n",
            "fn __ray_tls_upgrade(h: i64, host: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let sock = match __ray_tls_take_tcp(h, \"is not a plain TCP socket\") { Ok(s) => s, Err(e) => return __ray_tls_tag_err(e) };\n",
            "    match ray_runtime::tls::upgrade(sock, host) {\n",
            "        Ok(s) => { let a = std::sync::Arc::new(std::sync::Mutex::new(s));\n",
            "            __ray_reg().lock().unwrap().open.insert(h, __RayHandle::Tls(a.clone()));\n",
        ));
        if t.fibers {
            out.push_str("            __ray_ctx(|c| { c.tls.insert(h, Some(a)); });\n");
        } else {
            out.push_str("            __RAY_TLS_CACHE.with(|c| { c.borrow_mut().insert(h, Some(a)); });\n");
        }
        out.push_str(concat!(
            "            __ray_tls_tag_ok(h) }\n",
            "        Err(e) => __ray_tls_tag_err(e) } }\n",
        ));
        // M124: el resumen del certificado del peer → arreglo etiquetado PLANO de strings
        // (["ok", subject, issuer, nb_ms, na_ms, san...]); el wrapper de std/net construye el
        // struct PeerCert (patrón stat). La lógica (conducir el handshake pendiente + parsear el
        // DER) vive en ray_runtime::tls::TlsStream::peer_cert_summary — el parseo X.509
        // (ray_runtime::x509) es el MISMO código que usa la VM → resumen byte-idéntico.
        out.push_str(concat!(
            "fn __ray_tls_peer_cert(h: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    match __ray_tls_get(h) {\n",
            "        Some(t) => match t.lock().unwrap().peer_cert_summary() {\n",
            "            Ok(s) => { let mut v: Vec<Rc<str>> = vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(s.subject), Rc::<str>::from(s.issuer), Rc::<str>::from(s.not_before_ms.to_string()), Rc::<str>::from(s.not_after_ms.to_string())];\n",
            "                v.extend(s.san.into_iter().map(Rc::<str>::from)); Rc::new(std::cell::RefCell::new(v)) }\n",
            "            Err(e) => __ray_tls_tag_err(e) }\n",
            "        None => __ray_tls_tag_err(format!(\"handle {} is not a TLS connection\", h)) } }\n",
        ));
    }
    // Helpers de SQLite (P2.b Paso 2), solo si el programa usa SQLite. Los primitivos devuelven arreglos
    // ETIQUETADOS que los wrappers de `db/sqlite.ray` parsean: open → ["ok", handle]/["err", msg]; exec →
    // ["ok", n_afectadas]/["err", msg]; query → ["ok", ncols, celda0, celda1, …]/["err", msg]. La conexión
    // vive en el registro (variante Sqlite); exec/query la operan reteniendo el lock global (I/O local).
    if t.needs_rt_sqlite {
        out.push_str(concat!(
            "fn __ray_sqlite_tag(v: Vec<Rc<str>>) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> { Rc::new(std::cell::RefCell::new(v)) }\n",
            "fn __ray_sqlite_err(msg: String) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> { __ray_sqlite_tag(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(msg)]) }\n",
            "fn __ray_sqlite_open(path: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    match ray_runtime::sqlite::open(path) { Ok(c) => { let id = __ray_reg_insert(__RayHandle::Sqlite(c)); __ray_sqlite_tag(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(id.to_string())]) } Err(e) => __ray_sqlite_err(e) } }\n",
            // Colecta los parámetros [string] a Vec<String> para la firma de ray_runtime::sqlite.
            "fn __ray_sqlite_params(params: &Rc<std::cell::RefCell<Vec<Rc<str>>>>) -> Vec<String> { params.borrow().iter().map(|s| s.to_string()).collect() }\n",
            "fn __ray_sqlite_exec(h: i64, sql: &str, params: &Rc<std::cell::RefCell<Vec<Rc<str>>>>) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let p = __ray_sqlite_params(params); let reg = __ray_reg().lock().unwrap();\n",
            "    let r = match reg.open.get(&h) { Some(__RayHandle::Sqlite(c)) => c.exec(sql, &p), Some(_) => Err(\"the handle is not a SQLite connection\".to_string()), None => Err(\"invalid or already closed handle\".to_string()) };\n",
            "    match r { Ok(n) => __ray_sqlite_tag(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(n.to_string())]), Err(e) => __ray_sqlite_err(e) } }\n",
            "fn __ray_sqlite_query(h: i64, sql: &str, params: &Rc<std::cell::RefCell<Vec<Rc<str>>>>) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let p = __ray_sqlite_params(params); let reg = __ray_reg().lock().unwrap();\n",
            "    let r = match reg.open.get(&h) { Some(__RayHandle::Sqlite(c)) => c.query(sql, &p), Some(_) => Err(\"the handle is not a SQLite connection\".to_string()), None => Err(\"invalid or already closed handle\".to_string()) };\n",
            "    match r { Ok((ncols, cells)) => { let mut v = vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(ncols.to_string())]; for cell in cells { v.push(Rc::<str>::from(cell)); } __ray_sqlite_tag(v) } Err(e) => __ray_sqlite_err(e) } }\n",
        ));
    }
    // Helper de procesos del SO (M100, IDEAS §53.8), solo si el programa usa `__run`. Llama al MISMO
    // `ray_runtime::process` que la VM (run_opts_from_flat/run_encoded) → paridad por construcción;
    // aquí solo el marshalling de los tipos transpilados. Interinamente BLOQUEA el hilo del worker
    // (como SQLite; también bajo fibras) — el aparcado sobre los fds de los pipes es una fase
    // posterior de M100.
    if t.needs_rt_process {
        out.push_str(concat!(
            "#[allow(clippy::too_many_arguments)]\n",
            "fn __ray_run(program: &str, args: &Rc<std::cell::RefCell<Vec<Rc<str>>>>, dir: &str, env: &Rc<std::cell::RefCell<Vec<Rc<str>>>>, env_clear: bool, stdin: &[u8], has_stdin: bool, timeout_ms: i64, max_output: i64, merge_output: bool) -> Rc<std::cell::RefCell<Vec<Rc<[u8]>>>> {\n",
            "    let args: Vec<String> = args.borrow().iter().map(|s| s.to_string()).collect();\n",
            "    let env: Vec<String> = env.borrow().iter().map(|s| s.to_string()).collect();\n",
            "    let opts = ray_runtime::process::run_opts_from_flat(dir, env, env_clear, stdin, has_stdin, false, timeout_ms, max_output, merge_output);\n",
            "    let elems: Vec<Rc<[u8]>> = ray_runtime::process::run_encoded(program, &args, &opts).into_iter().map(|b| Rc::<[u8]>::from(&b[..])).collect();\n",
            "    Rc::new(std::cell::RefCell::new(elems)) }\n",
        ));
        // M100 v2 (IDEAS §53.9): el gemelo nativo de los primitivos del streaming. Los tags y los
        // mensajes son byte-idénticos a los de la VM (builtins.rs: proc_spawn_encoded/
        // proc_try_wait_encoded); el aparcado de la fibra vive en __ray_pipe_read.
        out.push_str(concat!(
            "fn __ray_proc_tag(v: Vec<Rc<[u8]>>) -> Rc<std::cell::RefCell<Vec<Rc<[u8]>>>> { Rc::new(std::cell::RefCell::new(v)) }\n",
            "fn __ray_proc_err(msg: String) -> Rc<std::cell::RefCell<Vec<Rc<[u8]>>>> { __ray_proc_tag(vec![Rc::<[u8]>::from(&b\"err\"[..]), Rc::<[u8]>::from(msg.as_bytes())]) }\n",
            "#[allow(clippy::too_many_arguments)]\n",
            "fn __ray_proc_spawn(program: &str, args: &Rc<std::cell::RefCell<Vec<Rc<str>>>>, dir: &str, env: &Rc<std::cell::RefCell<Vec<Rc<str>>>>, env_clear: bool, stdin: &[u8], has_stdin: bool, stdin_open: bool, merge_output: bool) -> Rc<std::cell::RefCell<Vec<Rc<[u8]>>>> {\n",
            "    let args: Vec<String> = args.borrow().iter().map(|s| s.to_string()).collect();\n",
            "    let env: Vec<String> = env.borrow().iter().map(|s| s.to_string()).collect();\n",
            "    let opts = ray_runtime::process::run_opts_from_flat(dir, env, env_clear, stdin, has_stdin, stdin_open, 0, 0, merge_output);\n",
            "    match ray_runtime::process::spawn_streamed(program, &args, &opts) {\n",
            "        Ok(s) => {\n",
            "            let h_child = __ray_reg_insert(__RayHandle::Child(s.child));\n",
            "            let h_in = s.stdin.map_or(-1, |f| __ray_reg_insert(__RayHandle::PipeW(std::sync::Arc::new(f))));\n",
            "            let h_out = s.out.map_or(-1, |f| __ray_reg_insert(__RayHandle::Pipe(std::sync::Arc::new(f))));\n",
            "            let h_err = s.err.map_or(-1, |f| __ray_reg_insert(__RayHandle::Pipe(std::sync::Arc::new(f))));\n",
            "            __ray_proc_bind(h_child);\n",
            "            __ray_proc_tag(vec![Rc::<[u8]>::from(&b\"ok\"[..]), Rc::<[u8]>::from(h_child.to_string().as_bytes()), Rc::<[u8]>::from(h_in.to_string().as_bytes()), Rc::<[u8]>::from(h_out.to_string().as_bytes()), Rc::<[u8]>::from(h_err.to_string().as_bytes())])\n",
            "        }\n",
            "        Err(e) => __ray_proc_err(e) } }\n",
            "fn __ray_proc_try_wait(h: i64) -> Rc<std::cell::RefCell<Vec<Rc<[u8]>>>> {\n",
            "    let mut reg = __ray_reg().lock().unwrap();\n",
            "    let Some(__RayHandle::Child(child)) = reg.open.get_mut(&h) else { return __ray_proc_err(format!(\"handle {} is not a child process\", h)); };\n",
            "    let result = match ray_runtime::process::try_wait(child) {\n",
            "        Ok(None) => return __ray_proc_tag(vec![Rc::<[u8]>::from(&b\"running\"[..])]),\n",
            "        Ok(Some(Ok(c))) => __ray_proc_tag(vec![Rc::<[u8]>::from(&b\"code\"[..]), Rc::<[u8]>::from(c.to_string().as_bytes())]),\n",
            "        Ok(Some(Err(s))) => __ray_proc_tag(vec![Rc::<[u8]>::from(&b\"signal\"[..]), Rc::<[u8]>::from(s.to_string().as_bytes())]),\n",
            "        Err(e) => __ray_proc_err(e) };\n",
            "    reg.open.remove(&h);\n",
            "    result }\n",
            "fn __ray_proc_kill(h: i64, force: bool) {\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    if let Some(__RayHandle::Child(child)) = reg.open.get(&h) { ray_runtime::process::kill_group(child.id() as i32, force); } }\n",
            "fn __ray_pipe_clone(h: i64) -> Option<std::sync::Arc<std::fs::File>> {\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get(&h) { Some(__RayHandle::Pipe(f)) => Some(std::sync::Arc::clone(f)), _ => None } }\n",
            "fn __ray_stdin_clone(h: i64) -> Option<std::sync::Arc<std::fs::File>> {\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get(&h) { Some(__RayHandle::PipeW(f)) => Some(std::sync::Arc::clone(f)), _ => None } }\n",
        ));
        // M100 v3: la escritura en el stdin de un hijo VIVO. Escribe TODO el dato; si el pipe se
        // llena, con fibras espera a que sea escribible en el reactor (`wait_writable`) y sin ellas
        // duerme 1 ms — el espejo de lo que hace la VM aparcando por interés de escritura. Tags y
        // mensajes idénticos a la VM (`["ok",""]`/`["err", msg]` de SocketWriteBytes).
        out.push_str(concat!(
            "fn __ray_proc_write(h: i64, data: &[u8]) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    use std::io::Write;\n",
            "    let tag = |a: &str, b: String| Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(a), Rc::<str>::from(b.as_str())]));\n",
            "    let Some(f) = __ray_stdin_clone(h) else { return tag(\"err\", format!(\"invalid child stdin handle: {}\", h)); };\n",
            // M175: el fd solo existe (y solo hace falta) con fibras — que son unix.
            "    #[cfg(unix)] let fd = std::os::fd::AsRawFd::as_raw_fd(&*f);\n",
            "    #[cfg(unix)] let _ = fd;\n",
            "    let mut off = 0usize;\n",
            "    while off < data.len() {\n",
            "        let mut w = &*f;\n",
            "        match w.write(&data[off..]) {\n",
            "            Ok(n) => off += n,\n",
            "            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::Interrupted => ",
        ));
        out.push_str(if t.fibers {
            "{ ray_runtime::fibers::wait_writable(fd); }\n"
        } else {
            "{ std::thread::sleep(std::time::Duration::from_millis(1)); }\n"
        });
        out.push_str(concat!(
            "            Err(e) => return tag(\"err\", e.to_string()),\n",
            "        }\n",
            "    }\n",
            "    tag(\"ok\", String::new()) }\n",
        ));
        // M100 fase 2e: la cosecha ESTRUCTURAL. Un proceso lanzado con stream() se ATA al scope
        // activo como un hijo más — vía el MISMO trait __RayScopeChild que las tareas: done()
        // siempre-true (el scope no espera al proceso: espera a las bombas), cancel_task() (una
        // hermana falló) y consume() (cierre con éxito: un proceso sin wait() no sobrevive al
        // scope) hacen KILL al grupo + cosecha + baja del registro — no-op si ya fue esperado
        // (la eliminación del registro en try_wait ES el desatado). Sin concurrencia emitida no
        // hay scopes: el bind es un no-op.
        if t.needs_concurrency {
            out.push_str(concat!(
                "fn __ray_proc_kill_reap(h: i64) {\n",
                "    let mut reg = __ray_reg().lock().unwrap();\n",
                "    if let Some(__RayHandle::Child(child)) = reg.open.get_mut(&h) {\n",
                "        ray_runtime::process::kill_group(child.id() as i32, true);\n",
                "        let _ = child.wait();\n",
                "        reg.open.remove(&h);\n",
                "    } }\n",
                "struct __RayProcChild(i64);\n",
                "impl __RayScopeChild for __RayProcChild {\n",
                "    fn failed(&self) -> Option<String> { None }\n",
                "    fn done(&self) -> bool { true }\n",
                "    fn cancel_task(&self) { __ray_proc_kill_reap(self.0); }\n",
                "    fn consume(&self) { __ray_proc_kill_reap(self.0); }\n",
                "}\n",
            ));
            if t.fibers {
                out.push_str("fn __ray_proc_bind(h: i64) { __ray_ctx(|c| { if let Some(fr) = c.scopes.last_mut() { fr.push(std::boxed::Box::new(__RayProcChild(h))); } }); }\n");
            } else {
                out.push_str("fn __ray_proc_bind(h: i64) { __SCOPES.with(|s| { if let Some(fr) = s.borrow_mut().last_mut() { fr.push(std::boxed::Box::new(__RayProcChild(h))); } }); }\n");
            }
        } else {
            out.push_str("fn __ray_proc_bind(_h: i64) {}\n");
        }
        // La lectura de la BOMBA: no-bloqueante; con fibras aparca la fibra en el fd (préstamo del
        // búfer fuera de la cesión); sin fibras (hilo-por-tarea) reintenta cediendo 1 ms, como el
        // intérprete. Búfer por HILO propio (el __RAY_RDBUF de la red no siempre se emite); tags y
        // mensajes idénticos a la VM (SocketReadBytes sobre un Pipe).
        out.push_str("thread_local! { static __RAY_PROC_RDBUF: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(vec![0u8; 65536]); }\n");
        if t.fibers {
            out.push_str(concat!(
                "fn __ray_proc_read(h: i64) -> Rc<std::cell::RefCell<Vec<Rc<[u8]>>>> {\n",
                "    use std::io::Read;\n",
                "    let Some(f) = __ray_pipe_clone(h) else { return __ray_proc_err(format!(\"handle {} is not a socket\", h)); };\n",
                "    let fd = std::os::fd::AsRawFd::as_raw_fd(&*f);\n",
                "    loop {\n",
                "        let res = __RAY_PROC_RDBUF.with(|__b| { let mut buf = __b.borrow_mut(); let mut r = &*f; match r.read(&mut buf[..]) {\n",
                "            Ok(n) => Some(__ray_proc_tag(vec![Rc::<[u8]>::from(&b\"ok\"[..]), Rc::<[u8]>::from(&buf[..n])])),\n",
                "            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::Interrupted => None,\n",
                "            Err(e) => Some(__ray_proc_err(e.to_string())) } });\n",
                "        if let Some(v) = res { return v; }\n",
                "        ray_runtime::fibers::wait_readable(fd);\n",
                "    } }\n",
            ));
        } else {
            out.push_str(concat!(
                "fn __ray_proc_read(h: i64) -> Rc<std::cell::RefCell<Vec<Rc<[u8]>>>> {\n",
                "    use std::io::Read;\n",
                "    let Some(f) = __ray_pipe_clone(h) else { return __ray_proc_err(format!(\"handle {} is not a socket\", h)); };\n",
                "    loop {\n",
                "        let res = __RAY_PROC_RDBUF.with(|__b| { let mut buf = __b.borrow_mut(); let mut r = &*f; match r.read(&mut buf[..]) {\n",
                "            Ok(n) => Some(__ray_proc_tag(vec![Rc::<[u8]>::from(&b\"ok\"[..]), Rc::<[u8]>::from(&buf[..n])])),\n",
                "            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::Interrupted => None,\n",
                "            Err(e) => Some(__ray_proc_err(e.to_string())) } });\n",
                "        if let Some(v) = res { return v; }\n",
                "        std::thread::sleep(std::time::Duration::from_millis(1)); } }\n",
            ));
        }
    }
    // Runtime de canales MPMC (concurrencia, M12.1/M12.2), solo si el programa usa spawn/canales. Es un
    // canal thread-safe propio (Arc<Mutex+Condvar>) — sin deps, ya que el `.rs` es standalone — con
    // backpressure (bounded) y cierre. FIFO como la VM. `T: Send` (primitivos en v1).
    if t.needs_concurrency {
        out.push_str(concat!(
            // `taken` cuenta los valores CONSUMIDOS (para el handshake rendezvous por generación) y
            // `senders` los emisores bloqueados (para que `close` los detecte, como la VM). Los panics
            // llevan el MISMO texto que el error de ejecución de la VM (exit code ≠ 70: diferido a H6).
            "struct __ChanState<T> { q: std::collections::VecDeque<T>, closed: bool, cap: Option<usize>, taken: u64, senders: usize }\n",
            // M116: el resultado interno de `try_recv` (repr SEND del payload); el sitio lo mapea al
            // enum del prelude `Received` (repr programa).
            "enum __TryRecv<T> { Got(T), Empty, Closed }\n",
            "struct __RayChan<T> { inner: std::sync::Arc<__RaySync<__ChanState<T>>> }\n",
            "impl<T> Clone for __RayChan<T> { fn clone(&self) -> Self { __RayChan { inner: self.inner.clone() } } }\n",
            // Un canal dentro de un struct/enum mostrable se renderiza `<channel>`, como la VM
            // (`format_value`: canal/tarea no se inspeccionan textualmente).
            "impl<T> RayShow for __RayChan<T> { fn ray_show(&self) -> String { \"<channel>\".to_string() } }\n",
            "impl<T: Send> __RayChan<T> {\n",
            "    fn make(cap: Option<usize>) -> Self { __RayChan { inner: std::sync::Arc::new(__ray_sync_new(__ChanState { q: std::collections::VecDeque::new(), closed: false, cap, taken: 0, senders: 0 })) } }\n",
            "    fn send(&self, v: T) {\n",
            "        let mut st = self.inner.0.lock().unwrap();\n",
            // `send` sobre un canal cerrado = error de ejecución, como la VM (antes: descarte silencioso).
            // El guard se suelta antes del panic para no envenenar el Mutex (los otros hilos verían
            // PoisonError en vez del mensaje real). Toda espera bloqueante usa `__ray_cv_wait` (timeout
            // corto) + chequeo de cancelación (H21-N3): una tarea cancelada aborta en su siguiente punto
            // bloqueante, deshaciendo su rastro (contador `senders`, su valor en cola).
            "        if st.closed { drop(st); __ray_rt_err(\"send on a closed channel\"); }\n",
            // Rendezvous (cap 0): la VM entrega el valor directamente y el emisor no continúa hasta que SU
            // valor se consume (M12.2). El handshake es por GENERACIÓN (`taken`), no por cola-vacía: con
            // ≥2 emisores, A podía despertar con el valor de B en cola y re-dormirse para siempre aunque el
            // suyo ya se consumió. `my` = el ordinal que consumirá su valor; A retorna cuando `taken >= my`.
            "        if st.cap == Some(0) {\n",
            "            st.senders += 1;\n",
            "            while !st.closed && !st.q.is_empty() { st = __ray_cv_wait(&self.inner, st); if __ray_cancelled() { st.senders -= 1; drop(st); __ray_rt_err(\"task cancelled (a sibling failed)\"); } }\n",
            "            if st.closed { st.senders -= 1; drop(st); __ray_rt_err(\"send on a closed channel\"); }\n",
            "            st.q.push_back(v);\n",
            "            let my = st.taken + 1; __ray_notify(&self.inner); __ray_bump();\n",
            "            while !st.closed && st.taken < my { st = __ray_cv_wait(&self.inner, st); if __ray_cancelled() { if st.taken < my { st.q.pop_back(); } st.senders -= 1; drop(st); __ray_rt_err(\"task cancelled (a sibling failed)\"); } }\n",
            "            st.senders -= 1;\n",
            "            if st.taken < my { drop(st); __ray_rt_err(\"send on a closed channel\"); }\n",
            "            return;\n",
            "        }\n",
            "        st.senders += 1;\n",
            "        while !st.closed && st.cap.map_or(false, |c| st.q.len() >= c) { st = __ray_cv_wait(&self.inner, st); if __ray_cancelled() { st.senders -= 1; drop(st); __ray_rt_err(\"task cancelled (a sibling failed)\"); } }\n",
            "        st.senders -= 1;\n",
            "        if st.closed { drop(st); __ray_rt_err(\"send on a closed channel\"); }\n",
            "        st.q.push_back(v); __ray_notify(&self.inner); drop(st); __ray_bump();\n",
            "    }\n",
            "    fn recv(&self) -> Option<T> {\n",
            "        let mut st = self.inner.0.lock().unwrap();\n",
            "        while st.q.is_empty() && !st.closed { st = __ray_cv_wait(&self.inner, st); if __ray_cancelled() { drop(st); __ray_rt_err(\"task cancelled (a sibling failed)\"); } }\n",
            "        let v = st.q.pop_front(); if v.is_some() { st.taken += 1; __ray_notify(&self.inner); } v\n",
            "    }\n",
            // M116: recepción NO bloqueante. Got(v) drena y despierta a un emisor (como recv); vacío y
            // abierto → Empty; vacío y cerrado → Closed. El sitio de llamada mapea `__TryRecv<sendrepr>`
            // al enum del prelude `Received<progrepr>` convirtiendo el payload (repr send → programa).
            "    fn try_recv(&self) -> __TryRecv<T> {\n",
            "        let mut st = self.inner.0.lock().unwrap();\n",
            "        match st.q.pop_front() { Some(v) => { st.taken += 1; __ray_notify(&self.inner); __TryRecv::Got(v) } None => if st.closed { __TryRecv::Closed } else { __TryRecv::Empty } }\n",
            "    }\n",
            // `close` con un emisor bloqueado = error de ejecución en el sitio del close, como la VM
            // (M12.2; antes el emisor hacía return silencioso y su valor quedaba consumible).
            "    fn close(&self) {\n",
            "        let mut st = self.inner.0.lock().unwrap();\n",
            "        if st.senders > 0 { drop(st); __ray_rt_err(\"close on a channel with a blocked sender\"); }\n",
            "        st.closed = true; __ray_notify(&self.inner); drop(st); __ray_bump();\n",
            "    }\n",
            "}\n",
            // Condvar-wait con timeout corto: el despertar normal sigue llegando por `notify` (sin
            // latencia añadida); el timeout solo acota cuánto tarda una tarea bloqueada en NOTAR su
            // cancelación (cooperativa, H21-N3) → sin busy-wait.
        ));
        // F3: la TRÍADA de sincronización por modo. Con fibras, cada condición (canal/tarea)
        // lleva además una LISTA DE ESPERAS (ray_runtime::fibers::WaitList): la espera de una
        // FIBRA se aparca de verdad (cero CPU; el ceder-en-bucle de F2 quemaba un worker por
        // esperador ocioso) con el protocolo anti despertar-perdido de la lista (prepare bajo el
        // lock → soltar → block_on; el registro re-lee la generación) y el pulso de 10 ms que
        // conserva la cadencia de cancelación (H21-N3). El hilo `main` sigue en la condvar. Los
        // SITIOS (send/recv/close/wait) comparten cadena en ambos modos: solo cambian estos
        // helpers y el alias del tipo.
        if t.fibers {
            out.push_str(concat!(
                "type __RaySync<T> = (std::sync::Mutex<T>, std::sync::Condvar, ray_runtime::fibers::WaitList);\n",
                "fn __ray_sync_new<T>(v: T) -> __RaySync<T> { (std::sync::Mutex::new(v), std::sync::Condvar::new(), ray_runtime::fibers::WaitList::new()) }\n",
                "fn __ray_cv_wait<'a, T>(inner: &'a __RaySync<T>, g: std::sync::MutexGuard<'a, T>) -> std::sync::MutexGuard<'a, T> {\n",
                "    if ray_runtime::fibers::in_fiber() {\n",
                "        let seen = inner.2.prepare();\n",
                "        drop(g);\n",
                "        ray_runtime::fibers::block_on(&inner.2, seen);\n",
                "        return inner.0.lock().unwrap();\n",
                "    }\n",
                "    inner.1.wait_timeout(g, std::time::Duration::from_millis(10)).unwrap().0\n}\n",
                "fn __ray_notify<T>(inner: &__RaySync<T>) { inner.1.notify_all(); inner.2.wake_all(); }\n",
            ));
        } else {
            out.push_str(concat!(
                "type __RaySync<T> = (std::sync::Mutex<T>, std::sync::Condvar);\n",
                "fn __ray_sync_new<T>(v: T) -> __RaySync<T> { (std::sync::Mutex::new(v), std::sync::Condvar::new()) }\n",
                // Condvar-wait con timeout corto: el despertar normal llega por notify; el timeout
                // solo acota cuánto tarda una tarea bloqueada en NOTAR su cancelación (H21-N3).
                "fn __ray_cv_wait<'a, T>(inner: &'a __RaySync<T>, g: std::sync::MutexGuard<'a, T>) -> std::sync::MutexGuard<'a, T> {\n",
                "    inner.1.wait_timeout(g, std::time::Duration::from_millis(10)).unwrap().0\n}\n",
                "fn __ray_notify<T>(inner: &__RaySync<T>) { inner.1.notify_all(); }\n",
            ));
        }
        out.push_str(concat!(
            // Token de cancelación del hilo actual (lo instala `__ray_spawn`; `main` no tiene → false).
        ));
        // F2 (--fibers): el token de cancelación viaja EN LA FIBRA (ctx) — la fibra puede
        // reanudarse en otro worker y un thread-local se quedaría en el anterior.
        if t.fibers {
            out.push_str("fn __ray_cancelled() -> bool { __ray_ctx(|c| c.cancel.as_ref().map_or(false, |f| f.load(std::sync::atomic::Ordering::Relaxed))) }\n");
        } else {
            out.push_str(concat!(
                "thread_local! { static __RAY_CANCEL: std::cell::RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> = std::cell::RefCell::new(None); }\n",
                "fn __ray_cancelled() -> bool { __RAY_CANCEL.with(|c| c.borrow().as_ref().map_or(false, |f| f.load(std::sync::atomic::Ordering::Relaxed))) }\n",
            ));
        }
        out.push_str(concat!(
            // Condvar GLOBAL de actividad (H21-N4): send/close/fin-de-tarea la notifican (generación
            // monótona); `select` y la salida del scope esperan en ella en vez de hacer poll con sleep.
            // Orden de locks: canal/tarea → actividad (nunca al revés) → sin ciclos.
            // M96b (perfilado bajo `wrk -c500`): la generación es un ATÓMICO y el mutex+notify solo
            // se tocan si HAY esperadores (`select`/salida de scope). Antes, cada send/close/fin-de-
            // tarea (~120k/s en el webserver) tomaba este mutex GLOBAL para notificar a nadie →
            // contención medible (23k muestras en __psynch_mutexwait). Sin esperadores, `bump` es un
            // fetch_add. Protocolo sin despertar perdido: el esperador se registra ANTES de releer la
            // generación bajo el lock; `bump` publica la generación ANTES de mirar el contador.\n
            "static __RAY_ACT_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);\n",
            "static __RAY_ACT_WAITERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);\n",
            "static __RAY_ACT_M: std::sync::Mutex<()> = std::sync::Mutex::new(());\n",
            "static __RAY_ACT_CV: std::sync::Condvar = std::sync::Condvar::new();\n",
        ));
        // F3 (--fibers): la actividad global también tiene su lista de esperas (para las FIBRAS en
        // select/salida-de-scope); el condvar sigue cubriendo al hilo main.
        if t.fibers {
            out.push_str(concat!(
                "fn __ray_act_wl() -> &'static ray_runtime::fibers::WaitList {\n",
                "    static W: std::sync::OnceLock<ray_runtime::fibers::WaitList> = std::sync::OnceLock::new();\n",
                "    W.get_or_init(ray_runtime::fibers::WaitList::new)\n}\n",
                "fn __ray_bump() {\n",
                "    __RAY_ACT_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);\n",
                "    if __RAY_ACT_WAITERS.load(std::sync::atomic::Ordering::SeqCst) > 0 { { let _g = __RAY_ACT_M.lock().unwrap(); __RAY_ACT_CV.notify_all(); } __ray_act_wl().wake_all(); }\n",
                "}\n",
            ));
        } else {
            out.push_str(concat!(
                "fn __ray_bump() {\n",
                "    __RAY_ACT_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);\n",
                "    if __RAY_ACT_WAITERS.load(std::sync::atomic::Ordering::SeqCst) > 0 { let _g = __RAY_ACT_M.lock().unwrap(); __RAY_ACT_CV.notify_all(); }\n",
                "}\n",
            ));
        }
        out.push_str(concat!(
            "fn __ray_wait_activity(act: u64) {\n",
            "    __RAY_ACT_WAITERS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);\n",
            "    let mut g = __RAY_ACT_M.lock().unwrap();\n",
            "    while __RAY_ACT_GEN.load(std::sync::atomic::Ordering::SeqCst) == act {\n",
        ));
        if t.fibers {
            out.push_str(concat!(
                "        g = if ray_runtime::fibers::in_fiber() {\n",
                "            let seen = __ray_act_wl().prepare();\n",
                "            drop(g);\n",
                "            if __RAY_ACT_GEN.load(std::sync::atomic::Ordering::SeqCst) == act { ray_runtime::fibers::block_on(__ray_act_wl(), seen); }\n",
                "            __RAY_ACT_M.lock().unwrap()\n",
                "        } else { __RAY_ACT_CV.wait_timeout(g, std::time::Duration::from_millis(10)).unwrap().0 };\n",
            ));
        } else {
            out.push_str("        g = __RAY_ACT_CV.wait_timeout(g, std::time::Duration::from_millis(10)).unwrap().0;\n");
        }
        out.push_str(concat!(
            "        if __ray_cancelled() { drop(g); __RAY_ACT_WAITERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst); __ray_rt_err(\"task cancelled (a sibling failed)\"); }\n",
            "    }\n",
            "    drop(g);\n",
            "    __RAY_ACT_WAITERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);\n",
            "}\n",
            // M116.1: espera de actividad ACOTADA (una sola vez). A diferencia de __ray_wait_activity
            // (que bloquea HASTA que la generación cambie), retorna tras el despertar por notify (un
            // canal listo) O el pulso de ~10 ms — el que llegue antes. Es lo que necesita
            // select_timeout: su bucle re-escanea y re-chequea el deadline tras cada retorno, así el
            // plazo vence aunque no haya ninguna actividad de canales que despierte.
            "fn __ray_wait_activity_once(act: u64) {\n",
            "    __RAY_ACT_WAITERS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);\n",
            "    let g = __RAY_ACT_M.lock().unwrap();\n",
            "    if __RAY_ACT_GEN.load(std::sync::atomic::Ordering::SeqCst) == act {\n",
        ));
        if t.fibers {
            out.push_str(concat!(
                "        if ray_runtime::fibers::in_fiber() {\n",
                "            let seen = __ray_act_wl().prepare();\n",
                "            drop(g);\n",
                "            if __RAY_ACT_GEN.load(std::sync::atomic::Ordering::SeqCst) == act { ray_runtime::fibers::block_on(__ray_act_wl(), seen); }\n",
                "        } else { let _ = __RAY_ACT_CV.wait_timeout(g, std::time::Duration::from_millis(10)); }\n",
            ));
        } else {
            out.push_str("        let _ = __RAY_ACT_CV.wait_timeout(g, std::time::Duration::from_millis(10));\n");
        }
        out.push_str(concat!(
            "    } else { drop(g); }\n",
            "    if __ray_cancelled() { __RAY_ACT_WAITERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst); __ray_rt_err(\"task cancelled (a sibling failed)\"); }\n",
            "    __RAY_ACT_WAITERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);\n",
            "}\n",
            // Structured concurrency (M12.3) + contención de fallos (H21-N1) + cancelación de hermanas
            // (M12.5, H21-N3): Task<T> = estado compartido (resultado + condvar) que el HILO HIJO rellena
            // al terminar (push, no join) + un token de cancelación. El cuerpo corre bajo `catch_unwind`
            // → un fallo queda CAPTURADO en la Task (`Err(msg)`, como el `Failed` de la VM) y NO mata el
            // proceso; se re-lanza cuando alguien lo OBSERVA (`join`/salida del scope) y encadena hacia
            // arriba hasta main. `wait` es la observación sin re-lanzar (base de `try_join`, H21-N2).
            // La cancelación es COOPERATIVA (como la VM, que solo cancela en los yields del scheduler
            // M:1): una tarea cancelada termina en su siguiente punto BLOQUEANTE (send/recv/join/select/
            // scope); código que corre sin bloquearse no se interrumpe (divergencia menor documentada).
            "struct __TaskState<T> { result: Option<Result<T, String>> }\n",
            // M97.1/M98.1: `consumed` = la tarea ya fue CONSUMIDA (join/try_join la toman; el scope
            // consume a sus hijas al cerrar). Un segundo join → error TASK_CONSUMED (byte-idéntico a
            // la VM, que libera el slot y detecta el handle stale). Un `Failed` consumido cuenta como
            // MANEJADO: `failed()` (el escaneo del scope) lo salta — semántica M97.1.
            "struct __RayTask<T> { inner: std::sync::Arc<__RaySync<__TaskState<T>>>, cancel: std::sync::Arc<std::sync::atomic::AtomicBool>, consumed: std::sync::Arc<std::sync::atomic::AtomicBool> }\n",
            "impl<T> Clone for __RayTask<T> { fn clone(&self) -> Self { __RayTask { inner: self.inner.clone(), cancel: self.cancel.clone(), consumed: self.consumed.clone() } } }\n",
            "impl<T> RayShow for __RayTask<T> { fn ray_show(&self) -> String { \"<task>\".to_string() } }\n",
            "const __RAY_TASK_CONSUMED: &str = \"task already consumed (join/try_join takes the task)\";\n",
            "impl<T: Send + Clone + 'static> __RayTask<T> {\n",
            "    fn wait(&self) -> Result<T, String> {\n",
            "        let mut st = self.inner.0.lock().unwrap();\n",
            "        while st.result.is_none() { st = __ray_cv_wait(&self.inner, st); if __ray_cancelled() { drop(st); __ray_rt_err(\"task cancelled (a sibling failed)\"); } }\n",
            "        st.result.clone().unwrap()\n",
            "    }\n",
            // try_join: consume la tarea entera (Ok y Err) — es la observación + la unión en una.
            "    fn wait_consume(&self) -> Result<T, String> {\n",
            "        if self.consumed.swap(true, std::sync::atomic::Ordering::SeqCst) { __ray_rt_err(__RAY_TASK_CONSUMED); }\n",
            "        self.wait()\n",
            "    }\n",
            // __task_failed directo (cuerpo del prelude): consume SOLO en Err — en Ok el join que
            // sigue en el envoltorio recoge el valor (y consume él).
            "    fn wait_failed(&self) -> Option<String> {\n",
            "        if self.consumed.load(std::sync::atomic::Ordering::SeqCst) { __ray_rt_err(__RAY_TASK_CONSUMED); }\n",
            "        match self.wait() { Ok(_) => None, Err(m) => { self.consumed.store(true, std::sync::atomic::Ordering::SeqCst); Some(m) } }\n",
            "    }\n",
            "    fn join(&self) -> T { if self.consumed.swap(true, std::sync::atomic::Ordering::SeqCst) { __ray_rt_err(__RAY_TASK_CONSUMED); } match self.wait() { Ok(v) => v, Err(m) => __ray_rt_err(&m) } }\n",
            "}\n",
            // La cara borrada-de-tipo que un scope guarda de cada hija: sondear su estado SIN bloquear
            // (el hilo hijo escribe su resultado al terminar) y cancelarla.
            "trait __RayScopeChild { fn failed(&self) -> Option<String>; fn done(&self) -> bool; fn cancel_task(&self); fn consume(&self); }\n",
            "impl<T> __RayScopeChild for __RayTask<T> {\n",
            "    fn failed(&self) -> Option<String> { if self.consumed.load(std::sync::atomic::Ordering::SeqCst) { return None; } match &self.inner.0.lock().unwrap().result { Some(Err(m)) => Some(m.clone()), _ => None } }\n",
            "    fn done(&self) -> bool { self.inner.0.lock().unwrap().result.is_some() }\n",
            "    fn cancel_task(&self) { self.cancel.store(true, std::sync::atomic::Ordering::Relaxed); __ray_bump(); }\n",
            // M98.1: el scope consume a sus hijas al cerrar (paridad con la VM, que libera los slots):
            // un `join` posterior sobre un handle que escapó del scope → error TASK_CONSUMED.
            "    fn consume(&self) { self.consumed.store(true, std::sync::atomic::Ordering::SeqCst); }\n",
            "}\n",
            // Cada scope activo (por hilo) acumula las tareas lanzadas dentro; `spawn` registra la suya
            // en el scope más interno, si hay.
        ));
        if !t.fibers {
            out.push_str("thread_local! { static __SCOPES: std::cell::RefCell<Vec<Vec<std::boxed::Box<dyn __RayScopeChild>>>> = std::cell::RefCell::new(Vec::new()); }\n");
        }
        out.push_str(concat!(
            ));
        // F2 (--fibers): spawn = FIBRA del scheduler M:N (ray_runtime::fibers) — el pool de hilos
        // NO se emite. El cuerpo es el mismo del modelo de hilos (catch_unwind → resultado en la
        // Task; un fallo cancela los scopes sin cerrar, sin nietos huérfanos, M12.5); el token de
        // cancelación y el registro en el scope del padre van por el CTX por-fibra (viaja con
        // ella entre workers). El JoinHandle de fibers se descarta: la observación es la __RayTask
        // (condvar), que main espera bloqueando y una fibra espera en rondas acotadas de
        // __ray_cv_wait (10 ms) — interino hasta F3 (esperas de fibra nativas).
        if t.fibers {
            out.push_str(concat!(
                "fn __ray_spawn<T: Send + Clone + 'static, F: FnOnce() -> T + Send + 'static>(f: F) -> __RayTask<T> {\n",
                "    let task = __RayTask { inner: std::sync::Arc::new(__ray_sync_new(__TaskState { result: None })), cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), consumed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)) };\n",
                "    let t = task.clone();\n",
                "    let _ = ray_runtime::fibers::spawn(move || {\n",
                "        __ray_ctx(|c| c.cancel = Some(t.cancel.clone()));\n",
                "        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|e| __ray_panic_msg(&*e));\n",
                "        if r.is_err() { let frames = __ray_ctx(|c| std::mem::take(&mut c.scopes)); for fr in frames { for c in fr { c.cancel_task(); } } }\n",
                "        let mut st = t.inner.0.lock().unwrap(); st.result = Some(r); drop(st); __ray_notify(&t.inner); __ray_bump();\n",
                "    });\n",
                "    let t2 = task.clone();\n",
                "    __ray_ctx(|c| { if let Some(frame) = c.scopes.last_mut() { frame.push(std::boxed::Box::new(t2)); } });\n",
                "    task\n}\n",
            ));
        } else {
            out.push_str(concat!(
                // Pool de hilos (M96): `spawn` REUSA un worker ocioso en vez de crear un hilo del SO por
                // tarea (el webserver spawn-ea por PETICIÓN → miles de creaciones/s bajo carga). Es un
                // thread-cache CRECIENTE (nunca bloquea al spawner: sin worker ocioso → hilo nuevo), porque
                // hay tareas que bloquean indefinidamente (fibras de conexión) y un pool fijo se moriría de
                // deadlock. Protocolo sin pérdida: un worker que agota su ocio solo SALE si logra quitarse
                // de la pila él mismo; si ya no está, es que un spawner lo pop-eó y su job llega (o llegó)
                // → recv bloqueante. El estado THREAD-LOCAL por tarea (token de cancelación, scopes) se
                // resetea entre jobs. El spawner recupera el job de un SendError (worker justo muerto).
                "type __RayJob = std::boxed::Box<dyn FnOnce() + Send + 'static>;\n",
                "type __RayPoolShard = std::sync::Mutex<Vec<(u64, std::sync::mpsc::Sender<__RayJob>)>>;\n",
                // M96e: el pool se SHARDEA (antes: un único Mutex<Vec<...>> global para TODO el proceso).
                // Cada request hace un spawn+retorno-a-pool (M56.5, panic→500), 2 adquisiciones del
                // mismo lock; bajo carga alta eso compite fuerte. Con N listas independientes
                // (round-robin atómico, sin relación entre el shard que elige el spawner y el que
                // elige el worker) la contención cae ~N× — a costa de que un pop puede fallar si el
                // único worker ocioso está en OTRO shard (crea un hilo nuevo de más; desperdicio
                // acotado, nunca deadlock: el invariante "sin worker ocioso → hilo nuevo" se preserva
                // igual, ahora por shard). N escala con los núcleos disponibles.
                "fn __ray_pool_shards() -> &'static [__RayPoolShard] {\n",
                "    static P: std::sync::OnceLock<Vec<__RayPoolShard>> = std::sync::OnceLock::new();\n",
                "    P.get_or_init(|| {\n",
                "        let n = std::thread::available_parallelism().map(|c| c.get()).unwrap_or(4).saturating_mul(2).clamp(4, 64);\n",
                "        (0..n).map(|_| std::sync::Mutex::new(Vec::new())).collect()\n",
                "    })\n}\n",
                "static __RAY_POOL_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);\n",
                "static __RAY_POOL_RR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);\n",
                "fn __ray_pool_next_shard(shards: &[__RayPoolShard]) -> usize {\n",
                "    __RAY_POOL_RR.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % shards.len()\n}\n",
                // M98.2: tras fallar el pop en el shard round-robin, se SONDEAN los demás shards antes de
                // crear un hilo. Sin el barrido había una TRAMPA DE PARIDAD: spawner (pop) y worker (park)
                // usan el MISMO contador round-robin; en churn secuencial `join(spawn(f))` las llamadas
                // alternan estrictamente (pops en valores pares, parks en impares) y con N shards PAR — N
                // siempre lo es: cores*2 — los residuos mod N son disjuntos → el spawner NUNCA veía al
                // worker aparcado → un hilo del SO nuevo por spawn → EAGAIN y crash en ~20k tareas. El
                // primer probe conserva la baja contención de M96e (el barrido solo corre en el miss).
                "fn __ray_pool_exec(job: __RayJob) {\n",
                "    let mut job = job;\n",
                "    let shards = __ray_pool_shards();\n",
                "    let start = __ray_pool_next_shard(shards);\n",
                "    for off in 0..shards.len() {\n",
                "        let idx = (start + off) % shards.len();\n",
                "        while let Some((_, tx)) = { let w = shards[idx].lock().unwrap().pop(); w } {\n",
                "            match tx.send(job) { Ok(()) => return, Err(e) => job = e.0 }\n",
                "        }\n",
                "    }\n",
                "    std::thread::spawn(move || {\n",
                "        let mut job = job;\n",
                "        loop {\n",
                "            job();\n",
                "            __RAY_CANCEL.with(|c| *c.borrow_mut() = None);\n",
                "            __SCOPES.with(|s| s.borrow_mut().clear());\n",
                "            let (tx, rx) = std::sync::mpsc::channel::<__RayJob>();\n",
                "            let id = __RAY_POOL_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);\n",
                "            let shards = __ray_pool_shards();\n",
                "            let shard_idx = __ray_pool_next_shard(shards);\n",
                "            shards[shard_idx].lock().unwrap().push((id, tx));\n",
                "            match rx.recv_timeout(std::time::Duration::from_secs(10)) {\n",
                "                Ok(next) => job = next,\n",
                "                Err(_) => {\n",
                "                    let mut pool = shards[shard_idx].lock().unwrap();\n",
                "                    if let Some(pos) = pool.iter().position(|(i, _)| *i == id) { pool.remove(pos); return; }\n",
                "                    drop(pool);\n",
                "                    match rx.recv() { Ok(next) => job = next, Err(_) => return }\n",
                "                }\n",
                "            }\n",
                "        }\n",
                "    });\n",
                "}\n",
                "fn __ray_spawn<T: Send + Clone + 'static, F: FnOnce() -> T + Send + 'static>(f: F) -> __RayTask<T> {\n",
                "    let task = __RayTask { inner: std::sync::Arc::new(__ray_sync_new(__TaskState { result: None })), cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), consumed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)) };\n",
                "    let t = task.clone();\n",
                "    __ray_pool_exec(std::boxed::Box::new(move || {\n",
                "        __RAY_CANCEL.with(|c| *c.borrow_mut() = Some(t.cancel.clone()));\n",
                "        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|e| __ray_panic_msg(&*e));\n",
                // Una hija que falla con tareas en vuelo cancela los hijos de sus scopes sin cerrar (el
                // unwinding se saltó los pops de __SCOPES) → transitiva, sin nietos huérfanos (M12.5).
                "        if r.is_err() { let frames = __SCOPES.with(|s| std::mem::take(&mut *s.borrow_mut())); for fr in frames { for c in fr { c.cancel_task(); } } }\n",
                "        let mut st = t.inner.0.lock().unwrap(); st.result = Some(r); drop(st); __ray_notify(&t.inner); __ray_bump();\n",
                "    }));\n",
                "    let t2 = task.clone();\n",
                "    __SCOPES.with(|s| { if let Some(frame) = s.borrow_mut().last_mut() { frame.push(std::boxed::Box::new(t2)); } });\n",
                "    task\n}\n",
            ));
        }
        out.push_str(concat!(
            // Salida del scope (ScopeEnd, M12.3+M12.5): espera a las hijas SIN orden fijo; si alguna
            // falló, cancela a las hermanas pendientes y propaga el fallo observado DE INMEDIATO (antes:
            // unión en orden de registro → un fallo podía esperar para siempre detrás de una hermana
            // bloqueada). La generación se lee ANTES de escanear: un cambio entre escaneo y espera
            // despierta al instante.
            "fn __ray_scope<R, F: FnOnce() -> R>(body: F) -> R {\n",
        ));
        // F2 (--fibers): la pila de scopes vive en el ctx de la fibra (body() puede aparcar y
        // reanudar en otro worker; el push/pop deben ver LA MISMA pila).
        if t.fibers {
            out.push_str("    __ray_ctx(|c| c.scopes.push(Vec::new()));\n    let r = body();\n    let frame = __ray_ctx(|c| c.scopes.pop().unwrap());\n");
        } else {
            out.push_str("    __SCOPES.with(|s| s.borrow_mut().push(Vec::new()));\n    let r = body();\n    let frame = __SCOPES.with(|s| s.borrow_mut().pop().unwrap());\n");
        }
        out.push_str(concat!(
            "    loop {\n",
            "        let act = __RAY_ACT_GEN.load(std::sync::atomic::Ordering::SeqCst);\n",
            "        if let Some(m) = frame.iter().find_map(|c| c.failed()) {\n",
            "            for c in &frame { c.cancel_task(); }\n",
            "            __ray_rt_err(&m);\n",
            "        }\n",
            "        if frame.iter().all(|c| c.done()) { break; }\n",
            "        __ray_wait_activity(act);\n",
            "    }\n",
            "    for c in &frame { c.consume(); }\n", // M98.1: las hijas no sobreviven al scope
            "    r\n}\n",
            // select (M12.4): espera a que algún canal de la lista esté LISTO para recibir (cola no vacía
            // ∨ cerrado) y devuelve el índice del PRIMERO listo (menor índice → determinista en el índice;
            // el ORDEN entre canales listos a la vez depende del scheduling, como la VM multicore por
            // default). Sin busy-wait (H21-N4): si ninguno está listo, espera en la condvar global de
            // actividad (la generación leída antes del escaneo evita perder un send concurrente).
            "fn __ray_select<T>(chs: &[__RayChan<T>]) -> i64 {\n",
            "    loop {\n",
            "        let act = __RAY_ACT_GEN.load(std::sync::atomic::Ordering::SeqCst);\n",
            "        for (i, ch) in chs.iter().enumerate() {\n",
            "            let st = ch.inner.0.lock().unwrap();\n",
            "            if !st.q.is_empty() || st.closed { return i as i64; }\n",
            "        }\n",
            "        __ray_wait_activity(act);\n",
            "    }\n}\n",
            // M116.1: select con PLAZO → índice listo, o -1 al vencer (el sitio lo envuelve en Option).
            // ms <= 0 = poll no bloqueante (escanea una vez, -1 si nada). El despertar por canal listo
            // es inmediato (notify de __ray_wait_activity); el vencimiento se nota en la vuelta siguiente
            // (el pulso de actividad acota la latencia a la cadencia de cancelación, como el resto).
            "fn __ray_select_timeout<T>(chs: &[__RayChan<T>], ms: i64) -> i64 {\n",
            "    let deadline = if ms > 0 { Some(std::time::Instant::now() + std::time::Duration::from_millis(ms as u64)) } else { None };\n",
            "    loop {\n",
            "        let act = __RAY_ACT_GEN.load(std::sync::atomic::Ordering::SeqCst);\n",
            "        for (i, ch) in chs.iter().enumerate() {\n",
            "            let st = ch.inner.0.lock().unwrap();\n",
            "            if !st.q.is_empty() || st.closed { return i as i64; }\n",
            "        }\n",
            "        match deadline { None => return -1, Some(d) => if std::time::Instant::now() >= d { return -1; } }\n",
            "        __ray_wait_activity_once(act);\n",
            "    }\n}\n",
        ));
    }
    // M107.2 (std/io.read): lectura de stdin POR BYTES. `poll(2)`+`read(2)` crudos en un mod propio
    // (los externs de signals declaran `read` a nivel raíz: el mod evita la colisión). Con fibras,
    // "¿listo YA?" (poll 0) y si no, aparcar la FIBRA en el reactor (`wait_readable(0)`) — el poll
    // previo cubre además el stdin-archivo regular, que epoll no acepta; sin fibras, lectura
    // bloqueante en el hilo de la tarea (correcto en hilo-por-tarea). EOF/error → vacío, como la VM.
    if t.needs_stdin {
        out.push_str(RT_WIN_STDIN);
        out.push_str(concat!(
            "#[cfg(unix)]
",
            "mod __ray_stdin {
",
            "    unsafe extern \"C\" {
",
            "        fn poll(fds: *mut PollFd, nfds: u64, timeout_ms: i32) -> i32;
",
            "        fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
",
            "    }
",
            "    #[repr(C)]
",
            "    struct PollFd { fd: i32, events: i16, revents: i16 }
",
            "    pub fn ready(timeout_ms: i32) -> bool {
",
            "        let mut pfd = PollFd { fd: 0, events: 0x1, revents: 0 };
",
            "        (unsafe { poll(&mut pfd, 1, timeout_ms) }) > 0
",
            "    }
",
            "    pub fn read_max(max: usize) -> Vec<u8> {
",
            "        let mut buf = vec![0u8; max];
",
            "        loop {
",
            "            let n = unsafe { read(0, buf.as_mut_ptr(), buf.len()) };
",
            "            if n >= 0 { buf.truncate(n as usize); return buf; }
",
            "            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted { return Vec::new(); }
",
            "        }
",
            "    }
",
            "}
",
        ));
        if t.fibers {
            out.push_str(concat!(
                "fn __ray_stdin_read(max: i64) -> Vec<u8> {
",
                "    let max = (max.max(1) as usize).min(1 << 20);
",
                "    #[cfg(unix)] { while !__ray_stdin::ready(0) { ray_runtime::fibers::wait_readable(0); } __ray_stdin::read_max(max) }
",
                "    #[cfg(windows)] { let _ = __ray_stdin::ready(-1); __ray_stdin::read_max(max) }
",
                "    #[cfg(not(any(unix, windows)))] { use std::io::Read; let mut b = vec![0u8; max]; let n = std::io::stdin().lock().read(&mut b).unwrap_or(0); b.truncate(n); b }
",
                "}
",
                "fn __ray_stdin_read_timeout(max: i64, ms: i64) -> Option<Vec<u8>> {
",
                "    let max = (max.max(1) as usize).min(1 << 20);
",
                "    #[cfg(unix)] {
",
                "        if !__ray_stdin::ready(0) {
",
                // wait_readable_timeout devuelve `true` si VENCIÓ (contrato de fibers.rs): la
                // negación de M107.2 lo tenía al revés — con dato listo devolvía None (lo
                // enmascaraba el re-chequeo de ready en el camino del timeout; lo destapó la
                // query DA1 de capabilities() dentro de raw, hallazgo de rallyx).
                "            if ms <= 0 || ray_runtime::fibers::wait_readable_timeout(0, ms) { return None; }
",
                "            if !__ray_stdin::ready(0) { return None; }
",
                "        }
",
                "        Some(__ray_stdin::read_max(max))
",
                "    }
",
                "    #[cfg(windows)] { if !__ray_stdin::ready(ms.clamp(0, i32::MAX as i64) as i32) { return None; } Some(__ray_stdin::read_max(max)) }
",
                "    #[cfg(not(any(unix, windows)))] { let _ = ms; use std::io::Read; let mut b = vec![0u8; max]; let n = std::io::stdin().lock().read(&mut b).unwrap_or(0); b.truncate(n); Some(b) }
",
                "}
",
            ));
        } else {
            out.push_str(concat!(
                "fn __ray_stdin_read(max: i64) -> Vec<u8> {
",
                "    let max = (max.max(1) as usize).min(1 << 20);
",
                "    #[cfg(unix)] { __ray_stdin::read_max(max) }
",
                "    #[cfg(windows)] { let _ = __ray_stdin::ready(-1); __ray_stdin::read_max(max) }
",
                "    #[cfg(not(any(unix, windows)))] { use std::io::Read; let mut b = vec![0u8; max]; let n = std::io::stdin().lock().read(&mut b).unwrap_or(0); b.truncate(n); b }
",
                "}
",
                "fn __ray_stdin_read_timeout(max: i64, ms: i64) -> Option<Vec<u8>> {
",
                "    let max = (max.max(1) as usize).min(1 << 20);
",
                "    #[cfg(unix)] {
",
                "        if !__ray_stdin::ready(ms.clamp(0, i32::MAX as i64) as i32) { return None; }
",
                "        Some(__ray_stdin::read_max(max))
",
                "    }
",
                "    #[cfg(windows)] { if !__ray_stdin::ready(ms.clamp(0, i32::MAX as i64) as i32) { return None; } Some(__ray_stdin::read_max(max)) }
",
                "    #[cfg(not(any(unix, windows)))] { let _ = ms; use std::io::Read; let mut b = vec![0u8; max]; let n = std::io::stdin().lock().read(&mut b).unwrap_or(0); b.truncate(n); Some(b) }
",
                "}
",
            ));
        }
    }
    // M107.3 (std/term): isatty/tamaño/modo crudo. El MISMO diseño que el host de la VM
    // (src/builtins.rs, mod term_host): termios como buffer OPACO de 128 bytes (el layout por SO
    // deja de importar), `cfmakeraw(3)` rellena los flags, y `atexit(3)` garantiza la
    // restauración en la salida normal y en `std::process::exit`. Unix; en otras plataformas
    // is_tty=false / size=[] / raw=err.
    if t.needs_term {
        out.push_str(RT_WIN_TERM);
        out.push_str(concat!(
            "#[cfg(unix)]\n",
            "mod __ray_term {\n",
            "    use std::sync::atomic::{AtomicBool, Ordering};\n",
            "    unsafe extern \"C\" {\n",
            "        fn isatty(fd: i32) -> i32;\n",
            "        fn tcgetattr(fd: i32, t: *mut u8) -> i32;\n",
            "        fn tcsetattr(fd: i32, act: i32, t: *const u8) -> i32;\n",
            "        fn cfmakeraw(t: *mut u8);\n",
            "        fn ioctl(fd: i32, req: u64, ...) -> i32;\n",
            "        fn atexit(f: extern \"C\" fn()) -> i32;\n",
            "        fn signal(sig: i32, handler: usize) -> usize;\n",
            "        fn raise(sig: i32) -> i32;\n",
            "    }\n",
            "    const SIG_DFL: usize = 0;\n",
            "    const SIG_ERR: usize = usize::MAX;\n",
            "    #[cfg(any(target_os = \"macos\", target_os = \"ios\", target_os = \"freebsd\"))]\n",
            "    const TIOCGWINSZ: u64 = 0x4008_7468;\n",
            "    #[cfg(not(any(target_os = \"macos\", target_os = \"ios\", target_os = \"freebsd\")))]\n",
            "    const TIOCGWINSZ: u64 = 0x5413;\n",
            "    #[repr(C)]\n",
            "    struct WinSize { rows: u16, cols: u16, xp: u16, yp: u16 }\n",
            "    static mut ORIGINAL: [u8; 128] = [0; 128];\n",
            "    static SAVED: AtomicBool = AtomicBool::new(false);\n",
            "    static ARMED: AtomicBool = AtomicBool::new(false);\n",
            "    static RAW_DEPTH: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);\n",
            "    pub fn is_tty(fd: i32) -> bool { unsafe { isatty(fd) == 1 } }\n",
            "    pub fn size() -> Option<(i64, i64)> {\n",
            "        for fd in [1, 0, 2] {\n",
            "            let mut ws = WinSize { rows: 0, cols: 0, xp: 0, yp: 0 };\n",
            "            if unsafe { ioctl(fd, TIOCGWINSZ, &mut ws as *mut WinSize) } == 0 && ws.cols > 0 { return Some((ws.cols as i64, ws.rows as i64)); }\n",
            "        }\n",
            "        None\n    }\n",
            "    pub fn size_px() -> Option<(i64, i64)> {\n",
            "        for fd in [1, 0, 2] {\n",
            "            let mut ws = WinSize { rows: 0, cols: 0, xp: 0, yp: 0 };\n",
            "            if unsafe { ioctl(fd, TIOCGWINSZ, &mut ws as *mut WinSize) } == 0 && ws.xp > 0 && ws.yp > 0 { return Some((ws.xp as i64, ws.yp as i64)); }\n",
            "        }\n",
            "        None\n    }\n",
            "    extern \"C\" fn restore() {\n",
            "        if SAVED.load(Ordering::Acquire) { unsafe { tcsetattr(0, 2, std::ptr::addr_of!(ORIGINAL) as *const u8) }; }\n",
            "    }\n",
            // atexit no corre ante señal fatal (ray dev relanza con SIGTERM): restaurar y
            // re-lanzar con la disposición default. Solo se arma sobre disposiciones DEFAULT
            // (signals()/SIG_IGN del programa mandan). Espejo exacto de src/builtins.rs.
            "    extern \"C\" fn restore_and_die(sig: i32) {\n",
            "        if SAVED.load(Ordering::Acquire) { unsafe { tcsetattr(0, 2, std::ptr::addr_of!(ORIGINAL) as *const u8) }; }\n",
            "        unsafe { signal(sig, SIG_DFL); raise(sig); }\n",
            "    }\n",
            "    fn arm_signal_restore() {\n",
            "        for sig in [1, 2, 15] {\n",
            "            let old = unsafe { signal(sig, restore_and_die as *const () as usize) };\n",
            "            if old != SIG_DFL && old != SIG_ERR { unsafe { signal(sig, old) }; }\n",
            "        }\n    }\n",
            "    fn disarm_signal_restore() {\n",
            "        let ours = restore_and_die as *const () as usize;\n",
            "        for sig in [1, 2, 15] {\n",
            "            let old = unsafe { signal(sig, SIG_DFL) };\n",
            "            if old != ours && old != SIG_ERR { unsafe { signal(sig, old) }; }\n",
            "        }\n    }\n",
            "    pub fn raw_on() -> Result<(), String> {\n",
            "        if RAW_DEPTH.load(Ordering::Acquire) > 0 { RAW_DEPTH.fetch_add(1, Ordering::AcqRel); return Ok(()); }\n",
            "        let mut cur = [0u8; 128];\n",
            "        unsafe {\n",
            "            if tcgetattr(0, cur.as_mut_ptr()) != 0 { return Err(format!(\"stdin is not a terminal: {}\", std::io::Error::last_os_error())); }\n",
            "            if !SAVED.load(Ordering::Acquire) { std::ptr::copy_nonoverlapping(cur.as_ptr(), std::ptr::addr_of_mut!(ORIGINAL) as *mut u8, 128); SAVED.store(true, Ordering::Release); }\n",
            "            if !ARMED.swap(true, Ordering::AcqRel) { atexit(restore); }\n",
            "            cfmakeraw(cur.as_mut_ptr());\n",
            "            if tcsetattr(0, 2, cur.as_ptr()) != 0 { return Err(format!(\"could not enter raw mode: {}\", std::io::Error::last_os_error())); }\n",
            "        }\n",
            "        arm_signal_restore();\n",
            "        RAW_DEPTH.fetch_add(1, Ordering::AcqRel);\n",
            "        Ok(())\n    }\n",
            "    pub fn raw_off() -> Result<(), String> {\n",
            "        if !SAVED.load(Ordering::Acquire) { return Ok(()); }\n",
            "        if RAW_DEPTH.load(Ordering::Acquire) > 1 { RAW_DEPTH.fetch_sub(1, Ordering::AcqRel); return Ok(()); }\n",
            "        if unsafe { tcsetattr(0, 2, std::ptr::addr_of!(ORIGINAL) as *const u8) } != 0 { return Err(format!(\"could not restore the terminal: {}\", std::io::Error::last_os_error())); }\n",
            "        disarm_signal_restore();\n",
            "        RAW_DEPTH.store(0, Ordering::Release);\n",
            "        Ok(())\n    }\n",
            "}\n",
            "fn __ray_term_is_tty(fd: i64) -> bool {\n",
            "    #[cfg(unix)] { __ray_term::is_tty(fd as i32) }\n",
            "    #[cfg(windows)] { __ray_term::is_tty(fd as i32) }\n",
            "    #[cfg(not(any(unix, windows)))] { let _ = fd; false }\n",
            "}\n",
            "fn __ray_term_size() -> Option<(i64, i64)> {\n",
            "    #[cfg(unix)] { __ray_term::size() }\n",
            "    #[cfg(windows)] { __ray_term::size() }\n",
            "    #[cfg(not(any(unix, windows)))] { None }\n",
            "}\n",
            "fn __ray_term_size_px() -> Option<(i64, i64)> {\n",
            "    #[cfg(unix)] { __ray_term::size_px() }\n",
            "    #[cfg(windows)] { __ray_term::size_px() }\n",
            "    #[cfg(not(any(unix, windows)))] { None }\n",
            "}\n",
            "fn __ray_term_raw(on: bool) -> Result<(), String> {\n",
            "    // El hilo escritor de print (M96f) es ASINCRONO: drenar ANTES de tocar el termios,\n",
            "    // o la salida encolada en modo cocido se escribiria ya en crudo (\\n sin \\r —\n",
            "    // escalera; hallazgo de raycode). Cubre entrar Y salir: ambos pasan por aqui.\n",
            "    __ray_flush_prints();\n",
            "    #[cfg(unix)] { if on { __ray_term::raw_on() } else { __ray_term::raw_off() } }\n",
            "    #[cfg(windows)] { if on { __ray_term::raw_on() } else { __ray_term::raw_off() } }\n",
            "    #[cfg(not(any(unix, windows)))] { let _ = on; Err(\"raw mode is not supported on this platform\".to_string()) }\n",
            "}\n",
        ));
    }
    // signals() (M88.1/M107.4): el canal de señales del SO (SIGTERM=15/SIGINT=2/SIGWINCH=28 — el
    // 28 coincide en macOS/BSD y Linux). El truco del self-pipe (como
    // la VM, `src/builtins.rs`): el handler (async-signal-safe: solo `write`) escribe el nº de señal a un
    // pipe; un hilo lector lo lee (bloqueante) y lo envía al canal. FFI a libc sin crates (siempre
    // enlazada). M168: en Windows el equivalente es `SetConsoleCtrlHandler` (Ctrl-C/Break → 2;
    // cierre/logoff/apagado → 15): el handler corre en un hilo del SO y envía al canal directo (sin
    // fibras en Windows, el canal es el de hilos). Otras plataformas: no compila (diferido).
    if t.needs_signals {
        out.push_str(concat!(
            "#[cfg(unix)] static __RAY_SIG_PIPE_W: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);\n",
            "#[cfg(unix)] unsafe extern \"C\" { fn pipe(fds: *mut i32) -> i32; fn read(fd: i32, buf: *mut u8, n: usize) -> isize; fn write(fd: i32, buf: *const u8, n: usize) -> isize; fn signal(sig: i32, handler: usize) -> usize; }\n",
            "#[cfg(unix)] extern \"C\" fn __ray_on_signal(sig: i32) {\n",
            "    let b = sig as u8; let w = __RAY_SIG_PIPE_W.load(std::sync::atomic::Ordering::Relaxed);\n",
            "    if w >= 0 { unsafe { let _ = write(w, &b as *const u8, 1); } }\n}\n",
            "#[cfg(windows)] #[link(name = \"kernel32\")] unsafe extern \"system\" { fn SetConsoleCtrlHandler(handler: Option<unsafe extern \"system\" fn(u32) -> i32>, add: i32) -> i32; }\n",
            "#[cfg(windows)] static __RAY_SIG_CHAN: std::sync::OnceLock<__RayChan<i64>> = std::sync::OnceLock::new();\n",
            "#[cfg(windows)] unsafe extern \"system\" fn __ray_on_ctrl(event: u32) -> i32 {\n",
            "    let sig: i64 = match event { 0 | 1 => 2, 2 | 5 | 6 => 15, _ => return 0 };\n",
            "    if let Some(ch) = __RAY_SIG_CHAN.get() { ch.send(sig); }\n",
            "    if sig == 15 { std::thread::sleep(std::time::Duration::from_millis(4000)); }\n",
            "    1\n}\n",
            "#[cfg(windows)] fn __ray_signals() -> __RayChan<i64> {\n",
            "    __RAY_SIG_CHAN.get_or_init(|| { let ch: __RayChan<i64> = __RayChan::make(None); unsafe { SetConsoleCtrlHandler(Some(__ray_on_ctrl), 1); } ch }).clone()\n}\n",
            "#[cfg(unix)] fn __ray_signals() -> __RayChan<i64> {\n",
            "    static CHAN: std::sync::OnceLock<__RayChan<i64>> = std::sync::OnceLock::new();\n",
            "    CHAN.get_or_init(|| {\n",
            "        let ch: __RayChan<i64> = __RayChan::make(None);\n",
            "        let mut fds = [0i32; 2];\n",
            "        unsafe { if pipe(fds.as_mut_ptr()) == 0 {\n",
            "            __RAY_SIG_PIPE_W.store(fds[1], std::sync::atomic::Ordering::Release);\n",
            "            signal(15, __ray_on_signal as *const () as usize);\n",
            "            signal(2, __ray_on_signal as *const () as usize);\n",
            "            signal(28, __ray_on_signal as *const () as usize);\n",
            "        } }\n",
            "        let rfd = fds[0]; let ch2 = ch.clone();\n",
            "        std::thread::spawn(move || loop {\n",
            "            let mut b = 0u8; let n = unsafe { read(rfd, &mut b as *mut u8, 1) };\n",
            "            if n == 1 { ch2.send(b as i64); } else if n == 0 { break; }\n",
            "        });\n",
            "        ch\n",
            "    }).clone()\n}\n",
        ));
    }
    // std/ffi.errno (revisión FFI jul 2026): lectura del errno del hilo actual. Mismo trío de
    // plataformas que src/ffi.rs (la impl de la VM) → mismo valor en ambos motores. Con externs
    // `blocking`, run_blocking (ray-runtime) ya repone en el worker el errno del hilo del pool.
    if t.needs_ffi_errno {
        out.push_str(concat!(
            "fn __ray_ffi_errno() -> i64 {\n",
            "    #[cfg(target_os = \"linux\")] unsafe extern \"C\" { #[link_name = \"__errno_location\"] fn __ray_errno_ptr() -> *mut i32; }\n",
            "    #[cfg(target_os = \"android\")] unsafe extern \"C\" { #[link_name = \"__errno\"] fn __ray_errno_ptr() -> *mut i32; }\n",
            "    #[cfg(all(unix, not(any(target_os = \"linux\", target_os = \"android\"))))] unsafe extern \"C\" { #[link_name = \"__error\"] fn __ray_errno_ptr() -> *mut i32; }\n",
            "    #[cfg(windows)] unsafe extern \"C\" { #[link_name = \"_errno\"] fn __ray_errno_ptr() -> *mut i32; }\n",
            "    unsafe { *__ray_errno_ptr() as i64 }\n",
            "}\n",
        ));
    }
    // PRNG (SplitMix64, mismo que la VM) + reloj monotónico, solo si el programa usa monotonic/random.
    // M96d: estado THREAD-LOCAL (antes: un único Mutex<u64> global). Bajo `log_requests` cada
    // request genera un trace_id/span_id vía `random.below` (net/trace.ray: 32+16 dígitos hex =
    // 48 llamadas) — con el estado global eso son 48 adquisiciones de lock POR PETICIÓN sobre un
    // único mutex, muchas más que las del registro de handles (M96c) y, medido bajo carga, el
    // cuello de botella dominante. Como el uso documentado es "identifican, no autentican — no
    // necesitan cripto" (net/trace.ray), no hace falta coordinación entre hilos: cada hilo lleva
    // su propia secuencia SplitMix64, sembrada distinto (reloj + un contador atómico) para que dos
    // hilos no repitan la misma secuencia. `random_seed` fija la semilla del hilo LLAMADOR
    // (semántica más simple que antes, no peor: ya no había reproducibilidad entre hilos con el
    // Mutex global tampoco, un `send`/mutación concurrente entre hilos competía igual por orden).
    if t.needs_time_rng {
        out.push_str(concat!(
            "fn __ray_monotonic_start() -> std::time::Instant {\n",
            "    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();\n",
            "    *START.get_or_init(std::time::Instant::now)\n}\n",
            "fn __ray_monotonic() -> i64 { __ray_monotonic_start().elapsed().as_millis() as i64 }\n",
            "fn __ray_monotonic_nanos() -> i64 { __ray_monotonic_start().elapsed().as_nanos() as i64 }\n",
            "fn __ray_rng_seed() -> u64 {\n",
            "    static CTR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);\n",
            "    let c = CTR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);\n",
            "    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9E37_79B9_7F4A_7C15);\n",
            "    t ^ c.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1)\n}\n",
            "thread_local! { static __RAY_RNG: std::cell::Cell<u64> = std::cell::Cell::new(__ray_rng_seed()); }\n",
            "fn __ray_next_u64() -> u64 {\n",
            "    __RAY_RNG.with(|c| {\n",
            "        let s = c.get().wrapping_add(0x9E37_79B9_7F4A_7C15); c.set(s);\n",
            "        let mut z = s;\n",
            "        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);\n",
            "        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);\n",
            "        z ^ (z >> 31)\n    })\n}\n",
            "fn __ray_random_f64() -> f64 { (__ray_next_u64() >> 11) as f64 / (1u64 << 53) as f64 }\n",
            "fn __ray_random_int(n: i64) -> i64 { if n <= 0 { 0 } else { (__ray_next_u64() % (n as u64)) as i64 } }\n",
            "fn __ray_random_seed(n: i64) { __RAY_RNG.with(|c| c.set(n as u64)); }\n",
        ));
    }
}
