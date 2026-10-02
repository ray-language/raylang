# Web API

[Español](api.md) · English

The notes leave the device: a **JSON API** with the `web` framework, the data in **Postgres**
through a connection pool, token authentication, compression, JSON logs and graceful shutdown. It
is the shape of a production service.

The complete project is in [`examples/apps/notes-api`](../examples/apps/notes-api/), with its
tests. The raylang blocks are copied from it and CI checks that they still are.

## 1. The project

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

`web` brings the HTTP server of the `net` package; `db` brings the Postgres client. The exact
versions are pinned in `ray.lock`. The code is split into three modules: `notes.ray` (the data),
`auth.ray` (the token) and `main.ray` (the routes).

| Route | What it does | Answers |
|---|---|---|
| `GET /health` | checks the database | 200, 503 |
| `GET /notes?q=…&limit=…` | lists and searches | 200 |
| `GET /notes/:id` | one note | 200, 404 |
| `POST /notes` | creates | 201 with `Location`, 422 |
| `PUT /notes/:id` | replaces | 200, 404, 422 |
| `DELETE /notes/:id` | deletes | 204, 404 |

Errors always have the same shape, `{"error":"…"}`, and the status says what kind they are. A
client only needs the status to decide, and the text to show:

| Status | When |
|---|---|
| 401 | the token is missing, or is not the right one |
| 404 | the note or the route does not exist |
| 422 | the body is not valid JSON, or the note cannot be saved: no title, a title over 200 characters or a body over 100,000 |
| 500 | the database returned an error |
| 503 | only on `/health`: the database is not answering |

## 2. A shared connection pool

The framework runs each request in its own fiber, with isolated memory. A connection kept in a
variable would not be shared between requests, and opening a new one per query is slow: every
connection negotiates authentication with the server. The answer is a **pool**: it travels through
a channel, so every fiber shares it; it opens connections on demand, reuses them and replaces the
ones that break.

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

The pool is created once in `main`. The configuration uses the standard Postgres variables, the
same ones `psql` and any deployment platform understand.

On start, the program creates the table if it does not exist, inside a transaction:

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

`pool_tx` takes a connection and runs `BEGIN`, the function and `COMMIT`; if the function fails, it
runs `ROLLBACK`. `pool_query` and `pool_exec` are for a single statement, and `pool_query` retries
once on a fresh connection if the reused one was cut (for example because the server restarted).

| Function | What for | If the connection was cut |
|---|---|---|
| `pool_query(p, sql, params)` | a query that returns rows | retries once on a fresh connection |
| `pool_exec(p, sql, params)` | a statement that writes; returns the affected rows | does not retry: it cannot know whether it ran |
| `pool_tx(p, f)` | several statements in one transaction | retries only the `BEGIN` |
| `pool_with(p, f)` | several statements on the same connection, no transaction | does not retry |
| `pool_with_retry(p, f)` | like `pool_with`, for a block that is safe to repeat | repeats the block |
| `pool_close(p)` | closes every connection, on shutdown | |

The pool size, 10 in this example, is the maximum number of simultaneous queries against Postgres.
A request that arrives while every connection is busy waits for one to be released; it does not
fail. It does not need to be large: a query takes milliseconds and the connection comes straight
back. What does matter is that the pools of all instances together fit in the server's
`max_connections`.

## 3. Queries with parameters

Values go in `$1`, `$2`, …, separately from the SQL. A quote in a search is data, not code. The
search uses `position` instead of `LIKE`, so `%` and `_` are ordinary characters too:

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

`db` returns every cell as text, so numbers are converted with `parse_int()`.

## 4. The routes

The app is built by a function that receives the pool. Each connection's fiber calls it and gets
its own copy of the app, but they all share the same pool:

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

- `log_requests()` writes one JSON line per request, with method, path, status, duration and a
  trace id.
- `gzip()` compresses responses of 512 bytes or more when the client accepts it.
- `use_on("/notes", …)` applies the middleware only to routes under `/notes`; `/health` stays open
  for the load balancer.

Reading a note has three endings, and the type of `notes.get` separates them:
`Result<Option<Note>, string>`. With nested patterns, each ending is one arm and one HTTP status:

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

The example returns the database's message as it is in the 500, which is handy while developing.
In a public service, write the detail to the log and answer with a generic text, so table names
are not shown to callers.

Creating a note reads the JSON body, validates it and answers 201 with the `Location` header:

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

Reading the body uses `?` to propagate both malformed JSON and an invalid note, and both end up as
a 422 with the reason:

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

## 5. Token authentication

Clients send `Authorization: Bearer <token>`. The server compares the token with the one in
`NOTES_API_TOKEN` in a time that does not depend on where they differ, so nobody can guess it byte
by byte by timing the answers:

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

`main` refuses to start if the token is shorter than 16 characters.

<!-- check: project=examples/apps/notes-api -->
```rust
/// Whether an `Authorization` header carries `token`.
pub fn authorized(header: string, token: string) -> bool {
    header.starts_with("Bearer ") && same(header.substring(7, header.len()), token)
}
```

A token is generated with `openssl rand -hex 32` and handed to the service as an environment
variable, never in the code or the repository. Changing it only takes a restart with the new
value.

## 6. Startup and shutdown

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

- `HOST` defaults to `127.0.0.1`. Inside a container, set `HOST=0.0.0.0`.
- `listen_graceful` handles SIGTERM and Ctrl-C: it stops accepting connections, waits up to 5
  seconds for the ones in flight and returns. Then `main` closes the pool. That is what Kubernetes,
  systemd and any orchestrator expect.

All the configuration comes from environment variables:

| Variable | What for | Default |
|---|---|---|
| `NOTES_API_TOKEN` | the token clients must send; 16 characters or more | none: it is required |
| `PGHOST`, `PGPORT` | where Postgres is | `127.0.0.1`, `5432` |
| `PGUSER`, `PGPASSWORD` | the credentials | `notes`, empty |
| `PGDATABASE` | the database | `notes` |
| `HOST` | the address it listens on | `127.0.0.1` |
| `PORT` | the port | `8080` |

The exit code tells the orchestrator what happened: 64 if the token is missing, which is a
configuration error that retrying will not fix; 1 if the database does not answer at startup,
which does deserve a retry; 0 after a graceful shutdown.

## 7. Tests with and without a database

The pure parts are always tested. The database tests run against a real Postgres when
`NOTES_TEST_PG=1` and the `PG*` variables say so; otherwise they report that they were skipped.
This one checks that twenty simultaneous requests share a pool of two connections:

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
ray test                                                    # no database
docker run -d --name notes-pg -e POSTGRES_USER=notes -e POSTGRES_PASSWORD=notes \
  -e POSTGRES_DB=notes -p 127.0.0.1:55432:5432 postgres:18-alpine
NOTES_TEST_PG=1 PGHOST=127.0.0.1 PGPORT=55432 PGUSER=notes PGPASSWORD=notes ray test
```

## 8. Try it and deploy it

```sh
export PGHOST=127.0.0.1 PGPORT=55432 PGUSER=notes PGPASSWORD=notes PGDATABASE=notes
export NOTES_API_TOKEN=a-secret-of-at-least-16
ray run
```

A complete session, with what each request answers:

```sh
# create: 201, the Location header and the note
curl -i -H "Authorization: Bearer $NOTES_API_TOKEN" -d '{"title":"Hello"}' http://127.0.0.1:8080/notes
```

```text
HTTP/1.1 201 Created
Location: /notes/01a0fa15-c9d5-7201-91f7-b7a2d893f9ca

{"id":"01a0fa15-c9d5-7201-91f7-b7a2d893f9ca","title":"Hello","body":"","updated_ms":1790902127000}
```

```sh
# a note without a title: 422
curl -H "Authorization: Bearer $NOTES_API_TOKEN" -d '{"title":""}' http://127.0.0.1:8080/notes
{"error":"title is required"}

# no token: 401
curl http://127.0.0.1:8080/notes
{"error":"missing or wrong bearer token"}

# the health check asks for no token
curl http://127.0.0.1:8080/health
{"ok": true}
```

For production, `ray build --native --release` produces a single binary. It is configured entirely
with environment variables (`PG*`, `NOTES_API_TOKEN`, `HOST`, `PORT`), so it fits systemd, a
container or an application platform alike. The framework's server, compiled to native, serves on
the order of 188,000 requests per second in the project's load benchmark.

What a production environment expects from a service, and how this one meets it:

| What | In Notes |
|---|---|
| Configuration | environment variables, no files |
| Health check | `GET /health`, no token: it queries the database and answers 200 or 503 |
| Logs | one JSON line per request on standard output, with a trace identifier |
| Shutdown | on SIGTERM it stops accepting, waits 5 seconds for requests in flight and closes the pool |
| Secrets | the token comes from the environment and is compared in constant time |
| HTTPS | a proxy in front, or `listen_tls` with the certificate and the key |
| Container | `HOST=0.0.0.0`, to listen outside the container itself |

## Next step

A [**site with a React frontend**](web-react.en.md) embedded in the binary and a JSON API behind it,
with the notes in Redis.

<!-- sync: sha256:53f9c2f5b319 -->
