# La máquina virtual

Con M1 raylang ya ejecuta programas: el intérprete recorre el AST y lo evalúa. Pero
ese recorrido es lento, porque vuelve a *caminar el árbol* en cada evaluación —cada
suma vuelve a inspeccionar qué clase de nodo es, cada llamada vuelve a recorrer el
cuerpo de la función. M2 ataca eso con una idea clásica: **compilar el árbol una
sola vez a un formato plano y rápido —el bytecode— y ejecutar ese bytecode en una
máquina virtual (VM).**

## Lo que cambia (y lo que no)

Lo más importante de M2 es lo que **no** cambia: **el lenguaje es idéntico.** El
front-end —lexer, parser, checker— se reutiliza intacto. Lo único que reescribimos
es el *backend de ejecución*. raylang pasa a tener dos motores que producen el mismo
resultado:

```
fuente → lexer → parser → checker → ┬─ [interpreter] ──────────── (M1)
                                    └─ [compiler] → [VM] ───────── (M2)
```

Las piezas nuevas:

- **Bytecode** — una lista de instrucciones simples y planas (`Constant`, `Add`,
  `Jump`, `Call`…) más una tabla de constantes.
- **Compilador** — recorre el AST y *emite* esas instrucciones.
- **VM** — un bucle que ejecuta las instrucciones sobre una **pila**.
- **Desensamblador** — para *ver* el bytecode, depurar y aprender.

## El oráculo

¿Cómo sabemos que la VM hace lo correcto? Tenemos una red de seguridad invaluable:
**el intérprete de M1.** Es lento pero simple y confiable, así que lo usamos como
**oráculo**. Los tests compilan y ejecutan programas en la VM *y* en el intérprete,
y exigen que el resultado coincida. Si algún día difieren, sabemos que la VM tiene
un bug —y dónde buscarlo. Tener primero el intérprete y *luego* la VM fue una
decisión deliberada justo por esto.

## Una decisión de representación

Una VM "de verdad" (la de Lua, la de CPython) empaqueta las instrucciones en
**bytes** para densidad de caché —de ahí el nombre *bytecode*. Nosotros usamos un
`enum` por instrucción (`Vec<OpCode>`): es lo idiomático en Rust y mucho más claro
para aprender, a costa de algo de densidad. Empaquetar a bytes sería una
optimización posterior.

## Una decisión de arquitectura: pila explícita

La VM reifica su propio estado: una **pila de operandos** para los valores
temporales y, más adelante, una **pila de marcos de llamada** propia —no la pila de
llamadas de Rust. Esto es lo estándar en una VM de bytecode, y además mantiene
abierta la puerta a la concurrencia (un día podríamos suspender y reanudar la
ejecución, algo imposible si los marcos vivieran en la pila de Rust).

## El plan de M2

Construimos la VM en tres sub-fases, cada una testeable contra el oráculo:

1. **Bytecode y la pila** — constantes, aritmética, comparación, unarios. La VM de
   pila más simple, y el desensamblador.
2. **Control de flujo** — saltos para `if` y el cortocircuito de `&&`/`||`. Aquí
   aparece el *instruction pointer* manipulable y el *backpatching*.
3. **Variables y llamadas** — locales en slots, `while`, y marcos de llamada con
   pila explícita. Al final, `fib` corre entero en la VM.

Empecemos por la pila.
