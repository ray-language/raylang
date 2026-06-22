# Genéricos, Option y Result

Hasta M5, un tipo en raylang era siempre concreto: un `int`, un `[bool]`, un
`Figura`. Una función que quería "el último elemento de un arreglo" tenía que elegir
un tipo de elemento y comprometerse con él. M6 añade **polimorfismo paramétrico** —los
*genéricos*—: escribir un código una vez y que sirva para cualquier tipo. Y sobre esa
base levanta lo que da nombre al norte del lenguaje (DESIGN §0): el manejo de **errores
como valores**, con `Option<T>`, `Result<T, E>` y el operador `?`.

Es el hito que más cambia el sistema de tipos desde M1 —y, paradójicamente, el que
**menos** toca el runtime—.

## La gran idea: borrado de tipos

raylang implementa los genéricos por **borrado de tipos** (*type erasure*): los
parámetros de tipo viven **solo en el checker**. En tiempo de ejecución no queda
rastro de ellos.

¿Por qué puede permitírselo? Porque los valores de raylang ya son **uniformes**: cada
valor carga su propia etiqueta en runtime (un `Value` en el intérprete, un `HeapValue`
en la VM). Una función genérica `fn identidad<T>(x: T) -> T` no necesita saber qué es
`T`: solo mueve un valor de la entrada a la salida. El intérprete y la VM la ejecutan
**sin enterarse** de que es genérica.

> **La consecuencia, dicha sin rodeos: los genéricos son una característica del type
> checker.** Casi todo M6 ocurre en `checker.rs`. El intérprete y la VM apenas cambian
> —y cuando cambian (el operador `?`), es por el manejo de errores, no por los
> genéricos—. La alternativa, *monomorfizar* (generar una copia del código por cada
> tipo concreto, como Rust o C++), no aportaría nada aquí: con valores uniformes no hay
> rendimiento que ganar, solo maquinaria que mantener.

## La otra cara: inferencia

Si los argumentos de tipo hubiera que escribirlos a mano —`identidad<int>(5)`,
`Option<int>.Some(5)`— los genéricos serían insoportables justo donde más se usan. Así
que el checker los **infiere**: de `identidad(5)` deduce `T = int`; de `Option.Some(5)`,
que es un `Option<int>`. La herramienta es la **unificación**, una pieza clásica de los
sistemas de tipos que aquí aparece en su forma más esencial.

Para los casos que los argumentos no determinan —`Option.None` no dice qué es su `T`—,
el checker mira el **tipo esperado** del contexto: si escribes `let x: Option<int> =
Option.None`, el `int` viene de la anotación. Es el **chequeo bidireccional**, que de
paso arregla una vieja aspereza: el arreglo vacío `[]` por fin sabe qué tipo es.

## Las tres sub-fases

1. **Funciones genéricas e inferencia** (M6.1): parámetros de tipo en funciones, y la
   inferencia desde los argumentos (sustitución + unificación).
2. **Tipos genéricos y chequeo bidireccional** (M6.2): `enum Caja<T>`, `struct
   Par<A, B>`, la inferencia en la construcción, y el tipo esperado.
3. **`Option`/`Result` y `?`** (M6.3): los tipos del manejo de errores —definidos como
   enums genéricos en un *prelude*— y el operador de propagación.

Al terminar, raylang tiene lo que prometió desde el principio: un lenguaje **sin
`null`**, donde "puede fallar" y "puede no haber valor" son tipos que el compilador te
obliga a tratar.
