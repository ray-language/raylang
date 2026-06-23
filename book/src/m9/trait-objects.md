# Trait objects: despacho dinámico

Hasta aquí, todo el despacho de raylang —impls concretos (M9.1), bounds (M9.2), defectos
(M9.3a)— se resuelve **en tiempo de chequeo**: el checker siempre sabe el tipo concreto y
elige la función. Pero a veces el tipo concreto **no se conoce hasta runtime**:

```rust
let figuras: [dyn Figura] = [Cuadrado { lado: 3 }, Rect { ancho: 4, alto: 5 }];
//                           ^^^^^^^^ un Cuadrado y un Rect en el MISMO arreglo
```

¿De qué tipo es `figuras[i]`? Depende de `i`, que se sabe al ejecutar. Eso es un **trait
object** (`dyn Figura`): un valor cuyo tipo concreto se borra, pero que sabe despachar los
métodos de su trait. Es la última pieza de M9, y la única donde el despacho es **dinámico**.

## El tipo `dyn Trait`

La palabra clave `dyn` introduce el tipo de un trait object. Vale en cualquier posición de
tipo —parámetros, anotaciones, elementos de arreglo, retorno—:

```rust
fn area_total(xs: [dyn Figura]) -> int {
    var s = 0;  var i = 0;
    while (i < len(xs)) { s = s + xs[i].area(); i = i + 1; }   // despacho por valor
    s
}
```

`xs[i].area()` no puede resolverse estáticamente —`xs[i]` es "alguna `Figura`"—, así que se
despacha **en runtime** según el valor concreto que haya en esa posición.

## La representación: un *fat value* que es un struct

Para despachar en runtime, el objeto debe **cargar su propia tabla de métodos** (su
*vtable*). La representación clásica es un *fat value*: el par `(dato, vtable)`. Y aquí viene
la decisión de diseño: en vez de inventar un valor nuevo en runtime, raylang realiza ese fat
value como un **struct sintetizado**:

```text
dyn Figura   se representa, en runtime, como:

   __dyn_Figura {
       data:   <el valor concreto>,     // p. ej. un Cuadrado
       area:   <Cuadrado#area>,         // la vtable: una función por método
       nombre: <Cuadrado#nombre>,
   }
```

Como raylang ya tiene **structs** (con su construcción, acceso a campo y trazado del GC) y
**funciones de primera clase** (M4), un trait object no necesita **nada nuevo en el runtime**:
es un struct que guarda el dato y sus funciones. Cero opcodes, cero cambios en el GC. Fiel al
tema de todo M9: el polimorfismo se construye sobre las piezas que ya existían.

## Coerción: construir el objeto

Cuando un valor **concreto** que implementa el trait fluye a una posición `dyn Trait` —un
argumento, un elemento de `[dyn Trait]`, un `let` anotado—, el checker inserta una
**coerción** que construye el struct, fijando la vtable con los métodos del tipo concreto
(los mangulados de M9.1, incluidos los defectos de M9.3a):

```text
[Cuadrado { lado: 3 }, Rect { ... }]   con tipo esperado [dyn Figura]
        │
        ▼  (coerción de cada elemento)
[ __dyn_Figura { data: Cuadrado{...}, area: Cuadrado#area, nombre: Cuadrado#nombre },
  __dyn_Figura { data: Rect{...},     area: Rect#area,     nombre: Rect#nombre     } ]
```

La vtable se fija **en la coerción**, donde el tipo concreto aún se conoce. Por eso, aunque
el despacho ocurra en runtime, *qué* funciones viajan en cada objeto se decidió
estáticamente. Si un tipo que no implementa el trait intenta coercionarse, es error.

## Despacho: leer la vtable

`obj.m(args)` con `obj: dyn Trait` se baja a llamar el campo-método con el `data` como
receptor. Para no evaluar `obj` dos veces (podría tener efectos), se usa un temporal:

```rust
obj.area()
   │
   ▼
{ let r = obj;  (r.area)(r.data) }    // r.area es la función; r.data, el receptor
```

Todo son accesos a campo y llamadas ordinarias: el intérprete y la VM despachan sin saber
que existe un trait object. El oráculo VM↔intérprete sigue valiendo sin tocar `vm.rs`.

## *Object safety*

Una vtable no puede llevar métodos que dependan del tipo concreto borrado. Si un método usa
`Self` fuera del receptor —`fn copia(self) -> Self`, `fn igual(self, otro: Self)`—, no hay
forma de tipar su resultado sobre un objeto cuyo tipo se perdió. Esos métodos **no son
invocables** sobre un `dyn Trait`:

```text
error de tipos: el método 'copia' usa 'Self': no es invocable sobre 'dyn Clon'
```

El resto de métodos —los de firma concreta— sí. Es la *object safety* de Rust, en pequeño.

## El cierre de M9

Con los trait objects, raylang completa su historia de polimorfismo:

| | Cuándo se elige la función | Toca el runtime |
|---|---|---|
| **impl concreto** (M9.1) | en chequeo (tipo conocido) | no |
| **bounds** (M9.2) | en chequeo (en el sitio de llamada) | no |
| **defectos** (M9.3a) | en chequeo (síntesis) | no |
| **trait objects** (M9.3b) | en **runtime** (por la vtable del valor) | no* |

\* *ni siquiera el despacho dinámico necesitó runtime nuevo: un trait object es un struct que
carga sus funciones.* Esa es la lección de M9 — desde los impls hasta el despacho dinámico,
el polimorfismo se construyó enteramente sobre genéricos *erasure*, structs y funciones de
primera clase, sin una sola primitiva nueva en los motores de ejecución.
