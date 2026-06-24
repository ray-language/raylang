# Map<K, V>: diccionarios

Hasta M11, la única colección de raylang era el arreglo `[T]`. Para un compilador —el
*capstone* del proyecto— eso no basta: una tabla de símbolos (`nombre → tipo`, ámbitos,
`(tipo, método) → manglado`) sin un mapa se haría con listas de asociación de búsqueda lineal,
correctas pero penosas. M13.1 añade **`Map<K, V>`**: el primer tipo compuesto nuevo desde los
enums de M5.

## La decisión central: un objeto del heap, no un almacén del host

Varias features anteriores (los handles de archivo de M11.8, los `args` de M11.2) viven en un
**almacén del proceso** del host: un `int` que indexa un `HashMap` en Rust. Tentaba hacer lo
mismo con `Map`. Pero no encaja: las **claves y los valores son `Value`s de raylang**, y cada
motor los representa distinto —el intérprete con `Rc`, la VM con handles del GC—. Un almacén
compartido en el host no podría guardar valores de ambos.

Así que `Map` sigue el molde de los arreglos (M3): es un **objeto del heap en cada motor**, con
semántica de referencia.

```rust
// intérprete
Value::Map(Rc<RefCell<HashMap<MapKey, Value>>>)
// VM
Obj::Map(HashMap<MapKey, HeapValue>)   // trazado por el GC
```

El **GC traza los valores** del mapa (pueden ser objetos del heap); las **claves no** —son
primitivos *inline*—. Una línea en la función `children` del recolector:

```rust
Obj::Map(m) => m.values().filter_map(HeapValue::handle).collect(),
```

## Claves hashables: `MapKey`

Una clave debe poder *hashearse* y compararse. Restringimos las claves a **primitivos
hashables**: `int`, `string`, `char`, `bool`. Se queda fuera `float` (su `==` no es fiable y no
implementa `Hash`/`Eq` en Rust) y todos los compuestos.

```rust
#[derive(PartialEq, Eq, Hash, PartialOrd, Ord)]
enum MapKey { Int(i64), Str(String), Char(char), Bool(bool) }
```

Esto evita, por ahora, la maquinaria de un trait `Hash` con paso de diccionarios (M9.2) —que
sería lo necesario para claves de tipos del usuario—. Cubre el 99% de un compilador, donde las
claves son nombres (string) o índices (int). Clave genérica → **diferida**.

La restricción se valida en `ensure_type`: una anotación `Map<float, int>` se rechaza con un
mensaje claro. El parser trae `Map<K, V>` como `Struct("Map", [K, V])` (no hay sintaxis especial);
el checker lo **reclasifica** a `Type::Map` en `resolve_type`, igual que reclasifica un nombre a
`Enum` o a `Var`.

## La API

`map_new` es el constructor; el resto son builtins (opcodes nuevos) salvo `get`/`remove`, que
envuelven un primitivo en `Option` —el patrón de M11.2: el runtime no sabe de `Option`—.

```rust
let m: Map<string, int> = map_new();
m.insert("uno", 1);              // insert(m, k, v)
m.insert("uno", 11);            // sobrescribe
m.contains_key("uno")           // true
m.get("uno")                    // Option.Some(11)
m.get("falta")                  // Option.None
len(m)                          // 1
m.remove("uno")                 // Option.Some(11), y lo quita
```

(Todo luce con UFCS: `m.insert(k, v)` es `insert(m, k, v)`.)

### `map_new()` es indeterminado, como `[]`

`map_new()` no recibe argumentos, así que no puede saber su tipo solo. Es el mismo caso que el
arreglo vacío `[]` o `None`: su tipo lo fija el **contexto** (chequeo bidireccional, M6.2).

```rust
let m: Map<string, int> = map_new();   // ✓ el tipo viene de la anotación
let m = map_new();                     // ✗ "no se puede inferir el tipo de map_new; anótalo"
```

### `keys` y `values`: orden determinista

`keys(m)` devuelve las claves y `values(m)` los valores. Un `HashMap` no tiene orden, pero el
**oráculo** exige que el intérprete y la VM den el mismo resultado. La solución: **ordenar por
clave** (de ahí el `Ord` en `MapKey`). `values` se emite en ese mismo orden de clave, así
`keys(m)[i]` y `values(m)[i]` se corresponden.

```rust
let m: Map<int, int> = map_new();
m.insert(3, 30); m.insert(1, 10); m.insert(2, 20);
keys(m)     // [1, 2, 3]
values(m)   // [10, 20, 30]
```

En un mapa concreto todas las claves son del mismo tipo (el checker fija un único `K`), así que el
orden entre variantes de `MapKey` nunca se observa.

## El precio que sí se pagó

`Map` fue la primera feature de M13 que **tocó el runtime y el GC**, no solo el front-end. El
patrón de la limpieza L1 —cada builtin es una fila en la tabla + un opcode + una impl por motor—
mantuvo el coste acotado, pero hubo que: añadir `Type::Map` y propagarlo por todos los recorridos
de tipos del checker (`resolve_type`, `subst`, `unify`, `ensure_type`…), un valor por motor, el
trazado del GC, y la conversión en el borde (`to_value`). Como toda forma del heap, se verifica con
el oráculo en **modo estrés** del GC: si una raíz faltara, un valor guardado en el mapa se liberaría
y el resultado cambiaría.

`print` de un `Map` queda **diferido** (no es *printable*): imprimir un mapa con orden no
determinista rompería el oráculo, y `Show` derivable para `Map` es trabajo aparte.

> **Por qué importa.** Con `Map`, las tablas de símbolos dejan de ser listas lineales. Es la
> pieza que vuelve *práctico* —no solo posible— escribir el compilador de raylang en raylang.
