# El intérprete auto-alojado

El back-end cierra el pipeline: ejecutar el AST validado. `selfhost/interprete.ray` es un port del
intérprete tree-walking de Rust, y es donde brillan las decisiones 1 y 3 (intérprete, y resolución en
runtime). Su lema: **cabalgar sobre el host**.

## Cabalgar sobre el host

El intérprete de Rust necesita maquinaria pesada: un `Value` con `Rc<RefCell>` para la semántica de
referencia, celdas para las closures, un GC. El intérprete auto-alojado **no implementa nada de
eso**: lo toma prestado de la VM anfitriona.

El `Value` del intérprete auto-alojado es **un enum de raylang**, que vive en el heap del motor que lo
ejecuta. Su GC lo recolecta. Su semántica de referencia es la de los arreglos/structs de raylang.

```rust
pub enum Value {
    VInt(int), VFloat(float), VBool(bool), VStr(string), VChar(char), VUnit,
    VArray([Value]), VStruct(string, [SField]), VEnum(string, string, [Value]),
    VFunc(Func), VClosure(FnExpr, [Capture]), VMap(MapData),
}
```

¿Y las **celdas** de las closures (M4.2), ese `Rc<RefCell<Value>>` que el intérprete de Rust necesita
para que dos closures compartan una variable mutable? En raylang, un `struct` de un campo **ya es** una
celda mutable compartida:

```rust
struct Cell { v: Value }      // un struct de raylang = el Rc<RefCell> de Rust, gratis
```

Los ámbitos son `Map<string, Cell>`. `define` crea una celda nueva (shadowing), `assign` **muta** la
celda (las closures ven el cambio), `lookup` lee `cell.v`. Ni GC propio, ni celdas propias: el host
los regala.

## Resolución en runtime = borrado gratis

Aquí se cobra la decisión 3. Como el checker auto-alojado no bajó nada, el intérprete resuelve en
**tiempo de ejecución**, mirando la etiqueta del valor. El corazón es `dispatch_method`, que resuelve
`recv.f(args)` por orden:

```text
campo-función del struct  →  método (tabla Tipo#metodo)  →  @derive (igual/mostrar)
                          →  UFCS a función libre f(recv, args)  →  builtin como método
```

La consecuencia es preciosa: **`dyn`, los bounds y los genéricos son no-ops**. El intérprete nunca
consulta un tipo —despacha por la etiqueta del valor concreto—, así que el *borrado de tipos* ocurre
**solo**, sin ninguna pasada de lowering. Un "trait object" no es más que el valor concreto; `[dyn T]`
es un arreglo de concretos. Esto diverge del intérprete de Rust (que es directo porque el lowering ya
pasó), pero el oráculo es **conductual**, así que la diferencia interna no se ve.

## El oráculo conductual

Un AST ejecutado no se vuelca como texto: se **comporta**. El oráculo compara el `stdout` (las
salidas de `print`) y el **código de salida** (el `int` que devuelve `main`, enmascarado a 8 bits),
corriendo la misma `.ray` por ambos pipelines:

```text
raylang fuente.ray                  ──> (stdout, exit)
raylang selfhost/run.ray fuente.ray ──> (stdout, exit)   # corre sobre el intérprete auto-alojado
                                          assert iguales
```

El corpus son los ejemplos **deterministas** (la I/O no determinista se excluye, como en los tests de
I/O de Rust). El driver es `selfhost/run.ray`: lexea, parsea, chequea y **ejecuta** el archivo de
punta a punta con el compilador en raylang.

## Por sub-fases

Igual que parser y checker:

- **Núcleo** — primitivos, aritmética, control, llamadas y recursión.
- **Datos** — arreglos/structs/enums/`match`, donde la **semántica de referencia del host** luce:
  `r.origen.x = 99` muta el struct compartido, porque son el `[SField]` del host.
- **Primera clase** — closures (las celdas), orden superior, Option/Result + `?`.
- **Despacho dinámico** — la tabla de métodos, UFCS/métodos/`dyn`/`@derive`/bounds, todo por etiqueta.

## El prelude, de vuelta a raylang

El checker no inyecta los cuerpos del prelude (solo firmas), pero el intérprete los **necesita** para
ejecutar `map`/`filter`/`fold`. La solución replica el `check()` de Rust: `selfhost/prelude.ray` trae
esas funciones escritas en raylang, y `run.ray` las **fusiona** en el programa del usuario (las que no
redefina). Fusionadas, son funciones ordinarias: `xs.map(f)` cae en la rama UFCS del despacho.

> **Por qué importa.** Con el intérprete, el pipeline auto-alojado está **completo**: raylang
> lexea/parsea/chequea/ejecuta raylang. Verificado: los ejemplos corren idénticos por ambos caminos.
> Falta una pieza para juntarlo todo sobre programas multi-archivo —el loader— y entonces llega el
> premio: correr el compilador sobre sí mismo.
