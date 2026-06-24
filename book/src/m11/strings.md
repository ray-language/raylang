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

**Buscar y reemplazar** (M11.4a, aditivo):

- **`contains(s, sub) -> bool`** — ¿`s` contiene la subcadena? Opcode `Contains`.
- **`replace(s, de, a) -> string`** — reemplaza **todas** las ocurrencias de `de` por `a`. Opcode
  `Replace`; asigna un string nuevo (de nuevo, oráculo + estrés del GC).

```rust
let s = "hola mundo, hola raylang";
print(s.contains("mundo"));            // true
print(s.replace("hola", "HOLA"));      // HOLA mundo, HOLA raylang
print("a.b.c".replace(".", "/"));      // a/b/c
```

Estas dos nacieron como *diferidos aditivos* de M11.1 y se saldaron en M11.4. La gracia: tras el
**registro único de builtins**, cada una fue **una fila en la tabla + un opcode + su impl por
motor** —ni el checker ni el compilador cambiaron—.

## El tipo `char` e indexado (M11.4c)

Hasta aquí el string era **opaco por dentro**: se medía y se partía, pero no se podía mirar carácter
a carácter. M11.4c añade el tipo **`char`** —el **primer tipo nuevo del lenguaje desde los enums de
M5**— y con él el indexado.

```rust
let c: char = 'a';            // literal con comillas simples; escapes \n \t \\ \'
print('a' == 'a');            // true (char es comparable con ==)
print(to_string('x') + "!");  // x!

let s = "hola";
print(s[0]);                  // h   — indexar un string da un char
let cs = chars(s);            // [char]  — sus caracteres
print(len(cs));               // 4
```

Detalles de diseño:

- `s[i]` reusa el operador de **indexado** de arreglos; out-of-bounds es un error de runtime, igual
  que en un arreglo. Los strings son **inmutables**: `s[i] = c` es un error de tipos (sí se lee).
- `chars(s) -> [char]` es un builtin (opcode `Chars`) que **asigna en el heap** → estrés del GC.
- Por ser un tipo, `char` tocó más sitios que un builtin (lexer, parser, checker, los dos motores),
  pero todo mecánico. **El oráculo cazó un bug de verdad**: faltaba la rama `char` en la igualdad del
  intérprete, así que `'a' == 'a'` daba `false` solo en un motor —exactamente lo que el oráculo
  existe para atrapar—.

## El resto de la stdlib de string (M11.7a)

M11.7a cierra los aditivos que faltaban, todos por **carácter** (consistentes con `len`/`chars`/
`s[i]`) y siguiendo el patrón de L1 (fila en `BUILTINS` + opcode + impl por motor):

| Builtin | Tipo | Notas |
|---|---|---|
| `starts_with(s, pre)` / `ends_with(s, suf)` | `-> bool` | — |
| `to_upper(s)` / `to_lower(s)` | `-> string` | asignan string nuevo (heap → estrés del GC) |
| `substring(s, i, j)` | `-> string` | `[i, j)` por índice de carácter, con *clamp* (nunca falla) |
| `repeat(s, n)` | `-> string` | `n <= 0` → `""` |
| `index_of(s, sub)` | `-> Option<int>` | primitivo `__index_of -> [int]` + envoltorio en el prelude |
| `join(arr, sep)` | `-> string` | une un `[string]` |

`index_of` reusa el truco de M11.2: el runtime devuelve un `[int]` de 0 o 1 elementos y el prelude
(raylang) lo traduce a `Option<int>` —el runtime sigue sin saber de `Option`—. `substring` **clampa**
los índices al rango válido en vez de fallar, así el oráculo es trivialmente determinista. Los helpers
puros (`char_index_of`, `substring_chars`, `repeat_str`) viven en `builtins.rs`, compartidos por los
dos motores.

> Nota de naming: como raylang **no tiene sobrecarga**, la búsqueda de posición se llama `index_of`
> para string y `position` para arreglos (M11.7b).

> La lección de M11.1 es un cambio de aire. Tras una larga racha de features *front-end* que
> presumían de "runtime intacto", esta nos recuerda por qué esa racha era valiosa: en cuanto algo
> toca de verdad la ejecución, hay que volver a pagar el peaje de mantener **dos motores en
> sincronía**. El oráculo es la red que hace ese peaje barato.
