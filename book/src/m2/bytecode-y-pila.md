# Bytecode y la pila

La primera sub-fase construye el esqueleto: una representación del bytecode, un
compilador que traduce expresiones, y una **máquina de pila** que las ejecuta.

## El Chunk

El bytecode compilado vive en un `Chunk`: las instrucciones, una tabla de
constantes, y la posición fuente de cada instrucción.

```rust
pub struct Chunk {
    pub code: Vec<OpCode>,
    pub constants: Vec<Value>,
    pub lines: Vec<(usize, usize)>, // posición de cada instrucción
}
```

Esa `lines` es la **line table**: el bytecode pierde el texto fuente, pero
conservamos de dónde vino cada instrucción para poder reportar errores de ejecución
con ubicación —el mismo principio de "errores con posición" de todo el proyecto.

Las instrucciones son un `enum`. Las que llevan operando lo guardan inline:

```rust
pub enum OpCode {
    Constant(usize),  // empuja constants[idx]
    True, False,
    Add, Sub, Mul, Div, Rem, Negate, Not,
    Equal, NotEqual, Less, LessEqual, Greater, GreaterEqual,
    Return,
}
```

## La máquina de pila

El modelo de ejecución es una *stack machine*: las instrucciones sacan sus
operandos de la cima de una pila y empujan sus resultados. No hay registros ni
variables (todavía): todo pasa por la pila. La expresión `1 + 2 * 3` se ejecuta así:

```text
Constant 1      ; pila: [1]
Constant 2      ; pila: [1, 2]
Constant 3      ; pila: [1, 2, 3]
Mul             ; pila: [1, 6]
Add             ; pila: [7]
Return          ; devuelve 7
```

Cada operador binario saca dos valores y empuja uno. Es un modelo asombrosamente
simple para lo potente que resulta.

## El compilador: recorrido en post-orden

El compilador traduce el AST a bytecode con un recorrido en **post-orden**: para un
nodo binario, primero compila sus dos hijos y *luego* emite la operación. Ese orden
es exactamente lo que la pila necesita —cuando se ejecuta el `Add`, sus operandos ya
están en la cima.

```rust
ExprKind::Binary { op, left, right } => {
    emit_expr(chunk, left)?;   // deja el izquierdo en la pila
    emit_expr(chunk, right)?;  // deja el derecho encima
    chunk.emit(opcode_de(op)); // los consume y deja el resultado
}
```

El compilador asume que el AST ya pasó el checker: confía en los tipos y no
re-verifica, igual que el intérprete.

## La VM: un bucle sobre las instrucciones

La VM recorre las instrucciones y, para cada una, manipula la pila:

```rust
OpCode::Constant(idx) => self.push(chunk.constants[*idx].clone()),
OpCode::Add | OpCode::Sub | /* ... */ => {
    let right = self.pop();
    let left = self.pop();
    self.push(apply_binary(op, left, right)?);
}
OpCode::Return => return Ok(self.pop()),
```

`apply_binary` tiene **la misma semántica** que el intérprete de M1 (división entera
que falla en cero, sin mezclas de tipos…). Esa igualdad deliberada es lo que hace
que el oráculo funcione: ambos motores *deben* coincidir.

## El desensamblador

Una VM es opaca si no puedes ver su bytecode. Por eso el `Chunk` sabe
**desensamblarse** a texto legible —índice, posición fuente, y para `Constant`
también el valor:

```text
== 1 + 2 * 3 ==
0000    1:1    Constant   0 -> 1
0001    1:5    Constant   1 -> 2
0002    1:9    Constant   2 -> 3
0003    1:5    Mul
0004    1:1    Add
0005    1:1    Return
```

Es la herramienta que vuelve tangible todo lo demás: cuando algo no cuadra, lo
primero es desensamblar y mirar.

## Lo que sigue

Esta VM ejecuta código en línea recta: una instrucción tras otra. Pero un programa
de verdad **decide** y **repite**. Para eso necesitamos que el flujo pueda saltar:
el control de flujo.

> Código: `src/bytecode.rs`, `src/compiler.rs`, `src/vm.rs`.
