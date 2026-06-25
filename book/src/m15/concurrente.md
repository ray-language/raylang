# El servidor concurrente: sockets y el scheduler

El capstone de M15 une dos hilos del proyecto que hasta ahora corrían por separado: las **redes**
(M15.2/3) y la **concurrencia** (M12). El objetivo es un **servidor concurrente**: uno que atienda
muchas conexiones *a la vez*, en vez de una tras otra.

## El problema: bloquear el hilo bloquea a todos

Recordemos dos hechos. Primero, los sockets de M15.3 son **bloqueantes**: `tcp_accept` y `socket_read`
detienen el hilo del sistema hasta que llega una conexión o unos datos. Segundo, la concurrencia de
M12 es **M:1**: todas las fibras comparten **un solo hilo**.

Júntalos y aparece el problema. Imagina un servidor que lanza una fibra por conexión:

```raylang
while (seguir) {
    let conn = tcp_accept(srv);
    spawn(fn() { atender(conn) });   // una fibra por conexión
}
```

Si `socket_read` (dentro de `atender`) **bloquea el hilo**, bloquea *todas* las fibras: mientras una
conexión espera datos que no llegan, ninguna otra avanza. El servidor es concurrente sobre el papel y
secuencial en la práctica. Para que la concurrencia sea real, `accept` y `read` tienen que **ceder la
fibra al scheduler** cuando no hay nada listo, igual que `recv` cede cuando un canal está vacío.

## La solución: no bloqueantes + busy-poll cooperativo

Aquí choca de frente la invariante de **cero dependencias**. La forma "de verdad" de esperar a varios
sockets a la vez es `epoll` (Linux) o `kqueue` (BSD/macOS) —y `std` **no los expone**—. Sin un *crate*
externo, no hay notificación de *readiness* del sistema. ¿Qué se puede hacer solo con `std`?

La respuesta honesta es un **busy-poll cooperativo**. La idea, en tres piezas:

1. **Sockets no bloqueantes.** La VM pone sus sockets en modo no bloqueante (`set_nonblocking`). Ahora
   un `read`/`accept` sin datos no espera: devuelve `WouldBlock` al instante.

2. **Aparcar la fibra.** Cuando el opcode `SocketRead`/`TcpAccept` ve `WouldBlock`, **rebobina su
   `ip`** (para reintentar la operación al despertar) y mete la fibra en una lista nueva, `io_parked`.
   Es la gemela de `parked` (las fibras bloqueadas en canales), pero **sin handle de GC**: un socket es
   un `int` del registro del host, no un objeto del heap, así que no hay nada del heap que rootear por
   él. Luego conmuta a otra fibra.

3. **Sondear cuando nadie está listo.** El scheduler, cuando la cola de listas se vacía pero hay fibras
   en `io_parked`, **duerme ~1 ms y las re-encola todas** para que reintenten su operación. Las que
   sigan sin datos vuelven a aparcarse; las que ya tengan datos progresan. Es un bucle de sondeo:
   ineficiente comparado con `epoll`, pero simple, sin dependencias y didáctico.

Nótese la simetría con M12: un socket que no tiene datos es, conceptualmente, lo mismo que un canal
vacío —un punto de yield—. La diferencia es *cómo* se despierta la fibra. A un `recv` lo despierta otra
fibra (haciendo `send`); a un `read` lo despierta el **mundo exterior**, y como no tenemos `epoll` para
que el sistema nos avise, lo descubrimos sondeando.

## Lo que NO cambió

La parte elegante es cuán poco hubo que tocar:

- **Cero opcodes nuevos.** Se reusan `SocketRead` y `TcpAccept`; solo cambia *cómo* los ejecuta la VM
  (no bloqueante + aparcar en vez de bloquear).
- **El intérprete intacto.** La concurrencia es VM-only. El intérprete sigue con sockets **bloqueantes**
  y un solo hilo —que es correcto para él, que ejecuta una fibra y nada más—. Los builtins compartidos
  crean sockets bloqueantes; solo la VM los voltea a no bloqueantes.
- **El GC y la cancelación, extendidos en una línea cada uno.** El recolector rootea las fibras de
  `io_parked` (como ya rooteaba las de `ready` y `parked`); la cancelación de M12.5 también las busca.

Y un nunca-jamás importante: mientras haya fibras en `io_parked`, **nunca hay deadlock** (se sigue
sondeando). El deadlock clásico —todas las fibras esperando en `recv`/`join` sin que nadie pueda
despertarlas— se conserva tal cual, y solo se detecta cuando `io_parked` está vacío.

## Probar la concurrencia sin medir tiempos

¿Cómo se prueba que un servidor es *concurrente* y no solo *correcto*? Medir velocidad sería frágil.
La prueba es de **ordenación**, no de tiempos.

El servidor de prueba atiende dos conexiones, cada una en su fibra. El test conecta dos clientes y
pide el eco del **segundo** *antes* de que el primero envíe nada:

- Un servidor **secuencial bloqueante** se quedaría atascado en el `read` del primer cliente (que no
  envía) y **nunca** atendería al segundo. El segundo cliente esperaría su respuesta para siempre.
- Un servidor **concurrente** atiende al segundo mientras el primero sigue esperando: el segundo
  recibe su eco.

Que el segundo cliente reciba respuesta es, por tanto, una **prueba** de concurrencia —no una
medición—. (Los clientes del test ponen un *read-timeout* para **fallar** en vez de colgarse si la
implementación se rompe.)

## El balance honesto

`socket_write` no es punto de cesión: en un socket no bloqueante hace un bucle que **gira** si el
buffer del sistema está lleno. Para cargas reales —líneas de eco, respuestas HTTP cortas— nunca gira;
una escritura gigante a un peer que no lee sí lo haría. La cesión en la escritura (que exige llevar el
*offset* entre cesiones) queda diferida, igual que `epoll` y un tipo `bytes`. Y el sondeo de 1 ms añade
algo de latencia frente a una notificación de *readiness* real.

Son límites conocidos y anotados, no defectos escondidos. El resultado es lo que importaba: con
`spawn` y un par de cambios quirúrgicos en el scheduler, raylang escribe un servidor que atiende
muchas conexiones concurrentes sobre un solo hilo, sin una sola dependencia externa. La concurrencia
de M12 y las redes de M15 resultaron ser, al final, la misma idea —una fibra que cede y un scheduler
que la despierta— aplicada a dos clases distintas de espera.
