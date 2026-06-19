# Decisiones fundacionales

Antes de escribir una sola línea de código, conviene decidir el carácter del
lenguaje. No todas las decisiones pesan igual: la mayoría de las features son
**aditivas** (se agregan después sin dolor), pero unas pocas son **estructurales**
—condicionan la forma del compilador o del sistema de tipos— y retrofitearlas
cuesta caro. El arte está en separar unas de otras.

Estas son las ocho decisiones que definen a raylang, y el porqué de cada una.

## 1. Lenguaje anfitrión: Rust

El lenguaje en el que *escribimos* el compilador. Elegimos **Rust** porque sus
`enum` con datos y el `match` exhaustivo son la herramienta perfecta para modelar
árboles de sintaxis: el compilador de Rust te avisa cuando olvidaste manejar un
caso. La fricción del *borrow checker* es, además, justo donde más se aprende sobre
propiedad y memoria.

## 2. Ejecución: intérprete primero, máquina virtual después

Hay varias formas de ejecutar un lenguaje. Empezamos con un **intérprete que
recorre el árbol** (*tree-walking*) porque es simple y correcto, y luego —en M2—
reescribimos el motor como **bytecode + máquina virtual**. La clave pedagógica: el
intérprete se vuelve nuestro **oráculo**. Cuando construyamos la VM, correremos los
mismos programas en ambos y compararemos resultados. Sin esa referencia, depurar
una VM es a ciegas.

## 3. Tipado: estático con anotaciones explícitas

raylang verifica los tipos **antes** de ejecutar: un programa mal tipado nunca
corre. Optamos por **anotaciones explícitas** (`let x: int = 0;`) en vez de
inferencia global. Así escribimos un *type checker* de verdad —resolución de
nombres, reglas de tipos— sin caer en el monstruo de la unificación
Hindley-Milner. La inferencia local llegará más adelante, como mejora.

## 4. Sintaxis: llaves estilo C/Rust

Bloques con `{ }`, sentencias claras. Familiar, fácil de parsear y sin las
ambigüedades de la indentación significativa. Preferimos verboso pero sin
adivinanzas.

## 5. Mutabilidad: `let` inmutable / `var` mutable

Dos palabras clave en vez de una con modificador. Esto da una **regla semántica
real** que el checker debe hacer cumplir —asignar a un `let` es un error— y prepara
el terreno para hablar de inmutabilidad sin costo conceptual alto.

## 6. Orientación a expresiones

La decisión más estructural del lote. En raylang, `if` y los bloques **producen un
valor** (estilo Rust/ML), no son meras sentencias:

```rust
let abs: int = if (x < 0) { -x } else { x };
```

El valor de un bloque es su última expresión sin `;`, y de ahí sale el **retorno
implícito**. Esto compone mucho mejor con un estilo funcional y con los *pipelines*
del futuro, e introduce conceptos preciosos: la distinción sentencia/expresión, el
tipo `unit`, el valor del bloque. Es caro de retrofitear, por eso se decide al
principio.

## 7. Manejo de errores: errores como valores

En vez de excepciones, raylang seguirá el modelo de **errores como valores**
(`Result<T, E>` / `Option<T>` con un operador `?` de propagación, estilo Rust). La
razón es pedagógica: este enfoque *obliga* a construir **tipos suma, genéricos y
pattern matching** —que son las partes más educativas de un sistema de tipos— y
esos cimientos pagan dividendos en todo el lenguaje, no solo en los errores. Como
consecuencia directa, raylang **no tiene `null`**: la ausencia se modela con
`Option`.

> Esta decisión impone una restricción de arquitectura desde el día uno: el tipo
> `Type` del checker se diseña **extensible**, para poder admitir mañana
> `Option<int>` o `Result<int, string>` sin cirugía.

## 8. Métodos y pipelines: UFCS

Como norte de diseño, los métodos de tipos nativos (`s.trim()`), los *pipelines*
(`x |> f`) y los futuros métodos de structs serán **el mismo mecanismo**: azúcar
sobre la llamada a función libre (*Uniform Function Call Syntax*). `s.trim()` será
exactamente `trim(s)`. Esto unifica tres features en una y mantiene el núcleo
pequeño.

---

Con el carácter del lenguaje decidido, podemos empezar a construir la tubería. La
primera parada: convertir texto en *tokens*. El lexer.

> La versión concisa y normativa de estas decisiones vive en `DESIGN.md` §0. Aquí
> contamos el porqué; allí está el contrato.
