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
web = "^0.6.0"
db = "^0.5.0"
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

Los errores tienen siempre la misma forma, `{"error":"…"}`, y el estado dice de qué clase son. Un
cliente solo necesita mirar el estado para decidir, y el texto para mostrarlo:

| Estado | Cuándo |
|---|---|
| 401 | falta el token, o no es el correcto |
| 404 | la nota o la ruta no existen |
| 422 | el cuerpo no es JSON válido, o la nota no se puede guardar: sin título, título de más de 200 caracteres o cuerpo de más de 100 000 |
| 500 | la base de datos devolvió un error |
| 503 | solo en `/health`: la base de datos no responde |

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

| Función | Para qué | Si la conexión se cortó |
|---|---|---|
| `pool_query(p, sql, params)` | una consulta que devuelve filas | reintenta una vez con una conexión nueva |
| `pool_exec(p, sql, params)` | una sentencia que escribe; devuelve las filas afectadas | no reintenta: no sabe si llegó a ejecutarse |
| `pool_tx(p, f)` | varias sentencias en una transacción | reintenta solo el `BEGIN` |
| `pool_with(p, f)` | varias sentencias sobre la misma conexión, sin transacción | no reintenta |
| `pool_with_retry(p, f)` | como `pool_with`, para un bloque que se puede repetir sin daño | repite el bloque |
| `pool_close(p)` | cierra todas las conexiones, al apagar | |

El tamaño del pool, 10 en este ejemplo, es el máximo de consultas simultáneas contra Postgres. Una
petición que llega con todas las conexiones ocupadas espera a que se libere una, sin fallar. No
hace falta que sea grande: una consulta dura milisegundos y la conexión vuelve enseguida. Lo que
sí importa es que la suma de los pools de todas las instancias quepa en el `max_connections` del
servidor.

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

Leer una nota tiene tres finales, y el tipo de `notes.get` los separa:
`Result<Option<Note>, string>`. Con patrones anidados, cada final es un brazo y un estado HTTP:

<!-- check: project=examples/apps/notes-api -->
```rust
    app.GET("/notes/:id", fn(c: Ctx, r: Res) {
        match (notes.get(db, c.param("id"))) {
            Result.Ok(Option.Some(n)) => r.json(n.to_json()),
            Result.Ok(Option.None) => fail(r, 404, "no such note"),
            Result.Err(e) => fail(r, 500, e),
        }
    });
```

<!-- check: project=examples/apps/notes-api -->
```rust
// An error as JSON: {"error": "..."}.
fn fail(r: Res, code: int, message: string) {
    r.status(code).json(json.render(json.obj().field("error", message)));
}
```

El ejemplo devuelve en el 500 el mensaje de la base de datos tal cual, que es cómodo mientras
desarrollas. En un servicio público conviene escribir el detalle en el log y responder un texto
genérico, para no enseñar nombres de tablas a quien llama.

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

<!-- check: project=examples/apps/notes-api -->
```rust
/// Whether an `Authorization` header carries `token`.
pub fn authorized(header: string, token: string) -> bool {
    header.starts_with("Bearer ") && same(header.substring(7, header.len()), token)
}
```

Un token se genera con `openssl rand -hex 32` y se entrega al servicio como variable de entorno,
nunca en el código ni en el repositorio. Para cambiarlo basta reiniciar con el valor nuevo.

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
- Si además quieres límites propios (un cuerpo máximo de pocos KiB para una API) o HTTPS sin proxy,
  `listen_with` lo combina todo en una llamada:
  `listen_with(fn() -> App { routes(db, token) }, host, port, options().with_limits(limits).with_drain(5000))`.

Toda la configuración llega por variables de entorno:

| Variable | Para qué | Por defecto |
|---|---|---|
| `NOTES_API_TOKEN` | el token que deben enviar los clientes; 16 caracteres o más | ninguno: es obligatoria |
| `PGHOST`, `PGPORT` | dónde está Postgres | `127.0.0.1`, `5432` |
| `PGUSER`, `PGPASSWORD` | las credenciales | `notes`, vacía |
| `PGDATABASE` | la base de datos | `notes` |
| `HOST` | la dirección en la que escucha | `127.0.0.1` |
| `PORT` | el puerto | `8080` |

El código de salida le dice al orquestador qué pasó: 64 si falta el token, que es un error de
configuración y reintentar no lo arregla; 1 si la base de datos no responde al arrancar, que sí
merece un reintento; 0 tras un apagado ordenado.

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
```

Una sesión completa, con lo que responde cada petición:

```sh
# crear: 201, la cabecera Location y la nota
curl -i -H "Authorization: Bearer $NOTES_API_TOKEN" -d '{"title":"Hola"}' http://127.0.0.1:8080/notes
```

```text
HTTP/1.1 201 Created
Location: /notes/01a0fa15-c9d5-7201-91f7-b7a2d893f9ca

{"id":"01a0fa15-c9d5-7201-91f7-b7a2d893f9ca","title":"Hola","body":"","updated_ms":1790902127000}
```

```sh
# una nota sin título: 422
curl -H "Authorization: Bearer $NOTES_API_TOKEN" -d '{"title":""}' http://127.0.0.1:8080/notes
{"error":"title is required"}

# sin token: 401
curl http://127.0.0.1:8080/notes
{"error":"missing or wrong bearer token"}

# la comprobación de salud no pide token
curl http://127.0.0.1:8080/health
{"ok": true}
```

Para producción, `ray build --native --release` produce un solo binario. Se configura entero con
variables de entorno (`PG*`, `NOTES_API_TOKEN`, `HOST`, `PORT`), así que encaja igual en systemd, en
un contenedor o en una plataforma de aplicaciones. El servidor del framework, compilado a nativo,
sirve del orden de 188 000 peticiones por segundo en el banco de carga del proyecto.

Lo que un entorno de producción espera de un servicio, y cómo lo cumple este:

| Qué | En Notes |
|---|---|
| Configuración | variables de entorno, sin archivos |
| Comprobación de salud | `GET /health`, sin token: consulta la base de datos y responde 200 o 503 |
| Logs | una línea JSON por petición en la salida estándar, con identificador de traza |
| Apagado | con SIGTERM deja de aceptar, espera 5 segundos a las peticiones en curso y cierra el pool |
| Secretos | el token llega por el entorno y se compara en tiempo constante |
| HTTPS | un proxy delante, o `listen_tls` con el certificado y la clave |
| Contenedor | `HOST=0.0.0.0`, para escuchar fuera del propio contenedor |

## Siguiente paso

Un [**sitio con frontend React**](web-react.md) embebido en el binario y una API JSON detrás, con
las notas en Redis.
