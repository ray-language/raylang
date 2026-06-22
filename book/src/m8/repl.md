# El REPL: un cliente externo

Un **REPL** (*read-eval-print loop*) lee una línea, la evalúa, muestra el resultado y
repite, manteniendo el estado entre líneas:

```text
> let x = 10
10
> fn doble(n: int) -> int { n * 2 }
definida 'doble'
> x.doble()
20
> [1, 2, 3] |> map(doble)
[2, 4, 6]
```

Lo interesante de M8.2 no es el bucle —eso son veinte líneas de leer de la entrada— sino
una decisión de diseño: **cómo mantener el estado entre líneas sin ensuciar el lenguaje.**

## La estrategia: re-ejecutar el preámbulo

Un REPL "de verdad" mantendría vivo un entorno del intérprete entre líneas. Pero el
intérprete de raylang está pensado para programas completos: toma el programa prestado y
ejecuta `main`. Mantener su entorno vivo y hacerlo crecer entre líneas pelearía con esa
forma.

La alternativa, más simple y sorprendentemente efectiva, es **no mantener estado vivo en
absoluto**. El REPL **acumula la fuente** de lo escrito y, en cada entrada, **reconstruye
un programa completo** y lo verifica y ejecuta desde cero:

```text
  <definiciones acumuladas (fn/struct/enum)>
  fn main() { <sentencias acumuladas>  <entrada nueva> }
```

El estado "vive" en el historial de fuente que se re-ejecuta. `let x = 10` no guarda un
valor en ningún sitio: guarda el **texto** `let x = 10;`, que se vuelve a ejecutar en cada
línea siguiente. Redefinir una variable se resuelve solo —la última `let` gana al
re-ejecutar (shadowing)— y una entrada que no compila se **descarta** sin contaminar el
historial. El coste es recomputar el historial en cada entrada; en un REPL pedagógico, es
un precio que ni se nota.

## La decisión de verdad: no tocar el núcleo

La primera versión de este REPL **sí** tocó el núcleo: añadió dos ganchos al front-end —uno
en el checker para que devolviera el tipo de una expresión, otro en el intérprete para
ejecutar una función que no fuera `main`—. Con ellos, el REPL podía mostrar `valor : tipo`.

Funcionaba, pero metía conceptos de REPL dentro del checker y el intérprete. Así que se
**revirtió** a favor de un **cliente 100% externo**: el REPL usa solo la API pública
—`lex`, `parse`, `check`, `run` y el builtin `print`—. El precio es que muestra el
**valor**, no el tipo: obtener el tipo estático exigía esa API del checker que decidimos no
añadir.

> **La lección.** Una herramienta de *tooling* no debería deformar el lenguaje para su
> comodidad. Que el REPL viva enteramente fuera —sobre la misma interfaz que usaría
> cualquier otra herramienta— es una prueba de que el front-end ya expone lo suficiente. La
> versión con ganchos era más bonita en pantalla; la externa es más honesta con la
> arquitectura, y esa fue la que se quedó.

## Cómo se muestra el valor

Si el REPL solo puede usar `print`, ¿cómo enseña el resultado de una expresión? Haciendo
que el `main` sintetizado **imprima** la entrada:

```text
  fn main() { <historial>  print(<entrada>); }
```

Hay un detalle: si la entrada es de tipo `unit` (como `print(x)` o un `while`),
`print(print(x))` no tiparía. El REPL lo detecta —el intento con `print(...)` no verifica— y
**reintenta** ejecutando la entrada como una sentencia normal, sin envolver. Así
`print(x)` ejecuta su propio efecto y no se rompe nada.

## Clasificar la línea

Lo único que el REPL necesita entender es **qué clase de entrada** es, para decidir si la
persiste y cómo mostrarla. Lo resuelve parseándola:

- empieza por `fn`/`struct`/`enum` → una **definición** (se acumula, se confirma "definida
  'f'");
- es un `let`/`var` o una asignación → una **sentencia** (se acumula al historial);
- cualquier otra cosa → una **expresión** (se evalúa y se imprime, no se persiste).

Todo lo demás —UFCS, pipelines, genéricos, inferencia— funciona en el REPL **gratis**,
porque cada línea termina siendo un programa raylang normal que pasa por el pipeline de
siempre.

> Código: `src/repl.rs` (todo el REPL), `src/main.rs` (arrancarlo cuando no hay archivo).
> El checker y el intérprete, intactos. Pruebas: unitarias del estado y de subproceso
> (`tests/repl_cli.rs`, que lanza el binario real).
