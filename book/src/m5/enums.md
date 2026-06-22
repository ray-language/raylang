# Enums: uniones etiquetadas

Un `enum` declara un tipo cuyos valores son **una de** varias variantes, cada una con
un *payload* posicional opcional:

```rust
enum Figura {
    Circulo(float),
    Rect(float, float),
    Punto,            // variante unit: sin payload
}
```

Un valor de tipo `Figura` es exactamente uno de esos tres casos, y lleva consigo la
**etiqueta** (qué variante es) y sus datos. Es nominal, como un struct: dos enums con
las mismas variantes son tipos distintos si tienen nombres distintos.

## El tipo, pensado para crecer

Desde M1 el tipo `Type` se diseñó como un enum **extensible** (DESIGN §1). M5 añade una
variante, `Type::Enum(String)`, que calca a `Type::Struct(String)`: un nombre, tipado
nominal. No hubo cirugía; era el plan desde el principio.

Pero aparece una sutileza. Cuando el parser ve un identificador en posición de tipo
—`f: Figura`— no sabe si `Figura` es un struct o un enum, así que produce siempre
`Type::Struct("Figura")`. El checker, que sí conoce la tabla de enums, **normaliza**:
reclasifica `Struct`→`Enum` cuando el nombre resulta ser un enum (`resolve_type`). Si
no lo hiciera, comparar el tipo declarado (`Struct("Figura")`) con el tipo de un valor
de enum (`Enum("Figura")`) daría un falso "no coinciden" —dos nombres iguales que el
sistema ve distintos—. Normalizar en los pocos puntos donde un tipo de la anotación
entra al checker mantiene todo coherente.

## Construir una variante, y la ambigüedad que esconde

Se construye una variante nombrándola **cualificada**, con el enum delante:

```rust
let a: Figura = Figura.Circulo(2.0);
let b: Figura = Figura.Punto;       // unit: sin paréntesis
```

Aquí está el problema interesante de M5.1. `Figura.Circulo(2.0)` es, carácter por
carácter, lo mismo que un acceso a campo seguido de una llamada: `objeto.metodo(2.0)`.
El parser **no puede** distinguirlos, porque para saber que `Figura` es un enum
necesitaría una tabla de símbolos que aún no existe (los enums pueden declararse
después de usarse).

La solución es no decidir en el parser. El parser emite los nodos genéricos de
siempre (`Field`/`Call`), y una **resolución** —dentro del checker, una vez registrados
los enums— recorre el AST y **reescribe** los `Field`/`Call` cuya cabeza es un nombre
de enum en un nodo explícito `EnumLit { enum_name, variant, args }`.

> **Por qué reescribir, y no consultar en cada motor.** La alternativa sería que el
> intérprete y la VM, cada uno, miraran "¿la cabeza de este `Field` es un enum?" en
> tiempo de ejecución. Reescribir una sola vez, en el front-end compartido, evita
> duplicar esa regla: ambos motores reciben un AST con `EnumLit` ya explícito. El
> precio es que `check` pasa a tomar `&mut Program` —el front-end ahora *transforma*
> el árbol, no solo lo valida—. Es la misma idea de *desugaring* que usan los
> compiladores de verdad.

La reescritura tiene un detalle fino: hay que detectar la construcción **antes** de
recorrer los hijos. Si no, al bajar al `callee` de la llamada (`Figura.Circulo`), se
reescribiría como una variante *sin* payload antes de que el `Call` que la envuelve
pudiera reclamar sus argumentos.

## Valores de enum en los dos motores

Cada motor representa el valor de enum a su manera, igual que con structs y arreglos:

- **El intérprete** usa `Value::Enum(Rc<EnumInstance>)`: la variante, su payload, y el
  nombre del enum (para imprimir). El `Rc` da semántica de referencia y permite enums
  **recursivos** sin tamaño infinito.
- **La VM** estrena un objeto de heap, `Obj::Enum`, con el *tag* de la variante (su
  índice) y el payload. Y aquí el GC de M4.3 entra en acción: hay que enseñarle a
  **trazar** el payload de un enum como uno de sus hijos. Una forma nueva de objeto,
  nada más; el algoritmo de marca y barrido no cambia.

## Enums recursivos: el porqué del heap

Que el valor viva en el heap no es un capricho: es lo que permite que una variante
**contenga su propio tipo**.

```rust
enum Lista {
    Cons(int, Lista),   // una celda: cabeza + el resto, que es otra Lista
    Nil,                // la lista vacía
}
```

Si `Lista` se guardara *inline*, `Cons` necesitaría espacio para otra `Lista`, que
necesitaría otra... tamaño infinito. Como el valor es nominal y vive tras un `Rc` (o un
handle en la VM), una `Lista` ocupa un tamaño fijo y la recursión vive en el heap.
Listas, árboles, expresiones: la estructura de datos clásica de los lenguajes
funcionales, ya expresable en raylang.

## Lo que un enum todavía no hace

Un enum **no se compara con `==`**. Puede ser recursivo y portar funciones, así que su
igualdad estructural abriría preguntas (ciclos, funciones no comparables) que no valen
la pena ahora; se consume por `match`, no por igualdad. Sí se **imprime**:
`Figura.Circulo(2)`, `Lista.Nil`. Un `@derive(Eq)` futuro podría abrir la comparación
cuando haga falta.

> Código: `src/ast.rs` (`Type::Enum`, `EnumDef`, `ExprKind::EnumLit`), `src/checker.rs`
> (resolución `Field`/`Call`→`EnumLit`, `resolve_type`, `check_enum_lit`),
> `src/interpreter.rs` (`Value::Enum`), `src/gc.rs` y `src/vm.rs` (`Obj::Enum` trazado,
> `MakeEnum`).
