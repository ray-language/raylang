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
curl -H "Authorization: Bearer $NOTES_API_TOKEN" -d '{"title":"Hello"}' http://127.0.0.1:8080/notes
```

For production, `ray build --native --release` produces a single binary. It is configured entirely
with environment variables (`PG*`, `NOTES_API_TOKEN`, `HOST`, `PORT`), so it fits systemd, a
container or an application platform alike. The framework's server, compiled to native, serves on
the order of 188,000 requests per second in the project's load benchmark.

## Next step

A site with a **React frontend embedded** in the binary and a JSON API behind it, with the notes in
Redis. It is the next handbook chapter.

<!-- sync: sha256:89d4e526fd27 -->
