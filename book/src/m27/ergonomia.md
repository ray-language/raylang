# Ergonomía del lenguaje I

Tras el gran arco de librerías (M20–M26: criptografía, DNS, OAuth2, WebSocket, protobuf, HTTP/2…),
escribir tanto raylang de verdad destapó las asperezas del **propio lenguaje**. No eran huecos de
potencia —se podía hacer todo—, sino de comodidad: patrones que se repetían con más ceremonia de la
necesaria. M27 los pule, en cinco pasos que atraviesan todo el pipeline (lexer → parser → checker →
ambos motores → loader → REPL), cada uno con su oráculo VM↔intérprete.

La regla que ordena estos cinco: casi todos son **azúcar de front-end** que se *baja* (desazucara) a
algo que el runtime ya sabía hacer. El intérprete y la VM apenas se enteran.

## Tuplas: matar los `struct XResult`

Devolver dos valores exigía inventar un `struct` de un solo uso (`struct DivResult { q: int, r: int }`).
Las **tuplas** `(a, b)` lo resuelven: `Type::Tuple`, un literal `(1, "hola")`, acceso posicional `t.0` y
desestructuración `let (a, b) = par`. Y aquí está el truco: una tupla es, en runtime, **un arreglo**. El
checker le da un `Type::Tuple` (para chequear la aridad y los tipos posición a posición), pero la baja a
un `ArrayLit`; `t.0` baja a `t[0]`. Erasure total: los motores solo ven arreglos.

El gotcha que aflora: `t.0.1` no se puede escribir, porque el lexer lee `0.1` como un **flotante**. La
salida es un binding intermedio. Y una tupla justo tras un `while {}` se parsea como *llamar* al unit del
while — el mismo compromiso sintáctico que arrastran los literales de struct.

## `for`: iterar sin contadores a mano

`for x in xs`, `for i in 0..n`, `for c in cadena`, `for (k, v) in mapa`. No hay un protocolo `Iterator`
genérico —se decidió **ejecución directa** por forma de iterable—: arreglo (elemento), rango (entero),
string (`char`), `Map` (tupla `(clave, valor)`, con las claves ordenadas para ser determinista). La VM lo
baja a un bucle contado con locales sintéticas (`$idx`, `$len`, `$arr`…).

`for` pasó a ser palabra clave, y eso chocó con `impl X for Y`, donde `for` era un identificador
contextual. Se resolvió consumiendo el nuevo token pero **conservando el mensaje de error antiguo**, para
que el oráculo del parser auto-alojado (que todavía trata `for` como identificador) siguiera cuadrando.

## Interpolación: `f"n = {expr}"`

El prefijo `f"…"` (como `b"…"` de bytes) marca una cadena interpolable; `"…"` normal **no** interpola.
Esta decisión no es cosmética: mucho código —JSON a mano, HPACK— usa `{` como carácter literal, y hacer
que todo `"…"` interpolara los habría roto (lo destapó el micro-framework web). El lexer parte la cadena
en fragmentos (`InterpPart`), el parser re-lexea y re-parsea cada `{expr}` y lo desazucara a
`+ to_string(expr)`; `{{`/`}}` escapan las llaves.

## Casts: `x as int`

`x as int`, `as float`, `c as int` (code point), `n as char`. A diferencia del resto de M27, un cast **no
es erasure**: cambia la representación en runtime (opcode `Cast`), así que cada motor lo ejecuta según el
tipo del valor. `float → int` trunca hacia cero; `int → char` valida el code point. Vive en un nivel de
precedencia propio, entre el unario y la multiplicación.

## `const`: constantes de nivel superior

`const MAX: int = 100;` — un nombre global ligado a un **literal** (los valores computados se difieren).
Cada motor lleva una tabla de valores (`eval_const_literal`, compartida) y una referencia `MAX` se
resuelve como ese valor. El loader las fusiona entre módulos.

---

Cinco features pequeñas, una lección repetida: **la mayoría de la ergonomía se paga en el front-end**.
Tuplas, `for` e interpolación desazucaran a arreglos, bucles y concatenación —cosas que el runtime ya
tenía—; solo el cast, que de verdad cambia bits, bajó al motor. Es el mismo patrón que M7 (UFCS,
pipelines): el lenguaje se siente más rico sin engordar el núcleo.
