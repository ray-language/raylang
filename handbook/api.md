# API web

Español · [English](api.en.md)

Las notas salen del dispositivo: una **API JSON** con el framework `web`, los datos en
**Postgres** a través de un pool de conexiones, autenticación por token, compresión, logs JSON y
apagado ordenado. Es la forma de un servicio de producción.

El proyecto completo está en [`examples/apps/notes-api`](../examples/apps/notes-api/), con sus
tests. Los bloques de raylang están copiados de él y el CI comprueba que sigan siéndolo.

## 1. El proyecto

```sh
ray new notes-api && cd notes-api
ray add web
ray add db
```

```toml
[dependencies]
web = "^0.4.6"
db = "^0.2.1"
```

`web` trae el servidor HTTP del paquete `net`; `db` trae el cliente de Postgres. Las versiones
exactas quedan en `ray.lock`. El código se reparte en tres módulos: `notes.ray` (los datos),
`auth.ray` (el token) y `main.ray` (las rutas).

| Ruta | Qué hace | Respuestas |
|---|---|---|
| `GET /health` | comprueba la base de datos | 200, 503 |
| `GET /notes?q=…&limit=…` | lista y busca | 200 |
| `GET /notes/:id` | una nota | 200, 404 |
| `POST /notes` | crea | 201 con `Location`, 422 |
| `PUT /notes/:id` | reemplaza | 200, 404, 422 |
| `DELETE /notes/:id` | borra | 204, 404 |

## 2. Un pool de conexiones compartido

El framework ejecuta cada petición en su propia fibra, con memoria aislada. Una conexión guardada
en una variable no se compartiría entre peticiones, y abrir una nueva por consulta es lento: cada
apertura negocia la autenticación con el servidor. La solución es un **pool**: viaja por un canal,
así que todas las fibras lo comparten, abre conexiones bajo demanda, las reutiliza y reemplaza las
que se rompen.

<!-- check: project=examples/apps/notes-api -->
```rust
/// Connects a pool with the standard Postgres variables: PGHOST, PGPORT, PGUSER, PGPASSWORD,
/// PGDATABASE. Nothing is dialed until the first query.
pub fn pool_from_env(size: int) -> Pool {
    postgres.pool(
        env("PGHOST").unwrap_or("127.0.0.1"),
        env("PGPORT").unwrap_or("5432").parse_int().unwrap_or(5432),
        env("PGUSER").unwrap_or("notes"),
        env("PGPASSWORD").unwrap_or(""),
        env("PGDATABASE").unwrap_or("notes"),
        size
    )
}
```

El pool se crea una vez en `main`. La configuración usa las variables estándar de Postgres, las
mismas que entienden `psql` y cualquier plataforma de despliegue.

Al arrancar, el programa crea la tabla si no existe, dentro de una transacción:

<!-- check: project=examples/apps/notes-api -->
```rust
/// Creates the table if it does not exist, in one transaction.
pub fn migrate(p: Pool) -> Result<int, string> {
    postgres.pool_tx(p, fn(c: postgres.Conn) -> Result<int, string> {
        let _ = postgres.exec(c, `CREATE TABLE IF NOT EXISTS notes (
                    id         TEXT PRIMARY KEY,
                    title      TEXT NOT NULL,
                    body       TEXT NOT NULL,
                    updated_ms BIGINT NOT NULL
                )`, [])?;
        postgres.exec(c, "CREATE INDEX IF NOT EXISTS notes_by_date ON notes (updated_ms DESC)", [])
    })
}
```

`pool_tx` toma una conexión, ejecuta `BEGIN`, la función y `COMMIT`; si la función falla, hace
`ROLLBACK`. `pool_query` y `pool_exec` sirven para una sola sentencia, y `pool_query` reintenta una
vez con una conexión nueva si la reutilizada se cortó (por ejemplo, porque el servidor se
reinició).

## 3. Consultas con parámetros

Los valores van en `$1`, `$2`, …, separados del SQL. Una comilla en la búsqueda es dato, no código.
La búsqueda usa `position` en lugar de `LIKE`, así que `%` y `_` también son caracteres normales:

<!-- check: project=examples/apps/notes-api -->
```rust
/// The notes whose title or body contains `query` (any case, taken literally: `%` and `_` are
/// just characters), newest first, at most `limit`.
pub fn search(p: Pool, query: string, limit: int) -> Result<[Note], string> {
    let rows = postgres.pool_query(p, `SELECT id, title, body, updated_ms FROM notes
         WHERE $1 = ''
            OR position(lower($1) in lower(title)) > 0
            OR position(lower($1) in lower(body)) > 0
         ORDER BY updated_ms DESC LIMIT $2`, [query.trim(), to_string(limit)])?;
    Result.Ok(rows.map(from_row))
}
```

`db` devuelve cada celda como texto, de modo que los números se convierten con `parse_int()`.

## 4. Las rutas

La app se construye en una función que recibe el pool. Cada fibra de conexión llama a esa función y
obtiene su propia copia de la app, pero todas comparten el mismo pool:

<!-- check: project=examples/apps/notes-api -->
```rust
// The routes. The pool is created once in `main`; this function builds the app for each
// connection's fiber and captures the pool, which every fiber shares through its channel.
fn routes(db: Pool, token: string) -> App {
    var app = new_app();
    app.log_requests();
    app.gzip();
    // Everything under /notes needs the token.
    app.use_on("/notes", fn(c: Ctx, r: Res) -> Step {
        if (auth.authorized(c.header_of("authorization"), token)) {
            return Step.Next;
        }
        fail(r, 401, "missing or wrong bearer token");
        Step.Done
    });
```

- `log_requests()` escribe una línea JSON por petición, con método, ruta, estado, duración y un
  identificador de traza.
- `gzip()` comprime las respuestas de 512 bytes o más cuando el cliente lo acepta.
- `use_on("/notes", …)` aplica el middleware solo a las rutas bajo `/notes`; `/health` queda libre
  para el balanceador de carga.

Crear una nota lee el cuerpo JSON, lo valida y responde 201 con la cabecera `Location`:

<!-- check: project=examples/apps/notes-api -->
```rust
    app.POST("/notes", fn(c: Ctx, r: Res) {
        match (note_input(c)) {
            Result.Err(e) => fail(r, 422, e),
            Result.Ok(input) => {
                let (title, body) = input;
                match (notes.create(db, title, body, time.now())) {
                    Result.Ok(n) => r.status(201).header("Location", "/notes/" + n.id).json(n.to_json()),
                    Result.Err(e) => fail(r, 500, e),
                }
            },
        }
    });
```

La lectura del cuerpo usa `?` para propagar tanto un JSON mal formado como una nota inválida, y
ambos acaban en un 422 con el motivo:

<!-- check: project=examples/apps/notes-api -->
```rust
// The title and body of a request, or the reason they are not valid.
fn note_input(c: Ctx) -> Result<(string, string), string> {
    let body = c.json_body()?;
    let title = json.get_string(body, "title").unwrap_or("");
    let text_ = json.get_string(body, "body").unwrap_or("");
    let why = notes.invalid(title, text_);
    if (why != "") {
        return Result.Err(why);
    }
    Result.Ok((title, text_))
}
```

## 5. Autenticación con token

Los clientes envían `Authorization: Bearer <token>`. El servidor compara el token con el de
`NOTES_API_TOKEN` en un tiempo que no depende de dónde difieren, para que nadie pueda adivinarlo
byte a byte midiendo las respuestas:

<!-- check: project=examples/apps/notes-api -->
```rust
/// Whether `a` and `b` are equal, in time that depends only on their lengths.
pub fn same(a: string, b: string) -> bool {
    let x = a.to_bytes();
    let y = b.to_bytes();
    if (x.len() != y.len()) {
        return false;
    }
    var diff = 0;
    var i = 0;
    while (i < x.len()) {
        diff = diff | (x[i] ^ y[i]);
        i = i + 1;
    }
    diff == 0
}
```

`main` se niega a arrancar si el token tiene menos de 16 caracteres.

## 6. Arranque y apagado

<!-- check: project=examples/apps/notes-api -->
```rust
fn main() -> int {
    let token = env("NOTES_API_TOKEN").unwrap_or("");
    if (token.len() < 16) {
        eprint("set NOTES_API_TOKEN to a secret of at least 16 characters");
        return 64;
    }
    let db = notes.pool_from_env(10);
    match (notes.migrate(db)) {
        Result.Ok(_) => { },
        Result.Err(e) => {
            eprint("database: " + e);
            return 1;
        },
    }
    // HOST=0.0.0.0 inside a container; the default only listens on this machine.
    let host = env("HOST").unwrap_or("127.0.0.1");
    let port = env("PORT").unwrap_or("8080").parse_int().unwrap_or(8080);
    print("notes-api: http://" + host + ":" + to_string(port));
    // SIGTERM/SIGINT: stop accepting, let the requests in flight finish (5 s), close the pool.
    let served = listen_graceful(fn() -> App { routes(db, token) }, host, port, 5000);
    postgres.pool_close(db);
    match (served) {
        Result.Ok(_) => 0,
        Result.Err(e) => {
            eprint(e);
            1
        },
    }
}
```

- `HOST` es `127.0.0.1` por defecto. Dentro de un contenedor se pone `HOST=0.0.0.0`.
- `listen_graceful` atiende SIGTERM y Ctrl-C: deja de aceptar conexiones, espera hasta 5 segundos a
  las que están en curso y vuelve. Entonces `main` cierra el pool. Es lo que esperan Kubernetes,
  systemd y cualquier orquestador.

## 7. Tests con y sin base de datos

Las partes puras se prueban siempre. Los tests de base de datos se ejecutan contra un Postgres real
cuando `NOTES_TEST_PG=1` y las variables `PG*` lo indican; si no, avisan de que se saltan. Este
comprueba que veinte peticiones simultáneas comparten un pool de dos conexiones:

<!-- check: project=examples/apps/notes-api -->
```rust
@test
fn the_pool_serves_concurrent_requests() {
    let p = match (database()) {
        Option.Some(p) => p,
        Option.None => return,
    };
    // 20 fibers share a pool of 2 connections: each waits for a free one.
    let done: Channel<bool> = Channel.bounded(20);
    var i = 0;
    while (i < 20) {
        let _ = spawn(fn() {
            send(done, postgres.pool_query(p, "SELECT 1", []).is_ok());
        });
        i = i + 1;
    }
    var ok = 0;
    var j = 0;
    while (j < 20) {
        if (recv(done).unwrap()) {
            ok = ok + 1;
        }
        j = j + 1;
    }
    assert_eq(ok, 20);
    postgres.pool_close(p);
}
```

```sh
ray test                                                    # sin base de datos
docker run -d --name notes-pg -e POSTGRES_USER=notes -e POSTGRES_PASSWORD=notes \
  -e POSTGRES_DB=notes -p 127.0.0.1:55432:5432 postgres:18-alpine
NOTES_TEST_PG=1 PGHOST=127.0.0.1 PGPORT=55432 PGUSER=notes PGPASSWORD=notes ray test
```

## 8. Probarla y desplegarla

```sh
export PGHOST=127.0.0.1 PGPORT=55432 PGUSER=notes PGPASSWORD=notes PGDATABASE=notes
export NOTES_API_TOKEN=un-secreto-de-al-menos-16
ray run
curl -H "Authorization: Bearer $NOTES_API_TOKEN" -d '{"title":"Hola"}' http://127.0.0.1:8080/notes
```

Para producción, `ray build --native --release` produce un solo binario. Se configura entero con
variables de entorno (`PG*`, `NOTES_API_TOKEN`, `HOST`, `PORT`), así que encaja igual en systemd, en
un contenedor o en una plataforma de aplicaciones. El servidor del framework, compilado a nativo,
sirve del orden de 188 000 peticiones por segundo en el banco de carga del proyecto.

## Siguiente paso

Un [**sitio con frontend React**](web-react.md) embebido en el binario y una API JSON detrás, con
las notas en Redis.
