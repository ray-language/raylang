# El handbook de raylang

Español · [English](index.en.md)

Guías para **construir cosas reales** con raylang, de `ray new` a la app instalada o el servicio
desplegado. Cada guía es un camino completo para un tipo de proyecto: no explica el lenguaje
módulo a módulo (para eso están el [manual](../MANUAL.md) y la [referencia](../REFERENCE.md)), sino
qué piezas usar, en qué orden y por qué.

Las guías se encadenan: la libreta de notas que empieza como app móvil vuelve como app de
escritorio, como API, como sitio con plantillas y como sitio con un frontend de React. Cada una
guarda sus datos de una forma distinta, así que al terminar has usado toda la gama: el almacén
`std/kv`, SQLite, Postgres, archivos y Redis.

Todo el código de estas páginas compila: el CI pasa `ray check` sobre cada bloque.

## Capítulos

1. [Empezar](getting-started.md): instalar, el editor, un proyecto, el lenguaje en quince minutos, la
   concurrencia, trabajar con un asistente LLM y las herramientas.
2. [App móvil para iOS y Android](mobile.md): una app de notas con frontend React + TypeScript y
   datos en `std/kv`, del proyecto al simulador, al emulador y a la recarga en caliente en el
   teléfono.
3. [App multiplataforma](cross-platform.md): la misma app en macOS, Linux y Windows, con SQLite,
   menús y diálogos nativos, y una interfaz que se adapta a cada pantalla.
4. [Ventanas a fondo](windows.md): todo lo que `std/ui` da a una app de escritorio, sobre un
   editor de texto: el puente con la página, menús, diálogos, tipos de ventana y cambios sin
   guardar.
5. [Sitio con plantillas](ssr.md): las notas como sitio renderizado en el servidor, con plantillas
   compiladas, formularios, Markdown y los datos en archivos.
6. [API web](api.md): las notas como API JSON con el framework `web`, Postgres con un pool de
   conexiones, autenticación por token y apagado ordenado.
7. [Sitio con frontend React](web-react.md): un frontend React embebido en el binario, una API JSON
   y las notas en Redis.
8. [Herramienta de terminal](cli.md): una herramienta de línea de comandos con opciones, entrada
   por tubería, salida para personas y para programas, y códigos de salida.
9. [LLM y MCP](llm-mcp.md): un asistente que escribe raylang verificado con `ray mcp`, un agente
   escrito en raylang con los paquetes `llm`, `mcp` y `agent`, y un servidor MCP propio.
10. [Rendimiento](performance.md): medir, perfilar con `ray profile`, arreglar el algoritmo,
    compilar a nativo y repartir el trabajo entre núcleos, con cifras medidas.
11. [Distribuir](shipping.md): empaquetar y firmar para cada sistema, publicar en las tiendas
   móviles y actualizaciones automáticas firmadas en escritorio.
12. [Más ejemplos](examples.md): las apps de la organización ray-language, por caso de uso, para lo
   que no cubren los capítulos.

Para cualquier función, la [referencia](../REFERENCE.md) tiene su firma, y el
[manual](../MANUAL.md) explica el lenguaje en detalle.
