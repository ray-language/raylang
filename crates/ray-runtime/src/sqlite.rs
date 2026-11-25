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

/// Por qué no terminó bien una llamada a la conexión. Es un TIPO y no un texto para que nada
/// fuera de este módulo tenga que reconocer un candado por su mensaje: los motores (VM,
/// intérprete, binario nativo) no lo ven siquiera — llaman a los métodos `*_tagged` de [`Conn`],
/// que devuelven ya los campos de la respuesta etiquetada del primitivo.
#[derive(Debug, PartialEq)]
pub enum Fail {
    /// Un error, con su texto.
    Message(String),
    /// Encontró un candado que se puede esperar. Si era un script, sus primeras sentencias
    /// quedaron aplicadas (`changed` filas) y la que lo encontró empieza en el carácter `at` del
    /// SQL (0 = no se aplicó nada). La conexión queda RESERVADA para quien repita la llamada con
    /// `prefix` delante del SQL.
    Busy { at: usize, changed: i64, prefix: String },
    /// La conexión está reservada por otra llamada que espera; repetir más tarde.
    Turn,
}

impl Fail {
    /// Los campos de la respuesta etiquetada: `["err", msg]`, `["busy", at, changed, prefix]` o
    /// `["turn"]`.
    pub fn into_fields(self) -> Vec<String> {
        match self {
            Fail::Message(message) => tagged_err(message),
            Fail::Busy { at, changed, prefix } => vec!["busy".to_string(), at.to_string(), changed.to_string(), prefix],
            Fail::Turn => vec!["turn".to_string()],
        }
    }
}

impl From<String> for Fail {
    fn from(message: String) -> Self {
        Fail::Message(message)
    }
}

/// La respuesta etiquetada de un error que no viene de la conexión (handle inválido, binario sin
/// SQLite…): `["err", msg]`.
pub fn tagged_err(message: String) -> Vec<String> {
    vec!["err".to_string(), message]
}

/// Las respuestas etiquetadas de los cuatro primitivos, compuestas en UN sitio para todos los
/// motores: `["ok", n]`, `["ok", ncols, celdas…]`, `["ok", ncols, máscara, celdas…]`, o las de
/// [`Fail::into_fields`].
impl Conn {
    pub fn exec_tagged(&self, sql: &str, params: &[String]) -> Vec<String> {
        match self.exec(sql, params) {
            Ok(n) => vec!["ok".to_string(), n.to_string()],
            Err(e) => e.into_fields(),
        }
    }

    pub fn exec_opt_tagged(&self, sql: &str, params: &[Option<String>]) -> Vec<String> {
        match self.exec_opt(sql, params) {
            Ok(n) => vec!["ok".to_string(), n.to_string()],
            Err(e) => e.into_fields(),
        }
    }

    pub fn query_tagged(&self, sql: &str, params: &[String]) -> Vec<String> {
        match self.query(sql, params) {
            Ok((ncols, cells)) => {
                let mut fields = Vec::with_capacity(cells.len() + 2);
                fields.push("ok".to_string());
                fields.push(ncols.to_string());
                fields.extend(cells);
                fields
            }
            Err(e) => e.into_fields(),
        }
    }

    /// M347 (rayauth R6): con una máscara de `n` (NULL) o `v` (valor) por celda delante de ellas.
    pub fn query_nulls_tagged(&self, sql: &str, params: &[String]) -> Vec<String> {
        match self.query_nulls(sql, params) {
            Ok((ncols, cells)) => {
                let mask: String = cells.iter().map(|c| if c.is_none() { 'n' } else { 'v' }).collect();
                let mut fields = Vec::with_capacity(cells.len() + 3);
                fields.push("ok".to_string());
                fields.push(ncols.to_string());
                fields.push(mask);
                fields.extend(cells.into_iter().map(|c| c.unwrap_or_default()));
                fields
            }
            Err(e) => e.into_fields(),
        }
    }
}

#[cfg(feature = "sqlite")]
mod imp {
    use super::Fail;

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
    /// ([`LEASE`] sin que el dueño vuelva: una fibra cancelada mientras esperaba no deja la
    /// conexión cerrada para siempre). Una
    /// llamada que no encuentra candado —la de siempre— solo paga mirar que no hay reserva.
    pub struct Conn {
        inner: rusqlite::Connection,
        parked: std::cell::Cell<bool>,
        /// Modo aparcado: ms que una llamada espera un candado antes de fallar; 0 = en el acto.
        wait_ms: std::cell::Cell<i64>,
        reserved: std::cell::RefCell<Option<Reservation>>,
    }

    struct Reservation {
        /// Al azar: quien no recibió el `busy` no puede acertarla.
        token: String,
        until: std::time::Instant,
    }

    /// Lo que dura una reserva sin que su dueño vuelva. El dueño reintenta cada 50 ms como mucho,
    /// así que un arriendo corto y fijo —no el plazo de la conexión, que puede ser de días— basta
    /// para él y acota lo que una fibra cancelada mientras esperaba deja cerrada la conexión.
    const LEASE: std::time::Duration = std::time::Duration::from_secs(2);

    /// El prefijo de una reserva es `/*ray-turn:<ficha>*/`: un comentario SQL, así que lo que
    /// sigue es el SQL tal cual.
    const TURN_PREFIX: &str = "/*ray-turn:";

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
    // Se apaga al entrar en cada llamada: un aviso que SQLite se tragó sin dar error (un
    // `wal_checkpoint` con un lector activo devuelve una fila `busy=1`) no debe llegar a la
    // siguiente, que lo leería como «este candado se puede esperar».
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

        /// Modo aparcado, al entrar en una llamada: ¿puede correr ahora? Devuelve el SQL sin
        /// prefijo de reserva y si quien llama es el dueño de la reserva vigente. El prefijo se
        /// quita SIEMPRE, también el de una reserva que caducó: lo que sigue se mide en
        /// caracteres del SQL y el prefijo no es parte de él.
        ///
        /// Fijar el plazo (`PRAGMA busy_timeout = N`, una sola sentencia) entra aunque la
        /// conexión esté reservada: es configuración de la conexión, no toca la base ni la
        /// transacción de nadie, y así `sqlite.busy_timeout` no puede perderse.
        fn admit<'s>(&self, sql: &'s str) -> Result<(&'s str, bool), Fail> {
            let (claim, body) = match sql.strip_prefix(TURN_PREFIX).and_then(|rest| rest.split_once("*/")) {
                Some((n, body)) => (Some(n), body),
                None => (None, sql),
            };
            let mut reserved = self.reserved.borrow_mut();
            let Some(r) = reserved.as_ref() else {
                return Ok((body, false));
            };
            if claim == Some(r.token.as_str()) {
                return Ok((body, true));
            }
            if std::time::Instant::now() >= r.until {
                *reserved = None; // caducó: su dueño no volvió a tiempo
                return Ok((body, false));
            }
            if mentions_busy_timeout(body) && busy_pragma(body) == Some(true) && !body.trim_end().trim_end_matches(';').contains(';') {
                return Ok((body, false));
            }
            Err(Fail::Turn)
        }

        /// Modo aparcado: la llamada encontró un candado que se puede esperar. Reserva la
        /// conexión para quien la repita (o renueva la reserva, si ya era suya): `at` = octetos de
        /// `sql` (ya sin prefijo) ejecutados, `changed` = filas afectadas.
        fn reserve(&self, sql: &str, at: usize, changed: i64, owner: bool) -> Fail {
            let until = std::time::Instant::now() + LEASE;
            let mut reserved = self.reserved.borrow_mut();
            let token = match reserved.as_mut().filter(|_| owner) {
                Some(r) => {
                    r.until = until;
                    r.token.clone()
                }
                None => {
                    // `RandomState` trae una semilla aleatoria por instancia: sin dependencia nueva.
                    use std::hash::{BuildHasher, Hasher};
                    let token = format!("{:016x}", std::collections::hash_map::RandomState::new().build_hasher().finish());
                    *reserved = Some(Reservation { token: token.clone(), until });
                    token
                }
            };
            Fail::Busy { at: sql[..at].chars().count(), changed, prefix: format!("{TURN_PREFIX}{token}*/") }
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
        pub fn exec(&self, sql: &str, params: &[String]) -> Result<i64, Fail> {
            self.run(sql, params)
        }

        /// Como [`Conn::exec`], con parámetros que pueden ser `NULL` (`None`).
        pub fn exec_opt(&self, sql: &str, params: &[Option<String>]) -> Result<i64, Fail> {
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
        /// y se puede esperar, reserva la conexión y devuelve `Fail::Busy` con lo que un script
        /// ya aplicó: `db/sqlite` aparca la fibra y llama
        /// otra vez con el prefijo de la reserva y el RESTO del texto. Por dónde va el script lo
        /// lleva quien llama, no la conexión.
        fn run<P: rusqlite::ToSql>(&self, sql: &str, params: &[P]) -> Result<i64, Fail> {
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
            if parked {
                BUSY_ASKED.with(|b| b.set(false));
            }
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
            let fail = |e: rusqlite::Error, at: Option<usize>, index: usize| -> Fail {
                // Un BUSY por el que SQLite ni preguntó es un interbloqueo (una transacción que ya
                // lee y quiere escribir mientras otra tiene el candado): esperar no lo arregla y
                // sale con el texto de siempre, `database is locked`.
                if !(parked && is_busy(&e) && take_asked()) {
                    self.release(owner);
                    return Fail::Message(e.to_string());
                }
                match at {
                    Some(at) => self.reserve(sql, at, conn.total_changes() as i64 - before, owner),
                    None => {
                        self.release(owner);
                        // Sin cuenta no hay por dónde seguir (SQLite no devolvió el texto de
                        // una sentencia): el mismo aviso que da `db/sqlite` al vencer el plazo.
                        let _ = index;
                        Fail::Message("database is locked (part of the script had run; the rest did not run)".to_string())
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
                    return Err(Fail::Message(
                        "sqlite: exec with parameters runs one statement, and the SQL has more; \
                         run the others in their own exec (without parameters they all run)"
                            .to_string(),
                    ));
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
            // Llevar la cuenta de dónde va el script cuesta copiar el texto de cada sentencia:
            // solo si puede haber una segunda (un `;` que no sea el final).
            let track = parked && sql.trim_end().trim_end_matches(';').contains(';');
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
                        let taken = if track && stmt.parameter_count() == 0 { stmt.expanded_sql() } else { None };
                        Self::step_all(&mut stmt, NONE).map(|()| taken)
                    }
                    Err(e) => Err(e),
                };
                match outcome {
                    Ok(taken) => {
                        if watch_pragma {
                            // Sin cuenta (una sola sentencia), el texto es el de la llamada.
                            self.sync_busy_timeout(taken.as_deref().unwrap_or(sql));
                        }
                        start = match (start, taken) {
                            (Some(0), None) if !track => Some(0),
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
        pub fn query(&self, sql: &str, params: &[String]) -> Result<(usize, Vec<String>), Fail> {
            self.rows(sql, params, value_str)
        }

        /// M347 (rayauth R6): como `query`, pero cada celda es `None` si era `NULL` (el `""` de `query`
        /// no distingue un NULL de una cadena vacía).
        pub fn query_nulls(&self, sql: &str, params: &[String]) -> Result<(usize, Vec<Option<String>>), Fail> {
            self.rows(sql, params, |v| if matches!(v, rusqlite::types::ValueRef::Null) { None } else { Some(value_str(v)) })
        }

        /// El cuerpo común de las dos consultas: `cell` convierte cada celda (sin vector intermedio).
        fn rows<T: From<String>>(
            &self,
            sql: &str,
            params: &[String],
            cell: impl Fn(rusqlite::types::ValueRef<'_>) -> T,
        ) -> Result<(usize, Vec<T>), Fail> {
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
            if parked {
                BUSY_ASKED.with(|b| b.set(false));
            }
            let text = |e: rusqlite::Error| {
                if !(parked && is_busy(&e) && take_asked()) {
                    self.release(owner);
                    return Fail::Message(e.to_string());
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
        })
    }

    #[cfg(test)]
    mod tests {
        use super::{busy_pragma, open, Fail, PARK_SWITCH};

        fn locked() -> Fail {
            Fail::Message("database is locked".to_string())
        }

        /// El prefijo de la reserva que trae un `Fail::Busy` con ese punto de reanudación.
        fn prefix_of(fail: Fail, at: usize, changed: i64) -> String {
            match fail {
                Fail::Busy { at: a, changed: c, prefix } if a == at && c == changed => prefix,
                other => panic!("se esperaba busy({at}, {changed}), llegó {other:?}"),
            }
        }

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
            assert_eq!(legacy.exec("INSERT INTO t VALUES (1)", &[]).unwrap_err(), locked());
            assert!(asked.elapsed() >= std::time::Duration::from_millis(150), "SQLite no esperó: {:?}", asked.elapsed());

            let parked = open(&path_text).unwrap();
            parked.exec(PARK_SWITCH, &[]).unwrap();
            assert_eq!(parked.query("PRAGMA busy_timeout", &[]).unwrap().1, vec!["5000".to_string()]);
            let asked = std::time::Instant::now();
            // Nada aplicado (carácter 0), y la conexión queda reservada para quien traiga el prefijo.
            let first = prefix_of(parked.exec("INSERT INTO t VALUES (1)", &[]).unwrap_err(), 0, 0);
            assert!(first.starts_with("/*ray-turn:") && first.ends_with("*/") && first.len() == 29, "{first}");
            assert!(asked.elapsed() < std::time::Duration::from_millis(100), "esperó dentro: {:?}", asked.elapsed());
            // Reservada: otra llamada no entra; leer el plazo sí (no ejecuta nada), y fijarlo también.
            assert_eq!(parked.exec("SELECT 1", &[]).unwrap_err(), Fail::Turn);
            assert_eq!(parked.exec_tagged("SELECT 1", &[]), vec!["turn"]);
            assert_eq!(parked.query_tagged("PRAGMA busy_timeout", &[]), vec!["ok", "1", "5000"]);
            assert_eq!(parked.exec("PRAGMA busy_timeout = 4000", &[]).unwrap(), 0);
            assert_eq!(parked.query_tagged("PRAGMA busy_timeout", &[]), vec!["ok", "1", "4000"]);
            // Una ficha que no es la suya no abre la reserva.
            assert_eq!(parked.exec("/*ray-turn:0000000000000000*/SELECT 1", &[]).unwrap_err(), Fail::Turn);
            // El dueño repite, sigue ocupada, y conserva SU reserva.
            assert_eq!(prefix_of(parked.exec(&format!("{first}INSERT INTO t VALUES (1)"), &[]).unwrap_err(), 0, 0), first);
            // …y la suelta sin ejecutar nada.
            assert_eq!(parked.exec(&format!("{first}PRAGMA ray_busy_wait = release"), &[]).unwrap(), 0);
            // A mitad de un script: dice dónde seguir, y la conexión no guarda el script.
            parked.exec("CREATE TEMP TABLE a (n INTEGER)", &[]).unwrap();
            let script = "INSERT INTO a VALUES (1); INSERT INTO t VALUES (2); INSERT INTO t VALUES (3);";
            let second = prefix_of(parked.exec(script, &[]).unwrap_err(), 25, 1);
            assert_ne!(second, first);
            // Un prefijo CADUCADO (el de la primera reserva) no cuenta como SQL: quien lo trae
            // espera su turno como cualquiera, y el carácter de reanudación se mide sin él.
            assert_eq!(parked.exec(&format!("{first}SELECT 1"), &[]).unwrap_err(), Fail::Turn);
            assert_eq!(prefix_of(parked.exec(&format!("{second}{}", &script[25..]), &[]).unwrap_err(), 0, 0), second);
            holder.exec("COMMIT", &[]).unwrap();
            assert_eq!(parked.exec(&format!("{second}{}", &script[25..]), &[]).unwrap(), 2);
            // Terminó: la reserva se soltó sola, y un prefijo viejo se quita igual.
            assert_eq!(parked.exec("SELECT 1", &[]).unwrap(), 0);
            assert_eq!(parked.exec(&format!("{second}SELECT 1"), &[]).unwrap(), 0);
            // Lo que esperar no arregla sale con el texto de siempre, sin reservar nada.
            let reader = open(&path_text).unwrap();
            reader.exec(PARK_SWITCH, &[]).unwrap();
            reader.exec("BEGIN", &[]).unwrap();
            reader.query("SELECT count(*) FROM t", &[]).unwrap();
            holder.exec("BEGIN IMMEDIATE", &[]).unwrap();
            assert_eq!(reader.exec("INSERT INTO t VALUES (9)", &[]).unwrap_err(), locked());
            assert_eq!(reader.exec("ROLLBACK", &[]).unwrap(), 0);
            holder.exec("COMMIT", &[]).unwrap();
            drop(reader);
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
        pub fn exec(&self, _sql: &str, _params: &[String]) -> Result<i64, super::Fail> {
            match self.0 {}
        }
        pub fn exec_opt(&self, _sql: &str, _params: &[Option<String>]) -> Result<i64, super::Fail> {
            match self.0 {}
        }
        pub fn query(&self, _sql: &str, _params: &[String]) -> Result<(usize, Vec<String>), super::Fail> {
            match self.0 {}
        }
        pub fn query_nulls(&self, _sql: &str, _params: &[String]) -> Result<(usize, Vec<Option<String>>), super::Fail> {
            match self.0 {}
        }
    }
    pub fn open(_path: &str) -> Result<Conn, String> {
        Err("SQLite not available (build without the 'sqlite' feature)".to_string())
    }
}

#[cfg(not(feature = "sqlite"))]
pub use stub::{open, Conn};
