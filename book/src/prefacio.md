# Prefacio

Este libro es el diario de un experimento: **construir un lenguaje de programación
desde cero para aprender**. No para competir con nadie, no para producción —
para entender, de primera mano y sin atajos, cada problema que aparece cuando uno
decide inventar un lenguaje y hacerlo funcionar.

El lenguaje se llama **raylang**. Es pequeño, estáticamente tipado, con sintaxis de
llaves y orientado a expresiones. Está implementado en **Rust**. Cuando termines de
leer, deberías ser capaz de seguir —y reproducir— el camino completo: del texto
fuente a un programa que se ejecuta.

## Por qué construir un lenguaje

Porque es uno de los pocos proyectos que te obliga a tocar, a la vez, casi todas
las áreas difíciles de las ciencias de la computación: autómatas y análisis de
texto, gramáticas y árboles, sistemas de tipos, semántica, gestión de memoria,
máquinas virtuales, generación de código. Cada una es un mundo; un lenguaje las
encadena todas en una sola tubería (*pipeline*) que tiene que funcionar de punta a
punta.

Y porque desmitifica. Después de escribir tu propio lenguaje, los compiladores y
los intérpretes que usas a diario dejan de ser cajas negras.

## Cómo está organizado

El libro sigue el orden en que el lenguaje se construyó, por **hitos** (milestones):

- **Diseño** — las decisiones que tomamos *antes* de escribir código, y por qué.
  Algunas decisiones tempranas habilitan o bloquean features futuras; aprender a
  distinguirlas es medio juego.
- **M1 — El front-end y el intérprete** — la tubería completa que *analiza* y
  *ejecuta*: lexer, parser, checker e intérprete.
- **M2 en adelante** — la máquina virtual, structs, closures, genéricos… se irán
  sumando a medida que el proyecto avance.

Cada capítulo cuenta no solo *qué* se hizo, sino *cómo se piensa* el problema y
*por qué* se eligió una solución sobre otra. El código vive en el repositorio; aquí
está la historia y la enseñanza.

## El método

raylang se construye **una fase a la vez**, con tests en cada fase y explicando el
diseño a fondo. Este libro es, en buena medida, la versión pulida de esas
explicaciones.

## Mapa de documentos

raylang tiene tres documentos vivos que conviene conocer; este libro los
complementa sin repetirlos:

| Documento | Qué es |
|-----------|--------|
| `DESIGN.md` | El **contrato** del lenguaje: la especificación concisa (gramática, tipos, reglas). |
| `IDEAS.md` | El **backlog**: features futuras y su impacto en el diseño. |
| Este libro | La **narrativa**: el viaje, el porqué de cada decisión, la enseñanza. |

Cuando una decisión cambia, se actualiza `DESIGN.md`. El libro puede conservar
"por qué lo decidimos así en su momento" como parte de la historia.

Empecemos por el principio: las decisiones de diseño.
