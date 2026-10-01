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

1. [Empezar](empezar.md): instalar, el editor, un proyecto, el lenguaje en quince minutos, la
   concurrencia, trabajar con un asistente LLM y las herramientas.
2. [App móvil para iOS y Android](movil.md): una app de notas con frontend React + TypeScript y
   datos en `std/kv`, del proyecto al simulador, al emulador y a la recarga en caliente en el
   teléfono.
3. [App multiplataforma](multiplataforma.md): la misma app en macOS, Linux y Windows, con SQLite,
   menús y diálogos nativos, y una interfaz que se adapta a cada pantalla.
4. [Sitio con plantillas](ssr.md): las notas como sitio renderizado en el servidor, con plantillas
   compiladas, formularios, Markdown y los datos en archivos.
5. [LLM y MCP](llm-mcp.md): un asistente que escribe raylang verificado con `ray mcp`, y un agente
   escrito en raylang que habla con Claude y usa herramientas por MCP.
6. [API web](api.md): las notas como API JSON con el framework `web`, Postgres con un pool de
   conexiones, autenticación por token y apagado ordenado.

## En camino

Estos capítulos se publican uno a uno, cada uno con su proyecto de ejemplo completo en
`examples/apps/`:

- **Sitio con frontend React embebido** en el binario, con Redis.
- **Distribuir**: empaquetar, firmar, notarizar y auto-actualizar.

El detalle de cada API de ventanas está en el manual:
[ventanas con `std/ui`](../MANUAL.md#ventanas-stdui) y
[empaquetado con `ray bundle`](../MANUAL.md#empaquetar-la-app-ray-bundle).
