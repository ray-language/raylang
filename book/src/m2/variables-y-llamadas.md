# Variables y llamadas

La última sub-fase de M2 es la más grande: variables locales, el bucle `while`, y
—la pieza central— las **llamadas a función con marcos de pila explícitos**. Al
terminar, `fib` corre entero en la VM.

## Variables locales: slots

Cada variable se asigna a un **slot**: un índice en un arreglo de locales. El
compilador lleva la cuenta de los slots y resuelve cada nombre al suyo. Declarar,
leer y asignar se vuelven tres instrucciones:

```text
let x: int = 5;   →   [compila 5]   SetLocal(slot_de_x)
x                 →   GetLocal(slot_de_x)
x = 7;            →   [compila 7]   SetLocal(slot_de_x)
```

El compilador resuelve los nombres de dentro hacia afuera (igual que el checker), lo
que da el *shadowing*. Al cerrar un ámbito, libera sus slots para reutilizarlos, y
lleva una marca de agua (`max_slots`) que dice cuántos slots necesita el marco.

> **Una decisión de diseño.** En clox, las locales viven en la misma pila de
> operandos. Aquí van en un **arreglo aparte por marco**; la pila de operandos solo
> guarda temporales. Es una simplificación didáctica: separa con claridad ambos
> roles, a cambio de algo de eficiencia. Es un buen ejemplo de cómo, al construir un
> lenguaje, eliges deliberadamente entre claridad y rendimiento.

## `while`: un salto hacia atrás

Con los saltos de la sub-fase anterior, `while` es casi inmediato: evaluamos la
condición, salimos si es falsa, ejecutamos el cuerpo, y **saltamos hacia atrás** a
revaluar la condición.

```text
loop_start:
  [cond]
  JumpIfFalse end
  Pop                ; descarta la condición
  [cuerpo]
  Pop                ; descarta el valor del cuerpo
  Jump loop_start    ; ← salto hacia atrás
end:
  Pop
  Unit               ; el while vale unit
```

Ahora `while` por fin es útil, porque las variables le dan algo que mutar para que
termine.

## Marcos de llamada: la pila explícita

Aquí está el corazón de M2.3, y la decisión de arquitectura que fijamos pensando en
la concurrencia. La VM tiene **dos** pilas:

- La **pila de operandos**, compartida, con los valores temporales.
- Una **pila de marcos** (`frames`) propia —no la pila de llamadas de Rust. Cada
  llamada empuja un `CallFrame`:

```rust
struct CallFrame {
    function: usize,    // qué función ejecuta
    ip: usize,          // su instruction pointer
    locals: Vec<Value>, // sus slots locales
}
```

Que los marcos vivan en *nuestra* estructura, y no en la pila de Rust, es lo que un
día permitiría suspender y reanudar la ejecución (concurrencia). Es estándar en una
VM de bytecode, y aquí lo hacemos explícito.

## Llamar y retornar

El protocolo de una llamada `f(a, b)`:

1. El compilador emite el código de los argumentos (quedan en la pila de operandos)
   y luego un `Call(idx, argc)`.
2. La VM, en `Call`, saca los `argc` argumentos de la pila y los coloca como las
   primeras locales de un marco nuevo (param 0 = primer argumento). Empuja el marco.
3. La función corre en su marco. Al terminar, `Return` saca el valor de retorno,
   **descarta el marco**, y empuja el valor a la pila para el llamador.

```rust
OpCode::Return => {
    let result = self.pop();
    self.frames.pop();
    if self.frames.is_empty() {
        return Ok(result);   // retornó main: fin del programa
    }
    self.push(result);       // entregamos el valor al llamador
}
```

La **recursión** sale gratis de este diseño: cada llamada a `fib` tiene su propio
marco con sus propias locales, así que `fib(n - 1)` y `fib(n - 2)` no se pisan. (Un
contador de marcos detecta la recursión infinita y la convierte en un error en vez
de colgarse.)

El builtin `print` es una instrucción más (`Print`): saca un valor, lo imprime, y
empuja `unit` —porque una llamada, como expresión, siempre deja un valor.

## El oráculo, en su máxima expresión

Ahora podemos compilar y ejecutar **programas completos**. Los tests `oracle_program`
corren el mismo programa en la VM y en el intérprete y exigen resultados idénticos:
`fib`, `gcd`, factorial iterativo, retorno temprano, *shadowing*… todos coinciden.
Esa coincidencia, verificada en cada `cargo test`, es la prueba de que la VM es una
implementación fiel del lenguaje.

## M2 completo

raylang tiene ahora **dos motores de ejecución** que producen el mismo resultado: el
intérprete (simple, la referencia) y la VM de bytecode (el camino hacia el
rendimiento). El CLI deja elegir: `raylang prog.ray` usa el intérprete; `raylang
--vm prog.ray` usa la VM.

## Lo que sigue

La VM está completa para el lenguaje de M1, pero el lenguaje aún es pequeño: solo
tipos primitivos. El siguiente hito, **M3**, le da **datos compuestos** —structs y
arreglos—, lo que a su vez desbloquea cosas que dejamos anotadas, como `args()` para
CLIs y el terreno para `@derive`.

> Código: `src/bytecode.rs`, `src/compiler.rs`, `src/vm.rs`.
