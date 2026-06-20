# Funciones de primera clase

Que una función sea un **valor de primera clase** significa que puede ir a donde va
cualquier valor: una variable, un argumento, un retorno, un campo de struct, un
elemento de arreglo. M4.1 lo habilita sin captura todavía (eso es M4.2).

## El tipo `fn(...) -> R` y la función anónima

Dos formas nuevas en la superficie del lenguaje:

```rust
let cuadrado: fn(int) -> int = fn(n: int) -> int { n * n };
```

- El **tipo** `fn(int) -> int` es la variante `Type::Fn(params, ret)` que dejamos
  anticipada en el diseño desde M1. Es **estructural**: dos `fn(int) -> int` son el
  mismo tipo siempre.
- La **función anónima** `fn(n: int) -> int { ... }` reutiliza la gramática de `fn`,
  sin nombre. No hay ambigüedad con la declaración: la `fn` de nivel superior lleva
  nombre; la de una expresión va seguida de `(`. Por eso `fn` no necesitó una palabra
  clave nueva: solo un uso nuevo.

Y un nombre de función también es un valor: `let g: fn(int) -> int = inc;` toma la
función global `inc` como dato.

> **Las funciones no son comparables ni se imprimen con identidad.** No tienen
> igualdad estructural; el checker rechaza `==` sobre ellas, y `print` las muestra
> como un marcador opaco `<fn>`. Es coherente: una función *es* su comportamiento, no
> un dato que se inspecciona.

## Identificar funciones sin punteros: un índice

Para que un valor-función funcione en los **dos** motores sin meter un *lifetime* ni
un puntero en el tipo `Value`, identificamos cada función por un **índice** en una
tabla: las nombradas ocupan `0..N` (por orden de declaración) y las anónimas `N + id`
(un `id` que el parser asigna a cada literal `fn`). Un `Value::Function(usize)` es,
literalmente, ese índice.

Como las funciones no se comparan ni se imprimen con identidad, el índice **no
necesita coincidir** entre el intérprete y la VM: a cada motor le basta con que sea
consistente consigo mismo. El oráculo nunca lo observa.

## Llamada directa vs. indirecta

Aquí hay una decisión de rendimiento que vale la pena ver. Una llamada `f(x)` puede
ser de dos clases:

- **Directa**: `f` es el nombre de una función global (o un builtin como `print`),
  no tapado por una variable. El compilador emite la llamada estática de siempre
  (`Call(idx, argc)`): rápida, sin construir un valor-función intermedio.
- **Indirecta**: `f` es una variable que *contiene* una función (o cualquier
  expresión que produce una). Hay que evaluar el valor-función y llamarlo a través de
  él. Un opcode nuevo, `CallValue(argc)`, saca el valor-función de la pila y arma el
  marco.

El compilador (y el intérprete) eligen el camino mirando si el nombre es una variable
o una función global —exactamente como lo resuelve el checker. Conservar la vía
directa mantiene barato el caso común (la inmensa mayoría de las llamadas siguen
siendo a funciones conocidas por nombre).

## Una semilla para M4.2

En M4.1 las funciones anónimas **no capturan**: su cuerpo solo ve sus parámetros y las
funciones globales. El checker lo impone verificando el cuerpo en un **ámbito
aislado**, y si intentas usar una variable externa, da un error claro —"captura aún no
soportada"—. Ese aislamiento es justo la costura que M4.2 abrirá para convertir el
error en *upvalues*.

> Código: `src/ast.rs` (`Type::Fn`, `ExprKind::Func`, `collect_fn_exprs`),
> `src/checker.rs` (función-como-valor, llamadas a valores), `src/interpreter.rs` y
> `src/vm.rs` (`Value::Function`, `CallValue`), `src/compiler.rs` (directa vs.
> indirecta).
