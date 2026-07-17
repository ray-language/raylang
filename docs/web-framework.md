# El framework web de raylang (`web/framework`, M93)

Framework de aplicación estilo **Express**, escrito en raylang puro sobre `net/webserver` (el
servidor HTTP de producción, M56). Este documento es la guía de uso; el diseño y su historia viven
en DESIGN.md (M56 §60, M93 §85).

> **Solo VM** (el runner por defecto): el servidor es concurrente — cada conexión corre en su
> propia fibra sobre el scheduler de M12/M15.5. El intérprete (`--interp`) da un error limpio.

## Arranque rápido

```toml
# ray.toml
[dependencies]
web = "path:../raylang/packages/web"
net = "path:../raylang/packages/net"   # web se apoya en net/webserver y net/log
```

```raylang
from web/framework import new_app, GET, listen, text, Ctx, Res;

fn main() -> int {
    var app = new_app();
    app.GET("/", fn(c: Ctx, r: Res) {
        r.text("hola");
    });
    match (app.listen("127.0.0.1", 8080)) {
        Result.Ok(_) => 0,
        Result.Err(e) => { eprint(e); 1 },
    }
}
```

`app.GET(...)`, `r.text(...)`, `c.param(...)` son funciones libres del paquete invocadas por
**UFCS**; impórtalas con `from web/framework import …` (la lista completa está abajo).

## Enrutado

```raylang
app.GET("/users/:id", fn(c: Ctx, r: Res) {
    r.json("{\"id\": \"" + c.param("id") + "\"}");
});
app.POST("/users", handler);
app.PUT("/users/:id", handler);
app.PATCH("/users/:id", handler);
app.DELETE("/users/:id", handler);
app.route("OPTIONS", "/users", handler);   // cualquier método, en mayúscula
```

- El patrón casa **por segmentos**; `:name` captura un parámetro (`c.param("name")`, `""` si no
  existe). Gana la **primera** ruta registrada que casa (método + patrón).
- La query string no forma parte del path: `/users/42?x=1` casa `/users/:id`; léela con
  `c.query("x")` (URL-decodificada, `""` si falta).
- Un **HEAD** enruta como GET; el servidor responde las cabeceras sin el cuerpo (RFC 9110).
- El cuerpo llega con `c.body()` (texto UTF-8; `""` si no es texto) o crudo en `c.req.body`
  (`bytes`). El resto de la petición está en `c.req` (`webserver.Request`: `headers`, `version`…).

## La respuesta

Los handlers **mutan** `r` (semántica de referencia); los helpers que devuelven `Res` se encadenan:

```raylang
r.text("plano");                          // text/plain
r.json("{\"ok\": true}");                 // application/json (componlo con std/json)
r.html(render_pagina(...));               // text/html (casa con los templates de `ray templ`)
r.status(201).text("creado");             // código de estado
r.header("X-Version", "2").text("hola");  // cabecera extra (gana sobre el Content-Type derivado)
r.cookie("sid=abc; HttpOnly");            // una línea Set-Cookie por llamada
r.redirect("/nueva");                     // 302 + Location (permanente: r.redirect(...); r.status(301);)
```

## Middleware

```raylang
fn auth(c: Ctx, r: Res) -> bool {
    if (c.query("token") != "secreto") {
        r.status(401).text("no autorizado");
        return false;                     // corta la cadena: se responde lo dejado en `r`
    }
    true                                  // sigue (middlewares restantes → enrutado)
}
app.use_mw(auth);
```

Corren **antes del enrutado**, en orden de registro, para todas las rutas (no hay middleware
por-ruta; componlo dentro del handler).

## Archivos estáticos

```raylang
app.static_files("/assets/", "static");
```

Monta el directorio `static/` (relativo al directorio de trabajo del servidor) bajo el prefijo de
URL `/assets/` — los dos nombres son independientes (`/assets/app.css` → `static/app.css`). Sobre
`webserver.static_mount` (M56.9): `Content-Type` por extensión (~26 tipos), saneo de path
traversal (`..` → 404), `index.html` en rutas con `/` final, **`ETag` fuerte** (tamaño+mtime) y
**`304 Not Modified`** cuando el `If-None-Match` casa (el navegador cachea y revalida gratis).
Los mounts se comprueban **antes que las rutas** y solo para GET/HEAD.

## 404 personalizado

```raylang
app.not_found(fn(c: Ctx, r: Res) {
    r.json("{\"error\": \"no existe\", \"path\": \"" + c.path + "\"}");
});
```

El código ya viene puesto a 404 (cámbialo con `r.status(...)` si quieres otro).

## Logging

```raylang
app.log_requests();
```

Una línea **JSON por petición** a stdout vía `net/log`, con `method`, `path`, `status` y `ms`
(duración): `{"ts":"…","level":"INFO","service":"web","msg":"request","method":"GET","path":"/",
"status":200,"ms":0}`. Para logging propio (otros campos, niveles, trace-id) usa `net/log`
directamente en tus handlers o un middleware.

## Despliegue

```raylang
app.listen("0.0.0.0", 8080);                        // keep-alive + límites por defecto
app.listen_tls("0.0.0.0", 8443, "cert.pem", "key.pem");  // HTTPS (M56.3, rustls)
app.listen_graceful("0.0.0.0", 8080, 5000);         // SIGTERM/SIGINT → drena 5 s y sale 0 (M88.1b)
app.listen_limits("0.0.0.0", 8080, mis_limits);     // webserver.Limits explícitos
```

Todas las variantes heredan de `net/webserver`: keep-alive HTTP/1.1, límites de seguridad
(cabeceras/cuerpo/conexiones/timeout de lectura), y un handler que hace `panic` responde 500 sin
tumbar el servidor. Con `ray dev` (M92) tienes watch+restart con drenado y **live-reload del
navegador** (la página se refresca sola al reiniciar; el snippet reintenta hasta que el servidor
vuelve). Con `--port N` además el supervisor retiene el socket (cero conexiones rechazadas al
reiniciar) y la app puede leer el puerto con `env("RAY_LISTEN_ADDR")` (`"host:puerto"`).

Nota de diseño: raylang usa actores de heap aislado — cada conexión ve su propia copia del estado.
Lo idiomático es el **handler puro** (la respuesta como función de la petición); el estado
compartido va por canales a una fibra que lo posee (ver `examples/web/ssr/README.md`).

## Referencia rápida

| Función | Qué hace |
|---|---|
| `new_app() -> App` | crea la aplicación |
| `route/GET/POST/PUT/PATCH/DELETE(app, patrón, handler)` | registra rutas (`:x` captura) |
| `use_mw(app, fn(Ctx, Res) -> bool)` | middleware pre-enrutado (`false` corta) |
| `static_files(app, prefijo_url, dir)` | estáticos con ETag/304 (M56.9) |
| `not_found(app, handler)` | 404 personalizado |
| `log_requests(app)` | JSON por petición (net/log) |
| `param/query/body(ctx, …)` | parámetro de ruta / query / cuerpo texto |
| `status/header/cookie(res, …) -> Res` | encadenables |
| `text/json/html/redirect(res, …)` | fijan cuerpo y Content-Type / 302 |
| `listen[_tls|_graceful|_limits](app, …)` | arranca el servidor (solo VM) |

Demo completo: `examples/web/framework/` (`ray run` y los curl de su README).
