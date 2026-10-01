# Site with a React frontend

[Español](web-react.md) · English

The notes as a website with a **React + TypeScript frontend**, a **JSON API** and the data in
**Redis**. During development Vite serves the page with hot reload; in production the built
frontend goes **inside the binary**, and a single executable serves the page, the assets and the
API.

The complete project is in [`examples/apps/notes-web`](../examples/apps/notes-web/), with its
tests. The raylang blocks are copied from it and CI checks that they still are.

## 1. The project

```sh
ray new notes-web && cd notes-web
ray add web
ray add net
npm create vite@latest frontend -- --template react-ts
npm --prefix frontend install
```

```toml
[dependencies]
web = "^0.4.6"
net = "^0.5.0"

[frontend]
dev = "npm --prefix frontend run dev -- --strictPort --clearScreen false"
url = "http://localhost:5173"
build = "npm --prefix frontend run build"
dist = "frontend/dist"
```

The `[frontend]` section connects the program with Vite:

- `ray dev` starts Vite next to the program and stops it on exit;
- `ray build --native` runs `build` and puts `dist` inside the binary.

`net` is added besides `web` because the program uses its Redis client directly.

## 2. The notes in Redis

Each note is a hash, `note:<id>`, with its title, body and date. A sorted set, `notes:by_date`,
keeps the ids by edit date: it is the index for listing from newest to oldest.

The pool hands each command to whichever connection is free, so two commands in a row are not a
unit: another request could slip in between them. Writes that touch both keys go as a Lua script,
and Redis runs a whole script without interruption:

<!-- check: project=examples/apps/notes-web -->
```rust
// Writes the hash and its place in the index together.
const SAVE: string = `redis.call('HSET', KEYS[1], 'title', ARGV[2], 'body', ARGV[3], 'updated_ms', ARGV[4])
redis.call('ZADD', KEYS[2], ARGV[4], ARGV[1])
return 1`;

// Removes the hash and its place in the index together; 0 if it did not exist.
const REMOVE: string = `local n = redis.call('DEL', KEYS[1])
redis.call('ZREM', KEYS[2], ARGV[1])
return n`;
```

<!-- check: project=examples/apps/notes-web -->
```rust
    let _ = ok(
        redis.pool_command_with(
            p,
            ["EVAL", SAVE, "2", key(n.id), INDEX, n.id, n.title, n.body, to_string(now_ms)],
            false
        )?
    )?;
```

The last argument of `pool_command_with`, `false`, turns off the automatic retry: a command that
writes must not be repeated blindly if the connection broke halfway.

Listing walks the index and reads each hash. The search filters in the program:

<!-- check: project=examples/apps/notes-web -->
```rust
/// The newest notes (at most `limit`) whose title or body contains `query` (any case).
pub fn list(p: Pool, query: string, limit: int) -> Result<[Note], string> {
    let ids = match (ok(redis.pool_command(p, ["ZREVRANGE", INDEX, "0", "-1"])?)?) {
        Reply.Arr(items) => items,
        _ => [],
    };
    let needle = query.trim().to_lower();
    var out: [Note] = [];
    for item in ids {
        if (out.len() >= limit) {
            break;
        }
        match (item) {
            Reply.Str(id) => match (get(p, id)?) {
                Option.Some(n) => {
                    if (needle == "" || (n.title + "\n" + n.body).to_lower().contains(needle)) {
                        out.push(n);
                    }
                },
                Option.None => { },
            },
            _ => { },
        }
    }
    Result.Ok(out)
}
```

For a few thousand notes that is enough. With many more, Redis has search modules, or you move to
Postgres as in the API chapter.

## 3. The frontend

The frontend is a normal React app. It talks to the program with `fetch` and relative URLs, which
work the same in development and in production:

```ts
// The JSON API of the raylang program. The same relative URLs work in development (Vite forwards
// /api) and in production (the program serves the page and the API from one origin).

export type Note = { id: string; title: string; body: string; updated_ms: number }

async function call<T>(method: string, url: string, body?: unknown): Promise<T> {
  const res = await fetch(url, {
    method,
    headers: body === undefined ? {} : { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  if (!res.ok) {
    const err = (await res.json().catch(() => ({}))) as { error?: string }
    throw new Error(err.error ?? `HTTP ${res.status}`)
  }
  return (res.status === 204 ? undefined : await res.json()) as T
}

export const listNotes = (q: string) => call<Note[]>('GET', `/api/notes?q=${encodeURIComponent(q)}`)
export const createNote = (title: string, body: string) => call<Note>('POST', '/api/notes', { title, body })
export const updateNote = (id: string, title: string, body: string) =>
  call<Note>('PUT', `/api/notes/${id}`, { title, body })
export const deleteNote = (id: string) => call<void>('DELETE', `/api/notes/${id}`)
```

During development the page comes from Vite, on `localhost:5173`, and Vite forwards `/api` to the
program:

```ts
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// In development the page comes from Vite (hot reload) and /api goes to the raylang program,
// which `ray dev` runs on PORT (8080 by default).
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: { '/api': `http://127.0.0.1:${process.env.PORT ?? '8080'}` },
  },
})
```

`ray dev` passes its environment to both processes, so `PORT` applies to the program and to the
proxy.

## 4. Serving the SPA from the binary

In production, the program serves the built files. Only `/assets/` is mounted: a mount answers
every GET under its prefix before the routes see it, so mounting `/` would hide the API too.

<!-- check: project=examples/apps/notes-web -->
```rust
    // The built frontend: read from disk under `ray run`, baked into the native binary. Only
    // /assets/ is mounted: a mount answers every GET under its prefix before the routes run, so
    // mounting "/" would swallow /api too. The page itself comes from `spa`.
    app.static_embedded("/assets/", "frontend/dist/assets");
    app.GET("/", spa);
```

The page itself is served by `spa`. It also answers any path that is neither the API nor a file,
so a URL of the browser's router (`/notes/123`) works on reload:

<!-- check: project=examples/apps/notes-web -->
```rust
// The single-page app: every GET that is not /api or a file gets index.html, so the browser's
// router (client-side URLs such as /notes/123) works on reload too.
fn spa(c: Ctx, r: Res) {
    if (c.req.method != "GET" || c.req.path.starts_with("/api/")) {
        fail(r, 404, "no such route");
        return;
    }
    match (embed.read("frontend/dist/index.html")) {
        Result.Ok(page) => r.status(200).html(from_utf8(page).unwrap_or("")),
        Result.Err(_) => r.text("frontend not built: run `npm --prefix frontend run build`"),
    }
}
```

`app.gzip()` compresses the assets: the Notes JavaScript goes from 222 KB to 69 KB.

## 5. Tests

The store tests run against a real Redis when `NOTES_TEST_REDIS=1`; otherwise they report that they
were skipped. This one checks that deleting also removes the index entry:

<!-- check: project=examples/apps/notes-web -->
```rust
@test
fn deleting_also_leaves_the_index() {
    let p = match (database()) {
        Option.Some(p) => p,
        Option.None => return,
    };
    let n = store.save(p, "", "Gone " + uuid.uuid_v4(), "", 1000).unwrap().unwrap();
    let _ = store.remove(p, n.id).unwrap();
    // The Lua script removed the hash and the index entry together.
    match (redis.pool_command(p, ["ZSCORE", "notes:by_date", n.id]).unwrap()) {
        redis.Reply.Nil => { },
        other => panic("still in the index: " + redis.reply_str(other)),
    }
    redis.pool_close(p);
}
```

```sh
docker run -d --name notes-redis -p 127.0.0.1:56379:6379 redis:8-alpine
NOTES_TEST_REDIS=1 REDIS_PORT=56379 ray test
```

## 6. Develop and deploy

```sh
REDIS_PORT=56379 ray dev                  # opens http://localhost:5173
ray build --native --release -o notes-web
REDIS_PORT=56379 ./notes-web              # http://127.0.0.1:8080: page, assets and API
```

The binary carries the built frontend, so it runs from any folder: the Notes one is about 3 MB. It
is configured with `HOST`, `PORT`, `REDIS_HOST` and `REDIS_PORT`.

## Next step

[**LLMs and MCP**](llm-mcp.en.md): how to use an assistant that writes verified raylang, and how to
build an agent in raylang that talks to Claude and uses tools.

<!-- sync: sha256:c362b3b6e8a0 -->
