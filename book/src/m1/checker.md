# El checker

El parser garantiza que el programa es *sintácticamente* válido: que los paréntesis
cierran y las palabras están en orden. El **checker** (o *analizador semántico*)
garantiza algo más fuerte: que el programa *tiene sentido*. Que no sumas un `bool`
con un `string`, que no usas variables sin declarar, que `fib` realmente devuelve un
`int`. Es la fase intelectualmente más rica del front-end, y la que de verdad le da
*significado* al árbol.

La promesa del checker es contundente: **un programa que lo pasa no puede fallar por
un error de tipos en tiempo de ejecución.** Esa garantía es la que permite que la
fase siguiente —el intérprete— *confíe* y no tenga que re-verificar nada.

## Dos pasadas

El checker recorre el programa en dos pasadas, y la razón es interesante.

Consideremos `fib`, que se llama a sí misma, y `main`, que llama a `fib` aunque
`fib` esté declarada antes. Si verificáramos los cuerpos en una sola pasada
descendente, al llegar a la llamada recursiva `fib(n - 1)` todavía no habríamos
terminado de procesar `fib`, y no conoceríamos su firma. La solución:

1. **Pre-pasada**: registrar la firma de *todas* las funciones (sus parámetros y su
   tipo de retorno) antes de verificar ningún cuerpo.
2. **Verificación**: ahora sí, recorrer cada cuerpo. Cuando aparezca una llamada,
   la firma ya está disponible —sea recursiva o hacia adelante.

Esta separación es lo que hace posible la recursión y las llamadas en cualquier
orden. Es un patrón que reaparece en casi todos los compiladores.

## La pila de ámbitos

Las variables viven en una **pila de ámbitos** (*scopes*): una pila de tablas
nombre → tipo. Cada bloque empuja un ámbito al entrar y lo retira al salir. Buscar
un nombre recorre la pila **de dentro hacia afuera**:

```rust
fn lookup(&self, name: &str) -> Option<&VarInfo> {
    self.scopes.iter().rev().find_map(|scope| scope.get(name))
}
```

Ese `.rev()` es lo que da el *shadowing* de forma natural: una variable declarada en
un bloque interior tapa a otra del mismo nombre en uno exterior, porque se encuentra
primero. raylang permite, por ejemplo, declarar `let x: int` afuera y `let x: bool`
en un bloque interno sin conflicto.

## Las reglas de tipos

El núcleo del checker calcula el tipo de cada expresión y verifica que las
operaciones son válidas. raylang es deliberadamente **estricto** —sin conversiones
implícitas— porque eso hace los errores de tipo visibles y enseñables:

- **Aritmética**: ambos operandos `int` → `int`, ambos `float` → `float`. Cualquier
  mezcla (`int + float`) es un error.
- **Comparaciones de orden** (`< <= > >=`): números del mismo tipo → `bool`.
- **Igualdad** (`== !=`): ambos del mismo tipo comparable → `bool`.
- **Lógicos** (`&& || !`): operan sobre `bool` → `bool`.
- **Condiciones**: la de un `if`/`while` debe ser `bool`. No hay "truthy": `if (x)`
  con `x: int` es un error.
- **Ramas del `if`**: si se usa como expresión con `else`, ambas ramas deben tener
  el **mismo** tipo (ese es el tipo del `if`). Un `if` sin `else` tiene tipo `unit`.
- **Asignación**: asignar a una variable `let` (inmutable) es un error; el tipo
  asignado debe coincidir.

Cada regla violada produce un `TypeError` con su ubicación:

```
error de tipos en 3:5: el operador '+' requiere ambos operandos int o ambos
float, no bool y int
```

## El detalle fino: análisis de divergencia

Como raylang es orientado a expresiones, el cuerpo de una función `-> int` debe
*producir* un `int` (el retorno implícito). Pero también es válido salir antes con
`return`. Considera:

```rust
fn signo(x: int) -> int {
    if (x < 0) { return -1; } else { return 1; }
}
```

Este cuerpo no tiene expresión final: termina en un `if` cuyas dos ramas hacen
`return`. ¿Cómo acepta el checker que "devuelve int" si su valor de bloque es
`unit`? Con un pequeño **análisis de divergencia**: si *todos los caminos* del
bloque terminan en `return`, el bloque "diverge" y no necesita un valor final.

```rust
fn expr_diverges(expr: &Expr) -> bool {
    match &expr.kind {
        // un if diverge solo si AMBAS ramas divergen (si falta el else, puede caer)
        ExprKind::If { then_branch, else_branch: Some(els), .. } =>
            block_diverges(then_branch) && expr_diverges(els),
        ExprKind::Block(b) => block_diverges(b),
        _ => false,
    }
}
```

Es una aproximación **conservadora y sólida**: si dice "diverge", es seguro que
todos los caminos retornan. Esto es exactamente el tipo de razonamiento sobre flujo
de control que un checker enseña a construir.

## Un guiño a Rust: soltar el préstamo

Verificar una llamada tiene un detalle que el *borrow checker* de Rust nos obliga a
pensar. Consultar la firma de la función toma `self` prestado de forma inmutable,
pero verificar los argumentos necesita `self` prestado *mutable*. La solución es
**clonar la firma** para soltar el primer préstamo antes de seguir:

```rust
let (param_types, ret) = match self.functions.get(&name) {
    Some(sig) => (sig.params.clone(), sig.ret),  // clona y suelta el préstamo
    None => return Err(/* función no declarada */),
};
// ahora podemos volver a tomar self prestado para verificar los argumentos
```

El compilador te obliga a estas cosas, y casi siempre tiene razón: te hace explícito
un conflicto que en otros lenguajes pasaría inadvertido.

## El builtin `print` y la entrada `main`

Dos reglas más del programa completo. `print` es una función **incorporada** que el
checker conoce: acepta un único argumento de un tipo imprimible y devuelve `unit`.
No es palabra clave —el lexer la vio como un identificador—; el checker es quien le
da significado. Y `main` es **obligatoria**: debe existir, sin parámetros, con
retorno `int` o `unit`.

## Lo que sigue

El árbol está validado: sabemos que el programa tiene sentido. Ahora falta lo más
satisfactorio —**ejecutarlo**. Como el checker ya garantizó que todo está bien
tipado, el intérprete podrá confiar y concentrarse solo en *evaluar*.

> El código de esta fase vive en `src/checker.rs`.
