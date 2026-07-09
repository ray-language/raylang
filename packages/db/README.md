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

### `db/bson` (M54.1)

**BSON** (el formato de documentos de MongoDB, bsonspec.org) en raylang puro — la base del cliente
MongoDB (M54, en curso). `enum Bson` recursivo (`Double`/`Str`/`Doc`/`Arr`/`Bin`/`ObjectId`/`Bool`/
`Null`/`Int`) + `encode(doc) -> bytes` y `decode(bytes) -> Result<[Field], string>` (errores como
valores, con la posición del octeto) + `dump` (repr JSON-ish para depurar).

```raylang
import db/bson;

let doc = [bson.field("hello", bson.Bson.Str("world"))];
let b = bson.encode(doc);                     // los bytes del vector canónico del spec
match (bson.decode(b)) {
    Result.Ok(fields) => { print(bson.dump_doc(fields)); },  // {hello: "world"}
    Result.Err(e) => { print(e); },
}
```

`Int` codifica como int64 y decodifica int32 e int64 (el int de raylang es i64). `Double` usa los
bits IEEE 754 (`math.float_bits`, M54.1a). Diferido: Date/Timestamp/Regex/Decimal128 (error claro).

**Puente JSON** (sobre `std/json`): `doc_from_json(s) -> Result<[Field], string>` — la ruta
ergonómica para filtros (`mongo.find(c, coll, bson.doc_from_json("{\"nombre\": \"ada\"}")?)`) —,
`from_json(Json) -> Bson` y `to_json(Bson) -> Json` (degradación documentada: `Int` → número JSON
con pérdida > 2^53, `ObjectId`/`Bin` → hex, orden de campos perdido).

### `db/mongo` (M54)

Cliente MongoDB en raylang puro sobre `db/bson`: framing **OP_MSG** (opCode 2013),
`connect(host, port, user, password, database, nonce)` con `hello` + **SCRAM-SHA-256 vía SASL**
(reusa `net/scram`, el mismo mecanismo que PostgreSQL; verifica la firma del servidor), CRUD
completo y `run_command(c, doc)` para cualquier otro comando.

```raylang
import db/mongo;
import db/bson;

var c = match (mongo.connect("127.0.0.1", 27017, "usuario", "clave", "test", nonce)) { … };
let docs = [[bson.field("nombre", bson.Bson.Str("ada")), bson.field("nota", bson.Bson.Int(36))]];
let _ = mongo.insert(c, "usuarios", docs);          // Result<int> (n; el _id lo asigna el servidor)
let filter = [bson.field("nombre", bson.Bson.Str("ada"))];
let rows = mongo.find(c, "usuarios", filter);       // Result<[[bson.Field]]> (firstBatch)
let set = [bson.field("$set", bson.Bson.Doc([bson.field("nota", bson.Bson.Int(37))]))];
let _ = mongo.update(c, "usuarios", filter, set, false);  // Result<int> (nModified)
let _ = mongo.delete(c, "usuarios", filter);        // Result<int> (n)
mongo.disconnect(c);
```

- Los filtros y documentos son **BSON estructurado** (`[bson.Field]`), no strings → anti-inyección
  por construcción. El `$set` del update lo arma el usuario (fiel al protocolo).
- **Diferido**: cursores `getMore` (v1 devuelve el firstBatch), tipos Date/Timestamp/Decimal128,
  compresión, TLS.

## Verificación

`tests/mysql_cli.rs`: el cliente corre contra un **servidor MySQL de juguete** (Rust std, TCP
plano) con scramble fijo y la respuesta de auth **precomputada** → offline y determinista; el
servidor verifica la auth octeto a octeto y sirve un result set (con `NULL`), un OK de `exec` y
un `ERR`. Oráculo conductual: VM e intérprete producen el mismo stdout.

`tests/sqlite_cli.rs`: sin servidor — `":memory:"` da una base determinista, así que el test es
un oráculo conductual puro (DDL + INSERT con parámetros + SELECT con `NULL` + transacción con
`ROLLBACK` + error SQL como valor + uso tras `disconnect`), mismo stdout en ambos motores.

`tests/bson_cli.rs`: la codificación reproduce **byte a byte** los vectores canónicos de
bsonspec.org; round-trip exacto de todos los tipos v1; errores como valores con la posición.

`tests/mongo_cli.rs`: servidor MongoDB **de juguete** que habla OP_MSG (BSON armado a mano en Rust)
y reusa las constantes SCRAM **precomputadas** del toy de PostgreSQL. Cubre la conexión, contraseña
mala (la firma del servidor no verifica → lo detecta el cliente), usuario desconocido (`errmsg`),
el CRUD completo (verificando que el documento insertado y el `$set` viajan dentro del comando) y
el error del servidor como valor. Ambos motores, mismo stdout.
