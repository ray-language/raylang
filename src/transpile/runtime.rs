//! Emisión de los BLOQUES DE RUNTIME del binario transpilado (extraído de transpile.rs): el registro de
//! handles (`__RayHandle`), las ops de archivo/socket/TLS/SQLite, el runtime de canales (concurrencia),
//! señales y time/RNG. Todo es Rust-como-texto, emitido condicionalmente según los `needs_*` del
//! `Transpiler`. Se separa de la lógica de `transpile_with` por volumen; el comportamiento no cambia.

use std::fmt::Write;

/// Emite (al final del programa Rust) los bloques de runtime que el programa necesita. Las variantes con
/// crate (`Tls`/`Sqlite` del `__RayHandle`, los helpers `__ray_tls_*`/`__ray_sqlite_*`) solo si el flag
/// correspondiente está activo. `t.needs_net` ya incluye el ajuste por TLS (lo hace el llamador).
pub(super) fn emit(out: &mut String, t: &super::Transpiler) {
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
            "enum __RayHandle {{ Reader(std::io::BufReader<std::fs::File>), Writer(std::fs::File), Tcp(std::net::TcpStream), Listener(std::net::TcpListener), Udp(std::net::UdpSocket){tls_variant}{sqlite_variant} }}"
        )
        .unwrap();
        out.push_str(concat!(
            "struct __RayReg { next: i64, open: __RayMap<i64, __RayHandle> }\n",
            "fn __ray_reg() -> &'static std::sync::Mutex<__RayReg> {\n",
            "    static R: std::sync::OnceLock<std::sync::Mutex<__RayReg>> = std::sync::OnceLock::new();\n",
            "    R.get_or_init(|| std::sync::Mutex::new(__RayReg { next: 1, open: __RayMap::new() }))\n}\n",
            "fn __ray_reg_insert(h: __RayHandle) -> i64 { let mut reg = __ray_reg().lock().unwrap(); let id = reg.next; reg.next += 1; reg.open.insert(id, h); id }\n",
            "fn __ray_close(h: i64) -> i64 { __ray_reg().lock().unwrap().open.remove(&h); 0 }\n",
        ));
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
            "fn __ray_sock_clone(h: i64) -> Result<std::net::TcpStream, Rc<str>> {\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get(&h) { Some(__RayHandle::Tcp(s)) => s.try_clone().map_err(|e| Rc::<str>::from(e.to_string())),\n",
            "        Some(_) => Err(Rc::<str>::from(format!(\"handle {} is not a socket\", h))), None => Err(Rc::<str>::from(format!(\"invalid handle: {}\", h))) } }\n",
            "fn __ray_tcp_connect(host: &str, port: i64) -> Result<i64, Rc<str>> {\n",
            "    match std::net::TcpStream::connect((host, port as u16)) { Ok(s) => Ok(__ray_reg_insert(__RayHandle::Tcp(s))), Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
            "fn __ray_tcp_listen(host: &str, port: i64) -> Result<i64, Rc<str>> {\n",
            "    match std::net::TcpListener::bind((host, port as u16)) { Ok(l) => Ok(__ray_reg_insert(__RayHandle::Listener(l))), Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
            "fn __ray_tcp_accept(h: i64) -> Result<i64, Rc<str>> {\n",
            "    let l = { let reg = __ray_reg().lock().unwrap(); match reg.open.get(&h) { Some(__RayHandle::Listener(l)) => l.try_clone().map_err(|e| Rc::<str>::from(e.to_string())), _ => return Err(Rc::<str>::from(format!(\"handle {} is not a listener\", h))) } }?;\n",
            "    match l.accept() { Ok((s, _)) => Ok(__ray_reg_insert(__RayHandle::Tcp(s))), Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
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
        write!(out, "fn __ray_socket_read(h: i64) -> Result<Rc<str>, Rc<str>> {{ use std::io::Read; let mut s = __ray_sock_clone(h)?; let mut buf = [0u8; 65536]; match s.read(&mut buf) {{ Ok(n) => Ok(Rc::<str>::from(String::from_utf8_lossy(&buf[..n]).into_owned())), Err(e) => Err(Rc::<str>::from(e.to_string())) }} }}\n").unwrap();
        write!(out, "fn __ray_socket_read_bytes(h: i64) -> Result<Rc<[u8]>, Rc<str>> {{ {tls_rdb}use std::io::Read; let mut s = __ray_sock_clone(h)?; let mut buf = [0u8; 65536]; match s.read(&mut buf) {{ Ok(n) => Ok(Rc::<[u8]>::from(&buf[..n])), Err(e) => Err(Rc::<str>::from(e.to_string())) }} }}\n").unwrap();
        write!(out, "fn __ray_socket_write(h: i64, bytes: &[u8]) -> Result<i64, Rc<str>> {{ {tls_wr}use std::io::Write; let mut s = __ray_sock_clone(h)?; let mut off = 0; while off < bytes.len() {{ match s.write(&bytes[off..]) {{ Ok(0) => return Err(Rc::<str>::from(\"the connection closed during the write\")), Ok(n) => off += n, Err(e) => return Err(Rc::<str>::from(e.to_string())) }} }} Ok(bytes.len() as i64) }}\n").unwrap();
        out.push_str(concat!(
            "fn __ray_local_port(h: i64) -> i64 {\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get(&h) { Some(__RayHandle::Tcp(s)) => s.local_addr().map(|a| a.port() as i64).unwrap_or(0), Some(__RayHandle::Listener(l)) => l.local_addr().map(|a| a.port() as i64).unwrap_or(0), Some(__RayHandle::Udp(s)) => s.local_addr().map(|a| a.port() as i64).unwrap_or(0), _ => 0 } }\n",
            "fn __ray_set_read_timeout(h: i64, ms: i64) {\n",
            "    let d = if ms <= 0 { None } else { Some(std::time::Duration::from_millis(ms as u64)) };\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    if let Some(__RayHandle::Tcp(s)) = reg.open.get(&h) { let _ = s.set_read_timeout(d); } }\n",
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
            // Clona el Arc<Mutex<TlsStream>> del handle (si es TLS) → la I/O va tras su lock, no el global.
            "fn __ray_tls_get(h: i64) -> Option<std::sync::Arc<std::sync::Mutex<ray_runtime::tls::TlsStream>>> {\n",
            "    let reg = __ray_reg().lock().unwrap(); match reg.open.get(&h) { Some(__RayHandle::Tls(a)) => Some(a.clone()), _ => None } }\n",
            "fn __ray_tls_tag_ok(id: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> { Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(id.to_string())])) }\n",
            "fn __ray_tls_tag_err(msg: String) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> { Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(msg)])) }\n",
            "fn __ray_tls_wrap(s: ray_runtime::tls::TlsStream) -> i64 { __ray_reg_insert(__RayHandle::Tls(std::sync::Arc::new(std::sync::Mutex::new(s)))) }\n",
            "fn __ray_tls_connect(host: &str, port: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    match ray_runtime::tls::connect(host, port) { Ok(s) => __ray_tls_tag_ok(__ray_tls_wrap(s)), Err(e) => __ray_tls_tag_err(e) } }\n",
            "fn __ray_tls_connect_h2(host: &str, port: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    match ray_runtime::tls::connect_h2(host, port) { Ok(s) => __ray_tls_tag_ok(__ray_tls_wrap(s)), Err(e) => __ray_tls_tag_err(e) } }\n",
            // Saca el TcpStream del handle `h` (debe ser TCP), lo deja fuera del registro y lo devuelve.
            "fn __ray_tls_take_tcp(h: i64) -> Result<std::net::TcpStream, String> {\n",
            "    let mut reg = __ray_reg().lock().unwrap(); match reg.open.remove(&h) {\n",
            "        Some(__RayHandle::Tcp(s)) => Ok(s),\n",
            "        Some(other) => { reg.open.insert(h, other); Err(format!(\"handle {} is not an accepted TCP socket\", h)) }\n",
            "        None => Err(format!(\"invalid handle: {}\", h)) } }\n",
            "fn __ray_tls_accept(h: i64, cert: &str, key: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let sock = match __ray_tls_take_tcp(h) { Ok(s) => s, Err(e) => return __ray_tls_tag_err(e) };\n",
            "    match ray_runtime::tls::accept(sock, cert, key) { Ok(s) => { __ray_reg().lock().unwrap().open.insert(h, __RayHandle::Tls(std::sync::Arc::new(std::sync::Mutex::new(s)))); __ray_tls_tag_ok(h) } Err(e) => __ray_tls_tag_err(e) } }\n",
            "fn __ray_tls_upgrade(h: i64, host: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let sock = match __ray_tls_take_tcp(h) { Ok(s) => s, Err(e) => return __ray_tls_tag_err(e) };\n",
            "    match ray_runtime::tls::upgrade(sock, host) { Ok(s) => { __ray_reg().lock().unwrap().open.insert(h, __RayHandle::Tls(std::sync::Arc::new(std::sync::Mutex::new(s)))); __ray_tls_tag_ok(h) } Err(e) => __ray_tls_tag_err(e) } }\n",
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
            "struct __ChanState<T> { q: std::collections::VecDeque<T>, closed: bool, cap: Option<usize> }\n",
            "struct __RayChan<T> { inner: std::sync::Arc<(std::sync::Mutex<__ChanState<T>>, std::sync::Condvar)> }\n",
            "impl<T> Clone for __RayChan<T> { fn clone(&self) -> Self { __RayChan { inner: self.inner.clone() } } }\n",
            "impl<T: Send> __RayChan<T> {\n",
            "    fn make(cap: Option<usize>) -> Self { __RayChan { inner: std::sync::Arc::new((std::sync::Mutex::new(__ChanState { q: std::collections::VecDeque::new(), closed: false, cap }), std::sync::Condvar::new())) } }\n",
            "    fn send(&self, v: T) {\n",
            "        let (m, cv) = &*self.inner; let mut st = m.lock().unwrap();\n",
            "        while !st.closed && st.cap.map_or(false, |c| st.q.len() >= c) { st = cv.wait(st).unwrap(); }\n",
            "        if st.closed { return; }\n",
            "        st.q.push_back(v); cv.notify_all();\n",
            "    }\n",
            "    fn recv(&self) -> Option<T> {\n",
            "        let (m, cv) = &*self.inner; let mut st = m.lock().unwrap();\n",
            "        while st.q.is_empty() && !st.closed { st = cv.wait(st).unwrap(); }\n",
            "        let v = st.q.pop_front(); if v.is_some() { cv.notify_all(); } v\n",
            "    }\n",
            "    fn close(&self) { let (m, cv) = &*self.inner; m.lock().unwrap().closed = true; cv.notify_all(); }\n",
            "}\n",
            // Structured concurrency (M12.3): Task<T> = un JoinHandle envuelto (Arc<Mutex>) que cachea el
            // resultado (join una vez ejecuta el hilo; joins posteriores devuelven el clon cacheado → una
            // tarea puede unirse explícitamente O por el scope, no dos veces).
            "struct __TaskState<T> { handle: Option<std::thread::JoinHandle<T>>, result: Option<T> }\n",
            "struct __RayTask<T> { inner: std::sync::Arc<std::sync::Mutex<__TaskState<T>>> }\n",
            "impl<T> Clone for __RayTask<T> { fn clone(&self) -> Self { __RayTask { inner: self.inner.clone() } } }\n",
            "impl<T: Send + Clone + 'static> __RayTask<T> {\n",
            "    fn join(&self) -> T {\n",
            "        let mut st = self.inner.lock().unwrap();\n",
            "        if let Some(h) = st.handle.take() { let r = h.join().unwrap(); st.result = Some(r); }\n",
            "        st.result.clone().unwrap()\n",
            "    }\n",
            "}\n",
            // Cada scope activo (por hilo) acumula clausuras que unen las tareas lanzadas dentro; al salir
            // el scope las ejecuta (une todas). `spawn` registra su tarea en el scope más interno, si hay.
            "thread_local! { static __SCOPES: std::cell::RefCell<Vec<Vec<Box<dyn FnOnce()>>>> = std::cell::RefCell::new(Vec::new()); }\n",
            "fn __ray_spawn<T: Send + Clone + 'static, F: FnOnce() -> T + Send + 'static>(f: F) -> __RayTask<T> {\n",
            "    let task = __RayTask { inner: std::sync::Arc::new(std::sync::Mutex::new(__TaskState { handle: Some(std::thread::spawn(f)), result: None })) };\n",
            "    let t = task.clone();\n",
            "    __SCOPES.with(|s| { if let Some(frame) = s.borrow_mut().last_mut() { frame.push(Box::new(move || { let _ = t.join(); })); } });\n",
            "    task\n}\n",
            "fn __ray_scope<R, F: FnOnce() -> R>(body: F) -> R {\n",
            "    __SCOPES.with(|s| s.borrow_mut().push(Vec::new()));\n",
            "    let r = body();\n",
            "    let frame = __SCOPES.with(|s| s.borrow_mut().pop().unwrap());\n",
            "    for j in frame { j(); }\n",
            "    r\n}\n",
            // select (M12.4): espera a que algún canal de la lista esté LISTO para recibir (cola no vacía
            // ∨ cerrado) y devuelve el índice del PRIMERO listo (menor índice → determinista en el índice;
            // el ORDEN entre canales listos a la vez depende del scheduling, como la VM multicore por
            // default). Poll con backoff (std no tiene un select multi-condvar; el resultado es correcto).
            "fn __ray_select<T>(chs: &[__RayChan<T>]) -> i64 {\n",
            "    loop {\n",
            "        for (i, ch) in chs.iter().enumerate() {\n",
            "            let (m, _) = &*ch.inner; let st = m.lock().unwrap();\n",
            "            if !st.q.is_empty() || st.closed { return i as i64; }\n",
            "        }\n",
            "        std::thread::sleep(std::time::Duration::from_micros(50));\n",
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
    // Estado global tras un Mutex/OnceLock; sembrado del reloj. No determinista → casa por propiedades.
    if t.needs_time_rng {
        out.push_str(concat!(
            "fn __ray_monotonic() -> i64 {\n",
            "    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();\n",
            "    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as i64\n}\n",
            "fn __ray_rng() -> &'static std::sync::Mutex<u64> {\n",
            "    static R: std::sync::OnceLock<std::sync::Mutex<u64>> = std::sync::OnceLock::new();\n",
            "    R.get_or_init(|| std::sync::Mutex::new(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9E37_79B9_7F4A_7C15)))\n}\n",
            "fn __ray_next_u64() -> u64 {\n",
            "    let mut s = __ray_rng().lock().unwrap();\n",
            "    *s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);\n",
            "    let mut z = *s;\n",
            "    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);\n",
            "    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);\n",
            "    z ^ (z >> 31)\n}\n",
            "fn __ray_random_f64() -> f64 { (__ray_next_u64() >> 11) as f64 / (1u64 << 53) as f64 }\n",
            "fn __ray_random_int(n: i64) -> i64 { if n <= 0 { 0 } else { (__ray_next_u64() % (n as u64)) as i64 } }\n",
            "fn __ray_random_seed(n: i64) { *__ray_rng().lock().unwrap() = n as u64; }\n",
        ));
    }
}
