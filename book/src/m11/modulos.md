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

## Tipos por módulo (M11.3c)

Hasta -b los **tipos** (`struct`/`enum`/`trait`) seguían siendo globales-únicos: dos módulos no
podían reusar un nombre, y un tipo no tenía encapsulamiento. M11.3c los pone **por módulo**, igual
que las funciones: un tipo de un módulo no-entrada se namespaca a `modulo::Tipo`, y `pub` controla
su exportación. Dos módulos ya pueden definir cada uno su `struct Node`; un tipo es **privado** a su
módulo salvo que sea `pub` **y** se importe.

```rust
// geo.ray
pub struct Punto { x: int, y: int }
pub enum Eje { X, Y }

// app.ray
from geo import Punto, Eje;        // -2: 'from' también trae tipos pub
fn coord(p: Punto, e: Eje) -> int {
    match (e) { Eje.X => p.x, Eje.Y => p.y, }
}
fn main() -> int {
    let p: Punto = Punto { x: 11, y: 31 };
    coord(p, Eje.X) + coord(p, Eje.Y)   // 42
}
```

El reto frente a las funciones: un nombre de tipo aparece en **muchas posiciones** —anotaciones
(params, retorno, `let`), campos de struct, payloads de variante, objetivo/trait de un `impl`,
bounds, `dyn Trait`— **y** en expresiones que lo nombran: literal de struct (`Punto { … }`),
construcción de enum (`Eje.X`, que llega como `Field`) y patrones de `match`. El loader las reescribe
todas con un **reescritor de tipos** aparte del de valores. La sutileza: el parser emite
`Type::Struct(name)` para **cualquier** identificador en posición de tipo —incluidos los
**parámetros de tipo** `T`—, así que el reescritor es *consciente de los `<…>`* en ámbito: un `T`
ligado se deja intacto; un tipo propio del módulo → `modulo::Tipo`; uno `from`-importado → su global.

`from M import Tipo [as T]` (M11.3c-2) cierra el cruce: trae un tipo `pub` al ámbito sin calificar,
con alias para esquivar colisiones. La plomería es la de -b, pero el nombre local se inyecta en el
mapa del **reescritor de tipos** (no en el de valores): una referencia a `Tipo`/`T` se reescribe a
`M::Tipo` igual que un tipo propio. Importar un tipo **privado** es un error de carga (pide `pub`).

## Referencias calificadas: `M.Tipo` y `M.Color.Rojo` (M11.3c-3)

La otra forma de cruzar un tipo es **calificarlo**, como ya se hacía con las funciones (`M.f(...)`).
Funciona en las cuatro posiciones donde aparece un tipo:

```rust
import geo;
fn dist(p: geo.Punto) -> int { p.x + p.y }      // anotación
fn valor(c: geo.Color) -> int {
    match (c) {                                     // patrón
        geo.Color.Rojo => 1,
        geo.Color.Verde(n) => n,
    }
}
fn main() -> int {
    let p: geo.Punto = geo.Punto { x: 10, y: 5 };  // literal de struct calificado
    valor(geo.Color.Verde(27)) + dist(p)           // construcción de enum calificada
}
```

El truco vuelve a ser **borrar en el front-end**. El parser guarda el `.` *dentro del nombre*:
`geo.Punto` llega como `Type::Struct("geo.Punto")`, el patrón como `enum_name: "geo.Color"`, el
literal como `StructLit { name: "geo.Punto" }`; la construcción `geo.Color.Rojo` llega como los
`Field`/`Call` anidados de siempre. El loader resuelve `M.X → M::X` **validando** que `M` esté
importado (`import M;`) y que `X` sea `pub`. El reparto sigue la línea valores-vs-tipos:

- Las posiciones de **valor** (`geo.Color.Rojo`) las colapsa el *resolutor* —es **consciente de los
  ámbitos**: una variable local llamada `geo` taparía al módulo—, extendiendo el acceso calificado
  para que resuelva también tipos `pub`.
- Las posiciones de **tipo** (anotación, nombre del literal, `enum_name` del patrón) las resuelve el
  *reescritor de tipos*: un nombre con `.` se parte y valida.

Si una referencia calificada **no resuelve** (módulo no importado, o tipo no `pub`), se **deja con el
`.`**: ningún tipo definido lleva un `.` en su nombre, así que el checker la rechaza como "tipo
desconocido". Eso *es* la encapsulación —un tipo privado de otro módulo no se alcanza ni calificándolo—.

## Módulos por directorios (M11.5)

Hasta aquí todos los módulos viven en **una sola carpeta** (el loader resuelve `import M;` contra
`<dir-de-entrada>/M.ray`). Para organizar un proyecto de verdad hace falta una **jerarquía de
directorios**, con la sintaxis tradicional estilo Unix:

```rust
import geo/formas/circulo;          // resuelve  <raíz>/geo/formas/circulo.ray
import geo/formas/circulo as c;     // ... con un nombre local distinto

fn main() -> int {
    let p = circulo.Punto { x: 2, y: 5 };   // se accede por el LEAF: 'circulo'
    circulo.area(4)
}
```

Tres decisiones fijan la forma:

- **Separador `/`.** En la línea de `import` no hay ninguna división, así que el parser reinterpreta
  `/` como separador de directorios sin ambigüedad con el operador. La ruta es **absoluta desde la
  raíz** del proyecto (el directorio del archivo de entrada): un submódulo que use a un vecino
  escribe la ruta completa (`import geo/util;`), no una relativa.
- **Solo *leaf-binding*.** `import geo/formas/circulo;` liga **el último segmento** (`circulo`) como
  nombre local. El acceso reusa el `.` de siempre (`circulo.area`, `circulo.Punto`). Dos rutas con el
  mismo último segmento colisionan → se pide `as` (igual que los `from`-imports).
- **Prohibido el acceso por ruta en expresiones.** Escribir `geo/formas/circulo.area(x)` sería
  ambiguo con la división —y además es mala práctica—. La ruta vive **solo** en la declaración
  `import`; toda referencia posterior usa el leaf. Así nunca aparece un `/` fuera de un `import`.

Lo bonito: el núcleo **no cambia nada**. La identidad de un módulo deja de ser su *stem* y pasa a ser
su **ruta** (`geo/formas/circulo`), pero el loader la namespaca con el mismo `::` de antes traduciendo
los `/` (`geo::formas::circulo::area`). Los mapas de acceso calificado pasan de un *conjunto* de
nombres a un **mapa leaf → ruta**. Un solo archivo, o una carpeta plana, quedan **idénticos** (sin
`/`, la traducción es la identidad y el leaf es el propio nombre). El intérprete y la VM siguen sin
enterarse: ven nombres planos con `::`.

## Aislar módulos: la cápsula `mod.ray` (M11.6a)

M11.5 organiza por carpetas, pero `pub` sigue siendo **binario**: un ítem es privado a su archivo o
alcanzable desde **todo** el proyecto por su ruta. No hay forma de decir *"esto es interno a `geo/`"*,
ni un directorio es **direccionable** (`geo/` no es nada, solo un prefijo). M11.6 lo arregla con un
modelo de **cápsula**, y la pieza es un archivo: **`mod.ray`**.

La regla: **la presencia de `geo/mod.ray` convierte `geo/` en una cápsula**.

```rust
// geo/formas/circulo.ray   (submódulo interno)
import geo/util;
pub struct Circulo { radio: int }
pub fn area(c: Circulo) -> int { 3 * util.cuadrado(c.radio) }

// geo/mod.ray   (la cara pública de la cápsula)
pub from geo/formas/circulo import Circulo, area;

// main.ray
import geo;                       // direccionable: carga geo/mod.ray
fn main() -> int {
    let c = geo.Circulo { radio: 4 };   // solo lo que mod.ray reexporta
    geo.area(c)
}
```

`import geo;` carga `geo/mod.ray` (su módulo de identidad es `geo`). La novedad de sintaxis es una
sola: **`pub from … import …`** — un `from`-import marcado `pub` que, además de traer los nombres al
ámbito de `mod.ray`, los **añade a la superficie pública** de `geo`. Así la cápsula expone una
**fachada** (`geo.Circulo`, `geo.area`) y oculta su estructura interna (`geo/formas/circulo`,
`geo/util`).

La pieza de implementación que lo hace posible: la **superficie pública** de cada módulo deja de ser
un *conjunto de nombres* y pasa a ser un mapa **nombre → global de destino**. Para un `pub`
**definido**, el global es el de siempre (`geo::formas::circulo::area`). Para un **reexport**, el
global es el del ítem **en su módulo de origen** —que se define en *otro* sitio—, resuelto a punto
fijo para encadenar reexports. Al acceder `geo.area`, el resolutor consulta esa superficie y baja
directo al global real. Todo en el loader; el runtime no se entera.

### El borde que aísla (M11.6b)

Hasta aquí la fachada es **ergonomía**: nada impide que alguien de fuera escriba todavía
`import geo/formas/circulo;` y se salte el `mod.ray`. M11.6b lo convierte en una **barrera**: importar
un submódulo **interno** desde fuera de su cápsula es un **error**.

```text
$ raylang app.ray
[app] el módulo 'geo/formas/circulo' es interno a la cápsula 'geo'; impórtalo con 'import geo;'
  1 | import geo/formas/circulo;
    | ^
```

La regla, por cada arista de `import`: se busca el **directorio-ancestro más cercano** del objetivo
que tenga un `mod.ray` (la cápsula `C` que lo envuelve); si el importador no está **dentro** de `C`
(no es la propia cápsula ni vive bajo `C/`), se rechaza. *Dentro* de la cápsula los submódulos se
siguen importando entre sí por ruta sin restricción. Las cápsulas anidadas componen solas: estar
dentro de la interior implica estar dentro de la exterior, así que basta comprobar la más cercana.
Todo es una comparación de rutas en el loader —el runtime nunca ve un módulo—.

## Lo que queda fuera (a propósito)

- **Imports relativos** (`import ./util;`), `pub` granular (por campo) → futuro.

> La lección de M11.3 es que una *feature* que parece muy de "sistema" —cargar archivos, espacios
> de nombres, visibilidad— cabe entera en el front-end si ya tienes la disciplina del **erasure**.
> No se tocó ni el checker, ni el intérprete, ni la VM: el oráculo siguió verde sin enterarse de
> que ahora un programa puede vivir en diez archivos.
