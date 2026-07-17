# Demo del framework web (`examples/web/framework`)

Proyecto **consumidor** del paquete [`web/framework`](../../../packages/web/) (M93), como lo usaría
un usuario del lenguaje: `ray.toml` declara los paquetes por ruta y `main.ray` importa con
`from web/framework import …`.

```sh
cd examples/web/framework
ray run                                    # escucha en el 8080
```

```sh
curl http://127.0.0.1:8080/                # HTML (r.html)
curl http://127.0.0.1:8080/users/42        # parámetro de ruta + JSON
curl -X POST -d 'eco esto' http://127.0.0.1:8080/echo
curl 'http://127.0.0.1:8080/saluda?nombre=Ada'
curl -i http://127.0.0.1:8080/assets/style.css   # estático con ETag (repite con If-None-Match → 304)
curl -i http://127.0.0.1:8080/teapot       # 418 + header custom
curl -i http://127.0.0.1:8080/antigua      # 302 → /
curl -i http://127.0.0.1:8080/nope         # el 404 personalizado (JSON)
```

Cada petición emite una línea JSON por stdout (`app.log_requests()`, vía `net/log`): método, ruta,
status y duración en ms. Guía completa del framework: [`docs/web-framework.md`](../../../docs/web-framework.md).
