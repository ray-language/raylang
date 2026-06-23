# Módulos y `pub`

Hasta M10 un programa raylang vivía en **un solo archivo**. M11.3 lo escala a varios, con
**encapsulamiento**: cada archivo es un **módulo**, y `pub` decide qué exporta. Es la pieza
arquitectónica de M11 —y un prerrequisito para que el lenguaje pueda, algún día, compilarse a sí
mismo—.

## La idea: aplanar en el front-end

El truco que define a raylang vuelve a aparecer: igual que UFCS, los diccionarios de traits o
`dyn`, **los módulos se borran antes del checker**. El intérprete y la VM nunca saben qué es un
módulo. Un *loader* (en `src/loader.rs`) lee el archivo de entrada, sigue sus `import`, carga cada
módulo una vez, y **fusiona todo en un único `Program`** con nombres globales únicos. Tras esa
fase, el resto del compilador corre **sin un solo cambio**.

¿Por qué un loader y no algo en el checker? Porque cargar archivos es **I/O del host** (Rust), no
una operación del lenguaje. El loader es un *cliente* de la librería, como el REPL, el runner de
`@test` o el LSP: usa el lexer y el parser públicos, hace su trabajo, y entrega un AST plano.

## Las tres fases del loader

1. **Cargar (BFS).** Desde la entrada, parsea, mira sus `import M;` y `from M import …;`, resuelve
   cada `M` a `<dir>/M.ray`, lo lee y parsea, y **recurre**. Cada módulo se carga **una vez**: los
   ciclos del grafo de imports (A importa B, B importa A) no son problema, porque las referencias
   cruzadas se resuelven *después* de fusionar.
2. **Namespacing.** Los ítems de los módulos **no-entrada** se renombran a nombres globales únicos
   `modulo::nombre`. El `::` es ilegal en un identificador del usuario, así que no colisiona con
   nada que se pueda escribir (el mismo truco que el `#` del *mangling* de traits en M9). El módulo
   de **entrada** —el del `fn main`— **no** se renombra: sus nombres ya son globales, y `main` debe
   seguir llamándose `main`.
3. **Resolución (consciente de ámbitos).** Por cada módulo se arma un mapa *nombre local → nombre
   global* y se reescribe cada referencia, **respetando los ámbitos**: una variable o parámetro que
   tape un nombre de nivel superior **no** se reescribe.

## `import M;` — el módulo como espacio de nombres (M11.3a)

```rust
// mates.ray
pub fn doble(n: int) -> int { n + n }
fn secreto() -> int { 99 }       // sin 'pub': privado a 'mates'

// app.ray
import mates;
fn doble(x: int) -> int { x + 1 }     // una 'doble' local, distinta
fn main() -> int {
  print(mates.doble(10));   // 20  — la del módulo
  print(doble(10));         // 11  — la local, no colisiona
  mates.doble(21)           // 42  (código de salida)
}
```

El acceso calificado **reusa el `.`** —no se inventa un `::` para el usuario—. Pero `mates.doble`
llega al parser **igual** que un acceso a campo o que UFCS: `Call(Field(Ident "mates", "doble"))`.
La desambiguación, en orden:

1. ¿Hay una **variable local** `mates` en ámbito? → es campo/UFCS sobre ese valor.
2. ¿Es `mates` un **módulo importado**? → es una **ruta calificada** → `mates::doble`.
3. Si no, sigue la resolución de siempre (campo de struct → método → función libre).

**Aquí se aplica `pub`:** referenciar `mates.secreto()` es un error de carga —el módulo no lo
exporta—. La visibilidad es **explícita** (estilo Rust, no por mayúscula estilo Go).

## `from M import a [as b];` — traer nombres al ámbito (M11.3b)

A veces calificar todo cansa. `from` trae nombres **al ámbito**, sin calificar, con **renombrado
opcional** para esquivar colisiones:

```rust
from mates import doble as md, triple as tri;
fn doble(x: int) -> int { x + 1 }     // mi propia 'doble'
fn main() -> int {
  print(doble(10));   // 11  — la local
  print(md(10));      // 20  — mates.doble, traída como 'md'
  md(10) + tri(10)    // 50
}
```

La implementación es minúscula sobre lo de -a: el nombre local (el alias, o el original) se
**inyecta en el mismo mapa de resolución** apuntando al global `mates::doble`. Una referencia a `md`
se reescribe a `mates::doble`, salvo que una local lo tape. Si el nombre choca con una función
propia (o con otro import) **sin `as`**, es error: el `as` es precisamente la salida.

Nota estilo Python: `from M import x` **no** trae `M` al ámbito —solo `x`—. Para usar `M.otra`
hace falta además `import M;`.

## Lo que queda fuera (a propósito)

- **Cruzar tipos entre módulos** (`from M import Punto`, `M.Punto` en una anotación, o construir
  `M.Color.Rojo`) está **diferido**. Hoy los tipos **no se namespacan**: son globales-únicos (un
  choque de nombres de tipo entre módulos es error), así que un tipo de otro módulo se referencia
  por su nombre tal cual —sin encapsulamiento de tipos—. Hacerlo bien exige namespacar tipos y
  reescribir todas las posiciones de tipo y de patrón; queda para más adelante. El loader, eso sí,
  da un error **claro** si intentas `from M import <Tipo>`.
- **Submódulos / jerarquía de directorios**, `pub` granular (por campo), *re-exports* → futuro.

> La lección de M11.3 es que una *feature* que parece muy de "sistema" —cargar archivos, espacios
> de nombres, visibilidad— cabe entera en el front-end si ya tienes la disciplina del **erasure**.
> No se tocó ni el checker, ni el intérprete, ni la VM: el oráculo siguió verde sin enterarse de
> que ahora un programa puede vivir en diez archivos.
