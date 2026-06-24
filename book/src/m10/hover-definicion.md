# Hover e ir-a-definición

M10.2 daba **diagnósticos**: corría el front-end y subrayaba el primer error. M10.2b añade las
dos features de IDE que faltaban: **hover** (poner el cursor sobre un identificador y ver su
tipo) e **ir-a-definición** (saltar del uso a la declaración).

## El cambio de fondo: el checker pasa de *validador* a *consultable*

Hasta aquí, todo lo que hicimos con el checker respetaba que su salida fuera un veredicto:
`check` devuelve `Result<(), TypeError>` y **tira** los tipos que calcula —pura mentalidad
*erasure*—. Para diagnósticos bastaba: solo queríamos el primer error.

Hover e ir-a-definición piden algo distinto: que el checker **exponga lo que sabe**. El tipo de
`x` bajo el cursor; dónde se declaró `doble`. Es justo la "API de tipos del checker" que
**evitamos a propósito** en el REPL (M8.2 muestra el *valor* con `print` para no abrirla). M10.2b
la abre —pero **contenida**—: no es una semántica nueva ni un cambio de runtime, es
**introspección**.

## Un `SemanticIndex` recolectado al vuelo

La idea: durante una pasada de chequeo, anotar en un índice lo que un editor necesita.

```rust
pub struct SemanticIndex {
    pub hovers: Vec<HoverEntry>,  // (línea, col, largo) -> texto (el tipo)
    pub defs:   Vec<DefEntry>,    // (línea, col, largo) -> (línea_def, col_def)
}
```

Tres decisiones lo hacen barato y limpio:

1. **Se factoriza el front-end.** `check` y una función nueva, `semantic_index`, comparten los
   pasos 0–1 (`prepare_program`: prelude, derivaciones, bajada de impls, resolución de enums) y
   `check_program`. La diferencia es un flag: el `Checker` en modo `gather` **apunta** cada uso
   de identificador en el índice. En una verificación normal el flag está apagado: **coste cero**.

2. **Se recolecta antes de cualquier *lowering*.** `check_program` corre sobre el AST con las
   posiciones **de la fuente original** (UFCS, diccionarios, trait objects se bajan *después*).
   Así el índice habla en coordenadas que el editor reconoce, no en términos de los nombres
   manglados (`Caja#medir`) ni de los closures sintéticos.

3. **Granularidad de identificador.** Por cada `Ident` que resuelve —variable, parámetro,
   función— se registra su **tipo** (para hover) y la **posición de su declaración** (para
   definición). Esto pidió un par de añadidos pequeños: que `VarInfo` lleve dónde se declaró la
   variable, y un mapa `nombre → posición` de las funciones.

`semantic_index` **tolera errores**: si el programa está a medio escribir, devuelve la info
parcial recolectada hasta el fallo. Es lo que uno quiere mientras teclea.

## El LSP gana estado

Los diagnósticos eran *stateless*: cada `didChange` traía el texto entero. Hover y definición no:
la petición trae solo la `uri` y la **posición** del cursor, no el texto. Así que el servidor
ahora **recuerda los documentos** (`didOpen`/`didChange` los guardan; `didClose` los olvida).

El resto es traducción. `initialize` anuncia `hoverProvider` y `definitionProvider`. Ante un
`textDocument/hover`, el servidor reconstruye el índice del documento, busca la entrada cuyo rango
contiene el cursor y responde el texto:

```text
hover sobre  let x = doble(21)
                 │
                 ▼
             x: int
```

`textDocument/definition` es igual, pero devuelve una `Location` (uri + rango de la declaración):
saltar del uso de `doble` a su `fn doble(...)`. Las coordenadas se convierten 1-basadas (fases) ↔
0-basadas (LSP), como en los diagnósticos.

## El precio de las posiciones-sin-spans

raylang nunca tuvo *spans* —solo un `(línea, col)` por nodo (DESIGN §3)—, y eso se nota aquí. El
rango que devolvemos para un identificador es `[col, col + largo_del_nombre)`, lo bastante bueno
para subrayar el uso. Pero la *declaración* de una `let x` apunta al `let` (no tenemos la posición
del nombre por separado), así que el salto cae en la línea correcta con un subrayado aproximado.
Es una **degradación honesta**, no un error: la promesa "todo nodo lleva su posición" da el 90% de
la ergonomía sin el coste de un sistema de spans.

## Find-references y rename (cluster 4)

El mismo índice `defs` da, casi gratis, dos features más. Una declaración se identifica por su
*clave* `(def_line, def_col)`; **todos los usos con la misma clave son el mismo símbolo** (los
ámbitos ya están resueltos por el checker: dos `x` en funciones distintas tienen claves distintas).
Así:

- **find-references**: dado el cursor, halla la clave del símbolo (por el uso bajo el cursor, o por
  el nombre de su declaración) y devuelve todos los usos con esa clave (y la declaración, si el
  cliente pide `includeDeclaration`).
- **rename**: las mismas posiciones, pero como un `WorkspaceEdit` que sustituye cada una por el
  nuevo nombre.

El único matiz es el de siempre —sin spans—: para *renombrar* la declaración necesitamos el rango
del **nombre**, no el del `let`/`fn`. Se resuelve en el cliente LSP escaneando la línea de la
declaración por el primer identificador igual al nombre (que el `let`/`fn` precede). Es heurística,
pero la gramática de raylang es lo bastante simple para que sea fiable —y, fiel al diseño del LSP,
**no toca el núcleo**: todo vive en `src/lsp.rs`—.

## Alcance

Hover y definición sobre **identificadores** (variables, parámetros, funciones); **find-references**
y **rename** sobre los mismos. Quedan fuera los **métodos** y **nombres de tipo** (que no llegan como
`Ident` sino en posición de tipo), y *completion* y *signature help*.

La lección de M10.2b es la contracara de la del REPL. Allí *evitamos* exponer los tipos del
checker porque no hacía falta; aquí, cuando sí hizo falta, abrirlo costó poco: un flag, un índice
y una función pública. **Runtime y semántica intactos** —M10.2b no cambia qué programas son
válidos ni qué significan; solo *cuenta* lo que el checker ya sabía—.
