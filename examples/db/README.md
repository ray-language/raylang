# Ejemplos de bases de datos (`examples/db`)

Demos de los clientes de base de datos de raylang (`packages/db`): **MySQL** (`db/mysql`, M53.1),
**PostgreSQL** (`db/postgres`, M53.2), **SQLite** (`db/sqlite`, M53.4) y **MongoDB** (`db/mongo`,
M54). A diferencia de la mayoría de
ejemplos —archivos sueltos que se corren directos—, estos importan un **paquete** (tier 2, no
embebido), así que este directorio es un **mini-proyecto** con su `ray.toml` que declara la
dependencia por ruta:

```toml
[dependencies]
db = "path:../../packages/db"
```

## Correrlos

Los demos de MySQL y PostgreSQL necesitan un servidor de base de datos **real** (abren un socket y
hablan el protocolo); el de SQLite corre **tal cual** (base embebida). Desde este directorio:

```sh
cd examples/db

# SQLite (embebido, sin servidor; sin argumento = base en memoria):
ray run sqlite_demo.ray
ray run sqlite_demo.ray demo.db     # persistida en un archivo

# MySQL (usuario con mysql_native_password, o caching_sha2 ya cacheado):
ray run mysql_demo.ray 127.0.0.1 3306 usuario clave base

# PostgreSQL (autenticación SCRAM-SHA-256):
ray run postgres_demo.ray 127.0.0.1 5432 usuario clave base

# MongoDB (autenticación SCRAM-SHA-256 sobre OP_MSG):
ray run mongo_demo.ray 127.0.0.1 27017 usuario clave base
```

Sin argumentos, los demos de mysql/postgres imprimen su uso y salen; asumen una tabla
`usuarios(id, nombre)` — ajústalos a tu esquema.

## Qué muestran

- **`sqlite_demo.ray`** — `connect` (`":memory:"` o archivo) → `exec` con **parámetros** (`?1`) →
  `query` (celdas como texto, NULL = "") → transacción con `ROLLBACK` → `disconnect`.
- **`mysql_demo.ray`** — `connect` → `query` (SELECT, filas como texto) → `exec` (INSERT, filas
  afectadas) → `disconnect`.
- **`postgres_demo.ray`** — `connect` (SCRAM) → `query` con **parámetro** (`$1` enlazado aparte del SQL,
  anti-inyección) → transacción (`BEGIN` / `INSERT` con parámetro / `COMMIT`) → `disconnect`.
- **`mongo_demo.ray`** — `connect` (SCRAM sobre OP_MSG) → `insert` de documentos BSON → `find` con
  filtro (un documento, no un string) → `update` con `$set` → `delete` → `disconnect`.

La API y los detalles de protocolo/limitaciones están en [`packages/db/README.md`](../../packages/db/README.md).
