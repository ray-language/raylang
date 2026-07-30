# SSR con templates compilados (`examples/web/ssr`)

Un servidor web que hace **server-side rendering** con **templates compilados que componen** (M55):
el HTML son archivos `vistas/*.ray.html` con firma tipada; el loader los compila **en memoria** a
funciones raylang al resolver sus imports (M102 — en el proyecto solo viven los templates), y el
servidor del paquete `net` las llama por petición. La respuesta usa `webserver.html_response`
(no `text`), que declara `Content-Type: text/html; charset=utf-8` — sin ese header el navegador lee
el UTF-8 como Latin-1 y estropea los acentos.

## Piezas

```
ssr/
├── ray.toml                          # dependencia por ruta a packages/net
├── main.ray                          # el servidor (handler puro: petición → HTML)
├── static/                           # assets EN DISCO, servidos con static_mount (M56.9)
│   └── app.css                       # el CSS que enlaza el layout (<link href="/assets/app.css">)
└── vistas/                           # los TEMPLATES (la única fuente: se compilan en memoria)
    ├── layout.ray.html               # el layout: la estructura, con {% block cuerpo %} como hueco
    ├── vista_inicio.ray.html         # la vista: {% extends layout %} + su bloque; incluye el partial
    └── tarjeta.ray.html              # el partial: un <li> por lenguaje
```

Los **assets estáticos** van montados bajo el prefijo de URL `/assets/` con
`webserver.static_mount("/assets/", "static", req)` (M56.9). Los dos argumentos tienen roles
distintos — el primero es el **prefijo de la URL** (lo que pide el navegador, el `href` del HTML) y
el segundo la **carpeta en disco** (relativa a donde corre el servidor) — y aquí son a propósito
diferentes para que no se confundan: el prefijo se recorta y el resto se busca bajo la carpeta
(`/assets/app.css` → `static/app.css`). Cada 200 lleva
un `ETag` (tamaño+mtime) y una petición con `If-None-Match` que casa recibe `304 Not Modified` sin
reenviar el archivo — el navegador cachea y revalida gratis.

Cada template declara su firma en la primera línea:

```html
{% params titulo: string, saludo: string, popular: bool, lenguajes: [string] %}
```

y usa `{{ expr }}` (autoescape HTML), `{% if %}`/`{% elif %}`/`{% else %}` y `{% for %}`. Un typo en
una variable **no compila** (a diferencia del motor runtime `std/template`, que renderiza `""`).

**Composición**: la vista **hereda** del layout con `{% extends layout %}` + `{% block cuerpo %}`
(fusión en compilación: la firma es la de la vista, y las variables que el layout usa —`titulo`—
deben estar en sus params, el checker lo exige); e **incluye** el partial por su ruta con
`{% include vistas/tarjeta(lang) %}` — sin imports manuales ni conocer el nombre de la función
generada (empalma HTML ya renderizado, sin re-escapar: cada nivel escapó sus datos). `main.ray`
solo llama `vista_inicio.render_vista_inicio(…)` — la página ya sale completa.

## Correrlo

```sh
cd examples/web/ssr
ray run                               # escucha en el 8080 (los templates se compilan al vuelo)
```

```sh
curl http://127.0.0.1:8080/           # página genérica
curl http://127.0.0.1:8080/lang/rust  # saludo a 'rust' (marcado como popular)
curl http://127.0.0.1:8080/lang/cobol # 'cobol' (nicho)
```

## Nota de diseño: sin estado compartido

raylang usa **actores de heap aislado** — cada conexión corre en su propia fibra, sin estado mutable
compartido. Por eso el handler es una **función pura de la petición** (lo idiomático para SSR): un
"contador de visitas" global no funcionaría (cada fibra vería su propia copia). El estado compartido,
cuando hace falta, va por un canal a una fibra que lo posee.
