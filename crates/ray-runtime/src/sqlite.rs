//! **SQLite embebido** (rusqlite, `bundled` → el C de SQLite dentro del binario) para el binario
//! transpilado (P2.b, Paso 2). Igual que la cripto/TLS: envolver la librería C a mano (dobles punteros,
//! lifetimes de statements) es peor ingeniería que delegar en el binding maduro.
//!
//! El binario transpilado guarda cada [`Conn`] en su registro de handles (una variante nueva). A
//! diferencia de TLS, la conexión NO nace de un handle TCP (es propia) y el I/O es LOCAL y rápido → se
//! opera reteniendo el lock global del registro (como la VM), sin `Arc<Mutex>` por conexión.
//!
//! El ciclo prepare→bind→step→finalize ocurre ENTERO dentro de cada método (el statement nunca escapa).
//! Las celdas se devuelven como texto (INTEGER/REAL → decimal, NULL → "", BLOB → hex), consistente con la
//! API `[[string]]` del wrapper `db/sqlite`. Sin la feature `sqlite`, todo es un *stub*.

#[cfg(feature = "sqlite")]
mod imp {
    /// Una conexión SQLite embebida, con el PLAZO de espera de un candado (rayhotel #491). El
    /// plazo vive aquí y no en SQLite porque su busy handler —el que instala `PRAGMA
    /// busy_timeout`— espera dentro de la librería C reteniendo el hilo; el nuestro no espera
    /// nunca (ver `note_busy`) y quien espera es `db/sqlite`, aparcando la fibra.
    pub struct Conn {
        inner: rusqlite::Connection,
        /// ms que una llamada espera un candado antes de fallar; 0 = falla en el acto.
        wait_ms: std::cell::Cell<i64>,
    }

    /// El plazo por defecto: el mismo que el binding instalaba por su cuenta en cada conexión.
    const DEFAULT_WAIT_MS: i64 = 5000;

    /// Representación de texto de una celda SQLite (para la API `[[string]]`).
    fn value_str(v: rusqlite::types::ValueRef<'_>) -> String {
        use rusqlite::types::ValueRef;
        match v {
            ValueRef::Null => String::new(),
            ValueRef::Integer(i) => i.to_string(),
            ValueRef::Real(f) => f.to_string(),
            ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
            ValueRef::Blob(b) => b.iter().map(|x| format!("{x:02x}")).collect(),
        }
    }

    // ¿Preguntó SQLite a nuestro busy handler durante el intento en curso? Cuando SQLite ve que
    // esperar sería un interbloqueo (una transacción que ya lee y quiere escribir mientras otra
    // tiene el candado) devuelve BUSY SIN preguntar: ese candado no se espera.
    thread_local! {
        static BUSY_ASKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    unsafe extern "C" fn note_busy(_arg: *mut std::ffi::c_void, _tries: std::ffi::c_int) -> std::ffi::c_int {
        BUSY_ASKED.with(|b| b.set(true));
        0 // no esperar: devolver SQLITE_BUSY ya
    }

    /// Instala el busy handler propio. Por la API de C y no por `Connection::busy_handler`, que
    /// pide `&mut` (aquí la conexión está prestada compartida mientras un script itera) y reserva
    /// memoria en cada instalación.
    fn install_busy_handler(conn: &rusqlite::Connection) {
        // SAFETY: `conn.handle()` es el `sqlite3*` vivo de esta conexión, que no se cierra durante
        // la llamada (la tenemos prestada). `note_busy` es una función sin estado propio (`arg`
        // nulo) que no desenrolla. `sqlite3_busy_handler` solo guarda el puntero a función.
        unsafe {
            rusqlite::ffi::sqlite3_busy_handler(conn.handle(), Some(note_busy), std::ptr::null_mut());
        }
    }

    fn is_busy(e: &rusqlite::Error) -> bool {
        matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::DatabaseBusy)
    }

    /// El texto de un BUSY que NO se puede esperar: SQLite ni preguntó.
    const NOT_WAITED: &str = "database is locked (not waited for: this transaction already holds a lock the other one needs — roll it back and retry, or start it with BEGIN IMMEDIATE)";

    /// ¿Es `sql` (UNA sentencia) un `PRAGMA busy_timeout`? `Some(true)` si fija el plazo (`= N` o
    /// `(N)`), `Some(false)` si solo lo lee. Mira que sea de verdad ese PRAGMA: una columna o un
    /// comentario con ese nombre no cuentan.
    fn busy_pragma(sql: &str) -> Option<bool> {
        let text = sql.trim().trim_end_matches(';').trim();
        let rest = text.get(..6).filter(|p| p.eq_ignore_ascii_case("pragma")).map(|_| text[6..].trim_start())?;
        let after = rest.get(..12).filter(|p| p.eq_ignore_ascii_case("busy_timeout")).map(|_| rest[12..].trim_start())?;
        match after.chars().next() {
            None => Some(false),
            Some('=' | '(') => Some(true),
            Some(_) => None, // `busy_timeout_x`…: otro nombre
        }
    }

    /// ¿Nombra `sql` ese PRAGMA en algún sitio? Filtro barato (sin copiar) antes de mirar
    /// sentencia a sentencia.
    fn mentions_busy_timeout(sql: &str) -> bool {
        sql.as_bytes().windows(12).any(|w| w.eq_ignore_ascii_case(b"busy_timeout"))
    }

    impl Conn {
        /// Tras ejecutar un `PRAGMA busy_timeout = N`: el plazo que SQLite acaba de tomar pasa a
        /// ser el nuestro (lo analiza SQLite, no nosotros), y su handler se cambia por el propio.
        fn adopt_busy_timeout(&self) {
            if let Ok(ms) = self.inner.query_row("PRAGMA busy_timeout", [], |r| r.get::<_, i64>(0)) {
                self.wait_ms.set(ms.max(0));
            }
            install_busy_handler(&self.inner);
        }

        /// Ejecuta UNA sentencia ya preparada y descarta sus filas.
        fn step_all(stmt: &mut rusqlite::Statement<'_>, params: &[Option<String>]) -> rusqlite::Result<()> {
            let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
            while rows.next()?.is_some() {}
            Ok(())
        }

        /// El fallo de la sentencia número `index` de la llamada (0 = la primera).
        /// - La primera con un candado que se puede esperar: `database is locked` A SECAS. No se
        ///   ha aplicado nada, así que quien llama (`db/sqlite`) puede repetir la llamada entera
        ///   tras aparcar la fibra.
        /// - SQLite ni preguntó (interbloqueo): no se espera, con su motivo.
        fn busy_text(e: &rusqlite::Error, index: usize) -> String {
            if !BUSY_ASKED.with(|b| b.get()) {
                return NOT_WAITED.to_string();
            }
            if index == 0 {
                return e.to_string();
            }
            format!("database is locked (after {index} statement(s) of the script had run; the rest did not run)")
        }

        /// Ejecuta SQL sin devolver filas; ver [`Conn::exec_opt`].
        pub fn exec(&self, sql: &str, params: &[String]) -> Result<i64, String> {
            let opt: Vec<Option<String>> = params.iter().cloned().map(Some).collect();
            self.exec_opt(sql, &opt)
        }

        /// Ejecuta SQL sin devolver filas (INSERT/UPDATE/DDL/BEGIN/…) con parámetros posicionales
        /// (`?1`…, `None` = `NULL`); devuelve el número de filas afectadas.
        ///
        /// rayreq #247: sin parámetros corre TODAS las sentencias en orden (como `sqlite3_exec`;
        /// las filas de un SELECT o un PRAGMA se descartan) y suma las filas afectadas; con
        /// parámetros exige UNA sentencia y lo comprueba ANTES de ejecutar nada.
        ///
        /// rayhotel #491, el candado. La PRIMERA sentencia que lo encuentra devuelve `database
        /// is locked` a secas: nada se aplicó y `db/sqlite` repite la llamada tras aparcar la
        /// fibra. Una sentencia POSTERIOR de un script no puede devolver el control (repetir la
        /// llamada aplicaría dos veces las anteriores), así que espera AQUÍ, reintentando esa
        /// sentencia hasta el plazo de la conexión — reteniendo el hilo, como hacía SQLite; es
        /// el caso raro, y el común (una sentencia por llamada) no retiene nada.
        pub fn exec_opt(&self, sql: &str, params: &[Option<String>]) -> Result<i64, String> {
            // rusqlite 0.41: `Batch` itera por el trait `FallibleIterator` (re-exportado).
            use rusqlite::fallible_iterator::FallibleIterator;
            let conn = &self.inner;
            install_busy_handler(conn);
            // Se cuenta por el delta de `total_changes`: `changes()` solo lo actualizan
            // INSERT/UPDATE/DELETE y tras un DDL conserva el valor anterior (se sumaría dos veces).
            let before = conn.total_changes() as i64;
            let mut batch = rusqlite::Batch::new(conn, sql);
            if !params.is_empty() {
                BUSY_ASKED.with(|b| b.set(false));
                let busy_or = |e: rusqlite::Error| if is_busy(&e) { Self::busy_text(&e, 0) } else { e.to_string() };
                let Some(mut stmt) = batch.next().map_err(busy_or)? else {
                    return Ok(0);
                };
                // Preparar la siguiente puede fallar (`no such table` de una tabla que crearía la
                // primera): también eso es «hay más de una».
                if !matches!(batch.next(), Ok(None)) {
                    return Err("sqlite: exec with parameters runs one statement, and the SQL has more; \
                                run the others in their own exec (without parameters they all run)"
                        .to_string());
                }
                Self::step_all(&mut stmt, params).map_err(busy_or)?;
                return Ok(conn.total_changes() as i64 - before);
            }
            let pragma_in_script = mentions_busy_timeout(sql);
            let mut index = 0usize;
            loop {
                // El plazo de la espera de ESTA sentencia (solo si no es la primera).
                let mut deadline: Option<std::time::Instant> = None;
                // Preparar. Un error aquí no avanza el `Batch`: reintentar prepara la misma.
                let mut stmt = loop {
                    BUSY_ASKED.with(|b| b.set(false));
                    match batch.next() {
                        Ok(None) => return Ok(conn.total_changes() as i64 - before),
                        Ok(Some(stmt)) => break stmt,
                        Err(e) => self.wait_or_fail(&e, index, &mut deadline)?,
                    }
                };
                // Ejecutar. Un BUSY deshace la sentencia entera: repetirla es seguro.
                loop {
                    BUSY_ASKED.with(|b| b.set(false));
                    match Self::step_all(&mut stmt, &[]) {
                        Ok(()) => break,
                        Err(e) => self.wait_or_fail(&e, index, &mut deadline)?,
                    }
                }
                if pragma_in_script && stmt.expanded_sql().as_deref().and_then(busy_pragma) == Some(true) {
                    drop(stmt);
                    self.adopt_busy_timeout();
                }
                index += 1;
            }
        }

        /// Ante el fallo de la sentencia `index`: `Ok(())` = era un candado que se puede esperar
        /// y queda plazo (ya se durmió un instante: reintentar); `Err` = el error que sale.
        fn wait_or_fail(&self, e: &rusqlite::Error, index: usize, deadline: &mut Option<std::time::Instant>) -> Result<(), String> {
            if !is_busy(e) {
                return Err(e.to_string());
            }
            if index == 0 || !BUSY_ASKED.with(|b| b.get()) {
                return Err(Self::busy_text(e, index));
            }
            let until = *deadline.get_or_insert_with(|| {
                std::time::Instant::now() + std::time::Duration::from_millis(self.wait_ms.get().max(0) as u64)
            });
            if std::time::Instant::now() >= until {
                return Err(Self::busy_text(e, index));
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
            Ok(())
        }

        /// Ejecuta una consulta con filas; devuelve `(ncols, celdas)` con las celdas aplanadas fila a fila
        /// (el consumidor reconstruye el `[[string]]`).
        pub fn query(&self, sql: &str, params: &[String]) -> Result<(usize, Vec<String>), String> {
            let (ncols, cells) = self.query_nulls(sql, params)?;
            Ok((ncols, cells.into_iter().map(|c| c.unwrap_or_default()).collect()))
        }

        /// M347 (rayauth R6): como `query`, pero cada celda es `None` si era `NULL` (el `""` de `query`
        /// no distingue un NULL de una cadena vacía).
        pub fn query_nulls(&self, sql: &str, params: &[String]) -> Result<(usize, Vec<Option<String>>), String> {
            let conn = &self.inner;
            // `PRAGMA busy_timeout` como consulta: leerlo da el plazo de ESTA conexión (el de SQLite
            // es 0 con nuestro handler); fijarlo lo adopta, como en `exec`.
            let pragma = if mentions_busy_timeout(sql) { busy_pragma(sql) } else { None };
            if pragma == Some(false) {
                return Ok((1, vec![Some(self.wait_ms.get().to_string())]));
            }
            install_busy_handler(conn);
            BUSY_ASKED.with(|b| b.set(false));
            let text = |e: rusqlite::Error| if is_busy(&e) { Self::busy_text(&e, 0) } else { e.to_string() };
            let mut stmt = conn.prepare(sql).map_err(text)?;
            let ncols = stmt.column_count();
            let mut rows = stmt.query(rusqlite::params_from_iter(params.iter())).map_err(text)?;
            let mut out = Vec::new();
            loop {
                match rows.next() {
                    Ok(Some(row)) => {
                        // Copiar cada celda ANTES de avanzar: el texto de un ValueRef solo vive hasta el
                        // siguiente paso del statement.
                        for i in 0..ncols {
                            let v = row.get_ref(i).map_err(|e| e.to_string())?;
                            out.push(if matches!(v, rusqlite::types::ValueRef::Null) { None } else { Some(value_str(v)) });
                        }
                    }
                    Ok(None) => break,
                    Err(e) => return Err(text(e)),
                }
            }
            drop(rows);
            drop(stmt);
            if pragma == Some(true) {
                self.adopt_busy_timeout();
            }
            Ok((ncols, out))
        }
    }

    /// Abre (o crea) la base en `path` (`":memory:"` = en memoria).
    pub fn open(path: &str) -> Result<Conn, String> {
        let inner = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
        // El binding abre con el busy handler de SQLite (5 s reteniendo el hilo): se cambia por el
        // propio, y esos 5 s pasan a ser el plazo de la conexión.
        install_busy_handler(&inner);
        Ok(Conn { inner, wait_ms: std::cell::Cell::new(DEFAULT_WAIT_MS) })
    }

    #[cfg(test)]
    mod tests {
        use super::busy_pragma;

        #[test]
        fn busy_pragma_is_only_the_pragma() {
            assert_eq!(busy_pragma("PRAGMA busy_timeout = 200"), Some(true));
            assert_eq!(busy_pragma("  pragma BUSY_TIMEOUT(0);"), Some(true));
            assert_eq!(busy_pragma("PRAGMA busy_timeout"), Some(false));
            assert_eq!(busy_pragma("PRAGMA busy_timeout;"), Some(false));
            assert_eq!(busy_pragma("UPDATE settings SET busy_timeout = 0 WHERE id = 1"), None);
            assert_eq!(busy_pragma("PRAGMA busy_timeout_ms = 3"), None);
            assert_eq!(busy_pragma("PRAGMA synchronous = NORMAL"), None);
            assert_eq!(busy_pragma("-- busy_timeout = 0"), None);
        }
    }
}

#[cfg(feature = "sqlite")]
pub use imp::{open, Conn};

// --- Stubs sin la feature `sqlite` (el binario compila sin rusqlite; el consumidor no los alcanza) ---
#[cfg(not(feature = "sqlite"))]
mod stub {
    pub struct Conn(std::convert::Infallible);
    impl Conn {
        pub fn exec(&self, _sql: &str, _params: &[String]) -> Result<i64, String> {
            match self.0 {}
        }
        pub fn exec_opt(&self, _sql: &str, _params: &[Option<String>]) -> Result<i64, String> {
            match self.0 {}
        }
        pub fn query(&self, _sql: &str, _params: &[String]) -> Result<(usize, Vec<String>), String> {
            match self.0 {}
        }
        pub fn query_nulls(&self, _sql: &str, _params: &[String]) -> Result<(usize, Vec<Option<String>>), String> {
            match self.0 {}
        }
    }
    pub fn open(_path: &str) -> Result<Conn, String> {
        Err("SQLite not available (build without the 'sqlite' feature)".to_string())
    }
}

#[cfg(not(feature = "sqlite"))]
pub use stub::{open, Conn};
