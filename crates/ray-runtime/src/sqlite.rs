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
    /// Una conexión SQLite embebida, con el punto de REANUDACIÓN de un script que se quedó a medias
    /// esperando un candado (ver [`Conn::exec_opt`]).
    pub struct Conn {
        inner: rusqlite::Connection,
        resume: std::cell::RefCell<Option<Resume>>,
    }

    /// Un script interrumpido por un candado: su texto, cuántas sentencias ya corrieron y las
    /// filas que afectaron. La misma llamada, repetida, sigue desde ahí.
    struct Resume {
        sql: String,
        done: usize,
        changes: i64,
    }

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

    // rayhotel #491: ¿preguntó SQLite a nuestro busy handler durante la llamada en curso? El
    // handler NUNCA espera (eso lo hace `db/sqlite`, aparcando la fibra): solo deja constancia de
    // que el candado era de los que se pueden esperar. Cuando SQLite ve que esperar sería un
    // interbloqueo (una transacción que ya lee y quiere escribir) devuelve BUSY sin preguntar.
    thread_local! {
        static BUSY_ASKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    unsafe extern "C" fn note_busy(_arg: *mut std::ffi::c_void, _tries: std::ffi::c_int) -> std::ffi::c_int {
        BUSY_ASKED.with(|b| b.set(true));
        0 // no esperar: devolver SQLITE_BUSY ya
    }

    /// Instala el busy handler propio. Por la API de C y no por `Connection::busy_handler`, que
    /// pide `&mut` (aquí la conexión está prestada compartida mientras un script itera) y reserva
    /// memoria en cada instalación. Un `PRAGMA busy_timeout` lo sustituye por la espera de SQLite,
    /// que retiene el hilo: por eso se reinstala antes de cada llamada y tras cada sentencia de
    /// un script que nombre ese PRAGMA.
    fn install_busy_handler(conn: &rusqlite::Connection) {
        // SAFETY: `conn.handle()` es el `sqlite3*` vivo de esta conexión, que no se cierra durante
        // la llamada (la tenemos prestada). `note_busy` es una función sin estado propio (`arg`
        // nulo) que no desenrolla. `sqlite3_busy_handler` solo guarda el puntero a función.
        unsafe {
            rusqlite::ffi::sqlite3_busy_handler(conn.handle(), Some(note_busy), std::ptr::null_mut());
        }
    }

    /// Antes de cada llamada: handler propio y marca limpia.
    fn arm_busy(conn: &rusqlite::Connection) {
        BUSY_ASKED.with(|b| b.set(false));
        install_busy_handler(conn);
    }

    fn is_busy(e: &rusqlite::Error) -> bool {
        matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::DatabaseBusy)
    }

    /// El texto de un BUSY que NO se puede esperar: SQLite ni preguntó.
    const NOT_WAITED: &str = "database is locked (not waited for: this transaction already holds a lock the other one needs — roll it back and retry, or start it with BEGIN IMMEDIATE)";

    /// El fallo de una ejecución: el texto y, si fue un candado que SÍ se puede esperar, cuántas
    /// sentencias del script habían terminado y las filas que afectaron (para reanudar).
    struct ExecFail {
        message: String,
        waitable: Option<(usize, i64)>,
    }

    fn fail(e: rusqlite::Error, done: usize, changes: i64) -> ExecFail {
        if !is_busy(&e) {
            return ExecFail { message: e.to_string(), waitable: None };
        }
        if !BUSY_ASKED.with(|b| b.get()) {
            return ExecFail { message: NOT_WAITED.to_string(), waitable: None };
        }
        ExecFail { message: e.to_string(), waitable: Some((done, changes)) }
    }

    /// Ejecuta SQL sin devolver filas (INSERT/UPDATE/DDL/BEGIN/…) con parámetros posicionales
    /// (`?1`…); devuelve el número de filas afectadas.
    ///
    /// rayreq #247: sin parámetros corre TODAS las sentencias en orden (como `sqlite3_exec`; las
    /// filas de un SELECT o un PRAGMA se descartan) y suma las filas afectadas; con parámetros
    /// exige UNA sentencia y lo comprueba ANTES de ejecutar nada. `skip`: sentencias del script que
    /// YA corrieron en un intento anterior — se preparan y se saltan.
    fn run_script(conn: &rusqlite::Connection, sql: &str, params: &[Option<String>], skip: usize) -> Result<i64, ExecFail> {
        // rusqlite 0.41: `Batch` itera por el trait `FallibleIterator` (re-exportado), ya no por un
        // `next` propio.
        use rusqlite::fallible_iterator::FallibleIterator;
        arm_busy(conn);
        // Un `PRAGMA busy_timeout` dentro del script devolvería la espera a SQLite para el resto
        // de la llamada: tras cada sentencia de un script que lo nombre se reinstala el handler.
        let rearm = sql.to_ascii_lowercase().contains("busy_timeout");
        let mut batch = rusqlite::Batch::new(conn, sql);
        // Se cuenta por el delta de `total_changes`: `changes()` solo lo actualizan
        // INSERT/UPDATE/DELETE y tras un DDL conserva el valor anterior (se sumaría dos veces).
        let before = conn.total_changes() as i64;
        let so_far = |done: usize| (done, conn.total_changes() as i64 - before);
        if !params.is_empty() {
            let Some(mut stmt) = batch.next().map_err(|e| fail(e, 0, 0))? else {
                return Ok(0);
            };
            // Preparar la siguiente puede fallar (`no such table` de una tabla que crearía la
            // primera): también eso es «hay más de una».
            if !matches!(batch.next(), Ok(None)) {
                return Err(ExecFail {
                    message: "sqlite: exec with parameters runs one statement, and the SQL has more; \
                              run the others in their own exec (without parameters they all run)"
                        .to_string(),
                    waitable: None,
                });
            }
            let mut rows = stmt.query(rusqlite::params_from_iter(params.iter())).map_err(|e| fail(e, 0, 0))?;
            while rows.next().map_err(|e| fail(e, 0, 0))?.is_some() {}
            drop(rows);
            return Ok(conn.total_changes() as i64 - before);
        }
        let mut done = 0usize;
        loop {
            let next = batch.next().map_err(|e| {
                let (d, c) = so_far(done);
                fail(e, d, c)
            })?;
            let Some(mut stmt) = next else { break };
            if done >= skip {
                let mut rows = stmt.query([]).map_err(|e| {
                    let (d, c) = so_far(done);
                    fail(e, d, c)
                })?;
                loop {
                    match rows.next() {
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(e) => {
                            let (d, c) = so_far(done);
                            return Err(fail(e, d, c));
                        }
                    }
                }
                drop(rows);
                if rearm {
                    install_busy_handler(conn);
                }
            }
            done += 1;
        }
        Ok(conn.total_changes() as i64 - before)
    }

    impl Conn {
        /// Ejecuta SQL sin devolver filas; ver [`Conn::exec_opt`].
        pub fn exec(&self, sql: &str, params: &[String]) -> Result<i64, String> {
            let opt: Vec<Option<String>> = params.iter().cloned().map(Some).collect();
            self.exec_opt(sql, &opt)
        }

        /// rayreq #248: `exec` con parámetros `NULL` (`None`).
        ///
        /// rayhotel #491: si una sentencia se topa con un candado que se puede esperar, el error
        /// es `database is locked` A SECAS y la conexión recuerda por dónde iba: REPETIR la misma
        /// llamada (lo que hace `db/sqlite`, tras aparcar la fibra) sigue desde la sentencia que
        /// falló, sin volver a aplicar las anteriores. Cualquier otra llamada olvida ese punto.
        pub fn exec_opt(&self, sql: &str, params: &[Option<String>]) -> Result<i64, String> {
            let (skip, carried) = match self.resume.borrow_mut().take() {
                Some(r) if r.sql == sql => (r.done, r.changes),
                _ => (0, 0),
            };
            match run_script(&self.inner, sql, params, skip) {
                Ok(n) => Ok(n + carried),
                Err(f) => {
                    if let Some((done, changes)) = f.waitable
                        && done > 0
                    {
                        *self.resume.borrow_mut() = Some(Resume { sql: sql.to_string(), done, changes: changes + carried });
                    }
                    Err(f.message)
                }
            }
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
            self.resume.borrow_mut().take(); // otra llamada: el script a medias ya no se reanuda
            let conn = &self.inner;
            arm_busy(conn);
            let text = |e: rusqlite::Error| fail(e, 0, 0).message;
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
            // Un `PRAGMA busy_timeout = N` enviado como consulta también cambia el handler.
            install_busy_handler(conn);
            Ok((ncols, out))
        }
    }

    /// Abre (o crea) la base en `path` (`":memory:"` = en memoria).
    pub fn open(path: &str) -> Result<Conn, String> {
        let inner = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
        // El binding abre con un busy_timeout de 5 s (la espera que retiene el hilo): fuera.
        install_busy_handler(&inner);
        Ok(Conn { inner, resume: std::cell::RefCell::new(None) })
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
