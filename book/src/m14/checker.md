# El checker auto-alojado

El tercer eslabón le da **significado** al AST: comprueba tipos. `selfhost/checker.ray` es la fase
más conceptual de M14, porque aquí se materializa la **decisión 2**: el checker auto-alojado es un
**validador**, no un compilador completo.

## Validador, no transformador

El checker de Rust hace dos trabajos: (1) comprueba tipos y (2) **baja** (*lowers*) el azúcar de M9
—UFCS, diccionarios de bounds, trait objects, construcción de enums— a construcciones simples que el
runtime entiende. El checker auto-alojado hace **solo (1)**: recorre el AST y produce un veredicto.

```text
ok
error de tipos en 12:7: no se puede asignar bool a una variable int
```

Eso es todo lo que emite. El oráculo es de **veredicto**: la misma fuente por ambos pipelines, y se
compara ese texto byte a byte —sobre un corpus de programas válidos **y** inválidos—. El lowering,
que el intérprete de M14.4 hará en runtime, simplemente no existe aquí.

Esta poda es lo que hace **abordable** portar un checker de miles de líneas: nos quedamos con la
lógica de tipos pura.

## La forma del checker

El `Type` del parser dobla como tipo inferido; el checker añade `type_eq`/`type_str` propios. Dos
pasadas (firmas → cuerpos), una pila de ámbitos modelada con `Map`:

```rust
struct Checker {
    funcs: Map<string, FnSig>,
    structs: Map<string, [FieldDef]>,
    enums: Map<string, [VariantDef]>,
    traits: Map<string, [MethodSig]>,
    methods: Map<string, FnSig>,        // Tipo#metodo
    scopes: [Map<string, VarInfo>],
    /* … */
}
```

Aquí `Map` (M13.1) deja de ser una comodidad y pasa a ser **necesario**: las tablas de símbolos, de
métodos manglados y de ámbitos serían listas de búsqueda lineal sin él.

## El prelude, registrado directamente

En Rust el prelude (Option/Result, Eq/Show/Ord, map/filter/fold) se **parsea** de un fuente raylang y
se inyecta en el programa. Pero el checker auto-alojado es un validador que solo recibe el AST: no
necesita los cuerpos del prelude, solo sus **firmas**. Así que las **registra directamente**.

```rust
// Option<T>/Result<T,E> como enums genéricos conocidos
insert(c.enums, "Option", [VariantDef { name: "Some", payload: [tvar("T")] }, /* None */]);
// map/filter/fold: solo la firma basta para resolver llamadas
inject_fn(c, "map", FnSig { params: [arr(t), fn1(t, u)], ret: arr(u), type_params: ["T","U"], /* … */ });
```

El usuario que declare un tipo o función con ese nombre **gana** (override), igual que en Rust.

## Construido por sub-fases

Como el parser, el checker se levantó por capas, cada una contra su oráculo de veredicto:

- **Núcleo monomórfico** — literales, operadores, variables, llamadas, control, anotaciones,
  divergencia.
- **Datos** — arreglos, structs, enums, `match` con **exhaustividad**, chequeo bidireccional mínimo.
- **Genéricos** — funciones y tipos genéricos, `unify`/`subst`, inferencia, chequeo bidireccional
  completo, Option/Result + `?`.
- **Traits** — UFCS y métodos, `trait`/`impl`, métodos por defecto, **bounds** (satisfacción en el
  sitio, sin paso de diccionarios), impls genéricos, `dyn`, `@derive`, anotaciones.

En cada una, los mensajes de error se construyen **byte-idénticos** a los de Rust —ese es el contrato
del oráculo de veredicto—.

## Bounds sin diccionarios

Un detalle que ilustra "validador, no transformador": en Rust, `x.metodo()` con `x: T` acotado
`T: Trait` se **baja** a una llamada a un parámetro-diccionario oculto. El checker auto-alojado solo
comprueba la **satisfacción**: que el tipo inferido para cada parámetro acotado tenga un `impl` del
trait (o sea un parámetro rígido del llamador con el mismo bound). El veredicto es idéntico; el paso
de diccionarios —el lowering— se omite por completo.

> **Por qué importa.** El checker auto-alojado valida **el lenguaje completo** (núcleo, datos,
> genéricos, traits, bounds, `dyn`, prelude, derive), byte-idéntico a Rust sobre 22 ejemplos reales y
> decenas de casos. Junto al lexer y el parser, la mitad **delantera** del compilador —análisis— está
> escrita en raylang. Queda darle ejecución: el back-end.
