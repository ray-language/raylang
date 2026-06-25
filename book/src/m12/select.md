# select: multiplexar canales

Con `recv` esperas a **un** canal. Pero muchas veces una fibra atiende **varias** fuentes
a la vez: un servidor que escucha peticiones y, además, una señal de apagado; un agregador
que junta resultados de varios *workers*; un `recv` con *timeout* (donde el timeout es solo
otro canal). Para eso está `select`: esperar al **primero** de varios canales que esté
listo. Es el último ladrillo del modelo CSP, y cierra M12.

## La superficie

```rust
let chs: [Channel<int>] = [a, b, c];
let i = select(chs);          // bloquea hasta que alguno esté listo; da su índice
match (recv(chs[i])) {        // y entonces lo recibes
    Option.Some(v) => print(v),
    Option.None    => print(0 - 1),   // ese canal estaba cerrado
}
```

`select(chs: [Channel<T>]) -> int` bloquea hasta que **algún** canal de la lista esté
**listo para recibir** y devuelve el **índice** del primero listo (el de menor índice, de
forma determinista). "Listo" significa: tiene un valor en cola, *o* tiene un emisor
bloqueado (un `recv` lo tomaría), *o* está cerrado (un `recv` daría `None`).

¿Por qué devolver un **índice** y no el valor? Por minimalismo y flexibilidad. Un índice no
necesita tipos ni tuplas nuevas, y deja al llamador decidir qué hacer: recibir, observar el
cierre, o ni siquiera recibir. Y es **seguro** que sea en dos pasos (`select` y luego
`recv`) precisamente por el modelo M:1 cooperativo: entre que `select` devuelve y haces
`recv(chs[i])` **no hay punto de yield**, así que ninguna otra fibra puede colarse y robar
el valor. La disponibilidad que `select` te prometió sigue ahí. (Una variante que devuelva
índice *y* valor en un solo paso, o un `select` para operaciones de `send`, son azúcar que
se puede construir encima.)

## Cómo bloquea y despierta

`select` es un punto de yield más, pero con una diferencia: una fibra normal espera a **un**
canal; un selector espera a **un conjunto**. Al evaluar `select`:

1. Escanea la lista en orden. Si encuentra un canal listo, devuelve su índice.
2. Si ninguno lo está, **bloquea**: se aparca anotando el conjunto de canales que espera
   (en la práctica, el handle del arreglo, que el GC ya rootea), rebobina el `ip` y
   re-ejecutará el `select` al despertar.

¿Quién lo despierta? Cualquier evento que vuelva *listo* a uno de sus canales. Así que las
operaciones que cambian el estado de un canal —encolar un valor en `send`, bloquear un
emisor, cerrar el canal— llaman a un `wake_select_waiters(canal)` que reencola a todo
selector cuyo conjunto contenga ese canal. El selector despierta, re-ejecuta el `select`,
y ahora encuentra algo listo (o, si otra fibra ya consumió el valor, no encuentra nada y se
**vuelve a bloquear** — un despertar espurio se reabsorbe solo).

El determinismo se mantiene: el `select` elige el **menor índice** listo, y los selectores
aparcados se despiertan en orden FIFO. Hay una regla de **prioridad** deliberada: un `send`
entrega antes a un `recv` plano bloqueado que a un `select` (que solo ve el valor cuando ya
está en la cola). Es una política simple y determinista, y queda documentada.

## Una arista: los canales cerrados

Hay un detalle que sorprende y conviene tener presente. Un canal **cerrado** está "listo"
para siempre: un `recv` sobre él devuelve `None` al instante, sin bloquear. Por eso, si
haces `select` sobre una lista **fija** que incluye un canal ya cerrado, `select` lo
elegirá una y otra vez (es el de menor índice y siempre está listo), ahogando a los demás.

El idiom de Go para esto es "poner ese caso a `nil`" (deshabilitarlo); aquí el equivalente
es **quitar el canal cerrado de la lista** que le pasas a `select`. No es un bug del
runtime: es la consecuencia natural de que "cerrado" cuente como "listo" (lo cuenta a
propósito, para que puedas *detectar* el cierre). Mientras tus canales no se cierren a mitad
—o los podes de la lista cuando lo hagan— `select` se comporta como esperas.

Con `select`, **M12 queda completo**: fibras, canales con backpressure, concurrencia
estructurada y multiplexación. Todo sobre un scheduler de un solo hilo, determinista, sin
una sola carrera de datos posible. Lo único que queda en el tintero es la **cancelación**
de tareas hermanas, el siguiente paso.
