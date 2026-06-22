# UFCS, pipelines y stdlib

Con M6, raylang ya tenía todo su sistema de tipos: primitivos, arreglos, structs, enums,
genéricos y el modelo de errores. Lo que le faltaba era **ergonomía** y una **biblioteca**
con la que escribir programas reales. M7 añade las dos cosas, y lo hace cumpliendo —por
fin del todo— el norte de diseño de la §0: unificar "método", "pipeline" y "función
libre" en **un solo mecanismo de llamada**.

## La idea que recorre todo M7: azúcar de front-end

M7 no introduce ningún concepto nuevo de tipos ni de ejecución. Las tres piezas que
añade son **azúcar**: formas de escribir, más cómodas, cosas que el lenguaje ya sabía
hacer. Y todas se resuelven **antes** de llegar al intérprete o a la VM.

- **UFCS** (`s.trim()` ≡ `trim(s)`): una reescritura del **checker**.
- **Pipelines** (`x |> f(a)` ≡ `f(x, a)`): una reescritura del **parser**.
- **stdlib** (`map`, `filter`, `fold`): funciones **escritas en raylang**, inyectadas
  como un prelude.

El resultado es contundente: **M7 entero no añadió un solo opcode a la VM**. Es el hito
más "de superficie" del proyecto, y por eso mismo el que mejor muestra cómo unas capas
bien puestas se dejan extender sin tocar el motor.

> **¿Por qué importa que sea azúcar?** Porque cada reescritura tiene un punto exacto
> donde ocurre, y elegirlo bien es la lección. UFCS necesita **tipos** (¿`s.f` es un
> campo o una función?), así que vive en el checker. El pipeline es **puramente
> sintáctico** (no depende de tipos), así que vive en el parser, antes incluso. Y la
> stdlib no necesita ni una cosa ni otra: es código raylang normal. Tres azúcares, tres
> capas distintas, según lo que cada uno necesita saber.

## Un solo mecanismo de llamada

El objetivo de §0 era que estas tres formas significaran lo mismo:

```rust
fold(filter(map(xs, doble), par), 0, suma)   // anidado: se lee de dentro hacia afuera
xs.map(doble).filter(par).fold(0, suma)       // UFCS: se lee de izquierda a derecha
xs |> map(doble) |> filter(par) |> fold(0, suma)  // pipeline: una tubería
```

Las tres producen **exactamente el mismo AST** —tres llamadas anidadas— y el mismo
resultado. UFCS y pipeline son dos maneras de desanidar la primera, según el gusto: el
punto y la tubería son la misma operación vista de dos formas. Que ambas se reduzcan a
la llamada de toda la vida es lo que las hace baratas y predecibles.

## Las tres sub-fases

1. **UFCS** (M7.1): el `.` como azúcar de llamada. Resolución de método en el checker,
   con la regla *campo del struct primero, luego función libre*.
2. **Pipelines** (M7.2): el operador `|>`, desazucarado en el parser, con el operando
   izquierdo insertado como primer argumento.
3. **stdlib** (M7.3): `map`/`filter`/`fold` escritos en el propio raylang, en el prelude,
   como prueba de que con genéricos y closures ya se puede construir librería.

Al terminar, raylang es un lenguaje **usable**: se pueden escribir transformaciones de
datos legibles, encadenadas, sobre una pequeña biblioteca propia.
