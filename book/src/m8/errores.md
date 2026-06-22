# Mejores errores: contexto de fuente

Desde M1, un error de raylang siempre supo **dónde** ocurrió: cada token y cada nodo carga
su `(línea, columna)`, y los mensajes la incluyen. Pero leer `error de tipos en 2:13: ...`
obliga a contar columnas a mano. M8.3 da el último paso: **mostrar la línea de fuente y
señalar la posición con un `^`**.

```text
error de tipos en 2:13: el operador '+' requiere ambos operandos int o ambos float
  2 |     let x = 1 + true;
    |             ^
```

Funciona en las **cuatro fases**: léxico, sintaxis, tipos y ejecución.

## La recompensa de un principio viejo

Aquí se cobra una decisión tomada en el día 1 (§1, principio 3): *"todo token y nodo carga
`(línea, columna)`; se diseña desde el principio, no se agrega después"*. Gracias a eso,
dibujar el cursor no necesitó **ningún** cambio en el lexer, el parser, el checker ni el
intérprete. Cada error ya traía su posición; solo faltaba **usarla** para recortar la línea
de la fuente y poner un `^` debajo.

> **Diseñar para el futuro, sin pagarlo antes.** En M1, arrastrar `(línea, columna)` por
> cada token parecía burocracia: los mensajes apenas la usaban. Siete hitos después, esa
> burocracia es lo que hace que M8.3 sea un módulo nuevo de cuarenta líneas en vez de una
> cirugía por todo el front-end. Es el patrón que recorre raylang: el `Type` extensible, el
> AST que anticipó UFCS, la posición en cada nodo. Pequeñas previsiones que después se
> pagan solas.

## Solo presentación

Todo M8.3 vive en un módulo nuevo, `src/diagnostic.rs`, con una sola función:

```rust
render(source, line, col, headline) -> String
```

Antepone la **cabecera** del error —su `Display` de siempre, que ya trae ubicación y
mensaje— y le añade la línea de fuente recortada y la línea del cursor. No es una fase del
compilador: es un **renderizador** que el cliente (el runner de archivos, el REPL) llama
cuando ya tiene un error y la fuente a mano. El lexer, el parser, el checker y el intérprete
siguen sin saber nada de esto.

## Las dos decisiones

- **Un solo `^`, no un subrayado `^^^^`.** Marcar el token o la expresión enteros sería más
  preciso, pero exigiría que cada token, nodo y error llevara un **span** (inicio..fin) en
  vez de un solo punto —un cambio que cruzaría todo el front-end—. Con el `(línea, columna)`
  que ya existe, un único cursor da el 90% del valor con el 0% de ese coste. Los spans
  quedan como mejora futura.
- **Texto plano, sin color.** Nada de ANSI: es portable (no depende de si la salida es una
  terminal) y, sobre todo, **testeable** —un diagnóstico es una `String` que se compara
  carácter a carácter en una prueba—.

## El borde del REPL

Hay una aspereza honesta. El REPL no ejecuta lo que escribes tal cual: lo envuelve en un
`fn main() { ... }` sintetizado. Cuando algo falla, el cursor apunta correctamente al token
ofensor (que es tu código), pero el **número de línea** es el de esa fuente sintetizada, no
el de tu entrada. Mapearlo de vuelta a la línea original es posible, pero pedía más
maquinaria de la que justificaba; se dejó anotado como límite conocido. Es un buen recordatorio
de que "mejores errores" es un pozo sin fondo: siempre hay un escalón más de pulido.

## M8 completo

raylang cumple su hoja de ruta. Es un lenguaje pequeño pero entero: dos motores que
coinciden, datos compuestos con GC, tipos suma con *pattern matching*, genéricos con manejo
de errores, azúcar de llamada con una stdlib propia, inferencia local, un REPL y
diagnósticos que se leen. No es práctico ni original —nunca quiso serlo—; quiso **tocar de
forma honesta cada fase** de construir un lenguaje. Y lo hizo.

> Código: `src/diagnostic.rs` (`render`), `src/main.rs` (las cuatro fases) y `src/repl.rs`
> (contra su fuente sintetizada). Pruebas: unitarias del renderizador e integración por
> subproceso (`tests/errors_cli.rs`).
