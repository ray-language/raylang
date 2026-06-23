# La stdlib de string

Durante diez hitos los strings de raylang fueron **casi opacos**: se podían escribir como
literales, imprimir con `print` y comparar con `==`, pero nada más. No se podían concatenar, ni
medir, ni transformar. M11.1 salda esa deuda —diferida desde M7.3— con el conjunto mínimo de
operaciones que vuelve los strings útiles.

## Vuelve el runtime (y el oráculo)

Casi todo lo de M7 a M10 fue *front-end*: azúcar, traits, tooling, todo bajo la disciplina del
**erasure** —el intérprete y la VM no se enteraban—. Las operaciones de string rompen esa racha:
son lo primero desde M6.3 (`?`) que **toca los dos motores**, porque operar sobre el `String`
ocurre en *tiempo de ejecución*.

Eso reactiva la disciplina que define el proyecto: el **oráculo**. Cada operación se prueba
ejecutándola en el intérprete y en la VM y exigiendo que coincidan, incluido el **modo estrés del
GC** —relevante porque `split` crea un arreglo nuevo en el heap, y si una raíz faltara, se
liberaría—.

## Cómo se exponen: builtins (y UFCS gratis)

Las operaciones son **builtins**, como `print`/`len`/`push` (DESIGN §16.4): el checker conoce su
firma, el compilador emite un **opcode** por cada una y la VM lo ejecuta.

Y aquí cae un regalo de capas anteriores. Como UFCS (M7.1) reescribe `recv.f(args)` a
`f(recv, args)`, definir una operación como builtin le da **sintaxis de método sin escribir nada
más**: basta añadir el nombre a la lista de invocables.

```rust
len(nombre)           ≡   nombre.len()
to_string(n)          ≡   n.to_string()
split(s, ",")         ≡   s.split(",")
"  x:y  ".trim().split(":")        // y encadena
```

## Las operaciones

**Construir** (M11.1a):

- **Concatenación** con `+`. No es un builtin: se **extiende el operador**. El checker, que antes
  exigía "ambos int o ambos float", ahora también acepta `string + string → string`; el
  intérprete y la VM extienden su `Add` para concatenar. **Sin opcode nuevo** —reusa `Add`—.
- **`len(s)`** devuelve el número de **caracteres** (Unicode scalar values), no de bytes —análogo
  a `len` de un arreglo—. Se extiende el opcode `Len` para aceptar también un string.
- **`to_string(x)`** convierte un primitivo (`int`/`float`/`bool`/`string`) a su texto, el mismo
  que imprimiría `print`. Opcode nuevo `ToString`. Que use la misma representación que `print` es
  lo que hace que el oráculo cuadre: ambos motores comparten el `Display`.

**Descomponer** (M11.1b):

- **`trim(s)`** quita el espacio en blanco de los extremos. Opcode `Trim`.
- **`split(s, sep)`** parte `s` por el separador y devuelve **`[string]`**. Opcode `Split`. Es la
  única que **asigna en el heap** (el arreglo resultante), de ahí la prueba de estrés del GC.

```rust
let campos = "rojo, verde, azul".split(",");   // ["rojo", " verde", " azul"]
print(campos[1].trim());                        // verde
```

## Lo que falta (a propósito)

raylang **no tiene un tipo `char`**, así que no se indexa un string ni se itera carácter a
carácter; se compone con `+` y se descompone con `split`. Tampoco hay `parse_int`/`int_of_string`
todavía —van con la **I/O** de M11.2, donde de verdad hacen falta para leer entrada— ni
`replace`/`contains`/`to_upper`, que son puramente aditivos y llegarán cuando se necesiten. El
conjunto de M11.1 es deliberadamente el mínimo: **concatenar, medir, convertir, recortar, partir**.

> La lección de M11.1 es un cambio de aire. Tras una larga racha de features *front-end* que
> presumían de "runtime intacto", esta nos recuerda por qué esa racha era valiosa: en cuanto algo
> toca de verdad la ejecución, hay que volver a pagar el peaje de mantener **dos motores en
> sincronía**. El oráculo es la red que hace ese peaje barato.
