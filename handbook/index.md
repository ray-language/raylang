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

## En camino

Estos capítulos se publican uno a uno, cada uno con su proyecto de ejemplo completo en
`examples/apps/`:

- **App móvil para iOS y Android** con un frontend React + TypeScript y datos en `std/kv`.
- **App multiplataforma**: la misma app en macOS, Linux y Windows, con SQLite.
- **API web** con el framework `web`, Postgres y pools de conexiones.
- **Sitio con plantillas** renderizado en el servidor, con los datos en archivos.
- **Sitio con frontend React embebido** en el binario, con Redis.
- **LLM y MCP**: un asistente que escribe raylang verificado, y un agente escrito en raylang.
- **Distribuir**: empaquetar, firmar, notarizar y auto-actualizar.

Mientras tanto, el detalle de escritorio y móvil está en el manual:
[ventanas con `std/ui`](../MANUAL.md#ventanas-stdui),
[empaquetado con `ray bundle`](../MANUAL.md#empaquetar-la-app-ray-bundle) y
[hot reload en el teléfono](../MANUAL.md#hot-reload-del-programa-en-el-teléfono-ray-dev---device).
