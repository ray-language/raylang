# Ejemplos de bases de datos (`examples/db`)

Demos de los clientes de base de datos de raylang (`packages/db`): **MySQL** (`db/mysql`, M53.1) y
**PostgreSQL** (`db/postgres`, M53.2). A diferencia de la mayoría de ejemplos —archivos sueltos que se
corren directos—, estos importan un **paquete** (tier 2, no embebido), así que este directorio es un
**mini-proyecto** con su `ray.toml` que declara la dependencia por ruta:

```toml
[dependencies]
db = "path:../../packages/db"
```

## Correrlos

Necesitan un servidor de base de datos **real** (los demos abren un socket y hablan el protocolo). Desde
este directorio:

```sh
cd examples/db

# MySQL (usuario con mysql_native_password, o caching_sha2 ya cacheado):
ray run mysql_demo.ray 127.0.0.1 3306 usuario clave base

# PostgreSQL (autenticación SCRAM-SHA-256):
ray run postgres_demo.ray 127.0.0.1 5432 usuario clave base
```

Sin argumentos, cada demo imprime su uso y sale. Los ejemplos asumen una tabla `usuarios(id, nombre)`;
ajústalos a tu esquema.

## Qué muestran

- **`mysql_demo.ray`** — `connect` → `query` (SELECT, filas como texto) → `exec` (INSERT, filas
  afectadas) → `disconnect`.
- **`postgres_demo.ray`** — `connect` (SCRAM) → `query` con **parámetro** (`$1` enlazado aparte del SQL,
  anti-inyección) → transacción (`BEGIN` / `INSERT` con parámetro / `COMMIT`) → `disconnect`.

La API y los detalles de protocolo/limitaciones están en [`packages/db/README.md`](../../packages/db/README.md).
