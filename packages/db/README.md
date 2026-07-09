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

### Próximos (plan M53, IDEAS §14)

- `db/postgres` (M53.2) — evolución del cliente de `packages/net`: conexión persistente +
  protocolo extendido (parámetros).
- `db/sqlite` (M53.4) — vía FFI a `libsqlite3` (tras los out-params del FFI, M53.3).

## Verificación

`tests/mysql_cli.rs`: el cliente corre contra un **servidor MySQL de juguete** (Rust std, TCP
plano) con scramble fijo y la respuesta de auth **precomputada** → offline y determinista; el
servidor verifica la auth octeto a octeto y sirve un result set (con `NULL`), un OK de `exec` y
un `ERR`. Oráculo conductual: VM e intérprete producen el mismo stdout.
