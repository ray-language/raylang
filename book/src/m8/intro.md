# Inferencia local, REPL y mejores errores

Con M7, raylang quedó **completo como lenguaje**: tipos, datos, closures, enums,
genéricos, manejo de errores y azúcar de llamada. M8 no añade nada al lenguaje en sí;
mira hacia **quien lo escribe**. Son tres mejoras de comodidad y *tooling* que cierran la
hoja de ruta:

1. **Inferencia local** (M8.1): poder escribir `let x = 3` sin anotar el tipo.
2. **REPL** (M8.2): un bucle interactivo para probar código línea a línea.
3. **Mejores errores** (M8.3): diagnósticos que muestran la línea de fuente y señalan la
   posición.

## Un hito de ergonomía, no de poder

Los tres cambios comparten una propiedad: **casi no tocan el núcleo**. La inferencia local
vive solo en el checker (y borra los tipos, como los genéricos). El REPL resultó ser un
**cliente externo** que no añade ni una línea al checker ni al intérprete. Los diagnósticos
son **solo presentación**: un módulo que lee la fuente y dibuja un `^`, sin que ninguna
fase cambie.

> **Por qué importa "casi no tocan el núcleo".** Un proyecto madura no solo añadiendo
> features, sino comprobando que las capas ya construidas **aguantan peso nuevo sin
> deformarse**. Que un REPL completo se pueda montar encima del front-end sin abrirlo, o
> que mostrar el código junto al error no exija cambiar el checker, es la señal de que las
> fronteras entre fases estaban bien puestas. M8 es, en parte, esa comprobación.

## La línea que M8 *no* cruza

La decisión de diseño más importante de M8 es lo que deja **fuera**. La inferencia es
**solo de locales**: `let x = 3` se infiere, pero las **firmas de función** —parámetros y
retorno— siguen siendo obligatorias. Es la frontera que fijó la §0 desde el día 1: un type
checker honesto, pero **sin inferencia global**. Inferir el tipo de un `let` es trivial (el
tipo está en el inicializador); inferir las firmas sería resolver un sistema de ecuaciones
en todo el programa. M8 se queda, deliberadamente, del lado barato de esa línea.

## Las tres sub-fases

- **Inferencia local** (M8.1): `let x = 3` sin anotación; el checker la deduce del
  inicializador.
- **El REPL** (M8.2): leer-evaluar-imprimir, como cliente externo del lenguaje.
- **Mejores errores** (M8.3): contexto de fuente y un cursor `^` en los diagnósticos.

Al terminar, raylang no es más potente, pero sí mucho más **agradable de usar** — que era
justo el objetivo del último hito.
