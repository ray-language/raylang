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
    /// Una conexión SQLite embebida. En un `RefCell` porque armar la espera del candado
    /// (`arm_busy`) pide `&mut` y el registro de handles del binario nativo la presta compartida;
    /// cada llamada toma y suelta el préstamo dentro de sí misma.
    pub struct Conn {
        inner: std::cell::RefCell<rusqlite::Connection>,
    }

    // rayhotel #491: ¿preguntó SQLite a nuestro busy handler durante la llamada en curso? El
    // handler NUNCA espera (eso lo hace `db/sqlite` aparcando la fibra); solo deja constancia de
    // que el candado era de los que se pueden esperar. Cuando SQLite ve que esperar sería un
    // interbloqueo (una transacción que ya lee y quiere escribir) devuelve BUSY sin preguntar.
    thread_local! {
        static BUSY_ASKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    fn note_busy(_tries: i32) -> bool {
        BUSY_ASKED.with(|b| b.set(true));
        false
    }

    /// Antes de CADA llamada: instala el busy handler propio (un `PRAGMA busy_timeout` de un
    /// script lo habría sustituido por la espera de SQLite, que retiene el hilo) y limpia la marca.
    pub fn arm_busy(conn: &mut rusqlite::Connection) {
        BUSY_ASKED.with(|b| b.set(false));
        let _ = conn.busy_handler(Some(note_busy as fn(i32) -> bool));
    }

    /// El texto de un error de SQLite. `database is locked` A SECAS significa «reintentar la
    /// llamada entera es seguro y puede servir» — es lo único que `db/sqlite` reintenta. Los otros
    /// dos casos de BUSY llevan su motivo: `done` sentencias del script ya se aplicaron (repetirlo
    /// las aplicaría dos veces), o SQLite ni preguntó (esperar no puede resolverlo).
    pub fn error_text(e: &rusqlite::Error, done: usize) -> String {
        let busy = matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::DatabaseBusy);
        if !busy {
            return e.to_string();
        }
        if done > 0 {
            return format!("database is locked (after {done} statement(s) of the script had run; the rest did not run)");
        }
        if !BUSY_ASKED.with(|b| b.get()) {
            return "database is locked (not waited for: this transaction already holds a lock the other one needs — roll it back and retry, or start it with BEGIN IMMEDIATE)".to_string();
        }
        e.to_string()
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

    /// Ejecuta SQL sin devolver filas (INSERT/UPDATE/DDL/BEGIN/…) con parámetros posicionales
    /// (`?1`…) enlazados como texto; devuelve el número de filas afectadas.
    ///
    /// rayreq #247: antes `execute` corría SOLO la primera sentencia y devolvía `Ok` (una
    /// migración `CREATE TABLE a …; CREATE TABLE b …;` dejaba el esquema a medias sin error).
    /// Ahora: sin parámetros corre TODAS en orden (como `sqlite3_exec`; las filas de un SELECT o
    /// un PRAGMA se descartan) y suma las filas afectadas; con parámetros exige UNA sentencia y
    /// lo comprueba ANTES de ejecutar nada (a qué sentencia irían `?1`… sería ambiguo).
    pub fn exec_conn(conn: &mut rusqlite::Connection, sql: &str, params: &[String]) -> Result<i64, String> {
        let opt: Vec<Option<String>> = params.iter().cloned().map(Some).collect();
        exec_conn_opt(conn, sql, &opt)
    }

    /// rayreq #248: como [`exec_conn`] con parámetros que pueden ser `NULL` (`None`).
    pub fn exec_conn_opt(conn: &mut rusqlite::Connection, sql: &str, params: &[Option<String>]) -> Result<i64, String> {
        // rusqlite 0.41: `Batch` itera por el trait `FallibleIterator` (re-exportado), ya no por un
        // `next` propio.
        use rusqlite::fallible_iterator::FallibleIterator;
        arm_busy(conn);
        let conn = &*conn;
        // Sentencias del script ya terminadas: un BUSY después de la primera no se reintenta.
        let mut done = 0usize;
        let mut batch = rusqlite::Batch::new(conn, sql);
        // Se cuenta por el delta de `total_changes`: `changes()` solo lo actualizan
        // INSERT/UPDATE/DELETE y tras un DDL conserva el valor anterior (se sumaría dos veces).
        let before = conn.total_changes() as i64;
        if !params.is_empty() {
            let Some(mut stmt) = batch.next().map_err(|e| error_text(&e, 0))? else {
                return Ok(0);
            };
            // Preparar la siguiente puede fallar (`no such table` de una tabla que crearía la
            // primera): también eso es «hay más de una».
            if !matches!(batch.next(), Ok(None)) {
                return Err("sqlite: exec with parameters runs one statement, and the SQL has more; \
                            run the others in their own exec (without parameters they all run)"
                    .to_string());
            }
            let mut rows = stmt.query(rusqlite::params_from_iter(params.iter())).map_err(|e| error_text(&e, 0))?;
            while rows.next().map_err(|e| error_text(&e, 0))?.is_some() {}
            drop(rows);
            return Ok(conn.total_changes() as i64 - before);
        }
        while let Some(mut stmt) = batch.next().map_err(|e| error_text(&e, done))? {
            let mut rows = stmt.query([]).map_err(|e| error_text(&e, done))?;
            while rows.next().map_err(|e| error_text(&e, done))?.is_some() {}
            drop(rows);
            done += 1;
        }
        Ok(conn.total_changes() as i64 - before)
    }

    impl Conn {
        /// Ejecuta SQL sin devolver filas; ver [`exec_conn`].
        pub fn exec(&self, sql: &str, params: &[String]) -> Result<i64, String> {
            exec_conn(&mut self.inner.borrow_mut(), sql, params)
        }

        /// rayreq #248: `exec` con parámetros `NULL` (`None`).
        pub fn exec_opt(&self, sql: &str, params: &[Option<String>]) -> Result<i64, String> {
            exec_conn_opt(&mut self.inner.borrow_mut(), sql, params)
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
            query_conn(&mut self.inner.borrow_mut(), sql, params)
        }
    }

    /// Una consulta con filas sobre una conexión: `(ncols, celdas)` aplanadas, `None` = `NULL`.
    /// La comparten la VM y el binario nativo (una sola espera del candado, un solo texto).
    pub fn query_conn(conn: &mut rusqlite::Connection, sql: &str, params: &[String]) -> Result<(usize, Vec<Option<String>>), String> {
        arm_busy(conn);
        let mut stmt = conn.prepare(sql).map_err(|e| error_text(&e, 0))?;
        let ncols = stmt.column_count();
        let mut rows = stmt.query(rusqlite::params_from_iter(params.iter())).map_err(|e| error_text(&e, 0))?;
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
                Err(e) => return Err(error_text(&e, 0)),
            }
        }
        Ok((ncols, out))
    }

    /// Abre (o crea) la base en `path` (`":memory:"` = en memoria).
    pub fn open(path: &str) -> Result<Conn, String> {
        rusqlite::Connection::open(path).map(|inner| Conn { inner: std::cell::RefCell::new(inner) }).map_err(|e| e.to_string())
    }
}

#[cfg(feature = "sqlite")]
pub use imp::{exec_conn, exec_conn_opt, open, query_conn, Conn};

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
