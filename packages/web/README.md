# `web` — el framework web de aplicación (M93)

Framework estilo **Express** escrito en raylang puro sobre `net/webserver` (el servidor HTTP de
producción, M56). Promovido desde `examples/web/framework.ray`; corre en la **VM y en el binario
nativo** (el servidor cede fibras; el intérprete no las tiene). Guía completa:
[`docs/web-framework.md`](../../docs/web-framework.md).

```raylang
from web/framework import new_app, GET, listen, static_files, log_requests, text, Ctx, Res;

fn main() -> int {
    var app = new_app();
    app.log_requests();                       // JSON por petición (net/log)
    app.static_files("/assets/", "static");   // estáticos con ETag/304 (M56.9)
    app.GET("/hola/:nombre", fn(c: Ctx, r: Res) {
        r.text("hola, " + c.param("nombre"));
    });
    match (app.listen("127.0.0.1", 8080)) {
        Result.Ok(_) => 0,
        Result.Err(e) => { eprint(e); 1 },
    }
}
```

- **Enrutado** por método + patrón con parámetros (`/users/:id` → `c.param("id")`); `GET`/`POST`/
  `PUT`/`PATCH`/`DELETE` + `route(app, método, …)` genérico; HEAD enruta como GET (el servidor
  quita el cuerpo, RFC 9110).
- **Middleware** (`use_mw`): corren antes del enrutado, en orden; devolver `false` corta la cadena.
- **Respuesta encadenable**: `r.status(418).header("X-K", "v").text(…)`; `json`/`html`/`redirect`/
  `cookie`; **404 personalizable** (`not_found`).
- **Estáticos** (`static_files(prefix, dir)`): sobre `static_mount` de M56.9 — mime por extensión,
  saneo de traversal, `ETag` fuerte y `304` de revalidación. Se comprueban antes que las rutas,
  solo GET/HEAD.
- **Logging** (`log_requests`): una línea JSON por petición (`net/log`) con método, ruta, status y
  duración en ms.
- **gzip** (`app.gzip()`, M306): la negociación de `webserver.gzip` para toda respuesta terminada —
  solo si el cliente acepta gzip, no es streaming, no trae `Content-Encoding`, mide ≥ 512 octetos
  y comprimir encoge; añade `Vary: Accept-Encoding`. Cuesta CPU por respuesta (`std/deflate` es
  raylang puro: barato en nativo, medible en la VM).
- **Despliegue**: `listen` (keep-alive + límites por defecto + panic-del-handler→500, herencia de
  `webserver.serve`), `listen_tls(cert, key)` (HTTPS, M56.3), `listen_graceful(drain_ms)` (apagado
  ordenado con SIGTERM/SIGINT, M88.1b) y `listen_limits(webserver.Limits)`.
  ⚠️ El **builder corre por CONEXIÓN**: `listen(build_app, …)` llama a `build_app()` en la fibra de
  cada conexión (y la usa para todas sus peticiones keep-alive). No abras recursos dentro del
  builder —una conexión SQLite, un archivo, un cliente— o tendrás una fuga por conexión (raydevbox:
  200 peticiones = 201 ficheros abiertos). El estado compartido va en una fibra dueña a la que los
  handlers hablan por canal (MANUAL §15, patrón actor), o se abre en el handler y se cierra al salir.

## Instalación

En tu `ray.toml`, por el **índice** (`ray add web` lo escribe por ti; `web` arrastra `net`):

```toml
[dependencies]
web = "^0.4"
```

En el monorepo, por ruta (`web = "path:../raylang/packages/web"` y `net = "path:../raylang/packages/net"`,
porque `web` se apoya en `net/webserver` y `net/log`); la dependencia git directa queda para un
pin sin índice.

Demo completo: [`examples/web/framework/`](../../examples/web/framework/).

## Licencia

[Apache License 2.0](LICENSE) (M281). Copyright 2026 Roberto Ayala.
