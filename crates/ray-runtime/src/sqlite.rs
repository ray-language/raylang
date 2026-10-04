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
    /// Una conexión SQLite embebida.
    pub struct Conn {
        inner: rusqlite::Connection,
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

    impl Conn {
        /// Ejecuta una sentencia sin filas (INSERT/UPDATE/DDL/BEGIN/…) con parámetros posicionales (`?1`…)
        /// enlazados como texto; devuelve el número de filas afectadas.
        pub fn exec(&self, sql: &str, params: &[String]) -> Result<i64, String> {
            self.inner
                .execute(sql, rusqlite::params_from_iter(params.iter()))
                .map(|n| n as i64)
                .map_err(|e| e.to_string())
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
            let mut stmt = self.inner.prepare(sql).map_err(|e| e.to_string())?;
            let ncols = stmt.column_count();
            let mut rows =
                stmt.query(rusqlite::params_from_iter(params.iter())).map_err(|e| e.to_string())?;
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
                    Err(e) => return Err(e.to_string()),
                }
            }
            Ok((ncols, out))
        }
    }

    /// Abre (o crea) la base en `path` (`":memory:"` = en memoria).
    pub fn open(path: &str) -> Result<Conn, String> {
        rusqlite::Connection::open(path).map(|inner| Conn { inner }).map_err(|e| e.to_string())
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
