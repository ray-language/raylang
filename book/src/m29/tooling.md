# Tooling: regex, formateador y optimización

Escribir librerías durante veinte hitos deja una lista de deseos que no son del *lenguaje*, sino del
**entorno de trabajo**. Tres se destacaban tanto que merecían un hito propio: no había expresiones
regulares (la ausencia más llamativa de una stdlib moderna), no había una forma canónica de escribir el
código (cada archivo con su estilo), y la VM seguía dejando rendimiento sobre la mesa. M29 los cierra. Y
el hilo que los une es que **ninguno toca el núcleo del lenguaje**: uno es librería raylang pura, otro es
un cliente externo que reusa el parser, y el tercero es cirugía interna de la VM guiada por la báscula.

## M29.1 — regex, o cómo evitar la explosión exponencial

La tentación al escribir un motor de regex es el **backtracking**: probar una rama, y si falla, retroceder
y probar la siguiente. Es fácil de escribir y es lo que usan Perl, Python o JavaScript. También es lo que
hace que un patrón inocente como `(a*)*b` sobre una cadena de aes tarde *años* — el buscador prueba una
cantidad exponencial de particiones. Ese fallo tiene nombre (ReDoS) y ha tumbado servicios en producción.

La alternativa es más vieja y más elegante: la que Ken Thompson publicó en 1968 y Russ Cox reexpuso como
una **"máquina virtual de regex"**. La idea es tratar el patrón como un pequeño programa de bytecode y
*simular* el autómata no-determinista manteniendo, no un camino con marcha atrás, sino el **conjunto de
todos los hilos vivos a la vez**. Como el conjunto tiene tamaño acotado (a lo sumo un hilo por instrucción)
y avanzamos por el texto un carácter y no volvemos atrás, el coste es **lineal**: `O(texto × patrón)`,
pase lo que pase. Sin blowup, por construcción.

Encajaba perfecto con raylang: es *exactamente* el tipo de programa que el lenguaje ya sabía expresar
—enums recursivos, structs mutables, recursión— así que el motor entero es **librería raylang pura**
(`examples/stdlib/regex.ray`), cero cambios de runtime.

### El patrón como bytecode

El patrón se parsea a un AST (`enum Re { Lit, Any, Concat, Alt, Star, ... }`, recursivo porque los enums
son objetos del heap) y de ahí se **compila** a un programa de instrucciones minúsculas:

```raylang
struct Inst { op: int, x: int, y: int, c: char }
//   Char(c):    casa el carácter c, avanza; si no, el hilo muere.
//   Any:        `.` — casa cualquier carácter.
//   Match:      estado de aceptación.
//   Jmp(x):     salto epsilon (sin consumir entrada).
//   Split(x,y): bifurca en dos hilos, a x y a y.
```

Toda la potencia sale de `Split`: `a*` se compila a un `Split` que bifurca entre "casar una `a` más y
volver" y "seguir de largo"; `a|b` bifurca entre las dos ramas. Los saltos son **epsilon** (no consumen
carácter), y ahí está el detalle fino de la simulación: al sembrar un hilo en una posición, `add_thread`
sigue *recursivamente* todos los `Jmp`/`Split` hasta llegar a instrucciones que sí consumen (`Char`,
`Any`), y usa un vector `seen` para no meter dos veces la misma instrucción — eso es lo que **acota** el
conjunto de hilos y garantiza la linealidad. El bucle principal, por cada carácter del texto, avanza todos
los hilos que casan y descarta los que mueren.

### Clases y anclas: aserciones de ancho cero

M29.1b añade las clases `[a-z]`, `[^0-9]`, los escapes `\d`/`\w`/`\s` (y sus negados) y las anclas `^`/`$`.
Las clases son un opcode más (`Class(x)`, que apunta a una tabla `Prog.classes` de rangos), pero las anclas
enseñan algo distinto: son **aserciones de ancho cero**. No consumen carácter; solo *condicionan* si un
hilo puede seguir. La solución es pasarle la posición a `add_thread`: un opcode `AssertStart` deja continuar
el hilo solo si `pos == 0`, y `AssertEnd` solo si `pos == n`. Como no consumen, se resuelven durante el
mismo cierre epsilon que los saltos.

### Localizar: leftmost-longest y tuplas

Reconocer (`full_match`, `search`) es un booleano; **localizar** es devolver *dónde*. M29.1c corre el
autómata anclado en cada posición y se queda con el match **más largo** desde el punto más a la izquierda
(la semántica clásica leftmost-longest de POSIX). La API resultante —`find -> Option<(int, int)>`,
`find_all`, `replace_all`— se apoya en las **tuplas de M27.1** para devolver el par `(inicio, fin)` sin
inventar un struct. Curiosamente, ese uso de tuplas es lo que **saca a `regex.ray` del corpus del parser
auto-alojado** (que se fijó antes de M27), un recordatorio de que las capas del proyecto tienen edades
distintas. Lo que queda fuera —grupos de captura, `{n,m}`, no-greedy, backreferences— pide una Pike VM que
arrastre listas de posiciones; el núcleo Thompson no captura, y está bien así.

## M29.2 — rayfmt: un formateador sin configuración

`rayfmt` (invocable como `raylang --fmt`) es la respuesta de raylang a `gofmt`: **una** forma canónica de
escribir cada programa, sin opciones que discutir. Como el LSP y el runner de tests, es un **cliente
externo** (`src/fmt.rs`): reusa el lexer y el parser del núcleo, y hace *pretty-print* del AST. No toca
ninguna fase del compilador.

La decisión de imprimir **desde el AST** (y no reescribir el texto original token a token) tiene dos
consecuencias, una buena y una que hay que aceptar. La buena es que la impresión sale **canónica** por
construcción: los paréntesis se emiten según la precedencia real (`bin_prec`/`expr_prec` son el espejo de
la jerarquía del parser), así que `a + b * c` pierde los paréntesis sobrantes y `(a + b) * c` los conserva
— exactamente los mínimos. La que hay que aceptar es que el AST **normaliza**: desazucara lo que el parser
desazucaraba de todos modos — la interpolación `"n = ${x}"` reaparece como `+ to_string(x)`, los pipelines
`|>` como llamadas ordinarias.

Los **comentarios** sí se preservan, aunque el lexer no los guarde: se recolectan aparte (respetando las
cadenas, para no confundir un `//` dentro de un literal) y se re-insertan durante la emisión con un cursor
que los va casando por línea — los doc-comments quedan encima de su ítem, los sueltos entre sentencias, los
de final de línea (*trailing*) pegados a su código, y los de fin de bloque (antes del `}`) acotados con la
posición de cierre que el AST guarda en `Block.end_line`. También se preservan las líneas en blanco entre
sentencias. La invariante: ningún comentario se pierde y cada uno queda en su sitio.

La propiedad que convierte esto en una herramienta de verdad y no en un juguete es la **idempotencia**:
formatear algo ya formateado no lo cambia.

```
fmt(fmt(x)) == fmt(x)
```

Es la garantía que hace seguro poner `rayfmt` en un hook de guardado o en CI: converge en un paso. El test
la verifica sobre catorce ejemplos, y añade una segunda red —**preservación del comportamiento**—: el
programa original y su versión formateada deben producir la misma salida y el mismo código de salida en
ambos motores. Reformatear no puede cambiar lo que el programa hace. Con esas dos propiedades, un
pretty-printer de AST de menos de 600 líneas cubre *todo* el lenguaje: imports, `const`, structs y enums
con sus `@derive`/`pub`/genéricos/bounds, traits e impls, y cada sentencia y expresión.

## M29.3 — optimizar la VM, o la disciplina de revertir

El tercer frente no añade nada: exprime la VM que ya teníamos. La regla, heredada del transversal de
optimización (§27), es **medir siempre y conservar solo lo que supera el ruido**. La herramienta es un
banco (`benchmarks/`) con un `measure.py` sin dependencias que reporta el **mejor de quince** ejecuciones
sobre el binario de release; la baseline de partida era fib(35) en 2.18 s, un bucle en 1.04 s, arrays en
0.196 s. Y el oráculo VM↔intérprete tiene que seguir verde después de cada cambio.

Se probaron dos optimizaciones, y el resultado ilustra por qué la disciplina importa.

La primera, **Opt.9 (dedup de constantes)**, hace que `add_constant` reutilice el índice de una constante
idéntica ya presente en el pool en vez de agregar siempre una nueva. Los literales se repiten muchísimo
—los `0`/`1`/`2`, los nombres de campo, los strings—, así que el pool encoge de forma notable. En
velocidad, sin embargo, es **neutra**: en runtime una constante se lee por índice, y da igual cuántas haya.
Se conservó de todos modos, por una razón simple: es la optimización estándar de todo VM de bytecode,
mejora la calidad y la memoria, y **no tiene contrapartida** (solo borra duplicados).

La segunda, **Opt.10**, es la lección de verdad. La idea era encoger `OpCode` de 32 a 24 bytes boxeando las
dos únicas variantes con un `String` inline (`GetField`/`SetField`) a `Box<str>`, para densificar el stream
de bytecode un 25 % y ganar en caché de instrucciones. Se implementó, se midió... y **no hizo nada** (fib
incluso empeoró un 2 %, el resto plano). La conclusión no es "falló", es *diagnóstica*: estos benchmarks
**no están limitados por el fetch ni por la caché** —los chunks caben de sobra en L1— sino por el trabajo
real (llamadas, aritmética, GC). Y ese diagnóstico se propaga: si encoger `OpCode` no mueve la aguja,
encoger `HeapValue` de 32 a 16 bytes (una cirugía de más de cien sitios) **tampoco pagaría** en estos
casos, así que ni se intentó. Opt.10 se **revirtió**.

Esa es la moraleja del hito: la optimización sin báscula es superstición. Las ganancias fáciles ya estaban
exprimidas en pasadas anteriores (Opt.1/2/4/7); lo que quedaba eran palancas de *tamaño* que la medición
demostró irrelevantes para esta carga. El salto siguiente sería algorítmico —locales en la pila estilo clox,
abaratar el coste de llamada y de GC—, un refactor grande de ROI decreciente. Saber eso, y **no** gastar la
semana en él, es tan valioso como una optimización que sí funciona.

---

Tres herramientas, tres formas de no tocar el núcleo. El regex vive **encima** del lenguaje (librería
pura); el formateador vive **al lado** (cliente que reusa el parser); la optimización vive **dentro** de la
VM, pero guiada desde fuera por la medición. Con M29, raylang deja de ser solo un lenguaje que corre y se
convierte en un lenguaje con **oficio alrededor**.
