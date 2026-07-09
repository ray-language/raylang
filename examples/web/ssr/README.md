# SSR con templates compilados (`examples/web/ssr`)

Un servidor web que hace **server-side rendering** con **templates compilados que componen** (M55):
el HTML son archivos `vistas/*.ray.html` con firma tipada; `ray templ` los compila a funciones
raylang, y el servidor del paquete `net` las llama por petición. La respuesta usa `webserver.html_response`
(no `text`), que declara `Content-Type: text/html; charset=utf-8` — sin ese header el navegador lee
el UTF-8 como Latin-1 y estropea los acentos.

## Piezas

```
ssr/
├── ray.toml                          # dependencia por ruta a packages/net
├── main.ray                          # el servidor (handler puro: petición → HTML)
└── vistas/                           # los TEMPLATES (fuente de verdad) + sus .ray GENERADOS
    ├── layout.ray.html               # el layout: envuelve el contenido ({% include contenido %})
    ├── vista_inicio.ray.html         # la vista: {% import %}a e incluye el partial por elemento
    └── tarjeta.ray.html              # el partial: un <li> por lenguaje
```

Cada template declara su firma en la primera línea:

```html
{% params titulo: string, saludo: string, popular: bool, lenguajes: [string] %}
```

y usa `{{ expr }}` (autoescape HTML), `{% if %}`/`{% elif %}`/`{% else %}` y `{% for %}`. Un typo en
una variable **no compila** (a diferencia del motor runtime `std/template`, que renderiza `""`).

**Composición**: la vista hace `{% import vistas/tarjeta %}` y `{% include
tarjeta.render_tarjeta(lang) %}` (empalma HTML ya renderizado, sin re-escapar: cada nivel escapó sus
datos); el layout es un template más con un param `contenido: string`, y `main.ray` compone:
`layout.render_layout(titulo, vista_inicio.render_vista_inicio(…))`.

## Correrlo

```sh
cd examples/web/ssr
ray run                               # escucha en el 8080 (regenera solo los .ray desactualizados)
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
