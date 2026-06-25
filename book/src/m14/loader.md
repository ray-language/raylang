# El loader: juntar módulos

El pipeline auto-alojado de M14.4 procesa **un solo archivo**. Pero el compilador se reparte en
varios (`from lexer import …`, `from parser import …`), y los drivers importan unos de otros. Falta la
pieza que ya teníamos en Rust: el **loader**, que aplana la entrada y sus dependencias en un único
`Program` plano. `selfhost/loader.ray` es un port **recortado** de `src/loader.rs`.

## Dos simplificaciones frente a Rust

El loader de Rust es grande: cápsulas, módulos por directorios, reexports, desplazamiento de
posiciones. El caso del self-hosting permite podar mucho.

**1. Solo `from M import …`.** Los módulos del compilador no usan `import M;` con acceso calificado
(`M.f()`), ni directorios, ni cápsulas, ni reexports. Basta con `from M import a, b, …` (de funciones
**y** tipos).

**2. Sin desplazamiento de posiciones.** Esta es la simplificación bonita. El loader de Rust desplaza
cada módulo a una **banda de líneas disjunta**, porque el checker de Rust **baja por posición** (el
lowering de M9 indexa los sitios por `(línea, col)`, y dos módulos en la misma posición colisionarían).
Pero el checker auto-alojado es un **validador** (no baja nada) y el intérprete despacha por **etiqueta
de valor** (no por posición). Para programas **válidos** —lo que ejecutamos— las posiciones son
irrelevantes al comportamiento. Así que el loader auto-alojado **no desplaza nada**.

## Qué hace `load`

```rust
pub fn load(entry_path: string) -> Result<Program, LoadError> { /* … */ }
```

En cuatro fases:

1. **BFS de dependencias.** Lee la entrada con `read_file` (M14.6d), la lexea+parsea, sigue sus
   `from`-imports (transitivos, ciclos seguros con un `Map` `visited`), resolviendo cada módulo a
   `dir(entrada)/dep.ray`.
2. **Superficies públicas.** Por módulo, qué exporta: cada función/tipo `pub` → su nombre global
   `modulo::nombre`. (El `::` es ilegal en identificadores del usuario, así que nunca choca.)
3. **Resolución.** El **Resolver** reescribe las referencias de **valor** (un `foo` propio →
   `modulo::foo`); el **TypeRewriter** reescribe las referencias de **tipo**. Ambos son *conscientes
   de ámbitos*: una variable local tapa a una función de nivel superior, un parámetro de tipo `T` no
   se confunde con un tipo nominal.
4. **Fusión.** Renombra las definiciones a su nombre global y junta todo en un `Program` plano.

## El cruce de funciones y de tipos

El Resolver (valores) y el TypeRewriter (tipos) son dos pasadas separadas. El Resolver recorre los
cuerpos reescribiendo cada `EIdent` que nombre una función propia o `from`-importada:

```rust
EKind.EIdent(name) => {
    if (!es_local(r, name)) {
        match (get(r.own, name)) { Option.Some(g) => { e.kind = EKind.EIdent(g); }, Option.None => {} }
    }
},
```

El TypeRewriter renombra las **definiciones** de tipo a `modulo::Tipo` y reescribe **todas** las
referencias: en posiciones de tipo (anotaciones, campos, payloads, target/trait de `impl`, bounds,
`dyn`, args de genérico) y en expresiones que **nombran** tipos (literal de struct, construcción de
enum `Tipo.Variante` —que llega como `Field`/`Call`—, patrones de `match`). El parser auto-alojado
emite `TNamed` para **todo** identificador en posición de tipo (incluido `Map` y los parámetros `T`);
el rewriter deja igual los que no encuentra en su mapa, así que cubre todos los casos sin lógica
especial.

Esta mutación in-place del AST se apoya, una vez más, en la **semántica de referencia del host**: los
nodos `Expr`/`Func` son structs compartidos, y reescribir un campo muta el nodo real.

## Verificado

El oráculo (conductual) corre programas multi-archivo por ambos pipelines: cruce de funciones (con
alias, *shadowing* local, cadenas transitivas A→B→C, función-como-valor) y cruce de tipos (struct +
enum con construcción y `match`, trait + impl + struct genérico + `dyn`, alias de tipo). Todos dan el
mismo `stdout` + código de salida que Rust. Y un archivo único, sin `from`-imports, pasa por el loader
como **identidad** —cero regresión—.

> **Por qué importa.** Con el loader, el compilador auto-alojado puede al fin cargar **sus propios
> módulos**. Solo falta un detalle de plomería —`args()`— y un puñado de builtins, y podremos hacer lo
> que da nombre a este módulo: correr el compilador sobre sí mismo.
