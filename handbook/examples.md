# Más ejemplos

Español · [English](examples.en.md)

Las apps de este handbook cubren los casos más comunes, cada una explicada paso a paso. La
organización [ray-language](https://github.com/ray-language) publica más apps escritas en raylang,
con su código completo, para otros casos de uso. Son un buen punto de partida cuando lo que quieres
construir no está en el handbook: busca la más parecida, clónala y cámbiala.

| Caso de uso | Apps |
|---|---|
| Escritorio y móvil | [ray808](https://github.com/ray-language/ray808) (caja de ritmos con React para escritorio, iOS y Android), [raydesk](https://github.com/ray-language/raydesk) (gestor de tareas), [raynote](https://github.com/ray-language/raynote) (bloc de notas con menús y diálogos nativos), [rayplay](https://github.com/ray-language/rayplay) (reproductor de audio en segundo plano), [raystage](https://github.com/ray-language/raystage) (tipos de ventana) |
| Servidores y redes | [raygate](https://github.com/ray-language/raygate) (API gateway), [raywatch](https://github.com/ray-language/raywatch) (monitor de servicios con panel SSE), [raystream](https://github.com/ray-language/raystream) (servidor de medios), [rayq](https://github.com/ray-language/rayq) (cola de mensajes con WAL), [raykv](https://github.com/ray-language/raykv) (servidor compatible con Redis), [raybot](https://github.com/ray-language/raybot) (bot por WebSocket), [raymail](https://github.com/ray-language/raymail) (cliente SMTP), [rayrelay](https://github.com/ray-language/rayrelay) (relay) |
| Sistemas distribuidos | [raymart](https://github.com/ray-language/raymart) (comercio electrónico con microservicios, gateway, cola y cuatro bases de datos), [raycall](https://github.com/ray-language/raycall) (microservicios con RPC y trazas) |
| Seguridad y P2P | [msg](https://github.com/ray-language/msg) (chat P2P cifrado), [takeit](https://github.com/ray-language/takeit) (transferencia de archivos cifrada), [raypass](https://github.com/ray-language/raypass) (bóveda de secretos), [raysync](https://github.com/ray-language/raysync) (sincronización cifrada) |
| Web | [store](https://github.com/ray-language/store) (tienda con frontend Astro y backend en raylang), [raysite](https://github.com/ray-language/raysite) (generador de sitios estáticos) |
| Terminal | [raytop](https://github.com/ray-language/raytop) (visor de procesos), [raylogs](https://github.com/ray-language/raylogs) (analizador de logs), [raytetris](https://github.com/ray-language/raytetris), [rallyx](https://github.com/ray-language/rallyx) y [1942](https://github.com/ray-language/1942) (juegos a 30 fps) |

**[raymart](https://github.com/ray-language/raymart)** es el ejemplo más completo: un comercio electrónico distribuido con
cuatro APIs de arquitectura hexagonal (productos, carrito, pedidos y pagos), cada una con su base
de datos (PostgreSQL, MySQL, MongoDB y raykv), detrás de raygate, comunicadas por rpc y gRPC y
coordinadas con la cola de rayq. Corre como binarios nativos con `docker compose`, e incluye
pruebas de punta a punta, de caos y de carga. Es la referencia para ver cómo encajan varias de
estas apps en un sistema real.

Los paquetes que usan están en el
[índice de paquetes](https://github.com/ray-language/ray-index), y se buscan con `ray search`.

Para cualquier función, la [referencia](../REFERENCE.md) tiene su firma, y el
[manual](../MANUAL.md) explica el lenguaje en detalle. La [portada](index.md) reúne todos los
capítulos.
