# match: consumir por casos

Construir un enum es la mitad fácil. La interesante es **consumirlo**: mirar qué
variante es, sacar su payload, y actuar según el caso. Eso es `match`:

```rust
fn area(f: Figura) -> float {
    match (f) {
        Figura.Circulo(r) => 3.14159 * r * r,
        Figura.Rect(w, h) => w * h,
        Figura.Punto      => 0.0,
    }
}
```

Cada brazo es un **patrón** y un cuerpo. El patrón `Figura.Circulo(r)` casa con esa
variante y, de paso, **liga** su payload a nombres (`r`) que el cuerpo puede usar. Es
deconstrucción y selección en un solo gesto.

## match es una expresión

Fiel a la orientación a expresiones de raylang, `match` **produce un valor**: el del
brazo que casa. Por eso todos los brazos deben converger a un mismo tipo, igual que las
ramas de un `if`. `let a: float = match (f) { ... };` es natural; el `match` es el
valor, no una sentencia que asigna por efecto.

## El escrutinio va entre paréntesis

`match (f) { ... }`, con la expresión examinada entre paréntesis. No es un capricho:
sin ellos, `match figura { ... }` chocaría con el literal de struct `Nombre { ... }`
—el parser no sabría si `{` abre los brazos o los campos de un struct—. raylang ya
resuelve esa clase de ambigüedad parentizando las condiciones de `if` y `while`; el
`match` sigue la misma convención. Una decisión pequeña que mantiene la gramática sin
casos especiales.

## Los patrones, planos

En M5 los patrones son de **un nivel** —suficiente para `Option`/`Result` y para
recorrer listas, y mucho más simples de verificar—. Hay tres formas, y como las
variantes van cualificadas (`Enum.Variante`), el parser las distingue sin ambigüedad:

- **Patrón de variante**: `Figura.Rect(w, h)`. Casa con esa variante; cada
  sub-binding liga una posición del payload a un nombre, o lo descarta con `_`.
- **Comodín** `_`: no liga nada y **cubre todo** lo restante.
- **Binding suelto** `otra`: liga el escrutinio **completo** a ese nombre; también
  cubre todo lo restante.

Los anidados (`Ok(Circulo(r))`) y los literales (`0 => ...`) se dejan para más
adelante: la estructura recursiva del matcher es una capa que M5 no necesita aún.

## Exhaustividad: la verificación que importa

Aquí está el corazón de M5. El checker, al verificar un `match`, lleva la cuenta de
**qué variantes cubren los brazos**. Al final exige una de dos cosas: o están **todas**
las variantes del enum, o hay un catch-all (`_` o un binding) que tapa el resto. Si
falta alguna, error —con el nombre de las que faltan—.

Esto convierte una clase entera de bugs en errores de compilación. Añades una variante
`Triangulo` al enum `Figura`, y de golpe **todos** los `match` que no la contemplan
dejan de compilar, señalándote exactamente dónde falta el caso. El compilador se vuelve
una lista de tareas.

El mismo análisis caza dos errores más:

- **Brazos inalcanzables**: un brazo después de un catch-all nunca se ejecuta —el
  catch-all ya lo tapó—. Es casi seguro un error del programador, así que se rechaza.
- **Variantes repetidas**: dos brazos para la misma variante; el segundo es muerto.

> **Por qué la exhaustividad es obligatoria, no opcional.** En un lenguaje con `null`,
> un `match` que no cubre todo podría "devolver null" en el caso olvidado. raylang no
> tiene esa salida: un `match` es una expresión que **debe** producir un valor del
> tipo correcto en todo camino. La exhaustividad no es una advertencia amable; es lo
> que hace que la expresión tenga sentido.

## En el intérprete

La ejecución es directa: evaluar el escrutinio, y probar los brazos **en orden**. Un
patrón de variante casa si su etiqueta coincide; entonces se ligan sus sub-bindings en
un ámbito nuevo y se evalúa el cuerpo. Un catch-all siempre casa. Como el checker ya
garantizó exhaustividad, algún brazo siempre casa —el "ningún brazo casó" final es
inalcanzable, y queda solo como red de seguridad—.

Las variables que liga un patrón son **inmutables** (como los parámetros) y viven solo
en el cuerpo de su brazo. Reusan la misma maquinaria de ámbitos y celdas que M4: nada
nuevo que inventar.

> Código: `src/ast.rs` (`ExprKind::Match`, `Pattern`/`PatternKind`, `MatchArm`),
> `src/parser.rs` (`match_expr`, `pattern`), `src/checker.rs` (`check_match`,
> `check_pattern`, exhaustividad), `src/interpreter.rs` (`match_pattern` y la
> evaluación de los brazos).
