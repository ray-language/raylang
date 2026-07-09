# SSR con templates compilados (`examples/web/ssr`)

Un servidor web que hace **server-side rendering** con un **template compilado** (M55): el HTML es un
archivo `vistas/vista_inicio.ray.html` con firma tipada; `ray templ` lo compila a la función raylang
`render_vista_inicio(...)`, y el servidor del paquete `net` la llama por petición. La respuesta usa `webserver.html_response`
(no `text`), que declara `Content-Type: text/html; charset=utf-8` — sin ese header el navegador lee
el UTF-8 como Latin-1 y estropea los acentos.

## Piezas

```
ssr/
├── ray.toml                          # dependencia por ruta a packages/net
├── main.ray                          # el servidor (handler puro: petición → HTML)
└── vistas/
    ├── vista_inicio.ray.html         # el TEMPLATE (la fuente de verdad)
    └── vista_inicio.ray              # GENERADO por `ray templ` (commiteado)
```

El template declara su firma en la primera línea:

```html
{% params titulo: string, saludo: string, popular: bool, lenguajes: [string] %}
```

y usa `{{ expr }}` (autoescape HTML), `{% if %}`/`{% elif %}`/`{% else %}` y `{% for %}`. Un typo en
una variable **no compila** (a diferencia del motor runtime `std/template`, que renderiza `""`).

## Correrlo

```sh
cd examples/web/ssr
ray templ vistas/                     # regenera vista_inicio.ray (solo si tocas el .ray.html)
ray run                               # escucha en el puerto 8080
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
