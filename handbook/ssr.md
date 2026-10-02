# Sitio con plantillas

Español · [English](ssr.en.md)

Las notas, ahora como **sitio web renderizado en el servidor**. Cada página es HTML que el servidor
construye con plantillas de raylang compiladas; el navegador no ejecuta nada de JavaScript. Los
formularios envían al servidor, que guarda y redirige. Las notas se guardan como **archivos**, un
JSON por nota.

Este diseño conviene cuando el contenido pesa más que la interacción: páginas que se leen,
formularios, paneles de administración. La primera respuesta ya es la página completa, funciona
sin JavaScript y no hay un frontend que construir. Para una interfaz con mucho estado en el
navegador, el capítulo [sitio con frontend React](web-react.md) es la otra opción.

El proyecto completo está en [`examples/apps/notes-ssr`](../examples/apps/notes-ssr/), con sus
tests. Los bloques de raylang están copiados de él y el CI comprueba que sigan siéndolo.

## 1. El proyecto

```sh
ray new notes-ssr && cd notes-ssr
ray add web
```

`ray add web` descarga el framework (y `net`, del que depende) y lo declara en el `ray.toml`. Se
añade a mano la sección `[native]`:

```toml
[package]
name = "notes-ssr"
version = "0.1.0"

# Archivos que van dentro del binario nativo (los sirve `static_embedded`).
[native]
embed = ["static"]

[dependencies]
web = "^0.4.6"
```

```
notes-ssr/
├── ray.toml
├── ray.lock            # versiones fijadas de web y net
├── src/
│   ├── main.ray          # el servidor: rutas y handlers
│   ├── pages.ray         # las páginas como funciones puras
│   ├── store.ray         # las notas en archivos
│   └── views/            # las plantillas .ray.html
├── static/app.css
└── tests/pages_test.ray
```

`web` es el framework de aplicación, al estilo de Express, y corre sobre el servidor HTTP del
paquete `net`. Las versiones exactas quedan fijadas en `ray.lock`. La [guía del framework](../docs/web-framework.md) tiene todo el detalle.

Estas son las rutas del sitio. Un formulario HTML solo sabe enviar `GET` y `POST`, así que editar
y borrar también son `POST`:

| Ruta | Qué hace | Responde |
|---|---|---|
| `GET /` | la lista de notas; `?q=` busca | 200 |
| `GET /notes/new` | el formulario vacío | 200 |
| `POST /notes` | crea una nota | 303 a la nota, o 422 con el formulario |
| `GET /notes/:id` | una nota, con su Markdown ya convertido | 200, 404 |
| `GET /notes/:id/edit` | el formulario con la nota | 200, 404 |
| `POST /notes/:id` | guarda los cambios | 303 a la nota, o 422 |
| `POST /notes/:id/delete` | borra | 303 a `/` |
| `GET /assets/…` | la hoja de estilos | 200, 304 |

## 2. El servidor

La aplicación se construye en una función de nivel superior. El servidor ejecuta cada conexión en
su propia fibra, con memoria aislada, y cada fibra llama a esa función para tener su copia de la
app. Por eso el mismo código corre igual en la VM y compilado a binario nativo.

<!-- check: project=examples/apps/notes-ssr -->
```rust
fn build_app() -> App {
    var app = new_app();
    app.log_requests();
    app.use_mw(same_origin);
    // `static/` is embedded: read live from disk under `ray run`, baked into the native binary.
    app.static_embedded("/assets/", "static");
```

- `log_requests()` escribe una línea JSON por petición, con método, ruta, estado y duración.
- `use_mw(same_origin)` registra un middleware: corre antes de las rutas y puede cortar la
  petición (sección 6).
- `static_embedded` sirve `static/` bajo `/assets/`, con `ETag` y `304`.

La línea que escribe `log_requests()` va a la salida estándar, lista para un recolector de logs:

```json
{"ts":"2026-10-02T00:48:47Z","level":"INFO","service":"web","trace_id":"9fe1bc0fa334f19c853116ea16dc31cd","msg":"request","method":"POST","path":"/notes","status":303,"ms":0}
```

Un handler recibe dos valores: `c`, la petición, y `r`, la respuesta que va construyendo.

| Para leer la petición | Devuelve |
|---|---|
| `c.param("id")` | el segmento `:id` de la ruta |
| `c.query("q")` | un parámetro de la URL, o `""` si no viene |
| `c.form_field("title")` | un campo del formulario enviado, o `""` |
| `c.header_of("origin")` | una cabecera, o `""` |
| `c.json_body()` | el cuerpo como JSON, en un `Result` |

| Para responder | Hace |
|---|---|
| `r.html(texto)`, `r.text(texto)`, `r.json(texto)` | fija el cuerpo y su tipo de contenido |
| `r.status(422)` | fija el estado; se encadena: `r.status(422).html(…)` |
| `r.header(nombre, valor)` | añade una cabecera |
| `r.redirect(url)` | redirige |

Una ruta lee la petición y devuelve una página. Esta crea una nota con los campos del formulario:

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

Si la nota no es válida, la respuesta es el mismo formulario con el error y lo que el usuario
escribió, con estado 422. Si se guarda, el navegador recibe una redirección:

<!-- check: project=examples/apps/notes-ssr -->
```rust
// After a POST, go to a page with GET (303 See Other).
fn go(r: Res, url: string) {
    r.redirect(url);
    let _ = r.status(303);
}
```

Es el patrón **POST → redirección → GET**: después de enviar el formulario el navegador queda en
una página normal, y recargarla no vuelve a enviar nada.

## 3. Las plantillas

Una plantilla `.ray.html` es un módulo de raylang. La primera línea declara sus parámetros con
tipos, y el compilador la convierte en una función `render(...)`. Un error en una variable de la
plantilla es un error de compilación, no un hueco vacío en la página.

El layout define la estructura común y un hueco, `content`:

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

La página de inicio **hereda** del layout, recibe la lista de notas tipada (`[store.Note]`) e
**incluye** una plantilla parcial por cada nota:

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

- `{{ expr }}` escapa el HTML: una nota titulada `<b>hola</b>` se muestra tal cual, como texto.
- `{% import store %}` trae el módulo para usar su tipo `Note` en los parámetros.
- `{% extends layout %}` va justo después de `{% params %}` y se resuelve junto a la plantilla;
  `{% include views/card(n) %}` se resuelve desde `src/`.

Todas las etiquetas:

| Etiqueta | Qué hace |
|---|---|
| `{% params a: T, b: U %}` | la primera línea: los parámetros de `render`, con sus tipos |
| `{{ expr }}` | escribe el valor, con el HTML escapado |
| `{{& expr }}` | escribe el valor tal cual, sin escapar |
| `{% if c %}` … `{% elif c %}` … `{% else %}` … `{% endif %}` | condicional |
| `{% for x in xs %}` … `{% endfor %}` | bucle |
| `{% let n = expr %}` | una variable local |
| `{% include ruta(args) %}` | inserta otra plantilla |
| `{% extends ruta %}` | hereda de un layout |
| `{% block nombre %}` … `{% endblock %}` | un hueco del layout, o lo que lo rellena |
| `{% import módulo %}` | trae un módulo para usar sus tipos y funciones |

Dentro de `{{ }}` y `{% %}` va raylang normal: `{{ n.body.split("\n")[0] }}` es una expresión
como cualquier otra, y el editor la autocompleta y la comprueba.

Una plantilla se usa como un módulo más: `import views/index;` y después
`index.render("All notes", query, notes)`. `ray build --templates-only` escribe en disco el módulo
que genera cada plantilla, por si quieres ver en qué se convierte.

En raylang las páginas se construyen con funciones puras: datos de entrada, HTML de salida. Así los
handlers solo leen la petición y los tests prueban cada página sin levantar un servidor.

## 4. Markdown sin riesgos

El cuerpo de una nota se escribe en Markdown y se muestra como HTML:

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

`{{& body_html }}` inserta el HTML sin escapar, así que tiene que ser seguro. Lo es:
`std/markdown` escapa cualquier HTML que el usuario escriba y descarta los enlaces `javascript:`.
Una nota con `<script>` muestra el texto `<script>`, y un test lo comprueba:

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

## 5. Los archivos como estado compartido

Cada conexión corre en su fibra con su propia memoria: dos peticiones nunca comparten una
variable. En este sitio, lo que comparten es el **disco**. Cada nota es un archivo JSON y cada
escritura es atómica:

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

Primero se escribe un temporal y luego se renombra sobre el archivo real. Un renombrado es atómico:
quien lea en ese momento ve la nota vieja o la nueva, nunca una mezcla.

El id de una nota forma parte de una ruta de archivo, así que el almacén solo acepta los ids que él
mismo genera. Una petición a `/notes/../../etc/passwd` nunca llega al disco:

<!-- check: project=examples/apps/notes-ssr -->
```rust
// An id becomes part of a file path, so only ids this store generates are accepted: a request
// for `../../etc/passwd` must not reach the filesystem.
fn valid_id(id: string) -> bool {
    uuid.is_uuid_v7(id)
}
```

Para un sitio con pocos escritores, los archivos bastan y no necesitan ningún servicio. Con muchas
escrituras concurrentes, el siguiente paso es una base de datos, como en el capítulo de la API.

## 6. Formularios y CSRF

Un formulario de otro sitio puede enviar un POST a tu servidor con las cookies del usuario. Es el
ataque CSRF. Los navegadores ponen siempre la cabecera `Origin` en un POST entre sitios, así que
basta con rechazar los que no coinciden con el propio host:

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

## 7. Probar y arrancar

```sh
ray test                       # el almacén y las páginas, sin servidor
ray run                        # http://127.0.0.1:8080
ray dev                        # recarga el navegador al guardar
```

Con el servidor en marcha, `curl` enseña el patrón POST, redirección, GET:

```sh
curl -i -d 'title=Compras&body=**leche**+y+pan' http://127.0.0.1:8080/notes
```

```text
HTTP/1.1 303 See Other
Location: /notes/01a0fa15-c9d5-7201-91f7-b7a2d893f9ca
```

Y lo que el sitio rechaza:

| Petición | Respuesta |
|---|---|
| un formulario con el título vacío | 422, el formulario con el error y lo escrito |
| un `POST` con la cabecera `Origin` de otro sitio | 403 |
| `GET /notes/../../etc/passwd` | 404: el id no es válido y no se toca el disco |
| `GET /assets/app.css` con el `ETag` que ya tiene el navegador | 304, sin cuerpo |

`main` lee el puerto de `PORT` y apaga el servidor con orden ante SIGTERM o Ctrl-C: deja de aceptar
conexiones y espera hasta 5 segundos a que terminen las que están en curso.

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

## 8. Desplegar un solo binario

```sh
ray build --native --release -o notes-ssr
PORT=8080 NOTES_DIR=/var/lib/notes ./notes-ssr
```

El binario lleva dentro las plantillas compiladas y la hoja de estilos (`[native] embed`), así que
funciona desde cualquier carpeta. El de Notes ocupa unos 3 MB. Detrás de un proxy como nginx o
Caddy, que pone el HTTPS, ya está listo para producción. Si prefieres que lo sirva el propio
programa, `listen_tls` recibe el certificado y la clave.

| Variable | Para qué | Por defecto |
|---|---|---|
| `PORT` | el puerto en el que escucha | 8080 |
| `NOTES_DIR` | la carpeta de las notas | `notes-data`, en el directorio actual |

El servidor escucha solo en `127.0.0.1`: el proxy está en la misma máquina y es lo único que debe
alcanzarlo. Como servicio de systemd:

```ini
[Unit]
Description=Notes
After=network.target

[Service]
ExecStart=/usr/local/bin/notes-ssr
Environment=PORT=8080 NOTES_DIR=/var/lib/notes
User=notes
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

`systemctl stop` envía SIGTERM, y el programa termina las peticiones en curso antes de salir.

## Siguiente paso

El mismo tipo de servidor, pero sin páginas: una [**API web**](api.md) que responde JSON, con
Postgres y un pool de conexiones.
