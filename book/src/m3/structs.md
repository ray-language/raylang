# Structs

Si el arreglo agrupa **muchos valores del mismo tipo**, el struct agrupa **pocos
valores de tipos distintos, con nombre**. Un `Punto` tiene un `x: int` y un
`y: int`; un `Rect` tiene un `origen: Punto` y un `ancho: int`. Es el segundo —y
último— tipo compuesto de M3, y el que más se parece a "modelar un dominio".

## Declaración y tipado nominal

Un struct se declara a **nivel superior**, como una función:

```rust
struct Punto { x: int, y: int }
```

A diferencia de los arreglos, los structs usan tipado **nominal**: el tipo es el
**nombre**, no la forma. Dos structs con campos idénticos pero nombres distintos
**no** son intercambiables:

```rust
struct Punto  { x: int, y: int }
struct Vector { x: int, y: int }
// un Punto NO es un Vector, aunque tengan la misma forma
```

Por eso `Type::Struct` guarda solo el nombre —`Struct(String)`— y se compara por
nombre. Es la decisión correcta para datos con significado: un punto y un vector
*son* cosas distintas aunque se midan igual.

## La pre-pasada del checker

Los structs se registran en una **pre-pasada**, exactamente como las firmas de las
funciones. ¿Por qué? Porque un struct puede mencionar a otro antes de que aparezca
en el archivo:

```rust
struct Rect { origen: Punto, ancho: int }   // usa Punto…
struct Punto { x: int, y: int }             // …definido más abajo
```

El checker primero recorre **todas** las declaraciones y las guarda en un mapa
`nombre → campos` (detectando duplicados y validando que los tipos de los campos
existan), y solo **después** revisa los cuerpos. Es la misma estrategia de dos
pasadas que ya usábamos para permitir funciones mutuamente recursivas: separar
*"qué existe"* de *"qué hace"*.

## Literal, acceso y mutación

```rust
let p = Punto { x: 1, y: 2 };   // literal: TODOS los campos, nombrados
p.x                             // acceso  → ExprKind::Field
p.x = 5;                        // mutación → Assign a un Field
```

El checker es estricto con el literal: deben estar **todos** los campos, **ninguno**
de más, y cada uno con el tipo correcto. No hay valores por defecto ni campos
opcionales (no hay `null` en raylang, por diseño).

El acceso `p.x` se parsea con el mismo `.` que un día servirá para UFCS (`x.trim()`),
y en el mismo bucle posfijo que `[i]` y `(...)`. Y la mutación `p.x = 5` **cayó por
gravedad**: como la asignación ya aceptaba cualquier expresión-lvalue, bastó con
sumar `Field` a la lista de lvalues válidos. La generalización que hicimos pensando
en los arreglos pagó por segunda vez.

## En el runtime: el orden importa

`Value` gana `Struct(Rc<RefCell<StructInstance>>)`, con la misma semántica de
referencia que los arreglos:

```rust
struct StructInstance {
    name: String,
    fields: Vec<(String, Value)>,
}
```

Hay una sutileza que costó decidir: **¿en qué orden se guardan los campos?** El
literal podría escribirlos en cualquier orden (`Punto { y: 2, x: 1 }`), pero si dos
instancias "iguales" guardaran sus campos en orden distinto, la **igualdad
estructural** (`==`) y la impresión fallarían. La regla: **ambos motores construyen
los campos en el orden de la declaración**, no en el del literal. Así `Punto { x: 1,
y: 2 }` y `Punto { y: 2, x: 1 }` son idénticos bit a bit.

### La VM y la tabla de structs

Aquí aparece una decisión de implementación elegante. En la VM, ¿cómo sabe el
opcode `MakeStruct` el orden de declaración, si en el bytecode no hay tipos?

La respuesta: el compilador construye una **tabla de structs** en el
`CompiledProgram` —cada struct con su lista de campos en orden— y le asigna un
índice. El literal compila a:

```text
[valor del campo 0]
[valor del campo 1]
…
MakeStruct(idx)      ; idx en la tabla de structs
```

`MakeStruct(idx)` mira la tabla, saca tantos valores como campos, y los empareja **en
orden de declaración**. El acceso, en cambio, va **por nombre**: el opcode lleva el
nombre del campo (`GetField("x")`, `SetField("x")`).

> **Por qué por nombre, y no por índice.** Acceder por nombre evita que el
> *compilador* tenga que inferir el tipo del objeto para resolver `p.x` a una
> posición. El compilador no sabe (ni quiere saber) que `p` es un `Punto`; emite
> "busca el campo `x`" y deja que la VM lo encuentre. Es un poco más lento que un
> índice fijo, pero mantiene al compilador simple e independiente del checker —una
> optimización que M-posteriores pueden hacer si hace falta.

Los tres opcodes nuevos:

| OpCode | Hace |
|--------|------|
| `MakeStruct(idx)` | saca N campos de la pila y arma la instancia en orden de declaración |
| `GetField(nombre)` | saca un struct, empuja el valor del campo |
| `SetField(nombre)` | saca struct y valor; escribe el campo |

## El oráculo, otra vez

Los tests corren en ambos motores structs anidados, mutación de campos, y el caso
estrella de la semántica de referencia: `r.origen.x = 99` cambiando el `Punto`
compartido y viéndose a través de `p`. VM e intérprete coinciden en todo.

## M3 completo

raylang ya no solo calcula: **modela**. Tiene arreglos dinámicos y structs
nominales, ambos con semántica de referencia, en sus dos motores de ejecución. Con
**79 tests verdes**, el oráculo certifica que la VM sigue siendo una implementación
fiel del lenguaje, ahora también para los datos compuestos.

Esto desbloquea cosas que dejamos anotadas: `args()` para CLIs (un `[string]`),
el terreno para `@derive`, y prerequisitos del self-hosting. Pero arrastra una deuda
explícita: el `Rc` filtra ciclos. El siguiente hito, **M4 (closures + recolección
de basura)**, sustituye ese conteo de referencias por una GC real —y de paso le da a
raylang funciones que capturan su entorno.

> Código: `src/ast.rs` (`StructDef`, `Type::Struct`, `StructLit`, `Field`),
> `src/checker.rs` (pre-pasada, `check_struct_lit`, `check_field`),
> `src/interpreter.rs` (`StructInstance`), `src/bytecode.rs` (`CompiledStruct`),
> `src/compiler.rs` (tabla de structs) y `src/vm.rs` (`MakeStruct`/`GetField`/`SetField`).
