# Concurrencia: CSP sobre la VM

Hasta aquí raylang ejecuta **una cosa a la vez**: el intérprete recorre el AST, la VM
recorre el bytecode, y el control va de una expresión a la siguiente sin más. M12 añade
**concurrencia**: la capacidad de tener varias tareas *en marcha* y coordinarlas. Es la
última gran problemática de diseño del proyecto, y la que más obliga a elegir un modelo
con cuidado, porque "concurrencia" significa cosas muy distintas según el lenguaje.

## La decisión: CSP, no memoria compartida

El peligro clásico de la concurrencia son las **carreras de datos**: dos hilos tocando la
misma memoria sin orden, con resultados impredecibles. Hay dos grandes familias de
solución. Una es controlar quién puede tocar qué con el sistema de tipos —el *ownership*
de Rust, las regiones—. La otra es **no compartir memoria**: que las tareas se comuniquen
pasándose mensajes por **canales**, y que el canal sea el único punto de paso. Es el
modelo **CSP** (Hoare), el de Go y, en espíritu, el de Erlang: *"no te comuniques
compartiendo memoria; comparte memoria comunicando"*.

raylang elige **CSP**. El ownership contradiría el modelo del lenguaje (GC, mutabilidad
compartida, semántica de referencia): "sería otro lenguaje". CSP, en cambio, encaja: da
el grueso del valor pedagógico —"concurrencia segura"— reutilizando piezas que ya
tenemos (closures, structs, el heap, el GC).

## M:1 cooperativo: concurrencia, no paralelismo

Hay una segunda decisión, igual de definitoria. ¿Las tareas corren **en paralelo** (en
varios núcleos, de verdad a la vez) o **concurrentemente** (intercaladas en un solo
hilo)? El paralelismo real (un modelo *M:N* preemptivo) exigiría un GC seguro para hilos
y valores que se puedan mover entre ellos —y el heap de la VM es de un solo hilo, con
`Rc` en el intérprete—. Así que raylang elige **M:1 cooperativo**: muchas tareas (las
**fibras**, *green threads*) multiplexadas sobre **un solo hilo del sistema**.

Esto tiene una consecuencia preciosa: **con un solo hilo no hay carreras de memoria por
construcción**. Las fibras solo cambian de turno en **puntos de yield** bien definidos
(esperar en un canal). Entre un yield y el siguiente, una fibra corre sin que nadie la
interrumpa. No hacen falta cerrojos, ni regiones, ni un GC concurrente: la seguridad sale
gratis del modelo. Lo que perdemos es el paralelismo (no usamos varios núcleos); lo que
ganamos es un modelo simple, **determinista** y didáctico.

## Determinismo y el oráculo

El scheduler es **determinista**: una cola FIFO de fibras listas, y un único punto de
yield (el `recv`/`send`/`join`/`select` que bloquea). Dado un programa, el orden de
ejecución es **fijo**, y por tanto su salida también. Eso resuelve un problema práctico:
¿cómo se *prueba* código concurrente, que en otros lenguajes es no determinista por
naturaleza? Aquí no: cada programa concurrente tiene una salida exacta, y los tests la
comparan carácter a carácter.

Hay una asimetría deliberada con el resto del proyecto. raylang tiene **dos motores** que
deben coincidir (el intérprete como oráculo, la VM como implementación optimizada), y casi
todo se verifica cruzando ambos. Pero la concurrencia vive **solo en la VM**: la VM tiene
su propia pila de marcos explícita, que es justo lo que hace falta para *guardar* una
fibra a medias y reanudarla luego. El intérprete, que cabalga sobre la pila de llamadas de
Rust, no puede hacer eso sin reescribirse entero. Así que el intérprete **da un error
limpio** si topa con `spawn`/`channel`/`send`/`recv`/`join`/`scope`/`select` ("requiere la
VM; ejecuta con `--vm`") y sigue siendo el oráculo del código **secuencial**. El código
concurrente se corre con `--vm` y, gracias al determinismo, se prueba contra salida
esperada exacta.

## La descomposición de M12

Como traits o self-hosting, la concurrencia se construye por cortes verticales:

- **M12.1 — el slice CSP.** Lo mínimo que demuestra el modelo de punta a punta:
  `spawn` de fibras, canales tipados (`channel`/`send`/`recv`/`close`) y el scheduler
  cooperativo determinista. Un productor y un consumidor comunicándose por un canal.
- **M12.2 — canales acotados / backpressure.** `channel(n)`: si el productor corre más
  rápido que el consumidor, su `send` se **bloquea** hasta que haya sitio, en vez de
  acumular sin límite.
- **M12.3 — structured concurrency.** Tareas con **valor de retorno** (`Task<T>` +
  `join`) y un `scope` que **posee** las tareas lanzadas dentro, las **une** al salir
  (ninguna se fuga) y **propaga** el fallo de una hija.
- **M12.4 — `select`.** Esperar al **primero** de varios canales que esté listo:
  multiplexar fuentes.

El hilo conductor de todo el capítulo: **una fibra es un par `(marcos, pila)`** que la VM
puede guardar y reanudar, el **canal** es un objeto del heap que el GC traza, y cada
construcción nueva (backpressure, join, scope, select) es un **nuevo punto de yield** o
una nueva forma de **despertar** a una fibra dormida. No hay magia: hay una cola de fibras
listas y una lista de fibras bloqueadas, y todo el diseño es decidir *cuándo* una pasa de
una a la otra.
