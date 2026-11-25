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

/// Marcas INTERNAS de este módulo: los errores con que `Conn` dice que la llamada encontró un
/// candado que se puede esperar (`<marca><carácter> <filas> <prefijo>`) o que la conexión está
/// reservada por otra llamada que espera. No salen de aquí como texto de error: [`error_fields`]
/// los convierte en las respuestas etiquetadas `busy` y `turn` del primitivo.
#[cfg_attr(not(feature = "sqlite"), allow(dead_code))]
const BUSY_MARK: &str = "\u{1}busy ";
#[cfg_attr(not(feature = "sqlite"), allow(dead_code))]
const TURN_MARK: &str = "\u{1}turn";

/// Los campos de la respuesta etiquetada de un primitivo `__sqlite_*` que falló:
/// - `["err", msg]`;
/// - `["busy", C, N, prefijo]`: encontró un candado que se puede esperar. Si era un script, sus
///   primeras sentencias quedaron aplicadas (`N` filas) y la que lo encontró empieza en el
///   carácter `C` del texto enviado (`C` = 0: no se aplicó nada). La conexión queda RESERVADA
///   para quien repita la llamada con `prefijo` delante del SQL;
/// - `["turn"]`: la conexión está reservada por otra llamada que espera; repetir más tarde.
///
/// Los dos motores (VM y binario nativo) construyen su arreglo con esto: la etiqueta se decide
/// en un solo sitio.
pub fn error_fields(message: String) -> Vec<String> {
    if message == TURN_MARK {
        return vec!["turn".to_string()];
    }
    if let Some(rest) = message.strip_prefix(BUSY_MARK) {
        let mut parts = rest.splitn(3, ' ');
        if let (Some(at), Some(changed), Some(prefix)) = (parts.next(), parts.next(), parts.next()) {
            return vec!["busy".to_string(), at.to_string(), changed.to_string(), prefix.to_string()];
        }
    }
    vec!["err".to_string(), message]
}

#[cfg(feature = "sqlite")]
mod imp {
    use super::{BUSY_MARK, TURN_MARK};

    /// Una conexión SQLite embebida, con dos modos de esperar un candado (rayhotel #491):
    /// - el de SQLite (por defecto, el de siempre): su busy handler espera dentro de la librería
    ///   C, reteniendo el hilo. Es lo que ve un `db` ≤ 0.10, que no sabe esperar por su cuenta.
    /// - APARCADO (`db` ≥ 0.11 lo pide al conectar con [`PARK_SWITCH`]): nuestro handler no espera
    ///   nunca (ver `note_busy`), el plazo se guarda aquí y quien espera es `db/sqlite`,
    ///   aparcando la fibra y repitiendo la llamada.
    ///
    /// **La reserva.** Mientras una llamada espera fuera (aparcada entre dos intentos), la
    /// conexión es SUYA: otra fibra que la comparta no debe ejecutar nada en medio — podría caer
    /// dentro de la transacción que un script dejó abierta. La reserva se toma en el mismo
    /// instante en que se devuelve el `busy` (con el registro de handles aún tomado: no hay
    /// hueco), la identifica un prefijo que solo conoce quien recibió ese `busy`, y se suelta
    /// cuando su llamada termina, cuando la suelta a propósito ([`RELEASE`]) o cuando caduca
    /// (una fibra cancelada mientras esperaba no deja la conexión cerrada para siempre). Una
    /// llamada que no encuentra candado —la de siempre— solo paga mirar que no hay reserva.
    pub struct Conn {
        inner: rusqlite::Connection,
        parked: std::cell::Cell<bool>,
        /// Modo aparcado: ms que una llamada espera un candado antes de fallar; 0 = en el acto.
        wait_ms: std::cell::Cell<i64>,
        reserved: std::cell::RefCell<Option<Reservation>>,
        /// Para que cada reserva tenga un prefijo distinto.
        reservations: std::cell::Cell<u64>,
    }

    struct Reservation {
        /// `/*ray-turn:N*/`: un comentario SQL, así que lo que sigue es el SQL tal cual.
        prefix: String,
        until: std::time::Instant,
    }

    /// La sentencia EXACTA con que `db/sqlite` pide el modo aparcado. Un runtime anterior se la
    /// pasa a SQLite, que ignora un PRAGMA que no conoce: el paquete sigue esperando como antes.
    pub const PARK_SWITCH: &str = "PRAGMA ray_busy_wait = park";

    /// Lo que sigue al prefijo de una reserva para soltarla sin ejecutar nada (plazo vencido).
    pub const RELEASE: &str = "PRAGMA ray_busy_wait = release";

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
    // En reposo vale `false`: solo se lee —y se apaga— al ver un BUSY (`take_asked`), así el
    // camino sin candado no toca el thread-local.
    thread_local! {
        static BUSY_ASKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    fn take_asked() -> bool {
        BUSY_ASKED.with(|b| b.replace(false))
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

    /// Salta espacios y comentarios SQL (`-- …` y `/* … */`) al principio de `text`.
    fn skip_blank(mut text: &str) -> &str {
        loop {
            text = text.trim_start();
            if let Some(rest) = text.strip_prefix("--") {
                text = rest.split_once('\n').map_or("", |(_, after)| after);
            } else if let Some(rest) = text.strip_prefix("/*") {
                text = rest.split_once("*/").map_or("", |(_, after)| after);
            } else {
                return text;
            }
        }
    }

    /// Quita de `text` la palabra `word` inicial (sin distinguir mayúsculas), si es una palabra entera.
    fn strip_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
        let head = text.get(..word.len()).filter(|h| h.eq_ignore_ascii_case(word))?;
        let rest = &text[head.len()..];
        if rest.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') { None } else { Some(rest) }
    }

    /// ¿Es `sql` (UNA sentencia) un `PRAGMA busy_timeout`? `Some(true)` si fija el plazo (`= N` o
    /// `(N)`), `Some(false)` si solo lo lee. Mira que sea de verdad ese PRAGMA —con comentarios
    /// delante o detrás y con esquema (`main.busy_timeout`)—: una columna con ese nombre no cuenta.
    fn busy_pragma(sql: &str) -> Option<bool> {
        let mut rest = skip_blank(strip_word(skip_blank(sql), "pragma")?);
        if strip_word(rest, "busy_timeout").is_none() {
            // `esquema.busy_timeout`
            let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
            rest = skip_blank(skip_blank(&rest[end..]).strip_prefix('.')?);
        }
        let after = skip_blank(strip_word(rest, "busy_timeout")?);
        match after.chars().next() {
            None | Some(';') => Some(false),
            Some('=' | '(') => Some(true),
            Some(_) => None,
        }
    }

    /// ¿Nombra `sql` ese PRAGMA en algún sitio? Filtro barato (sin copiar): busca el `_` —raro en
    /// SQL corriente— y mira alrededor.
    fn mentions_busy_timeout(sql: &str) -> bool {
        let bytes = sql.as_bytes();
        bytes.iter().enumerate().any(|(i, &b)| {
            b == b'_' && i >= 4 && bytes.len() >= i + 8 && bytes[i - 4..i + 8].eq_ignore_ascii_case(b"busy_timeout")
        })
    }

    impl Conn {
        /// Modo aparcado, tras una sentencia cuyo texto nombra `busy_timeout`: si fijó el plazo,
        /// pasa a ser el nuestro y el handler de SQLite se cambia otra vez por el propio. Un valor
        /// distinto de 0 en SQLite solo puede venir de ese PRAGMA (instalar nuestro handler lo deja
        /// a 0), se escriba como se escriba; el 0 se reconoce por el texto.
        fn sync_busy_timeout(&self, statement: &str) {
            let set = self.inner.query_row("PRAGMA busy_timeout", [], |r| r.get::<_, i64>(0)).unwrap_or(0);
            if set > 0 {
                self.wait_ms.set(set);
            } else if busy_pragma(statement) == Some(true) {
                self.wait_ms.set(0);
            }
            // Siempre: si el PRAGMA corrió escrito de una forma que el texto no reconoce, el
            // handler de SQLite sustituyó al nuestro igual.
            install_busy_handler(&self.inner);
        }

        /// Modo aparcado, al entrar en una llamada: ¿puede correr ahora? Devuelve el SQL sin el
        /// prefijo de reserva y si quien llama es el dueño de la reserva vigente.
        fn admit<'s>(&self, sql: &'s str) -> Result<(&'s str, bool), String> {
            let mut reserved = self.reserved.borrow_mut();
            let Some(r) = reserved.as_ref() else {
                return Ok((sql, false));
            };
            if let Some(rest) = sql.strip_prefix(r.prefix.as_str()) {
                return Ok((rest, true));
            }
            if std::time::Instant::now() >= r.until {
                *reserved = None; // caducó: su dueño ya no va a volver
                return Ok((sql, false));
            }
            Err(TURN_MARK.to_string())
        }

        /// Modo aparcado: la llamada encontró un candado que se puede esperar. Reserva la
        /// conexión para quien la repita (o renueva la reserva, si ya era suya) y compone el
        /// error marcado: `at` = octetos de `sql` ya ejecutados, `changed` = filas afectadas.
        fn reserve(&self, sql: &str, at: usize, changed: i64, owner: bool) -> String {
            let until = std::time::Instant::now() + std::time::Duration::from_millis(self.wait_ms.get().max(0) as u64 + 1000);
            let mut reserved = self.reserved.borrow_mut();
            match reserved.as_mut().filter(|_| owner) {
                Some(r) => r.until = until,
                None => {
                    let n = self.reservations.get() + 1;
                    self.reservations.set(n);
                    *reserved = Some(Reservation { prefix: format!("/*ray-turn:{n}*/"), until });
                }
            }
            let prefix = reserved.as_ref().map(|r| r.prefix.as_str()).unwrap_or("");
            // El carácter se cuenta sobre el texto que llegó SIN prefijo: quien llama guarda el
            // SQL que queda sin él y se lo antepone en cada intento.
            format!("{BUSY_MARK}{} {changed} {prefix}", sql[..at].chars().count())
        }

        /// La llamada del dueño de la reserva terminó (bien o con un error que no es un candado
        /// esperable): la conexión vuelve a ser de todos.
        fn release(&self, owner: bool) {
            if owner {
                *self.reserved.borrow_mut() = None;
            }
        }

        /// Ejecuta UNA sentencia ya preparada y descarta sus filas.
        fn step_all<P: rusqlite::ToSql>(stmt: &mut rusqlite::Statement<'_>, params: &[P]) -> rusqlite::Result<()> {
            let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
            while rows.next()?.is_some() {}
            Ok(())
        }

        /// Ejecuta SQL sin devolver filas; ver [`Conn::exec_opt`].
        pub fn exec(&self, sql: &str, params: &[String]) -> Result<i64, String> {
            self.run(sql, params)
        }

        /// Como [`Conn::exec`], con parámetros que pueden ser `NULL` (`None`).
        pub fn exec_opt(&self, sql: &str, params: &[Option<String>]) -> Result<i64, String> {
            self.run(sql, params)
        }

        /// Ejecuta SQL sin devolver filas (INSERT/UPDATE/DDL/BEGIN/…) con parámetros posicionales
        /// (`?1`…, `None` = `NULL`); devuelve el número de filas afectadas.
        ///
        /// rayreq #247: sin parámetros corre TODAS las sentencias en orden (como `sqlite3_exec`;
        /// las filas de un SELECT o un PRAGMA se descartan) y suma las filas afectadas; con
        /// parámetros exige UNA sentencia y lo comprueba ANTES de ejecutar nada.
        ///
        /// rayhotel #491, modo aparcado. Esta llamada no espera nunca un candado. Si lo encuentra
        /// y se puede esperar, reserva la conexión y devuelve el error marcado (`busy`, ver
        /// `error_fields`) con lo que un script ya aplicó: `db/sqlite` aparca la fibra y llama
        /// otra vez con el prefijo de la reserva y el RESTO del texto. Por dónde va el script lo
        /// lleva quien llama, no la conexión.
        fn run<P: rusqlite::ToSql>(&self, sql: &str, params: &[P]) -> Result<i64, String> {
            // rusqlite 0.41: `Batch` itera por el trait `FallibleIterator` (re-exportado).
            use rusqlite::fallible_iterator::FallibleIterator;
            let conn = &self.inner;
            if sql == PARK_SWITCH {
                // El plazo que SQLite tuviera (los 5 s con que abre el binding) pasa a ser el nuestro.
                let current = conn.query_row("PRAGMA busy_timeout", [], |r| r.get::<_, i64>(0)).unwrap_or(0);
                if !self.parked.replace(true) {
                    self.wait_ms.set(current.max(0));
                }
                install_busy_handler(conn);
                return Ok(0);
            }
            let parked = self.parked.get();
            let (sql, owner) = if parked { self.admit(sql)? } else { (sql, false) };
            if owner && sql == RELEASE {
                self.release(true);
                return Ok(0);
            }
            let watch_pragma = parked && mentions_busy_timeout(sql);
            // Se cuenta por el delta de `total_changes`: `changes()` solo lo actualizan
            // INSERT/UPDATE/DELETE y tras un DDL conserva el valor anterior (se sumaría dos veces).
            let before = conn.total_changes() as i64;
            let mut batch = rusqlite::Batch::new(conn, sql);
            // El fallo de la sentencia que empieza en el octeto `at` (`None` = se perdió la cuenta).
            let fail = |e: rusqlite::Error, at: Option<usize>, index: usize| -> String {
                if !(parked && is_busy(&e)) {
                    self.release(owner);
                    return e.to_string();
                }
                if !take_asked() {
                    self.release(owner);
                    return NOT_WAITED.to_string();
                }
                match at {
                    Some(at) => self.reserve(sql, at, conn.total_changes() as i64 - before, owner),
                    None => {
                        self.release(owner);
                        format!("database is locked (after {index} statement(s) of the script had run; the rest did not run)")
                    }
                }
            };
            if !params.is_empty() {
                let Some(mut stmt) = batch.next().map_err(|e| fail(e, Some(0), 0))? else {
                    self.release(owner);
                    return Ok(0);
                };
                // Preparar la siguiente puede fallar (`no such table` de una tabla que crearía la
                // primera): también eso es «hay más de una».
                if !matches!(batch.next(), Ok(None)) {
                    self.release(owner);
                    return Err("sqlite: exec with parameters runs one statement, and the SQL has more; \
                                run the others in their own exec (without parameters they all run)"
                        .to_string());
                }
                Self::step_all(&mut stmt, params).map_err(|e| fail(e, Some(0), 0))?;
                drop(stmt);
                if watch_pragma {
                    self.sync_busy_timeout(sql);
                }
                self.release(owner);
                return Ok(conn.total_changes() as i64 - before);
            }
            let mut index = 0usize;
            // Dónde empieza (en octetos de `sql`) la sentencia en curso; `None` si se perdió la
            // cuenta (una sentencia con `?` sin enlazar no devuelve su texto tal cual).
            let mut start = Some(0usize);
            const NONE: &[&str] = &[];
            loop {
                let outcome = match batch.next() {
                    Ok(None) => {
                        self.release(owner);
                        return Ok(conn.total_changes() as i64 - before);
                    }
                    Ok(Some(mut stmt)) => {
                        // El texto de la sentencia tal como SQLite lo tomó: de donde quedó la
                        // anterior hasta su final. Sirve para saber dónde empieza la siguiente.
                        let taken = if parked && stmt.parameter_count() == 0 { stmt.expanded_sql() } else { None };
                        Self::step_all(&mut stmt, NONE).map(|()| taken)
                    }
                    Err(e) => Err(e),
                };
                match outcome {
                    Ok(taken) => {
                        if watch_pragma {
                            self.sync_busy_timeout(taken.as_deref().unwrap_or(""));
                        }
                        start = match (start, taken) {
                            (Some(at), Some(t)) if sql[at..].starts_with(t.as_str()) => Some(at + t.len()),
                            _ => None,
                        };
                        index += 1;
                    }
                    Err(e) => return Err(fail(e, start, index)),
                }
            }
        }

        /// Ejecuta una consulta con filas; devuelve `(ncols, celdas)` con las celdas aplanadas fila a fila
        /// (el consumidor reconstruye el `[[string]]`). Un `NULL` es `""`.
        pub fn query(&self, sql: &str, params: &[String]) -> Result<(usize, Vec<String>), String> {
            self.rows(sql, params, value_str)
        }

        /// M347 (rayauth R6): como `query`, pero cada celda es `None` si era `NULL` (el `""` de `query`
        /// no distingue un NULL de una cadena vacía).
        pub fn query_nulls(&self, sql: &str, params: &[String]) -> Result<(usize, Vec<Option<String>>), String> {
            self.rows(sql, params, |v| if matches!(v, rusqlite::types::ValueRef::Null) { None } else { Some(value_str(v)) })
        }

        /// El cuerpo común de las dos consultas: `cell` convierte cada celda (sin vector intermedio).
        fn rows<T: From<String>>(
            &self,
            sql: &str,
            params: &[String],
            cell: impl Fn(rusqlite::types::ValueRef<'_>) -> T,
        ) -> Result<(usize, Vec<T>), String> {
            let conn = &self.inner;
            // Modo aparcado, `PRAGMA busy_timeout` como consulta: leerlo da el plazo de ESTA conexión
            // (el de SQLite es 0 con nuestro handler); fijarlo lo adopta, como en `exec`.
            let parked = self.parked.get();
            let watch_pragma = parked && mentions_busy_timeout(sql);
            // Leer el plazo no ejecuta nada: no pasa por la reserva (lo pregunta quien espera).
            if watch_pragma && busy_pragma(sql) == Some(false) {
                return Ok((1, vec![T::from(self.wait_ms.get().to_string())]));
            }
            let (sql, owner) = if parked { self.admit(sql)? } else { (sql, false) };
            let text = |e: rusqlite::Error| {
                if !(parked && is_busy(&e)) {
                    self.release(owner);
                    return e.to_string();
                }
                if !take_asked() {
                    self.release(owner);
                    return NOT_WAITED.to_string();
                }
                self.reserve(sql, 0, 0, owner)
            };
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
                            let v = row.get_ref(i).map_err(text)?;
                            out.push(cell(v));
                        }
                    }
                    Ok(None) => break,
                    Err(e) => return Err(text(e)),
                }
            }
            drop(rows);
            drop(stmt);
            if watch_pragma {
                self.sync_busy_timeout(sql);
            }
            self.release(owner);
            Ok((ncols, out))
        }
    }

    /// Abre (o crea) la base en `path` (`":memory:"` = en memoria).
    pub fn open(path: &str) -> Result<Conn, String> {
        let inner = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
        // El binding abre con el busy handler de SQLite (5 s reteniendo el hilo). Se queda así
        // hasta que `db/sqlite` pida el modo aparcado: un `db` anterior no espera por su cuenta.
        Ok(Conn {
            inner,
            parked: std::cell::Cell::new(false),
            wait_ms: std::cell::Cell::new(0),
            reserved: std::cell::RefCell::new(None),
            reservations: std::cell::Cell::new(0),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::{busy_pragma, open, PARK_SWITCH};

        /// Un `db` ≤ 0.10 no pide el modo aparcado ni sabe esperar: el runtime debe seguir
        /// esperando el candado dentro de SQLite, como hasta 1.27.112.
        #[test]
        fn without_the_switch_sqlite_waits_and_with_it_the_call_returns_at_once() {
            let path = std::env::temp_dir().join(format!("ray_sqlite_modes_{}.db", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let path_text = path.to_string_lossy().into_owned();
            let holder = open(&path_text).unwrap();
            holder.exec("CREATE TABLE t (n INTEGER)", &[]).unwrap();
            holder.exec("BEGIN IMMEDIATE", &[]).unwrap();

            let legacy = open(&path_text).unwrap();
            legacy.exec("PRAGMA busy_timeout = 200", &[]).unwrap();
            let asked = std::time::Instant::now();
            assert_eq!(legacy.exec("INSERT INTO t VALUES (1)", &[]).unwrap_err(), "database is locked");
            assert!(asked.elapsed() >= std::time::Duration::from_millis(150), "SQLite no esperó: {:?}", asked.elapsed());

            let parked = open(&path_text).unwrap();
            parked.exec(PARK_SWITCH, &[]).unwrap();
            assert_eq!(parked.query("PRAGMA busy_timeout", &[]).unwrap().1, vec!["5000".to_string()]);
            let asked = std::time::Instant::now();
            // Nada aplicado (carácter 0), y la conexión queda reservada para quien traiga el prefijo.
            assert_eq!(parked.exec("INSERT INTO t VALUES (1)", &[]).unwrap_err(), "\u{1}busy 0 0 /*ray-turn:1*/");
            assert!(asked.elapsed() < std::time::Duration::from_millis(100), "esperó dentro: {:?}", asked.elapsed());
            assert_eq!(parked.exec("SELECT 1", &[]).unwrap_err(), "\u{1}turn");
            assert_eq!(super::super::error_fields("\u{1}turn".to_string()), vec!["turn"]);
            assert_eq!(super::super::error_fields("\u{1}busy 0 0 /*ray-turn:1*/".to_string()), vec!["busy", "0", "0", "/*ray-turn:1*/"]);
            // El dueño la suelta sin ejecutar nada.
            assert_eq!(parked.exec("/*ray-turn:1*/PRAGMA ray_busy_wait = release", &[]).unwrap(), 0);
            // A mitad de un script: dice dónde seguir, y la conexión no guarda nada.
            parked.exec("CREATE TEMP TABLE a (n INTEGER)", &[]).unwrap();
            assert_eq!(
                parked.exec("INSERT INTO a VALUES (1); INSERT INTO t VALUES (2);", &[]).unwrap_err(),
                "\u{1}busy 25 1 /*ray-turn:2*/"
            );
            holder.exec("COMMIT", &[]).unwrap();
            assert_eq!(parked.exec("/*ray-turn:2*/ INSERT INTO t VALUES (2);", &[]).unwrap(), 1);
            // Terminó: la reserva se soltó sola.
            assert_eq!(parked.exec("SELECT 1", &[]).unwrap(), 0);
            drop((holder, legacy, parked));
            let _ = std::fs::remove_file(&path);
        }

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
            // Con comentarios delante o detrás y con esquema.
            assert_eq!(busy_pragma("-- wait less\nPRAGMA busy_timeout = 100;"), Some(true));
            assert_eq!(busy_pragma("/* x */ PRAGMA busy_timeout = 200"), Some(true));
            assert_eq!(busy_pragma("PRAGMA main.busy_timeout = 250"), Some(true));
            assert_eq!(busy_pragma("PRAGMA main . busy_timeout"), Some(false));
            assert_eq!(busy_pragma("PRAGMA busy_timeout -- c"), Some(false));
            assert_eq!(busy_pragma("PRAGMATIC busy_timeout = 1"), None);
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
