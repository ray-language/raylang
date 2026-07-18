# El framework web de raylang (`web/framework`, M93)

Framework de aplicación estilo **Express**, escrito en raylang puro sobre `net/webserver` (el
servidor HTTP de producción, M56). Este documento es la guía de uso; el diseño y su historia viven
en DESIGN.md (M56 §60, M93 §85).

> **VM o binario nativo** (M93.3): el servidor es concurrente — cada conexión corre en su
> propia fibra (VM: scheduler M12/M15.5; nativo: hilos con heap aislado). El mismo fuente corre
> con `ray run`/`ray dev` y compila con `ray build --native`. El intérprete (`--interp`) da un
> error limpio.

## Arranque rápido

```toml
# ray.toml
[dependencies]
web = "path:../raylang/packages/web"
net = "path:../raylang/packages/net"   # web se apoya en net/webserver y net/log
```

```raylang
from web/framework import new_app, GET, listen, text, App, Ctx, Res;

// La app se construye en una función TOP-LEVEL (patrón builder): la fibra de cada conexión la
// llama UNA vez — el mismo fuente corre en la VM y compila con `ray build --native` (M93.3).
fn build_app() -> App {
    var app = new_app();
    app.GET("/", fn(c: Ctx, r: Res) {
        r.text("hola");
    });
    app
}

fn main() -> int {
    match (listen(build_app, "127.0.0.1", 8080)) {
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
app.ALL("/ping", handler);                 // TODOS los métodos (M93.2b)
app.GET("/files/*path", handler);          // catch-all FINAL: captura el resto, "/" incluidas
app.GET_re("^/v(\\d+)/estado$", handler);  // regex sobre el path entero (ancla tú); capturas
                                           // numeradas: c.param("1"). route_re(m, pat, h) para
                                           // otros métodos.
```

- El patrón casa **por segmentos**; `:name` captura un parámetro (`c.param("name")`, `""` si no
  existe). Gana la **primera** ruta registrada que casa (método + patrón).
- Las rutas **regex** se compilan **una vez al registrar** (`std/regex`, Pike VM de tiempo
  lineal → inmunes a ReDoS, a diferencia del `path-to-regexp` de Express); un patrón malformado
  hace `panic` al arrancar con el error del compilador de regex.
- Método con patrón de OTRA ruta → **`405 Method Not Allowed` + `Allow`** (RFC 9110; Express
  responde 404). El `not_found` custom sigue siendo solo para 404.
- **Sub-aplicaciones** (M93.2b): un "router" ES una `App` que no escucha —
  `app.mount("/api", api)` re-prefija sus rutas y estáticos y envuelve sus handlers con los
  middlewares del grupo (corren solo para sus rutas). Su `not_found`/`after` se ignoran; una
  ruta regex no se puede re-prefijar (panic al montar).
- La query string no forma parte del path: `/users/42?x=1` casa `/users/:id`; léela con
  `c.query("x")` (URL-decodificada, `""` si falta).
- Un **HEAD** enruta como GET; el servidor responde las cabeceras sin el cuerpo (RFC 9110).
- El cuerpo llega con `c.body()` (texto UTF-8; `""` si no es texto) o crudo en `c.req.body`
  (`bytes`). El resto de la petición está en `c.req` (`webserver.Request`: `headers`, `version`…).

### El contexto conecta la stdlib (M93.2c)

```raylang
c.header_of("user-agent")      // cabecera ("" si falta; nombres en minúscula)
c.cookie_of("sid")             // cookie de la petición (net/cookie por debajo)
c.form()                       // cuerpo x-www-form-urlencoded → Map (std/url, decodificado)
c.form_field("user")           // un campo del form ("" si falta)
c.json_body()                  // Result<Json, string> (std/json) — errores como valores;
                               // recórrelo con jsonlib.get_int/get_string/… (importa
                               // `import std/json as jsonlib;`: el leaf `json` choca con json())
```

Convención: los *lookups* devuelven `""` si falta (como `param`/`query`); lo falible de verdad
(parsear JSON) devuelve `Result`.

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

Un middleware devuelve un **`Step`** (M93.2a): `Step.Next` sigue la cadena, `Step.Done` la corta
y se responde lo construido en `r`.

```raylang
fn auth(c: Ctx, r: Res) -> Step {
    if (c.query("token") != "secreto") {
        r.status(401).text("no autorizado");
        return Step.Done;                 // corta: se responde lo dejado en `r`
    }
    c.put_local("user", "ada");           // dato por-petición para el handler y los `after`
    Step.Next
}

app.use_mw(auth);                         // global: antes del enrutado, en orden de registro
app.use_on("/api/", auth);                // global restringido a un prefijo de URL
app.GET("/admin", with_mw([auth], h));    // POR RUTA: envuelve el handler (combinador);
                                          // la cadena corre con los params YA capturados
```

El "después" (cabeceras comunes, página de error propia, logging a medida) no es un envoltorio
estilo `next()` de Express sino una **segunda cadena explícita** que corre tras el enrutado —
siempre, también en 404 o con la cadena pre cortada:

```raylang
app.after(fn(c: Ctx, r: Res) {
    r.header("X-Frame-Options", "DENY");
    if (r.code >= 500) { r.html(pagina_error()); }
});
```

Datos por-petición entre fases: `c.put_local("k", "v")` / `c.local("k")` (`""` si falta) — `Ctx`
viaja por referencia, lo que escribe un middleware lo ven el handler y los `after`.

## Archivos estáticos

```raylang
app.static_files("/assets/", "static");
app.static_files_cached("/assets/", "static", 3600);   // + Cache-Control: public, max-age=3600
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

Una línea **JSON por petición** a stdout vía `net/log`, con `method`, `path`, `status`, `ms`
(duración) y **`trace_id`** (M93.2d: adopta el `traceparent` W3C entrante vía `net/trace`, o
estrena uno — correlación detrás de un gateway gratis). Para logging propio (otros campos,
niveles) usa `net/log` directamente en tus handlers o un middleware.

## CORS y respuestas JSON tipadas (M93.2d)

```raylang
app.cors("*");                 // preflight OPTIONS (204) + Access-Control-Allow-Origin en todo

struct User { id: int, name: string }
impl ToJson for User {
    fn to_json(self) -> string {
        "{\"id\": " + to_string(self.id) + ", \"name\": " + jsonlib.stringify(jsonlib.Json.JStr(self.name)) + "}"
    }
}
app.GET("/yo", fn(c: Ctx, r: Res) { r.json_of(User { id: 7, name: "Ada" }); });
```

`json_of<T: ToJson>` despacha estático por bounds (M9.2) — sin reflexión. Los primitivos ya
traen `impl ToJson`; para el escapado de strings delega en `std/json.stringify`. (Un
`@derive(ToJson)` queda anotado como candidata de compilador en IDEAS.md.) Nota: los mounts
estáticos responden antes de la cadena `after`, así que no llevan las cabeceras CORS (los
assets suelen ser same-origin).

## Despliegue

Una **única** familia de arranque (M93.3), siempre sobre el builder top-level:

```raylang
listen(build_app, "0.0.0.0", 8080);                        // keep-alive + límites por defecto
listen_tls(build_app, "0.0.0.0", 8443, cert_pem, key_pem); // HTTPS (M56.3, rustls)
listen_graceful(build_app, "0.0.0.0", 8080, 5000);         // SIGTERM/SIGINT → drena 5 s, sale 0
listen_limits(build_app, "0.0.0.0", 8080, mis_limits);     // webserver.Limits explícitos
```

Por qué el builder y no una `App` construida: una `App` contiene **closures** (los handlers) y
el modelo de actores de heap aislado no deja que un closure cruce hilos en el backend nativo. El
builder cruza como función plana y la `App` se construye **dentro de la tarea de cada petición**
(`webserver.serve_with`; cada petición corre aislada en su tarea por el panic→500 de M56.5) —
el mismo fuente corre en la VM y compila con `ray build --native`. El registro es barato;
construir la App una vez por conexión (reusarla en el keep-alive) queda diferido (exigiría
`catch_unwind` en nativo o defuncionalización). Consecuencia del modelo: el estado del builder
es **por petición**; el estado compartido va por canales a una fibra que lo posee (ver
`examples/web/ssr/README.md`).

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
| `route/GET/POST/PUT/PATCH/DELETE/ALL(app, patrón, handler)` | rutas (`:x` captura; `*resto` final; `ALL` = cualquier método) |
| `route_re/GET_re(app, regex, handler)` | rutas regex (compiladas al registrar; capturas → `param("1")`) |
| `mount(app, prefijo, sub_app)` | sub-aplicación re-prefijada con sus middlewares |
| `use_mw(app, fn(Ctx, Res) -> Step)` | middleware pre-enrutado (`Step.Done` corta) |
| `use_on(app, prefijo, mw)` | middleware global restringido a un prefijo |
| `with_mw([mws], handler) -> handler` | cadena POR RUTA (combinador; params ya capturados) |
| `after(app, hook)` | corre tras el enrutado, siempre; puede mutar `res` |
| `static_files[_cached](app, prefijo, dir[, max_age])` | estáticos con ETag/304 (+ Cache-Control) |
| `not_found(app, handler)` | 404 personalizado (405+Allow es automático) |
| `log_requests(app)` | JSON por petición con `trace_id` (net/log + net/trace) |
| `cors(app, origin)` | preflight + Allow-Origin en las respuestas enrutadas |
| `param/query/body(ctx, …)` | parámetro de ruta / query / cuerpo texto |
| `header_of/cookie_of/form/form_field/json_body(ctx, …)` | cabecera / cookie / form urlencoded / JSON (`Result`) |
| `put_local/local(ctx, …)` | datos por-petición entre middleware, handler y after |
| `status/header/cookie(res, …) -> Res` | encadenables |
| `text/json/html/redirect(res, …)` | fijan cuerpo y Content-Type / 302 |
| `json_of(res, v: T: ToJson)` | el JSON de un valor tipado (trait `ToJson`) |
| `listen[_tls|_graceful|_limits](build_app, …)` | arranca el servidor desde el builder (VM y nativo) |

Demo completo: `examples/web/framework/` (`ray run` y los curl de su README).
