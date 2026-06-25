# Structured concurrency

El `spawn` del slice es "dispara y olvida", estilo Go: la fibra corre suelta, su resultado
se descarta, y si falla, hoy tumba todo el programa. Eso tiene dos problemas conocidos.
Primero, no hay forma de **recoger el resultado** de una tarea. Segundo, las tareas se
**fugan**: nada garantiza que terminen antes de que la función que las lanzó retorne, y un
fallo en una de ellas se pierde o se propaga de forma impredecible.

La **concurrencia estructurada** (Trio, Kotlin) es la respuesta: las tareas tienen un
**dueño** y un **ámbito léxico**, igual que las variables. M12.3 la trae con tres piezas.

## `Task<T>` y join

`spawn` **cambia de firma**: ya no devuelve unit, sino un **handle tipado** a la tarea.

```rust
let a: Task<int> = spawn(fn() -> int { caro(3) });
let b: Task<int> = spawn(fn() -> int { caro(4) });
let total = join(a) + join(b);   // bloquea hasta tener cada valor
```

- `Task<T>` es un tipo nuevo (como `Channel<T>`): el handle al resultado futuro.
- `join(t: Task<T>) -> T` **bloquea** hasta que la tarea termina y devuelve su valor. Si
  la tarea **falló** (hizo panic), `join` **re-lanza** ese fallo.

Es retrocompatible: `spawn(fn() { ... });` como sentencia simplemente descarta el `Task`,
así que el código de M12.1/M12.2 sigue compilando.

## scope: ownership y auto-join

`join` da el valor de retorno, pero no la *estructura*. Esa la da `scope`:

```rust
let total = scope(fn() -> int {
    let a = spawn(fn() -> int { caro(3) });
    let b = spawn(fn() -> int { caro(4) });
    join(a) + join(b)
});   // al salir: TODAS las tareas lanzadas dentro están unidas
```

`scope(body) -> R` corre `body` y, al volver, **une a todas** las tareas que se lanzaron
mientras estuvo activo —espera a que terminen— y solo entonces devuelve el valor del
cuerpo. Ninguna tarea sobrevive a su `scope`: no hay fugas. Y si alguna **falló**, el
`scope` **propaga** ese fallo al unir. La adscripción es **dinámica**: cualquier `spawn`
que ocurra mientras el `scope` está en la pila de la fibra queda adscrito a él (no hace
falta pasar un objeto explícito).

## Cómo se sostiene en el runtime

`Task<T>` es un objeto del heap, `Obj::Task`, con un estado: `Pending`, `Done(valor)` o
`Failed(mensaje)`. La fibra de una tarea **guarda su handle**; al terminar normal escribe
`Done(resultado)`, al fallar escribe `Failed(mensaje)`; en ambos casos **despierta** a
quien la estuviera uniendo. Cada fibra gana, además de `(marcos, pila)`, su `Task` y una
pila de **scopes** activos; la VM las salva y restaura al cambiar de fibra.

`join` y `scope` son nuevos puntos de yield. Cuando bloquean (la tarea sigue `Pending`),
usan el truco de **rebobinar el `ip`** y re-ejecutar el opcode al despertar: la fibra se
aparca, y cuando la tarea se completa se la reencola para que re-ejecute el `join`/cierre
de `scope` y ahora vea el resultado. El `scope` se compila a tres pasos —`ScopeBegin`,
llamar al cuerpo, `ScopeEnd`— que el compilador intercala como hace con `channel`; el
`ScopeEnd` es el que espera a los hijos uno a uno.

## Propagación de fallos: capturar, no abortar

La pieza más delicada es la propagación. Hasta M12.3, cualquier error de ejecución en
cualquier fibra abortaba el programa entero (el error se propagaba hasta el borde de la
VM). Para la concurrencia estructurada hace falta lo contrario: el fallo de una **hija**
no debe tumbar todo, sino **quedar guardado** en su `Task` para que el `join`/`scope` que
la observe lo re-lance.

La solución es un cambio quirúrgico en el bucle de la VM: cada instrucción se ejecuta
dentro de un **cierre** que devuelve `Ok`/`Err`, y el bucle **captura** el error:

- Si lo produjo una fibra **hija** (con marcos activos, no `main`) → se guarda en su
  `Task` como `Failed` y se planifica la siguiente fibra, **sin abortar**.
- Los errores de `main` y los del **scheduler** (un deadlock: frames vacíos porque la
  fibra ya se aparcó) → siguen abortando.

Un `Failed` se re-lanza en el primer `join`/`ScopeEnd` que lo observe, y desde ahí
encadena hacia arriba: si llega a `main`, aborta. La propagación recorre el árbol de
tareas como una excepción recorre la pila de llamadas. El GC, claro, gana más raíces: el
valor de cada `Done`, el handle de tarea de cada fibra que espera un `join`, y los hijos
de cada `scope` activo.

## Lo que queda abierto: cancelación

Hay un límite honesto. La concurrencia estructurada "de verdad" (Trio) **cancela a las
hermanas** cuando una tarea falla: si `a` revienta, `b` se aborta en vez de seguir
corriendo en vano. raylang no tiene aún un primitivo de **cancelación**, así que si el
*cuerpo* de un `scope` hace panic, las tareas en curso quedan **huérfanas** en vez de
cancelarse. Es la siguiente puerta a abrir, y la abordamos justo después de cerrar M12.
