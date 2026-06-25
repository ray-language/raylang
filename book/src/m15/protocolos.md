# Protocolos en raylang: JSON y HTTP

Aquí está el cambio de registro de M15. Hasta ahora, cada capacidad nueva del mundo era un **builtin**
en Rust. M15.4 demuestra la otra mitad de la tesis: que los **protocolos** —JSON, HTTP— se escriben
**en el propio raylang** y se traen con `import`. Cero líneas de Rust, cero builtins, cero cambios de
runtime. Son archivos `.ray` que cualquiera puede leer y modificar.

Es, además, una prueba de fuego del lenguaje: ¿es raylang lo bastante expresivo para escribir su
propia stdlib de protocolos cómodamente? La respuesta —enums recursivos, `Map`, `Result`, manejo de
`char`, módulos— es que sí.

## JSON (M15.4a): un enum recursivo y un descenso recursivo

Un valor JSON es, naturalmente, un tipo suma recursivo:

```raylang
pub enum Json {
    JNull, JBool(bool), JNum(float), JStr(string),
    JArray([Json]),
    JObject(Map<string, Json>),
}
```

La decisión de modelar los objetos con **`Map<string, Json>`** (el `Map` de M13) tiene una
consecuencia agradable: al serializar, las claves salen **ordenadas** (`keys` es determinista), así
que `stringify` produce una salida **canónica** y el *round-trip* `parse → stringify → parse` es
estable. Los números se modelan todos como `float` (un solo caso, simple).

El parser es un **descenso recursivo** clásico, con un `struct P { s, i, n }` que lleva el texto y el
cursor, **mutado por referencia** —la misma técnica que el lexer auto-alojado de M14, apoyada en la
semántica de referencia de los structs (M3)—. Recorre la cadena con `s[i]`, `chars`, comparación de
`char` y `substring`, y convierte los números con `parse_float`. Lo importante: los errores son
**valores** (`Result<Json, string>`), nunca `panic`. Un JSON mal formado devuelve `Result.Err` con un
mensaje, igual que cualquier otra operación falible del lenguaje.

Hay una limitación honesta y documentada: los escapes `\uXXXX` no se soportan, porque convertir un
*code point* a `char` necesitaría un builtin nuevo —y eso rompería la regla de "solo librería"—. Es
el tipo de frontera que aparece cuando te atas a escribir algo en el propio lenguaje, y vale la pena
dejarla a la vista en vez de esconderla.

## HTTP (M15.4b): un protocolo sobre el transporte

El cliente HTTP se escribe **sobre los builtins TCP de M15.2**, también en raylang:

```raylang
pub fn fetch(url) -> Result<Response, string>            // atajo GET
pub fn request(method, url, body) -> Result<Response, string>
pub fn header(r, name) -> Option<string>                 // case-insensitive
pub struct Response { status: int, headers: Map<string, string>, body: string }
```

Hace lo que un cliente HTTP/1.1 mínimo debe hacer: parsea la URL (`http://host[:port]/path`), arma la
petición con `Connection: close`, y **lee hasta EOF** —acumulando `socket_read` hasta `""`—. Ese
truco del `Connection: close` es la forma más simple y correcta de delimitar el cuerpo sin tener que
implementar `Content-Length` ni *chunked encoding*: el servidor cierra al terminar, y el cliente sabe
que terminó cuando la lectura da `""`. La respuesta se parte en cabeceras y cuerpo por el **primer**
`\r\n\r\n` (con `index_of`, no `split` —que partiría también dentro del cuerpo—), y las cabeceras van
a un `Map` con la clave en minúsculas para que el lookup sea *case-insensitive*.

### Un gotcha del lenguaje: `fetch`, no `get`

El atajo para GET se llama **`fetch`**, no `get`, y la razón es instructiva. `get` ya es el accesor de
`Map` en el prelude, y raylang **no tiene sobrecarga de funciones**. Si la librería definiera un
`fn get(url)`, ese `get` taparía al `get` de `Map` *dentro del módulo* —y `header` necesita el `get` de
`Map` para buscar en las cabeceras—. La implementación misma chocó con esto; la salida limpia fue
renombrar el atajo. Es un recordatorio de que las decisiones del núcleo (sin sobrecarga) tienen ecos
hasta en la ergonomía de las librerías.

## Componer librerías

El broche de M15.4 es ver las dos librerías **componerse**, que es la prueba de que el sistema de
módulos (M11) funciona de verdad:

```raylang
from http import fetch, header;
from json import parse, stringify;

// fetch de un endpoint, y parsear su cuerpo JSON:
match (fetch(url)) {
    Result.Ok(resp) => match (parse(resp.body)) {
        Result.Ok(j)  => print(stringify(j)),
        Result.Err(e) => print("json: " + e),
    },
    Result.Err(e) => eprint(e),
}
```

Dos librerías independientes, escritas en raylang, importadas por un tercer archivo, encajando sin
fricción. El test lo verifica contra un servidor HTTP de juguete en Rust que responde con un cuerpo
JSON: el `.ray` hace `fetch`, parsea el cuerpo con la librería JSON y emite el resultado canónico —y
da idéntico en el intérprete y en la VM—. El runtime no se enteró de nada: para él, todo esto son
funciones, structs, enums y strings corrientes.
