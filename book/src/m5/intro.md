# Tipos suma y pattern matching

Hasta M4, los tipos compuestos de raylang eran **productos**: un struct `Punto`
tiene un `x` **y** un `y`, un arreglo tiene sus elementos. M5 añade la otra mitad del
álgebra de tipos: las **sumas**. Un `enum` modela un valor que es **una de** varias
formas —un círculo **o** un rectángulo **o** un punto—, cada una con sus propios
datos. Y con `match`, su forma de consumo: abrir la unión por casos, con la garantía
de no olvidar ninguno.

No es un añadido cualquiera. Los tipos suma + pattern matching son la base sobre la
que M6 construirá `Option<T>` y `Result<T, E>` —el sistema de **errores como
valores** que es el norte del lenguaje (DESIGN §0)—. Un lenguaje sin `null` necesita
una forma de decir "puede que no haya valor", y esa forma es un enum: `Algun(v)` o
`Ninguno`. M5 pone los cimientos; M6, al sumar genéricos, los hace reutilizables.

## La lección central: exhaustividad

Si hay una idea que justifica M5 por sí sola, es esta: **el checker exige que un
`match` cubra todas las variantes.** Olvidar el caso "lista vacía" no es un fallo en
ejecución que descubres tarde —es un error de tipos, antes de correr nada—. Como
raylang es orientado a expresiones y no tiene `null`, un `match` *debe* producir un
valor en todo camino; el compilador lo verifica caminando el conjunto de variantes.
Es el primer análisis de M5 que va más allá de "los tipos cuadran" y razona sobre la
**completitud** de un programa.

## Las tres sub-fases

M5 se construyó en tres pasos, cada uno cerrando una capa del pipeline:

1. **Enums y construcción** (M5.1): declarar `enum`, construir variantes
   (`Figura.Circulo(2.0)`), y representar valores de enum en los dos motores. Todavía
   no se pueden consumir.
2. **`match` y exhaustividad** (M5.2): el pattern matching y su verificación en el
   checker, ejecutado por el intérprete.
3. **`match` en la VM** (M5.3): bajar `match` a bytecode, reuniendo de nuevo a los dos
   motores bajo el oráculo.

## Dos hilos que recorren el capítulo

M5 toca cada fase, pero dos ideas se repiten y conviene anticiparlas:

- **La resolución como parte del front-end.** Construir una variante,
  `Figura.Circulo(2.0)`, es sintácticamente idéntico a acceder a un campo,
  `p.posicion(2.0)`. El parser no puede distinguirlos —no sabe aún qué nombres son
  enums—. Así que el parser emite los nodos genéricos y una **resolución** en el
  checker los reescribe. Es una mini-lección de *name resolution*: la ambigüedad se
  decide una sola vez, y los dos motores reciben un AST sin rastro de ella.
- **El GC ya estaba listo.** Los valores de enum viven en el heap (pueden ser
  recursivos: una lista, un árbol). En la VM eso significa un nuevo tipo de objeto que
  el recolector de M4.3 debe trazar. No hubo que rediseñar el GC: solo enseñarle una
  forma más. Que M4 dejara un trazador bien hecho es lo que hace barato a M5.

Empecemos por hacer que el lenguaje tenga uniones etiquetadas.
