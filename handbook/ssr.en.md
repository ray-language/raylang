# Server-rendered site

[Español](ssr.md) · English

The notes, now as a **website rendered on the server**. Every page is HTML that the server builds
with compiled raylang templates; the browser runs no JavaScript at all. Forms post to the server,
which saves and redirects. The notes are stored as **files**, one JSON file per note.

The complete project is in [`examples/apps/notes-ssr`](../examples/apps/notes-ssr/), with its
tests. The raylang blocks are copied from it and CI checks that they still are.

## 1. The project

```sh
ray new notes-ssr && cd notes-ssr
ray add web
```

`ray add web` downloads the framework (and `net`, which it depends on) and declares it in
`ray.toml`. The `[native]` section is added by hand:

```toml
[package]
name = "notes-ssr"
version = "0.1.0"

# Files baked into the native binary (served by `static_embedded`).
[native]
embed = ["static"]

[dependencies]
web = "^0.4.6"
```

```
notes-ssr/
├── ray.toml
├── ray.lock            # pinned versions of web and net
├── src/
│   ├── main.ray          # the server: routes and handlers
│   ├── pages.ray         # the pages as pure functions
│   ├── store.ray         # the notes as files
│   └── views/            # the .ray.html templates
├── static/app.css
└── tests/pages_test.ray
```

`web` is the application framework, in the style of Express, and runs on the HTTP server of the
`net` package. The exact versions are pinned in `ray.lock`. The [framework guide](../docs/web-framework.md) (Spanish) has every detail.

## 2. The server

The application is built by a top-level function. The server runs every connection in its own
fiber, with isolated memory, and each fiber calls that function to get its own copy of the app.
That is why the same code runs the same on the VM and compiled to a native binary.

<!-- check: project=examples/apps/notes-ssr -->
```rust
fn build_app() -> App {
    var app = new_app();
    app.log_requests();
    app.use_mw(same_origin);
    // `static/` is embedded: read live from disk under `ray run`, baked into the native binary.
    app.static_embedded("/assets/", "static");
```

- `log_requests()` writes one JSON line per request, with method, path, status and duration.
- `use_mw(same_origin)` registers a middleware: it runs before the routes and can stop the request
  (section 6).
- `static_embedded` serves `static/` under `/assets/`, with `ETag` and `304`.

A route reads the request and returns a page. This one creates a note from the form fields:

<!-- check: project=examples/apps/notes-ssr -->
```rust
    app.POST("/notes", fn(c: Ctx, r: Res) {
        let title = c.form_field("title");
        let body = c.form_field("body");
        match (store.save(notes(), "", title, body, time.now())) {
            Result.Ok(n) => go(r, "/notes/" + n.id),
            Result.Err(e) => r.status(422).html(pages.edit_page("/notes", title, body, e)),
        }
    });
```

If the note is not valid, the answer is the same form with the error and what the user typed, with
status 422. If it is saved, the browser gets a redirect:

<!-- check: project=examples/apps/notes-ssr -->
```rust
// After a POST, go to a page with GET (303 See Other).
fn go(r: Res, url: string) {
    r.redirect(url);
    let _ = r.status(303);
}
```

This is the **POST → redirect → GET** pattern: after submitting the form the browser lands on a
normal page, and reloading it does not submit anything again.

## 3. The templates

A `.ray.html` template is a raylang module. Its first line declares its parameters with types, and
the compiler turns it into a `render(...)` function. A mistake in a template variable is a compile
error, not an empty hole in the page.

The layout defines the common structure and a slot, `content`:

```html
{% params title: string %}
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{ title }} · Notes</title>
<link rel="stylesheet" href="/assets/app.css">
</head>
<body>
<header class="top">
    <a class="brand" href="/">Notes</a>
    <a class="button" href="/notes/new">New note</a>
</header>
<main>
{% block content %}
{% endblock %}
</main>
</body>
</html>
```

The home page **inherits** from the layout, receives the typed list of notes (`[store.Note]`) and
**includes** a partial template for each note:

```html
{% params title: string, query: string, notes: [store.Note] %}
{% extends layout %}
{% import store %}
{% block content %}
<form class="search" method="get" action="/">
    <input type="search" name="q" value="{{ query }}" placeholder="Search notes">
</form>
{% if notes.len() == 0 %}
<p class="empty">{% if query == "" %}No notes yet.{% else %}No notes match “{{ query }}”.{% endif %}</p>
{% else %}
<ul class="notes">
    {% for n in notes %}
    {% include views/card(n) %}
    {% endfor %}
</ul>
{% endif %}
{% endblock %}
```

```html
{% params n: store.Note %}
{% import store %}
<li>
    <a href="/notes/{{ n.id }}">
        <strong>{{ n.title }}</strong>
        <span>{{ n.body.split("\n")[0] }}</span>
    </a>
</li>
```

- `{{ expr }}` escapes HTML: a note titled `<b>hi</b>` shows exactly that, as text.
- `{% import store %}` brings in the module to use its `Note` type in the parameters.
- `{% extends layout %}` goes right after `{% params %}` and resolves next to the template;
  `{% include views/card(n) %}` resolves from `src/`.

In raylang, pages are built with pure functions: data in, HTML out. The handlers only read the
request, and the tests check every page without starting a server.

## 4. Markdown without risks

A note's body is written in Markdown and shown as HTML:

<!-- check: project=examples/apps/notes-ssr -->
```rust
/// One note, its Markdown body rendered to HTML. `None` if it does not exist.
pub fn note_page(s: Store, id: string) -> Option<string> {
    let n = store.get(s, id)?;
    // std/markdown escapes any HTML the user wrote and drops `javascript:` links, so the result
    // is safe to insert unescaped with {{& … }}.
    Option.Some(note.render(n.title, n, markdown.to_html(n.body)))
}
```

```html
{% params title: string, n: store.Note, body_html: string %}
{% extends layout %}
{% import store %}
{% block content %}
<article class="note">
    <h1>{{ n.title }}</h1>
    <div class="body">{{& body_html }}</div>
    <div class="actions">
        <a class="button" href="/notes/{{ n.id }}/edit">Edit</a>
        <form method="post" action="/notes/{{ n.id }}/delete">
            <button class="danger" type="submit">Delete</button>
        </form>
    </div>
</article>
{% endblock %}
```

`{{& body_html }}` inserts the HTML unescaped, so it has to be safe. It is: `std/markdown`
escapes any HTML the user writes and drops `javascript:` links. A note with `<script>` shows the
text `<script>`, and a test checks it:

<!-- check: project=examples/apps/notes-ssr -->
```rust
@test
fn the_note_page_renders_markdown_safely() {
    let s = fresh("note");
    let n = store.save(s, "", "Doc", "**strong** <script>alert(1)</script>", 1000).unwrap();
    let html = pages.note_page(s, n.id).unwrap();
    assert(html.contains("<strong>strong</strong>"));
    assert(!html.contains("<script>"));
    assert(pages.note_page(s, "no-such-id").is_none());
}
```

## 5. Files as shared state

Each connection runs in its fiber with its own memory: two requests never share a variable. On
this site, what they share is the **disk**. Each note is a JSON file and every write is atomic:

<!-- check: project=examples/apps/notes-ssr -->
```rust
    let note = Note { id: key, title: clean, body: body, updated_ms: now_ms };
    // Write to a temporary file, then rename it over the real one (atomic).
    let tmp = store.dir + "/." + key + ".tmp";
    let _ = fs.write_file(tmp, note.to_json())?;
    let _ = fs.rename(tmp, path_of(store, key))?;
    Result.Ok(note)
}
```

First a temporary file is written, then it is renamed over the real one. A rename is atomic: a
reader at that moment sees the old note or the new one, never a mix.

A note's id becomes part of a file path, so the store only accepts the ids it generates itself. A
request for `/notes/../../etc/passwd` never reaches the disk:

<!-- check: project=examples/apps/notes-ssr -->
```rust
// An id becomes part of a file path, so only ids this store generates are accepted: a request
// for `../../etc/passwd` must not reach the filesystem.
fn valid_id(id: string) -> bool {
    uuid.is_uuid_v7(id)
}
```

For a site with few writers, files are enough and need no service. With many concurrent writes,
the next step is a database, as in the API chapter.

## 6. Forms and CSRF

A form on another site can send a POST to your server with the user's cookies. That is the CSRF
attack. Browsers always set the `Origin` header on a cross-site POST, so it is enough to refuse the
ones that do not match the server's own host:

<!-- check: project=examples/apps/notes-ssr -->
```rust
// CSRF guard: a form posted from ANOTHER site carries that site's Origin. Browsers always send
// Origin on a cross-site POST, so refusing a mismatch blocks forged requests.
fn same_origin(c: Ctx, r: Res) -> Step {
    if (c.req.method == "POST") {
        let origin = c.header_of("origin");
        if (origin != "" && origin_host(origin) != c.header_of("host").to_lower()) {
            r.status(403).text("cross-site request refused");
            return Step.Done;
        }
    }
    Step.Next
}
```

## 7. Test and run

```sh
ray test                       # the store and the pages, without a server
ray run                        # http://127.0.0.1:8080
ray dev                        # reloads the browser on save
```

`main` reads the port from `PORT` and shuts the server down cleanly on SIGTERM or Ctrl-C: it stops
accepting connections and waits up to 5 seconds for the ones in flight.

<!-- check: project=examples/apps/notes-ssr -->
```rust
fn main() -> int {
    let port = env("PORT").unwrap_or("8080").parse_int().unwrap_or(8080);
    print("notes: http://127.0.0.1:" + to_string(port));
    // SIGTERM/SIGINT: stop accepting, let the requests in flight finish (5 s), exit 0.
    match (listen_graceful(build_app, "127.0.0.1", port, 5000)) {
        Result.Ok(_) => 0,
        Result.Err(e) => {
            eprint(e);
            1
        },
    }
}
```

## 8. Ship a single binary

```sh
ray build --native --release -o notes-ssr
PORT=8080 NOTES_DIR=/var/lib/notes ./notes-ssr
```

The binary carries the compiled templates and the stylesheet inside (`[native] embed`), so it runs
from any folder. The Notes one is about 3 MB. Behind a proxy such as nginx or Caddy, which handles
HTTPS, it is ready for production. If you prefer the program to serve HTTPS itself, `listen_tls`
takes the certificate and the key.

## Next step

The same kind of server, without pages: a **web API** that answers JSON, with Postgres and
connection pools. It is the next handbook chapter.

<!-- sync: sha256:9d6ecf3a03a6 -->
