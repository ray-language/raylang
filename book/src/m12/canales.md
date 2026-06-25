# Fibras y canales

Empecemos por el corte vertical de M12.1: lo mínimo que demuestra CSP funcionando. Tres
piezas que encajan: lanzar fibras, comunicarlas por canales y un scheduler que las
multiplexa.

## La superficie

Todo se expone como **builtins** (cero gramática nueva, en el espíritu del proyecto):

```rust
fn main() -> int {
    let ch: Channel<int> = channel();        // un canal tipado
    spawn(fn() {                             // lanza una fibra (green thread)
        var i = 1;
        while (i <= 5) { send(ch, i * 10); i = i + 1; }
        close(ch);                           // "no hay más"
    });
    var total = 0;
    var seguir = true;
    while (seguir) {
        match (recv(ch)) {                   // recibe; bloquea si está vacío
            Option.Some(v) => { print(v); total = total + v; },
            Option.None    => { seguir = false; },   // canal cerrado y vacío
        }
    }
    total
}
```

- `spawn(f)` toma una función sin parámetros y la lanza como fibra. (En M12.3 devolverá
  un `Task<T>`; en el slice, su resultado se descarta.)
- `Channel<T>` es un **tipo nuevo**, como `Map<K, V>`. El parser lo trae como
  `Struct("Channel", [T])` y el checker lo reclasifica. `channel()` es **indeterminado**
  (como `[]`, `None` o `map_new()`): su tipo lo fija el contexto (la anotación o el uso).
- `send(ch, v)` encola un valor; `recv(ch) -> Option<T>` saca uno, devolviendo `None` si
  el canal está **cerrado y vacío**. `close(ch)` marca el fin.
- Y gracias a UFCS (M7), todo esto se escribe también `ch.send(v)`, `ch.recv()`,
  `ch.close()`.

`recv` devuelve `Option<T>` siguiendo el mismo patrón que toda la I/O del lenguaje
(M11.2): un primitivo `__recv` que devuelve un `[T]` (vacío o de un elemento) y un
envoltorio en el *prelude* que lo convierte en `Option`. El runtime no sabe de `Option`.

## Una fibra es un par (marcos, pila)

¿Qué es una fibra, mecánicamente? La VM ejecuta con dos campos: una pila de **marcos de
llamada** (`frames`) y una **pila de operandos** (`stack`). Eso es *todo* el estado de
ejecución. Así que una fibra suspendida es, literalmente:

```rust
struct Fiber { frames: Vec<CallFrame>, stack: Vec<HeapValue>, is_main: bool }
```

El heap es **compartido** entre fibras (un solo hilo → sin carreras). El scheduler tiene
una cola FIFO de fibras **listas** (`ready`) y una lista de fibras **bloqueadas**
(`parked`). El programa arranca como la fibra `main`.

`spawn` no cede el turno: solo construye una fibra nueva (un marco para `f`, pila vacía) y
la **encola** en `ready`. El **único punto de yield** es un `recv` sobre un canal vacío y
abierto: la fibra no tiene nada que hacer, así que se guarda en `parked` (anotando qué
canal espera) y el scheduler carga la siguiente de `ready`. Cuando alguien hace `send` a
ese canal, se **despierta** a la fibra (se le deja el valor en su pila y vuelve a
`ready`).

El **fin del programa** es cuando `main` retorna (semántica Go): el código de salida es el
suyo y las fibras pendientes se abandonan. Y si la fibra en curso se bloquea, no queda
ninguna lista y aún hay bloqueadas → **deadlock**: nadie puede despertar a nadie, y la VM
lo reporta como un error de ejecución limpio.

## El canal y el GC

Un canal es un objeto del heap, `Obj::Channel`, con una cola de valores y un *flag* de
cerrado. El GC lo traza como a cualquier otro objeto. Pero la concurrencia obliga a
ampliar las **raíces**: ya no basta con rootear los marcos y la pila *actuales*; hay que
rootear **todas las fibras** —la que corre, las de `ready` y las de `parked`— más el canal
que cada fibra bloqueada espera. Si una fibra dormida es la única que referencia un valor,
ese valor debe sobrevivir. Es la primera vez en el proyecto que el GC es **multi-raíz**.

## Backpressure: canales acotados (M12.2)

El canal del slice es **no acotado**: su cola crece sin límite y `send` nunca bloquea. Eso
es un problema si el productor va más rápido que el consumidor: la cola se infla sin
control. La solución es **acotar** la capacidad y aplicar **backpressure** —frenar al
productor cuando no hay sitio—.

```rust
let ch: Channel<int> = channel(2);   // capacidad 2
let sync: Channel<int> = channel(0); // capacidad 0: rendezvous (síncrono)
```

`channel(n)` crea un canal con capacidad `n`. El tipo de elemento sigue indeterminado (la
capacidad es un valor de runtime, no parte del tipo). Con esto, `send` pasa a ser el
**segundo punto de yield** del modelo. Al enviar:

1. Si hay un **receptor bloqueado** en el canal, se le entrega el valor directo
   (*rendezvous*) y se le despierta.
2. Si no, y la cola **tiene hueco** (no acotada, o `len < cap`), se encola y `send` sigue.
3. Si la cola está **llena**, el emisor se **bloquea** (se aparca con su valor pendiente)
   hasta que un `recv` libere un hueco. Eso es backpressure.

El caso `channel(0)` es especial y elegante: capacidad cero significa que la cola siempre
está vacía, así que un `send` *solo* puede progresar por la vía (1) —un receptor
esperando—. Es un canal **síncrono**: emisor y receptor se citan (*rendezvous*); el `send`
y el `recv` se completan juntos.

Al recibir, ahora `recv` hace lo simétrico: tras sacar un valor y liberar un hueco, si hay
un **emisor bloqueado** en ese canal, mete su valor pendiente en la cola y lo despierta.
Para distinguir las dos clases de fibra dormida, la lista de bloqueadas anota *qué espera*
cada una: recibir (`Recv`) o enviar (`Send(valor)`). El valor que sostiene un emisor
bloqueado es, naturalmente, una **raíz del GC** más.

Una última esquina: cerrar un canal con un **emisor bloqueado** es un error de programa
(alguien todavía esperaba enviar). A diferencia de "hacer panic en otra fibra", esto sí es
detectable de forma determinista **en el sitio del `close`**, así que ahí se reporta.
