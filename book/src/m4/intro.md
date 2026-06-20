# Funciones, closures y memoria

Hasta M3 las funciones de raylang eran de **segunda clase**: existían, se llamaban,
pero no eran *valores*. No podías guardar una función en una variable, pasarla a otra
función o devolverla. M4 cambia eso —y, al hacerlo, abre una caja que obliga a
resolver el problema más profundo del runtime: **la gestión de memoria**.

## Una feature que arrastra a la otra

M4 son tres pasos encadenados por una necesidad:

1. **Funciones de primera clase** (M4.1): una función es un valor. Se pasa, se
   devuelve, se guarda.
2. **Closures** (M4.2): una función anónima que **captura** variables de su entorno.
   Aquí aparece lo interesante —y el problema.
3. **Recolección de basura** (M4.3): las closures hacen que un valor capturado
   **sobreviva al marco** que lo creó, escapando al heap; y los valores en el heap se
   referencian libremente, formando **ciclos** que el conteo de referencias (`Rc`) no
   sabe liberar. M4.3 introduce un recolector trazador que sí.

No es casualidad que la hoja de ruta junte closures y GC: **las closures crean
exactamente el patrón de memoria que fuerza un GC de verdad.**

## La división que ordena todo M4

raylang tiene dos motores que deben coincidir (el intérprete como oráculo, la VM como
camino al rendimiento). M4 los hace divergir en un punto, por una razón hermosa:

- El **intérprete** es un *tree-walker*: sus valores vivos están dispersos en la pila
  de llamadas de Rust, raíces imposibles de enumerar. **Se queda con `Rc`** (conteo de
  referencias) —y eso es, en sí, una lección: usa `Rc` *porque* no puede trazar sus
  raíces.
- La **VM** tiene su estado **reificado** (pila de operandos, marcos, locales): sus
  raíces son explícitas y enumerables. **Aquí vive el recolector trazador.**

El oráculo compara resultados observables, no memoria, así que ambos motores siguen
debiendo dar el mismo resultado —aunque por dentro gestionen la memoria de forma
distinta.

## El plan de M4

1. **Funciones de primera clase** — el tipo `fn(...) -> R`, la función anónima, las
   llamadas directas e indirectas.
2. **Closures** — la captura de entorno por referencia, y el mecanismo de *upvalues*.
3. **El recolector de basura** — un heap propio en la VM, marca y barrido, y la
   liberación de ciclos.

Empecemos por hacer de las funciones, valores.
