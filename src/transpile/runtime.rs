//! El runtime embebido que emite el backend nativo (movimiento puro; usar `git log --follow`).
//!
//! Texto Rust literal (`out.push_str`/`concat!`), no lógica: el preámbulo SIEMPRE presente
//! (manejo de errores/panic, la repr SEND universal, aritmética checked/fast, helpers de
//! string/Map, `RayShow`) y los bloques BAJO DEMANDA según lo que use el programa (handles,
//! red, TLS, SQLite, concurrencia/canales/Task/scope/select, señales, reloj/PRNG, cripto).

use super::*;

pub(super) fn emit_core_runtime(out: &mut String, fast: bool, ahash: bool) {
    out.push_str("// Generado por el transpilador raylang→Rust (P2.b).\n");
    out.push_str("#![allow(unused_parens, unused_mut, dead_code, unused_variables)]\n");
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
        "T(Vec<__RaySend>), E(usize, Vec<__RaySend>) }\n",
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
    out.push_str("fn __ray_join(a: &Rc<std::cell::RefCell<Vec<Rc<str>>>>, sep: &str) -> Rc<str> {\n");
    out.push_str("    let v = a.borrow();\n");
    out.push_str("    let parts: Vec<&str> = v.iter().map(|s| &**s).collect();\n");
    out.push_str("    Rc::<str>::from(parts.join(sep))\n}\n");
    // index_of(s, sub) -> Option<int>: índice por CARÁCTER de la primera aparición de sub (como la VM;
    // sub vacío → Some(0)). Rust `str::find` da índice de BYTE, así que se compara por char.
    out.push_str("fn __ray_index_of(s: &str, sub: &str) -> Option<i64> {\n");
    out.push_str("    let chars: Vec<char> = s.chars().collect(); let sub: Vec<char> = sub.chars().collect();\n");
    out.push_str("    if sub.is_empty() { return Some(0); }\n");
    out.push_str("    if sub.len() > chars.len() { return None; }\n");
    out.push_str("    (0..=chars.len() - sub.len()).find(|&i| chars[i..i + sub.len()] == sub[..]).map(|i| i as i64)\n}\n");
    // N2: los `Map` del programa van tras el alias `__RayMap`. Con aHash (default), el mismo hasher que
    // el `MapStore` de la VM (P0.1) — SipHash es lento en claves string; con `--without ahash`, el
    // HashMap std puro. Todo el código generado construye con `__RayMap::default()`/`from_iter` (valen
    // para ambos hashers); los registros internos (sockets/TLS, clave i64) siguen en HashMap std.
    if ahash {
        out.push_str("type __RayMap<K, V> = std::collections::HashMap<K, V, ray_runtime::RandomState>;\n");
    } else {
        out.push_str("use std::collections::HashMap as __RayMap;\n");
    }
    out.push_str("fn __ray_sort<T: Ord + Clone>(a: &Rc<std::cell::RefCell<Vec<T>>>) -> Rc<std::cell::RefCell<Vec<T>>> {\n");
    out.push_str("    let mut v = a.borrow().clone(); v.sort(); Rc::new(std::cell::RefCell::new(v))\n}\n");
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
    // TLS reusa el registro de handles + `TcpStream` (accept/upgrade parten de un handle TCP) → implica net.
    if t.needs_rt_tls {
        t.needs_net = true;
    }
    // Registro global de handles de archivo (M11.8), solo si el programa los usa. Rust permite items
    // top-level en cualquier orden, así que va al final. Espejo del `FileRegistry` de la VM: un contador +
    // mapa handle→archivo tras un Mutex/OnceLock; los mensajes de error son byte-idénticos a la VM.
    // Registro de handles (M11.8): compartido por archivos y sockets. Se emite si el programa usa cualquiera.
    if t.needs_handles || t.needs_net || t.needs_rt_sqlite {
        // Variantes con-crate del registro, añadidas solo si el programa usa el subsistema: `Tls` (conexión
        // TLS bloqueante tras `Arc<Mutex>` propio → el I/O no retiene el lock global) y `Sqlite` (conexión
        // rusqlite; I/O local → se opera reteniendo el lock global, como la VM).
        let tls_variant = if t.needs_rt_tls {
            ", Tls(std::sync::Arc<std::sync::Mutex<ray_runtime::tls::TlsStream>>)"
        } else {
            ""
        };
        let sqlite_variant = if t.needs_rt_sqlite { ", Sqlite(ray_runtime::sqlite::Conn)" } else { "" };
        writeln!(
            out,
            "enum __RayHandle {{ Reader(std::io::BufReader<std::fs::File>), Writer(std::fs::File), Tcp(std::sync::Arc<std::net::TcpStream>), Listener(std::net::TcpListener), Udp(std::net::UdpSocket){tls_variant}{sqlite_variant} }}"
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
        let sock_evict = if t.needs_net {
            "__RAY_SOCK_CACHE.with(|c| { c.borrow_mut().remove(&h); }); "
        } else {
            ""
        };
        let tls_evict = if t.needs_rt_tls {
            "__RAY_TLS_CACHE.with(|c| { c.borrow_mut().remove(&h); }); "
        } else {
            ""
        };
        write!(out, "fn __ray_close(h: i64) -> i64 {{ __ray_reg().lock().unwrap().open.remove(&h); {sock_evict}{tls_evict}0 }}\n").unwrap();
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
            "fn __ray_write(h: i64, s: &str) -> Result<i64, Rc<str>> {\n",
            "    use std::io::Write; let mut reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get_mut(&h) {\n",
            "        Some(__RayHandle::Writer(f)) => f.write_all(s.as_bytes()).map(|_| s.chars().count() as i64).map_err(|e| Rc::<str>::from(e.to_string())),\n",
            "        Some(__RayHandle::Reader(_)) => Err(Rc::<str>::from(\"the handle is open for reading, not writing\")),\n",
            "        _ => Err(Rc::<str>::from(format!(\"invalid file handle: {}\", h))) } }\n",
        ));
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
            "thread_local! { static __RAY_SOCK_CACHE: std::cell::RefCell<std::collections::HashMap<i64, std::sync::Arc<std::net::TcpStream>>> = std::cell::RefCell::new(std::collections::HashMap::new()); }\n",
            "fn __ray_sock_clone(h: i64) -> Result<std::sync::Arc<std::net::TcpStream>, Rc<str>> {\n",
            "    if let Some(s) = __RAY_SOCK_CACHE.with(|c| c.borrow().get(&h).cloned()) { return Ok(s); }\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get(&h) { Some(__RayHandle::Tcp(s)) => { let s = std::sync::Arc::clone(s); drop(reg);\n",
            "            __RAY_SOCK_CACHE.with(|c| { c.borrow_mut().insert(h, std::sync::Arc::clone(&s)); }); Ok(s) },\n",
            "        Some(_) => Err(Rc::<str>::from(format!(\"handle {} is not a socket\", h))), None => Err(Rc::<str>::from(format!(\"invalid handle: {}\", h))) } }\n",
            "fn __ray_tcp_connect(host: &str, port: i64) -> Result<i64, Rc<str>> {\n",
            "    match std::net::TcpStream::connect((host, port as u16)) { Ok(s) => { let _ = s.set_nodelay(true); Ok(__ray_reg_insert(__RayHandle::Tcp(std::sync::Arc::new(s)))) }, Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
            "fn __ray_tcp_listen(host: &str, port: i64) -> Result<i64, Rc<str>> {\n",
            "    match std::net::TcpListener::bind((host, port as u16)) { Ok(l) => Ok(__ray_reg_insert(__RayHandle::Listener(l))), Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
            "fn __ray_tcp_accept(h: i64) -> Result<i64, Rc<str>> {\n",
            "    let l = { let reg = __ray_reg().lock().unwrap(); match reg.open.get(&h) { Some(__RayHandle::Listener(l)) => l.try_clone().map_err(|e| Rc::<str>::from(e.to_string())), _ => return Err(Rc::<str>::from(format!(\"handle {} is not a listener\", h))) } }?;\n",
            "    match l.accept() { Ok((s, _)) => { let _ = s.set_nodelay(true); Ok(__ray_reg_insert(__RayHandle::Tcp(std::sync::Arc::new(s)))) }, Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
        ));
        // socket_read/read_bytes/write DESPACHAN a TLS si el handle es una conexión TLS (solo si el
        // programa usa TLS): se clona el `Arc<Mutex<TlsStream>>` del registro y se hace I/O tras SU lock
        // (no el global) → conexiones concurrentes no se serializan. Si no, la vía TCP de siempre (clona el
        // stream para no retener el lock durante la I/O bloqueante).
        // Como la VM: SOLO las variantes `_bytes` despachan a TLS (socket_read/write string dan el error de
        // no-socket sobre un handle TLS). read tiene helper propio (matchea la VM: sin TLS); el `write`
        // compartido cubre write_bytes (el uso real de TLS) → lleva el despacho.
        let (tls_rdb, tls_wr) = if t.needs_rt_tls {
            (
                "if let Some(__t) = __ray_tls_get(h) { let mut __g = __t.lock().unwrap(); let mut buf = [0u8; 65536]; return match __g.read(&mut buf) { Ok(n) => Ok(Rc::<[u8]>::from(&buf[..n])), Err(e) => Err(Rc::<str>::from(e.to_string())) }; } ",
                "if let Some(__t) = __ray_tls_get(h) { let mut __g = __t.lock().unwrap(); return match __g.write_all(bytes) { Ok(()) => Ok(bytes.len() as i64), Err(e) => Err(Rc::<str>::from(e.to_string())) }; } ",
            )
        } else {
            ("", "")
        };
        write!(out, "fn __ray_socket_read(h: i64) -> Result<Rc<str>, Rc<str>> {{ use std::io::Read; let s = __ray_sock_clone(h)?; let mut r = &*s; let mut buf = [0u8; 65536]; match r.read(&mut buf) {{ Ok(n) => Ok(Rc::<str>::from(String::from_utf8_lossy(&buf[..n]).into_owned())), Err(e) => Err(Rc::<str>::from(e.to_string())) }} }}\n").unwrap();
        write!(out, "fn __ray_socket_read_bytes(h: i64) -> Result<Rc<[u8]>, Rc<str>> {{ {tls_rdb}use std::io::Read; let s = __ray_sock_clone(h)?; let mut r = &*s; let mut buf = [0u8; 65536]; match r.read(&mut buf) {{ Ok(n) => Ok(Rc::<[u8]>::from(&buf[..n])), Err(e) => Err(Rc::<str>::from(e.to_string())) }} }}\n").unwrap();
        write!(out, "fn __ray_socket_write(h: i64, bytes: &[u8]) -> Result<i64, Rc<str>> {{ {tls_wr}use std::io::Write; let s = __ray_sock_clone(h)?; let mut w = &*s; let mut off = 0; while off < bytes.len() {{ match w.write(&bytes[off..]) {{ Ok(0) => return Err(Rc::<str>::from(\"the connection closed during the write\")), Ok(n) => off += n, Err(e) => return Err(Rc::<str>::from(e.to_string())) }} }} Ok(bytes.len() as i64) }}\n").unwrap();
        out.push_str(concat!(
            "fn __ray_local_port(h: i64) -> i64 {\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get(&h) { Some(__RayHandle::Tcp(s)) => s.local_addr().map(|a| a.port() as i64).unwrap_or(0), Some(__RayHandle::Listener(l)) => l.local_addr().map(|a| a.port() as i64).unwrap_or(0), Some(__RayHandle::Udp(s)) => s.local_addr().map(|a| a.port() as i64).unwrap_or(0), _ => 0 } }\n",
            "fn __ray_set_read_timeout(h: i64, ms: i64) {\n",
            "    let d = if ms <= 0 { None } else { Some(std::time::Duration::from_millis(ms as u64)) };\n",
            // M96c: mismo fast-path que __ray_sock_clone — si ya está en la caché de ESTE hilo, ni
            // toca el lock global (es el llamador más frecuente: una vez por ciclo de lectura).
            "    if let Some(s) = __RAY_SOCK_CACHE.with(|c| c.borrow().get(&h).cloned()) { let _ = s.set_read_timeout(d); return; }\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    if let Some(__RayHandle::Tcp(s)) = reg.open.get(&h) { let s2 = std::sync::Arc::clone(s); let _ = s2.set_read_timeout(d); drop(reg);\n",
            "        __RAY_SOCK_CACHE.with(|c| { c.borrow_mut().insert(h, s2); }); } }\n",
            // UDP: los primitivos devuelven ARREGLOS ETIQUETADOS (bind/send → [\"ok\"/\"err\", ...]; recv →
            // [b\"ok\"/b\"err\", host, port, data]) que los wrappers de raylang (udp.ray) parsean. recv es
            // BLOQUEANTE (con hilos de SO reales; la VM usa no-bloqueante + scheduler → mismo efecto).
            "fn __ray_udp_bind(host: &str, port: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    match std::net::UdpSocket::bind((host, port as u16)) {\n",
            "        Ok(s) => { let id = __ray_reg_insert(__RayHandle::Udp(s)); Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(id.to_string())])) }\n",
            "        Err(e) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e.to_string())])) } }\n",
            "fn __ray_udp_clone(h: i64) -> Option<std::net::UdpSocket> { let reg = __ray_reg().lock().unwrap(); match reg.open.get(&h) { Some(__RayHandle::Udp(s)) => s.try_clone().ok(), _ => None } }\n",
            "fn __ray_udp_send_to(h: i64, host: &str, port: i64, data: &[u8]) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let r = match __ray_udp_clone(h) { Some(s) => s.send_to(data, (host, port as u16)).map_err(|e| e.to_string()), None => Err(format!(\"handle {} is not a UDP socket\", h)) };\n",
            "    match r { Ok(n) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(n.to_string())])), Err(e) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e)])) } }\n",
            "fn __ray_udp_recv_from(h: i64) -> Rc<std::cell::RefCell<Vec<Rc<[u8]>>>> {\n",
            "    match __ray_udp_clone(h) {\n",
            "        Some(s) => { let mut buf = vec![0u8; 65536]; match s.recv_from(&mut buf) {\n",
            "            Ok((n, addr)) => { buf.truncate(n); Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"ok\"[..]), Rc::<[u8]>::from(addr.ip().to_string().as_bytes()), Rc::<[u8]>::from(addr.port().to_string().as_bytes()), Rc::<[u8]>::from(&buf[..])])) }\n",
            "            Err(e) => Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"err\"[..]), Rc::<[u8]>::from(e.to_string().as_bytes())])) } }\n",
            "        None => Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"err\"[..]), Rc::<[u8]>::from(format!(\"handle {} is not a UDP socket\", h).as_bytes())])) } }\n",
        ));
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
            "thread_local! { static __RAY_TLS_CACHE: std::cell::RefCell<std::collections::HashMap<i64, Option<std::sync::Arc<std::sync::Mutex<ray_runtime::tls::TlsStream>>>>> = std::cell::RefCell::new(std::collections::HashMap::new()); }\n",
            "fn __ray_tls_get(h: i64) -> Option<std::sync::Arc<std::sync::Mutex<ray_runtime::tls::TlsStream>>> {\n",
            "    if let Some(v) = __RAY_TLS_CACHE.with(|c| c.borrow().get(&h).cloned()) { return v; }\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    let v = match reg.open.get(&h) { Some(__RayHandle::Tls(a)) => Some(a.clone()), _ => None };\n",
            "    drop(reg);\n",
            "    __RAY_TLS_CACHE.with(|c| { c.borrow_mut().insert(h, v.clone()); });\n",
            "    v\n}\n",
            "fn __ray_tls_tag_ok(id: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> { Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(id.to_string())])) }\n",
            "fn __ray_tls_tag_err(msg: String) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> { Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(msg)])) }\n",
            "fn __ray_tls_wrap(s: ray_runtime::tls::TlsStream) -> i64 { __ray_reg_insert(__RayHandle::Tls(std::sync::Arc::new(std::sync::Mutex::new(s)))) }\n",
            "fn __ray_tls_connect(host: &str, port: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    match ray_runtime::tls::connect(host, port) { Ok(s) => __ray_tls_tag_ok(__ray_tls_wrap(s)), Err(e) => __ray_tls_tag_err(e) } }\n",
            "fn __ray_tls_connect_h2(host: &str, port: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    match ray_runtime::tls::connect_h2(host, port) { Ok(s) => __ray_tls_tag_ok(__ray_tls_wrap(s)), Err(e) => __ray_tls_tag_err(e) } }\n",
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
            "        Some(__RayHandle::Tcp(s)) => { drop(reg); __RAY_SOCK_CACHE.with(|c| { c.borrow_mut().remove(&h); });\n",
            "            match std::sync::Arc::try_unwrap(s) { Ok(t) => Ok(t), Err(a) => a.try_clone().map_err(|e| e.to_string()) } }\n",
            "        Some(other) => { reg.open.insert(h, other); Err(format!(\"handle {} {}\", h, not_tcp_msg)) }\n",
            "        None => Err(format!(\"invalid handle: {}\", h)) } }\n",
            "fn __ray_tls_accept(h: i64, cert: &str, key: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let sock = match __ray_tls_take_tcp(h, \"is not an accepted TCP socket\") { Ok(s) => s, Err(e) => return __ray_tls_tag_err(e) };\n",
            "    match ray_runtime::tls::accept(sock, cert, key) {\n",
            "        Ok(s) => { let a = std::sync::Arc::new(std::sync::Mutex::new(s));\n",
            "            __ray_reg().lock().unwrap().open.insert(h, __RayHandle::Tls(a.clone()));\n",
            "            __RAY_TLS_CACHE.with(|c| { c.borrow_mut().insert(h, Some(a)); });\n",
            "            __ray_tls_tag_ok(h) }\n",
            "        Err(e) => __ray_tls_tag_err(e) } }\n",
            "fn __ray_tls_upgrade(h: i64, host: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let sock = match __ray_tls_take_tcp(h, \"is not a plain TCP socket\") { Ok(s) => s, Err(e) => return __ray_tls_tag_err(e) };\n",
            "    match ray_runtime::tls::upgrade(sock, host) {\n",
            "        Ok(s) => { let a = std::sync::Arc::new(std::sync::Mutex::new(s));\n",
            "            __ray_reg().lock().unwrap().open.insert(h, __RayHandle::Tls(a.clone()));\n",
            "            __RAY_TLS_CACHE.with(|c| { c.borrow_mut().insert(h, Some(a)); });\n",
            "            __ray_tls_tag_ok(h) }\n",
            "        Err(e) => __ray_tls_tag_err(e) } }\n",
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
    // Runtime de canales MPMC (concurrencia, M12.1/M12.2), solo si el programa usa spawn/canales. Es un
    // canal thread-safe propio (Arc<Mutex+Condvar>) — sin deps, ya que el `.rs` es standalone — con
    // backpressure (bounded) y cierre. FIFO como la VM. `T: Send` (primitivos en v1).
    if t.needs_concurrency {
        out.push_str(concat!(
            // `taken` cuenta los valores CONSUMIDOS (para el handshake rendezvous por generación) y
            // `senders` los emisores bloqueados (para que `close` los detecte, como la VM). Los panics
            // llevan el MISMO texto que el error de ejecución de la VM (exit code ≠ 70: diferido a H6).
            "struct __ChanState<T> { q: std::collections::VecDeque<T>, closed: bool, cap: Option<usize>, taken: u64, senders: usize }\n",
            "struct __RayChan<T> { inner: std::sync::Arc<(std::sync::Mutex<__ChanState<T>>, std::sync::Condvar)> }\n",
            "impl<T> Clone for __RayChan<T> { fn clone(&self) -> Self { __RayChan { inner: self.inner.clone() } } }\n",
            "impl<T: Send> __RayChan<T> {\n",
            "    fn make(cap: Option<usize>) -> Self { __RayChan { inner: std::sync::Arc::new((std::sync::Mutex::new(__ChanState { q: std::collections::VecDeque::new(), closed: false, cap, taken: 0, senders: 0 }), std::sync::Condvar::new())) } }\n",
            "    fn send(&self, v: T) {\n",
            "        let (m, cv) = &*self.inner; let mut st = m.lock().unwrap();\n",
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
            "            while !st.closed && !st.q.is_empty() { st = __ray_cv_wait(cv, st); if __ray_cancelled() { st.senders -= 1; drop(st); __ray_rt_err(\"task cancelled (a sibling failed)\"); } }\n",
            "            if st.closed { st.senders -= 1; drop(st); __ray_rt_err(\"send on a closed channel\"); }\n",
            "            st.q.push_back(v);\n",
            "            let my = st.taken + 1; cv.notify_all(); __ray_bump();\n",
            "            while !st.closed && st.taken < my { st = __ray_cv_wait(cv, st); if __ray_cancelled() { if st.taken < my { st.q.pop_back(); } st.senders -= 1; drop(st); __ray_rt_err(\"task cancelled (a sibling failed)\"); } }\n",
            "            st.senders -= 1;\n",
            "            if st.taken < my { drop(st); __ray_rt_err(\"send on a closed channel\"); }\n",
            "            return;\n",
            "        }\n",
            "        st.senders += 1;\n",
            "        while !st.closed && st.cap.map_or(false, |c| st.q.len() >= c) { st = __ray_cv_wait(cv, st); if __ray_cancelled() { st.senders -= 1; drop(st); __ray_rt_err(\"task cancelled (a sibling failed)\"); } }\n",
            "        st.senders -= 1;\n",
            "        if st.closed { drop(st); __ray_rt_err(\"send on a closed channel\"); }\n",
            "        st.q.push_back(v); cv.notify_all(); drop(st); __ray_bump();\n",
            "    }\n",
            "    fn recv(&self) -> Option<T> {\n",
            "        let (m, cv) = &*self.inner; let mut st = m.lock().unwrap();\n",
            "        while st.q.is_empty() && !st.closed { st = __ray_cv_wait(cv, st); if __ray_cancelled() { drop(st); __ray_rt_err(\"task cancelled (a sibling failed)\"); } }\n",
            "        let v = st.q.pop_front(); if v.is_some() { st.taken += 1; cv.notify_all(); } v\n",
            "    }\n",
            // `close` con un emisor bloqueado = error de ejecución en el sitio del close, como la VM
            // (M12.2; antes el emisor hacía return silencioso y su valor quedaba consumible).
            "    fn close(&self) {\n",
            "        let (m, cv) = &*self.inner; let mut st = m.lock().unwrap();\n",
            "        if st.senders > 0 { drop(st); __ray_rt_err(\"close on a channel with a blocked sender\"); }\n",
            "        st.closed = true; cv.notify_all(); drop(st); __ray_bump();\n",
            "    }\n",
            "}\n",
            // Condvar-wait con timeout corto: el despertar normal sigue llegando por `notify` (sin
            // latencia añadida); el timeout solo acota cuánto tarda una tarea bloqueada en NOTAR su
            // cancelación (cooperativa, H21-N3) → sin busy-wait.
            "fn __ray_cv_wait<'a, T>(cv: &std::sync::Condvar, g: std::sync::MutexGuard<'a, T>) -> std::sync::MutexGuard<'a, T> { cv.wait_timeout(g, std::time::Duration::from_millis(10)).unwrap().0 }\n",
            // Token de cancelación del hilo actual (lo instala `__ray_spawn`; `main` no tiene → false).
            "thread_local! { static __RAY_CANCEL: std::cell::RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> = std::cell::RefCell::new(None); }\n",
            "fn __ray_cancelled() -> bool { __RAY_CANCEL.with(|c| c.borrow().as_ref().map_or(false, |f| f.load(std::sync::atomic::Ordering::Relaxed))) }\n",
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
            "fn __ray_bump() {\n",
            "    __RAY_ACT_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);\n",
            "    if __RAY_ACT_WAITERS.load(std::sync::atomic::Ordering::SeqCst) > 0 { let _g = __RAY_ACT_M.lock().unwrap(); __RAY_ACT_CV.notify_all(); }\n",
            "}\n",
            "fn __ray_wait_activity(act: u64) {\n",
            "    __RAY_ACT_WAITERS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);\n",
            "    let mut g = __RAY_ACT_M.lock().unwrap();\n",
            "    while __RAY_ACT_GEN.load(std::sync::atomic::Ordering::SeqCst) == act {\n",
            "        g = __ray_cv_wait(&__RAY_ACT_CV, g);\n",
            "        if __ray_cancelled() { drop(g); __RAY_ACT_WAITERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst); __ray_rt_err(\"task cancelled (a sibling failed)\"); }\n",
            "    }\n",
            "    drop(g);\n",
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
            "struct __RayTask<T> { inner: std::sync::Arc<(std::sync::Mutex<__TaskState<T>>, std::sync::Condvar)>, cancel: std::sync::Arc<std::sync::atomic::AtomicBool>, consumed: std::sync::Arc<std::sync::atomic::AtomicBool> }\n",
            "impl<T> Clone for __RayTask<T> { fn clone(&self) -> Self { __RayTask { inner: self.inner.clone(), cancel: self.cancel.clone(), consumed: self.consumed.clone() } } }\n",
            "const __RAY_TASK_CONSUMED: &str = \"task already consumed (join/try_join takes the task)\";\n",
            "impl<T: Send + Clone + 'static> __RayTask<T> {\n",
            "    fn wait(&self) -> Result<T, String> {\n",
            "        let (m, cv) = &*self.inner; let mut st = m.lock().unwrap();\n",
            "        while st.result.is_none() { st = __ray_cv_wait(cv, st); if __ray_cancelled() { drop(st); __ray_rt_err(\"task cancelled (a sibling failed)\"); } }\n",
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
            "thread_local! { static __SCOPES: std::cell::RefCell<Vec<Vec<std::boxed::Box<dyn __RayScopeChild>>>> = std::cell::RefCell::new(Vec::new()); }\n",
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
            "    let task = __RayTask { inner: std::sync::Arc::new((std::sync::Mutex::new(__TaskState { result: None }), std::sync::Condvar::new())), cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), consumed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)) };\n",
            "    let t = task.clone();\n",
            "    __ray_pool_exec(std::boxed::Box::new(move || {\n",
            "        __RAY_CANCEL.with(|c| *c.borrow_mut() = Some(t.cancel.clone()));\n",
            "        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|e| __ray_panic_msg(&*e));\n",
            // Una hija que falla con tareas en vuelo cancela los hijos de sus scopes sin cerrar (el
            // unwinding se saltó los pops de __SCOPES) → transitiva, sin nietos huérfanos (M12.5).
            "        if r.is_err() { let frames = __SCOPES.with(|s| std::mem::take(&mut *s.borrow_mut())); for fr in frames { for c in fr { c.cancel_task(); } } }\n",
            "        let (m, cv) = &*t.inner; let mut st = m.lock().unwrap(); st.result = Some(r); cv.notify_all(); drop(st); __ray_bump();\n",
            "    }));\n",
            "    let t2 = task.clone();\n",
            "    __SCOPES.with(|s| { if let Some(frame) = s.borrow_mut().last_mut() { frame.push(std::boxed::Box::new(t2)); } });\n",
            "    task\n}\n",
            // Salida del scope (ScopeEnd, M12.3+M12.5): espera a las hijas SIN orden fijo; si alguna
            // falló, cancela a las hermanas pendientes y propaga el fallo observado DE INMEDIATO (antes:
            // unión en orden de registro → un fallo podía esperar para siempre detrás de una hermana
            // bloqueada). La generación se lee ANTES de escanear: un cambio entre escaneo y espera
            // despierta al instante.
            "fn __ray_scope<R, F: FnOnce() -> R>(body: F) -> R {\n",
            "    __SCOPES.with(|s| s.borrow_mut().push(Vec::new()));\n",
            "    let r = body();\n",
            "    let frame = __SCOPES.with(|s| s.borrow_mut().pop().unwrap());\n",
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
            "            let (m, _) = &*ch.inner; let st = m.lock().unwrap();\n",
            "            if !st.q.is_empty() || st.closed { return i as i64; }\n",
            "        }\n",
            "        __ray_wait_activity(act);\n",
            "    }\n}\n",
        ));
    }
    // signals() (M88.1): el canal de señales del SO (SIGTERM=15/SIGINT=2). El truco del self-pipe (como
    // la VM, `src/builtins.rs`): el handler (async-signal-safe: solo `write`) escribe el nº de señal a un
    // pipe; un hilo lector lo lee (bloqueante) y lo envía al canal. FFI a libc sin crates (siempre
    // enlazada). Unix; en otras plataformas signals() no se soporta (el checker lo permite, pero aquí
    // no compilaría → se documenta como diferido no-unix).
    if t.needs_signals {
        out.push_str(concat!(
            "static __RAY_SIG_PIPE_W: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);\n",
            "unsafe extern \"C\" { fn pipe(fds: *mut i32) -> i32; fn read(fd: i32, buf: *mut u8, n: usize) -> isize; fn write(fd: i32, buf: *const u8, n: usize) -> isize; fn signal(sig: i32, handler: usize) -> usize; }\n",
            "extern \"C\" fn __ray_on_signal(sig: i32) {\n",
            "    let b = sig as u8; let w = __RAY_SIG_PIPE_W.load(std::sync::atomic::Ordering::Relaxed);\n",
            "    if w >= 0 { unsafe { let _ = write(w, &b as *const u8, 1); } }\n}\n",
            "fn __ray_signals() -> __RayChan<i64> {\n",
            "    static CHAN: std::sync::OnceLock<__RayChan<i64>> = std::sync::OnceLock::new();\n",
            "    CHAN.get_or_init(|| {\n",
            "        let ch: __RayChan<i64> = __RayChan::make(None);\n",
            "        let mut fds = [0i32; 2];\n",
            "        unsafe { if pipe(fds.as_mut_ptr()) == 0 {\n",
            "            __RAY_SIG_PIPE_W.store(fds[1], std::sync::atomic::Ordering::Release);\n",
            "            signal(15, __ray_on_signal as *const () as usize);\n",
            "            signal(2, __ray_on_signal as *const () as usize);\n",
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
            "fn __ray_monotonic() -> i64 {\n",
            "    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();\n",
            "    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as i64\n}\n",
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
