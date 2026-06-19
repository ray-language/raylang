# Control de flujo

Hasta ahora la VM ejecuta una instrucción tras otra, en línea recta. Pero un
programa decide (`if`) y repite (`while`). Para eso, el flujo de ejecución tiene que
poder **saltar**. Aquí la VM empieza a tomar decisiones.

## El instruction pointer y los saltos

La VM lleva un *instruction pointer* (`ip`): el índice de la instrucción actual.
Hasta ahora avanzaba +1 cada paso. Los saltos lo cambian:

```rust
OpCode::Jump(target) => { ip = *target; continue; }           // salta siempre
OpCode::JumpIfFalse(target) => {                              // salta si la cima es false
    if matches!(self.peek(), Value::Bool(false)) {
        ip = *target; continue;
    }
}
```

Por eso el bucle de la VM dejó de ser un `for` y pasó a ser un `while` con `ip`
explícito: los saltos lo manipulan.

## El problema del backpatching

Cuando el compilador emite un salto, todavía **no sabe a dónde irá**. Al compilar un
`if`, emitimos el `JumpIfFalse` *antes* de compilar la rama `else` —así que aún no
conocemos el índice de destino. La solución es el **backpatching**: emitimos el
salto con un destino provisional (`0`), seguimos compilando, y cuando llegamos al
destino real, *parcheamos* la instrucción.

```rust
let to_else = chunk.emit(OpCode::JumpIfFalse(0)); // destino provisional
// ... compilar la rama 'then' ...
patch_jump(chunk, to_else, chunk.code.len());     // ahora sí conocemos el destino
```

## `if` como expresión, en bytecode

Recordemos que en raylang el `if` *produce un valor*. Compilarlo deja exactamente un
valor en la pila (el de la rama tomada). Si desensamblamos `if (3 < 5) { 10 } else
{ 20 }`:

```text
0000    1:14   Constant   0 -> 3
0001    1:18   Constant   1 -> 5
0002    1:14   Less
0003    1:10   JumpIfFalse(7)   ; si 3<5 es falso, salta al else (índice 7)
0004    1:10   Pop              ; descarta la condición (rama then)
0005    1:23   Constant   2 -> 10
0006    1:10   Jump(9)          ; brinca el else
0007    1:10   Pop              ; descarta la condición (rama else)
0008    1:35   Constant   3 -> 20
0009    1:10   Return
```

`JumpIfFalse(7)` salta a la rama `else`; `Jump(9)` se brinca el `else` tras ejecutar
el `then`. Cada rama hace `Pop` de la condición y deja su valor. El resultado: `10`.

## El truco del cortocircuito

Los operadores `&&` y `||` no evalúan su lado derecho si el izquierdo ya decide el
resultado. Esto **cambia la semántica**, no es solo una optimización: `false && (1 /
0 == 0)` debe dar `false` *sin* ejecutar la división por cero.

El truco está en un detalle de `JumpIfFalse`: **ojea la condición sin sacarla** de
la pila. Así, para `a && b`:

```text
[a]
JumpIfFalse end   ; si a es false, lo deja en la pila y salta: 'a' (false) es el resultado
Pop               ; a era true: lo descarta...
[b]               ; ...y el resultado es b
end:
```

Si `a` es `false`, queda en la pila como resultado y nunca se toca `b`. Si es
`true`, se descarta y manda `b`. El `||` usa la misma idea con un salto extra. Que
`JumpIfFalse` ojee en vez de sacar es lo que hace todo esto posible.

## Bloques balanceados

Una invariante sostiene todo: **cada expresión deja exactamente un valor en la
pila**, incluido un bloque. Las sentencias internas de un bloque se ejecutan y su
valor se descarta con `Pop`; el valor final (o `Unit`) es el resultado. Mantener la
pila balanceada es lo que permite componer `if`, bloques y operadores sin que la
pila se desequilibre.

## Lo que sigue

Ya decidimos y nos brincamos código. Falta lo que hace a un programa un programa:
recordar valores en **variables**, **repetir** con `while`, y **llamar** a
funciones. Eso necesita marcos.

> Código: `src/compiler.rs` (emisión de saltos y `patch_jump`), `src/vm.rs` (el `ip`).
