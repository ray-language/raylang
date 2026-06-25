# Arreglos, `sort` y el registro de builtins

El capítulo de strings cubrió la mitad de la stdlib que faltaba. La otra mitad son los **arreglos** y
el **orden** (`sort`), y por el camino merece la pena contar una limpieza arquitectónica que hizo
baratos todos estos añadidos: el **registro único de builtins**.

## La limpieza L1: una tabla, no cuatro sitios

Para cuando llegamos a M11, raylang ya tenía ~13 builtins (`print`, `len`, `split`, `args`…). Cada
uno estaba **repetido en cuatro sitios**: dos en el checker (¿es llamable? ¿qué tipo devuelve?), uno
en el intérprete y uno en el compilador. Añadir un builtin era tocar los cuatro, y era fácil que se
desincronizaran.

L1 lo consolidó en **una sola tabla** en Rust, `BUILTINS`: cada fila es el nombre, el opcode y la
**regla de tipado**.

```rust
struct Builtin {
    name: &'static str,
    opcode: OpCode,
    check: fn(&[Type]) -> Result<Type, (Option<usize>, String)>,  // tipo de retorno, o (arg culpable, msg)
}
```

La consultan el checker (`check_named_call`, `name_is_callable`), el compilador (`emit_call`) y el
despacho del intérprete. Las **implementaciones de ejecución** siguen en `eval_builtin` (intérprete) y
el `match` por opcode (VM) —eso es código, no metadatos—.

¿Por qué una tabla en Rust y no `@builtin fn` escrito en raylang? Porque cuatro builtins son **ad-hoc
polimórficos** (`print`/`eprint`/`len`/`to_string`): aceptan tipos que ninguna firma raylang ordinaria
podría expresar. La tabla los acomoda con una `fn` de Rust por regla.

A partir de aquí, **añadir un builtin es una fila más** (+ opcode + impl por motor). Es la frase que se
repite en todo M11; esta es su explicación.

## M11.7b — la stdlib de arreglos

Con ese patrón, los arreglos se ponen al día con los strings:

| Operación | Tipo | Notas |
|---|---|---|
| `a + b` | `[T]` | **concatenación**: extiende la regla de `Add` (como `string + string`) |
| `reverse(a)` | `-> [T]` | arreglo nuevo (heap → estrés del GC) |
| `pop(a)` | `-> Option<T>` | muta `a` quitando el último; primitivo `__pop -> [T]` + envoltorio |
| `contains(a, x)` | `-> bool` | pertenencia por igualdad estructural (`values_equal`) |
| `position(a, x)` | `-> Option<int>` | índice de la 1ª ocurrencia; primitivo `__position -> [int]` |

Dos detalles. La **concatenación** `a + b` reusa el opcode `Add`: en el checker, `Add` ahora admite
dos arreglos del mismo tipo (además de int/float/string); en los motores, dos objetos del heap bajo
`+` son arreglos (los strings van *inline*). Y el **naming sin sobrecarga**: como raylang no permite
dos funciones con el mismo nombre, la búsqueda de posición se llama `index_of` para string y
`position` para arreglos.

`pop` y `position` reusan el truco de M11.2: el primitivo devuelve un `[T]` de 0 o 1 elementos, y un
envoltorio en el prelude (raylang) lo traduce a `Option` —el runtime nunca sabe de `Option`—.

## M11.7d — `sort` y el trait `Ord`

`sort` es el añadido más elegante de M11.7, porque **no toca el runtime en absoluto**: se escribe en
raylang y reusa toda la maquinaria de traits de M9.

Primero, un trait de orden en el prelude, con impls para los primitivos:

```rust
trait Ord { fn menor(self, otro: Self) -> bool; }
impl Ord for int    { fn menor(self, otro: int)    -> bool { self < otro } }
impl Ord for string { fn menor(self, otro: string) -> bool { self < otro } }
// … float, char
```

Y `sort` como una función **genérica acotada**, en raylang puro:

```rust
fn sort<T: Ord>(a: [T]) -> [T] {        // insertion sort
    var out: [T] = [];
    var i = 0;
    while (i < len(a)) {
        let x = a[i];
        push(out, x);
        var j = len(out) - 1;
        while (j > 0 && x.menor(out[j - 1])) { out[j] = out[j - 1]; j = j - 1; }
        out[j] = x;
        i = i + 1;
    }
    out
}
```

El `T: Ord` se baja al **paso de diccionarios** de M9.2: `sort` recibe el método `menor` como un
argumento oculto. Así que `sort` es **front-end puro, cero opcodes nuevos**. Cualquier tipo del
usuario que implemente `Ord` es ordenable —y el oráculo lo prueba—.

Dos habilitadores pequeños hicieron falta: (1) extender los comparadores `< <= > >=` a **string**
(lexicográfico) y **char** (por *code point*) en el checker y los dos motores —el `self < otro` de los
impls los necesita—; y (2) que el prelude pueda **inyectar impls** (no solo funciones y traits), con un
paso idempotente en `check`.

> **Por qué importa.** `sort` es el ejemplo de libro de por qué construimos los traits y los
> diccionarios en M9: una función de stdlib, genérica y acotada, escrita en el propio lenguaje, que
> funciona igual para los primitivos y para los tipos del usuario, **sin tocar el runtime**. Y la
> tabla L1 es por qué todo M11 pudo añadir tanta stdlib sin que el coste se disparara.
