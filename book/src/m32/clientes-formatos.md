# Clientes y formatos: PostgreSQL, CSV/TOML y plantillas

M32 cierra el plan post-M26 (DESIGN §36) con tres piezas que no añaden nada al lenguaje: son
**librerías escritas en raylang** que ejercitan lo ya construido. Un cliente de base de datos con
autenticación criptográfica de verdad, dos formatos de configuración y un motor de plantillas HTML.
El hilo que las une es el mismo de siempre: **cero dependencias externas, todo raylang**, y cada una
verificada contra un patrón oro (el RFC, un round-trip, o un servidor de juguete).

## M32.1 — PostgreSQL: coronar la pila cripto

El cliente de Redis (M20.6) fue nuestro primer "cliente cloud" en raylang puro. PostgreSQL es el
siguiente, pero con una diferencia que lo hace mucho más interesante: **autenticación por desafío**.
Un servidor moderno no acepta la contraseña en claro; ejecuta **SCRAM-SHA-256** (RFC 5802/7677), un
baile de mensajes en el que ninguno de los dos lados revela el secreto y ambos se autentican
mutuamente. Y aquí está lo bonito: SCRAM no es un algoritmo nuevo, es un **ensamblaje** de todo lo
que M20 y M30 habían ido construyendo.

### SCRAM: apilar cripto que ya teníamos

La cuenta que hay que calcular es la `ClientProof`, y su receta es una torre:

```
SaltedPassword = PBKDF2-HMAC-SHA256(password, salt, i)
ClientKey      = HMAC(SaltedPassword, "Client Key")
StoredKey      = SHA-256(ClientKey)
ClientSignature= HMAC(StoredKey, AuthMessage)
ClientProof    = ClientKey ⊕ ClientSignature
```

Cada capa es un módulo previo: **HMAC-SHA256** (M20.2), **SHA-256** (M20.1), **base64** (al que solo
hubo que añadirle el `base64_decode` estándar). La única pieza nueva fue **PBKDF2-HMAC-SHA256**, y con
`dkLen = 32` es un solo bloque, casi trivial: `resultado = U1 ⊕ U2 ⊕ … ⊕ Uc`, donde cada `Ui` es el
HMAC del anterior. En raylang cabe en una función:

```raylang
fn pbkdf2_sha256(pw: bytes, salt: [int], iterations: int) -> [int] {
    var u = hmac_sha256(pw, bytes_of(salt_con_INT1));   // U1
    var result = u;
    var c = 1;
    while (c < iterations) {
        u = hmac_sha256(pw, bytes_of(u));               // Ui = HMAC(pw, U_{i-1})
        result = xor_bytes(result, u);
        c = c + 1;
    }
    result
}
```

El cliente son tres funciones: `scram_first` arma el `client-first` (usuario + nonce); `scram_final`
recibe el `server-first` (nonce combinado, salt, iteraciones), calcula la prueba y devuelve el
`client-final`, guardando de paso la firma del servidor **esperada**; y `scram_verify` comprueba que
la `ServerSignature` del `server-final` coincide — eso es la **autenticación mutua**, el cliente
también verifica que el servidor conocía el secreto. La prueba de fuego es el ejemplo completo del
**RFC 7677 §3**: el `client-final` sale byte a byte idéntico y la firma verifica, en los dos motores.

### El protocolo wire, mensaje a mensaje

Con SCRAM resuelto, `pg_query` conduce el protocolo v3. Todo son mensajes con la forma
`[tipo:1][longitud:4][carga]` — salvo el primero, el StartupMessage, que va sin octeto de tipo (una
verruga histórica). El cliente abre el TCP, lo envía, y entra en un bucle que reacciona al octeto de
tipo de cada respuesta: `'R'` (Authentication, que cablea las tres fases SASL con `scram.ray`), `'Z'`
(ReadyForQuery, momento de mandar la `Query`), `'D'` (DataRow, la fila a parsear), `'E'` (error). Es
una **máquina de estados** clásica, con el detalle práctico de que un mensaje puede partirse entre
varias lecturas del socket, así que `pg_read` acumula en un buffer hasta tener el mensaje completo.

Verificarlo de extremo a extremo sin un PostgreSQL real es el truco pedagógico: se escribió un
**servidor de juguete a mano** (solo `std` de Rust, TCP plano) que reproduce el intercambio SASL con
valores **precomputados** — nonce y salt fijos, e `i=64` en vez de los 4096 habituales, para no
ralentizar la suite. Así el test no ejecuta cripto en Rust; solo comprueba que el cliente raylang
autentica, verifica la firma y devuelve la fila. Un cliente de base de datos real, con handshake
criptográfico completo, escrito enteramente en el lenguaje.

## M32.2 — CSV y TOML: formatos como librería pura

Los dos formatos de configuración son **puro cómputo, cero runtime**, y eso tiene una consecuencia
elegante: al no usar operaciones bit a bit ni builtins exóticos, **pasan el oráculo del parser
auto-alojado** (M14). Son raylang tan limpio que el propio raylang los sabe leer.

**CSV** (RFC 4180) parece trivial hasta que aparecen los campos entrecomillados: dentro de `"…"` puede
haber comas, saltos de línea y comillas escapadas como `""`. `parse_csv` devuelve `[[string]]` — filas
de campos como strings. La decisión de dejar *todo* como string, en vez de inferir tipos por columna,
es la idiomática en un lenguaje estáticamente tipado: las columnas son heterogéneas, así que el que
convierta sea quien conoce el esquema. `write_csv` hace el camino inverso, entrecomillando solo donde
hace falta, y el test comprueba el **round-trip**.

**TOML** (un subconjunto) es un parser de cursor sobre los caracteres. El valor es un enum recursivo:

```raylang
pub enum TomlValue { TStr(string), TInt(int), TFloat(float), TBool(bool), TArray([TomlValue]) }
```

y cada entrada es una `clave = valor` donde la clave es una **ruta con puntos** (`server.port`), de
modo que una tabla `[server]` con `port = 8080` se aplana a la clave `"server.port"`. Soporta
comentarios, claves desnudas, strings con escapes, números, booleanos y arrays (incluso multilínea),
con helpers `toml_get`/`toml_show`. Se dejaron fuera, marcadas explícitamente como diferidas, las
tablas en línea `{…}`, los arrays de tablas `[[…]]` y las fechas: es un subconjunto honesto, no un
TOML completo.

## M32.3 — Plantillas HTML: tokenizar, parsear, renderizar

El motor de plantillas cierra la capa web con la arquitectura de un compilador en miniatura, estilo
Jinja/Django: **tokenizador → parser de árbol → render**. El tokenizador parte la plantilla en texto
literal, interpolaciones y etiquetas; el parser construye un árbol de nodos:

```raylang
enum Node { NText(string), NVar(string), NRaw(string),
            NIf(string, [Node], [Node]), NFor(string, string, [Node]) }
```

y el render lo recorre con un contexto de variables tipadas (`enum TVal`). La sintaxis cubre `{{ var }}`
con **autoescape HTML** (`< > & " '` → entidades), la variante cruda `{{& var }}` para cuando de verdad
quieres inyectar marcado, el condicional `{% if %}…{% else %}…{% endif %}` y el bucle
`{% for x in lista %}…{% endfor %}`, que **shadowa** la variable del bucle anteponiéndola al contexto.

El autoescape por defecto no es un adorno: es la decisión de seguridad correcta. Escapar es lo normal y
*no* escapar es lo que hay que pedir explícitamente (`{{&`) — al revés que en tantos motores donde el
descuido abre un XSS. La condición de un `if` es hoy una variable evaluada por truthiness; expresiones,
filtros (`{{ x|upper }}`), `elif` y herencia de plantillas quedaron como diferidos claros.

---

Con esto, **el plan post-M26 está completo** (§36: ergonomía, tooling, cripto avanzada, gRPC,
clientes/formatos). Y cierra con una simetría que da gusto: M30 levantó la pila criptográfica pieza a
pieza, y M32.1 la **coronó** — PostgreSQL/SCRAM la reusa entera, sin añadir un solo builtin. Que la
cripto quedara además limpia se lo debemos a M28.3 (enteros con tamaño), que hizo natural la aritmética
de octetos que antes se enmascaraba a mano. Es la tesis del proyecto llevada hasta el final: un núcleo
pequeño y estable, y todo lo demás —bases de datos, formatos, HTML— construido *encima*, en el propio
lenguaje.
