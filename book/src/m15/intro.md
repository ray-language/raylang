# Redes y la base moderna

Hasta M14, raylang es un lenguaje **completo y auto-alojado**: se lexea, parsea, chequea y
ejecuta a sí mismo. Pero mira hacia adentro. M15 lo hace mirar **afuera**: lo que un lenguaje
moderno necesita para tocar el mundo —reloj, aleatoriedad, matemáticas y, sobre todo,
**redes**— sin abandonar las dos invariantes del proyecto: **cero dependencias de Cargo** (todo
sobre la `std` de Rust) y el **oráculo** (intérprete ↔ VM) allí donde el comportamiento sea
determinista.

## La decisión: builtins para el transporte, librerías para los protocolos

Cuando un lenguaje "tiene redes", ¿qué significa eso exactamente? Hay una distinción que vale la
pena hacer explícita, porque ordena todo M15:

- El **transporte** —abrir un socket TCP, escribir y leer bytes— necesita hablar con el sistema
  operativo. Eso solo puede venir de **builtins** sobre `std::net`. No hay forma de escribir un
  socket "en raylang puro".
- Los **protocolos** —HTTP, el formato JSON, parsear una URL— son **lógica sobre el transporte**:
  manipular strings, estructurar datos. Eso *sí* se puede escribir en el propio raylang.

Así que la regla de M15 es: **el transporte son builtins; los protocolos son librerías escritas en
raylang** y traídas con `import`. Es la materialización de un principio que el proyecto viene
defendiendo desde la stdlib de M7 (`map`/`filter`/`fold` en raylang) y que el self-hosting llevó al
extremo: *lo que se puede escribir en el lenguaje, se escribe en el lenguaje*. El cliente HTTP de
raylang no es código de Rust con un nombre bonito: es un `.ray` que cualquiera puede leer, copiar y
modificar.

## La base moderna: lo que faltaba

Antes de las redes, M15 cierra tres huecos que un lenguaje moderno da por sentados y que raylang
**no tenía**:

- **Matemáticas** (`sqrt`, `pow`, `sin`, `ln`, `abs`, `min`/`max`, `pi`/`e`…). Las funciones
  trascendentes necesitan los intrínsecos de `f64`, así que son builtins. Como casi todas son
  uniformes (`float -> float`), en vez de un opcode por función se usa **un opcode parametrizado**
  (`MathF(MathFn)`): el opcode dice "aplica una función matemática" y un `enum` dice cuál. La VM y el
  intérprete tienen **una sola** rama que delega en un helper compartido, lo que mantiene pequeño el
  `match` gigante de la VM —cuyo *layout* afecta al *codegen*, como aprendimos optimizando—. Y como
  son deterministas, se validan con el **oráculo** (incluido un caso de borde con `NaN`).

- **Reloj** (`now`, `monotonic`, `sleep`) y **aleatoriedad** (`random`, `random_int`). Aquí aparece
  un detalle revelador de la invariante "cero dependencias": **`std` no trae un generador de números
  aleatorios**. La mayoría de los lenguajes tiran de una librería; raylang no puede. Así que lleva su
  propio PRNG —un **SplitMix64**, sembrado del reloj la primera vez— en unas pocas líneas. No es
  criptográfico, pero sirve para simulación, *jitter* e identificadores. Reloj y RNG **no son
  deterministas**, así que no entran al oráculo: se prueban por **propiedades** (rangos, monotonía,
  variedad) ejecutando el binario, como el I/O de M11.

## El mapa de M15

M15 se construye por cortes verticales, de la base al capstone:

- **M15.1 — la base moderna.** Matemáticas (oráculo) + reloj/RNG (subproceso).
- **M15.2 — cliente TCP.** `tcp_connect`/`socket_read`/`socket_write`, sobre `std::net`, reusando el
  **molde de handles** de los archivos (M11.8).
- **M15.3 — servidor TCP.** `tcp_listen`/`tcp_accept`: escuchar y aceptar conexiones.
- **M15.4 — protocolos en raylang.** Una librería **JSON** y un cliente **HTTP**, escritos en el
  propio lenguaje e importados. El cambio de registro de M15.
- **M15.5 — el servidor concurrente (capstone).** Sockets no bloqueantes integrados con el scheduler
  de M12, para atender muchas conexiones a la vez sobre un solo hilo.

El hilo conductor: **los builtins de red reusan piezas que ya existen** (el registro de handles de
M11.8, el patrón de arreglo etiquetado → `Result` del prelude, las fibras y el scheduler de M12), y
**el runtime solo cambia donde es inevitable**. Tocar el mundo no exigió rediseñar el lenguaje: lo
exigió usarlo.
