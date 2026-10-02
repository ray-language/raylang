# Sitio con frontend React

Español · [English](web-react.en.md)

Las notas como sitio web con un **frontend React + TypeScript**, una **API JSON** y los datos en
**Redis**. En desarrollo, Vite sirve la página con recarga en caliente; en producción, el frontend
construido va **dentro del binario**, y un solo ejecutable sirve la página, los recursos y la API.

El proyecto completo está en [`examples/apps/notes-web`](../examples/apps/notes-web/), con sus
tests. Los bloques de raylang están copiados de él y el CI comprueba que sigan siéndolo.

## 1. El proyecto

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

La sección `[frontend]` conecta el programa con Vite:

- `ray dev` arranca Vite junto al programa y lo detiene al salir;
- `ray build --native` ejecuta `build` y mete `dist` dentro del binario.

Quién sirve cada cosa cambia entre desarrollo y producción, pero el navegador ve siempre un solo
origen:

| | En desarrollo, con `ray dev` | En producción, el binario |
|---|---|---|
| La página | Vite, en `localhost:5173`, con recarga en caliente | el programa, desde el frontend embebido |
| `/assets/…` | Vite | el programa, con `ETag` y gzip |
| `/api/…` | Vite lo reenvía al programa | el programa |
| Procesos | dos: Vite y el programa | uno |

Como la página y la API comparten origen en los dos casos, no hace falta configurar CORS, y las
rutas relativas del frontend no cambian.

`net` se añade además de `web` porque el programa usa directamente su cliente de Redis.

## 2. Las notas en Redis

Cada nota es un hash, `note:<id>`, con su título, su cuerpo y su fecha. Un conjunto ordenado,
`notes:by_date`, guarda los ids por fecha de edición: es el índice para listar de la más nueva a la
más vieja.

| Clave | Tipo | Contiene |
|---|---|---|
| `note:<id>` | hash | los campos `title`, `body` y `updated_ms` |
| `notes:by_date` | conjunto ordenado | los ids, con la fecha de edición como puntuación |

Se pueden mirar con `redis-cli` mientras la app corre:

```sh
redis-cli -p 56379 ZREVRANGE notes:by_date 0 -1      # los ids, de la más nueva a la más vieja
redis-cli -p 56379 HGETALL note:<id>                 # una nota
```

El pool reparte cada comando a la conexión que esté libre, así que dos comandos seguidos no son una
unidad: otra petición podría colarse entre ellos. Las escrituras que tocan las dos claves van como
un script Lua, y Redis ejecuta un script entero sin interrupciones:

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

El último argumento de `pool_command_with`, `false`, desactiva el reintento automático: un
comando que escribe no debe repetirse a ciegas si la conexión se cortó a mitad.

Listar recorre el índice y lee cada hash. La búsqueda filtra en el programa:

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

Para unos miles de notas es suficiente. Con muchas más, Redis tiene módulos de búsqueda, o se pasa
a Postgres como en el capítulo de la API.

## 3. El frontend

El frontend es una app de React normal. Habla con el programa por `fetch` con rutas relativas, que
funcionan igual en desarrollo y en producción:

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

En desarrollo la página viene de Vite, en `localhost:5173`, y Vite redirige `/api` al programa:

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

`ray dev` pasa su entorno a los dos procesos, así que `PORT` vale para el programa y para el proxy.

## 4. Servir la SPA desde el binario

En producción, el programa sirve los archivos construidos. Solo se monta `/assets/`: un montaje
responde a todas las peticiones GET bajo su prefijo antes de que lleguen a las rutas, así que montar
`/` taparía también la API.

<!-- check: project=examples/apps/notes-web -->
```rust
    // The built frontend: read from disk under `ray run`, baked into the native binary. Only
    // /assets/ is mounted: a mount answers every GET under its prefix before the routes run, so
    // mounting "/" would swallow /api too. The page itself comes from `spa`.
    app.static_embedded("/assets/", "frontend/dist/assets");
    app.GET("/", spa);
```

La página en sí la sirve `spa`. También responde a cualquier ruta que no sea de la API ni un
archivo, para que una URL del enrutador del navegador (`/notes/123`) funcione al recargar:

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

`app.gzip()` comprime los recursos: el JavaScript de Notes pasa de 222 KB a 69 KB.

Las rutas del programa, completas:

| Ruta | Qué hace | Responde |
|---|---|---|
| `GET /api/notes?q=…&limit=…` | lista y busca | 200, 503 |
| `POST /api/notes` | crea | 201, 422, 503 |
| `PUT /api/notes/:id` | reemplaza | 200, 404, 422, 503 |
| `DELETE /api/notes/:id` | borra | 204, 404, 503 |
| `GET /assets/…` | los recursos del frontend | 200, 304 |
| cualquier otro `GET` | `index.html` | 200 |

La última fila es `app.not_found(spa)`: lo que ninguna ruta atiende va a la misma función. Las
rutas de la API separan sus finales con el tipo que devuelve el almacén,
`Result<Option<Note>, string>`:

<!-- check: project=examples/apps/notes-web -->
```rust
    app.PUT("/api/notes/:id", fn(c: Ctx, r: Res) {
        match (note_input(c)) {
            Result.Err(e) => fail(r, 422, e),
            Result.Ok(input) => {
                let (title, body) = input;
                match (store.save(db, c.param("id"), title, body, time.now())) {
                    Result.Ok(Option.Some(n)) => r.json(n.to_json()),
                    Result.Ok(Option.None) => fail(r, 404, "no such note"),
                    Result.Err(e) => fail(r, 503, e),
                }
            },
        }
    });
```

El 503 dice que falló una dependencia, Redis, y no la petición: un cliente puede reintentarla.

## 5. Tests

Los tests del almacén se ejecutan contra un Redis real cuando `NOTES_TEST_REDIS=1`; si no, avisan
de que se saltan. Este comprueba que borrar quita también la entrada del índice:

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

## 6. Desarrollar y desplegar

```sh
REDIS_PORT=56379 ray dev                  # abre http://localhost:5173
ray build --native --release -o notes-web
REDIS_PORT=56379 ./notes-web              # http://127.0.0.1:8080: página, recursos y API
```

El binario lleva dentro el frontend construido, así que funciona desde cualquier carpeta: el de
Notes ocupa unos 3 MB. Se configura con `HOST`, `PORT`, `REDIS_HOST` y `REDIS_PORT`.

| Variable | Para qué | Por defecto |
|---|---|---|
| `HOST` | la dirección en la que escucha; `0.0.0.0` en un contenedor | `127.0.0.1` |
| `PORT` | el puerto; en desarrollo lo lee también el proxy de Vite | `8080` |
| `REDIS_HOST` | dónde está Redis | `127.0.0.1` |
| `REDIS_PORT` | su puerto | `6379` |

Al arrancar, el programa envía un `PING` a Redis y sale con el código 1 si no responde, en lugar
de aceptar peticiones que van a fallar. Con SIGTERM deja de aceptar, espera 5 segundos a las
peticiones en curso y cierra el pool, igual que la [API](api.md).

Para publicar una versión nueva basta reemplazar el binario: el frontend va dentro, así que la
página y la API nunca quedan en versiones distintas.

## Siguiente paso

Del navegador a la terminal: una [**herramienta de línea de comandos**](cli.md), el entregable más
pequeño de raylang.
