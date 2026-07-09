# `db` — clientes de bases de datos (paquete adicional, **no** embebido)

Clientes de bases de datos **escritos en raylang** sobre los sockets de `std/net` y la cripto de
`std/crypto`. Tier 2 del ecosistema (paquete adicional; la política de tiers está en
[DESIGN.md](../../DESIGN.md) §53; el plan del paquete en IDEAS.md §14 / DESIGN §55).

## Cómo usarlo

Declara el paquete en tu `ray.toml` como dependencia por ruta (o git):

```toml
[dependencies]
db = "path:../ruta/a/packages/db"
```

## Módulos

### `db/mysql` (M53.1)

Cliente del protocolo wire de MySQL (handshake v10 + `COM_QUERY` en protocolo de texto):

```raylang
import db/mysql;

fn main() -> int {
    var c = match (mysql.connect("127.0.0.1", 3306, "usuario", "clave", "esquema")) {
        Result.Ok(conn) => conn,
        Result.Err(e) => { print(e); return 1; },
    };
    match (mysql.query(c, "SELECT nombre, nota FROM alumnos")) {
        Result.Ok(rows) => {
            for fila in rows { print(fila.join(" | ")); }
        },
        Result.Err(e) => { print(e); },
    }
    match (mysql.exec(c, "UPDATE alumnos SET nota = 10")) {
        Result.Ok(n) => { print("afectadas: " + to_string(n)); },
        Result.Err(e) => { print(e); },
    }
    mysql.disconnect(c);
    0
}
```

- **API**: `connect(host, port, user, password, database) -> Result<Conn, string>` ·
  `query(c, sql) -> Result<[[string]], string>` (filas como texto; `NULL` → `""`) ·
  `exec(c, sql) -> Result<int, string>` (filas afectadas) · `disconnect(c)`.
- **Auth**: `mysql_native_password` (completa) y `caching_sha2_password` solo en su **fast-path**
  (funciona cuando el servidor ya cacheó la contraseña); el full-path exige TLS *upgrade* a mitad
  de conexión, que `std/net` aún no ofrece → error claro con el remedio.
- **Diferido**: protocolo binario (prepared statements / parámetros / tipos), TLS.

### `db/postgres` (M53.2)

Cliente del protocolo wire v3 de PostgreSQL con **conexión persistente**, autenticación
**SCRAM-SHA-256** (reusa `net/scram`) y el **protocolo extendido** (Parse/Bind/Describe/Execute/
Sync) — que trae **parámetros** (`$1`, `$2`, … enlazados aparte del SQL → anti-inyección) y devuelve
**todas** las filas:

```raylang
import db/postgres;

fn main() -> int {
    var c = match (postgres.connect("127.0.0.1", 5432, "usuario", "clave", "basedatos", "nonce")) {
        Result.Ok(conn) => conn,
        Result.Err(e) => { print(e); return 1; },
    };
    match (postgres.query(c, "SELECT nombre FROM alumnos WHERE nota > $1", ["5"])) {
        Result.Ok(rows) => { for f in rows { print(f.join(" | ")); } },
        Result.Err(e) => { print(e); },
    }
    let _ = postgres.exec(c, "BEGIN", []);
    let _ = postgres.exec(c, "INSERT INTO alumnos (nombre) VALUES ($1)", ["ada"]);
    let _ = postgres.exec(c, "COMMIT", []);
    postgres.disconnect(c);
    0
}
```

- **API**: `connect(host, port, user, password, database, nonce) -> Result<Conn, string>` ·
  `query(c, sql, params) -> Result<[[string]], string>` · `exec(c, sql, params) -> Result<int, string>`
  (filas afectadas) · `disconnect(c)`. Las **transacciones** son SQL corriente (`exec(c, "BEGIN", [])`
  / `"COMMIT"` / `"ROLLBACK"`).
- **Parámetros** en formato texto (v1); usa `$1`, `$2`, … en el SQL. `nonce` = nonce del cliente
  (aleatorio en producción). El cliente de una-consulta de `net/postgres` (protocolo simple) se
  conserva aparte.
- **Diferido**: parámetros binarios/tipados, sentencias preparadas con estado, TLS, COPY.

### `db/sqlite` (M53.4)

Base de datos **embebida**: sin servidor ni socket. A diferencia de mysql/postgres (protocolo wire
en raylang puro), SQLite es una librería C: los primitivos `__sqlite_*` viven en el host sobre
**`rusqlite`** (patrón `ring`/M43 — el binding maduro resuelve dobles punteros, lifetimes de
statements y destructores de bind). SQLite va **compilado dentro del binario** (`bundled`): cero
dependencias del sistema.

```raylang
import db/sqlite;

fn main() -> int {
    var c = match (sqlite.connect(":memory:")) {   // o una ruta de archivo
        Result.Ok(conn) => conn,
        Result.Err(e) => { print(e); return 1; },
    };
    let sin: [string] = [];
    let _ = sqlite.exec(c, "CREATE TABLE u (id INTEGER, nombre TEXT)", sin);
    let _ = sqlite.exec(c, "INSERT INTO u VALUES (?1, ?2)", ["1", "ada"]);
    match (sqlite.query(c, "SELECT nombre FROM u WHERE id = ?1", ["1"])) {
        Result.Ok(rows) => { print(rows[0][0]); },
        Result.Err(e) => { print(e); },
    }
    sqlite.disconnect(c);
    0
}
```

- **API uniforme** con los otros clientes: `connect(path) -> Result<Conn, string>`,
  `query`/`exec` con `params: [string]` (marcadores **`?1`, `?2`, …** enlazados aparte del SQL →
  anti-inyección), `disconnect`. Transacciones = SQL corriente (`BEGIN`/`COMMIT`/`ROLLBACK`).
- **Celdas como texto** (misma convención): INTEGER/REAL → repr decimal, `NULL` → `""`, BLOB → hex.
- `disconnect` libera el handle; usar la conexión después falla **limpio** (error como valor, no
  crash: el ciclo prepare→step→finalize ocurre entero dentro del host, un statement nunca escapa).
- **No disponible en el playground web** (wasm no compila la librería C).

## Verificación

`tests/mysql_cli.rs`: el cliente corre contra un **servidor MySQL de juguete** (Rust std, TCP
plano) con scramble fijo y la respuesta de auth **precomputada** → offline y determinista; el
servidor verifica la auth octeto a octeto y sirve un result set (con `NULL`), un OK de `exec` y
un `ERR`. Oráculo conductual: VM e intérprete producen el mismo stdout.

`tests/sqlite_cli.rs`: sin servidor — `":memory:"` da una base determinista, así que el test es
un oráculo conductual puro (DDL + INSERT con parámetros + SELECT con `NULL` + transacción con
`ROLLBACK` + error SQL como valor + uso tras `disconnect`), mismo stdout en ambos motores.
