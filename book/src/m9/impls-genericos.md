# Impls genéricos: diccionarios anidados

M9.1 implementa un trait para un tipo **concreto** (`impl Mostrable for Punto`). M9.2b da el
salto a un **constructor de tipos**: implementar un trait para toda la familia `Caja<T>`,
opcionalmente condicionado a que `T` también cumpla un trait.

```rust
impl<T> Contar for Caja<T> { fn contar(self) -> int { 1 } }      // para cualquier T
impl<T: Medir> Medir for Caja<T> {                               // si T es Medir
    fn medir(self) -> int { self.contenido.medir() + 1 }
}
```

Es lo que vuelve los traits **composicionales** sobre contenedores: la base de un `Iterable<T>`,
de comparar listas, de envolver cualquier cosa medible. Y, fiel al patrón del proyecto, casi
todo se reduce a maquinaria que ya existía.

## La idea: un método de impl genérico *es* una función genérica acotada

Recuerda cómo M9.1 baja un método: cada `fn` de un `impl` se reescribe a una función ordinaria
con nombre manglado (`Caja#medir`) y `self` del tipo implementador. Y cómo M9.2 baja un bound:
una función con `T: Trait` gana parámetros-diccionario ocultos.

M9.2b junta las dos. El método de `impl<T: Medir> Medir for Caja<T>` se baja a una función
manglada que **hereda los `type_params` y `bounds` del impl**:

```text
impl<T: Medir> Medir for Caja<T> { fn medir(self) -> int { self.contenido.medir() + 1 } }
        │
        ▼  (paso 0c, ahora con type_params=[T] y bounds=[T: Medir])
fn Caja#medir<T: Medir>(self: Caja<T>) -> int { self.contenido.medir() + 1 }
```

A partir de ahí, **todo lo demás de M9.2 aplica solo**: `append_dict_params` le añade su
parámetro-diccionario `T#Medir#medir`, y dentro del cuerpo `self.contenido.medir()` (con
`contenido: T` acotado) se baja a una llamada a ese diccionario. No hizo falta código nuevo
para el cuerpo: es una función con bounds más.

## Resolver la instancia: la clave es el constructor

¿Cómo despacha `caja.medir()` con `caja: Caja<int>`? La tabla de métodos se indexa por el
**constructor** del tipo (`Caja`), no por sus argumentos. Así que `caja.medir()` encuentra
`Caja#medir`; como ahora es genérica, la inferencia de M6 deduce `T = int` y el sitio de
llamada registra el diccionario que necesita (el de `int`). La llamada directa funciona reusando
el camino de M9.2.

> *Alcance.* Solo impls **plenamente genéricos**: el objetivo es `Caja<T>` (sus propios
> parámetros), no `Caja<int>`. Un impl por `(constructor, trait)` —sin instancias solapadas ni
> especializadas, que se difieren—.

## El caso nuevo: diccionarios anidados

Todo lo anterior se apoyaba en M9.2. El punto genuinamente nuevo aparece al pasar un `Caja<int>`
a **otro** genérico acotado:

```rust
fn medir_dos<X: Medir>(a: X, b: X) -> int { a.medir() + b.medir() }
medir_dos(c, c)        // c: Caja<int>
```

El diccionario de `Caja<int>` que hay que pasar a `medir_dos` ya **no es una función plana**.
`Caja#medir` espera `(self, «int#medir»)` —dos argumentos, porque su impl está acotado—, pero
dentro de `medir_dos` se le llamará con uno solo (`«X#Medir#medir»(a)`). La aridad no casa.

La solución es pasar un **closure que captura el diccionario interno** y presenta la aridad que
el llamador espera:

```rust
medir_dos(c, c, fn(__d0: Caja<int>) -> int { Caja#medir(__d0, int#medir) })
//                                                       └── el dict interno, capturado
```

Ese closure-que-cierra-sobre-otro-diccionario **es** el diccionario anidado. Y, una vez más,
**los closures de M4 ya hacen exactamente eso**: capturan valores de su entorno (aquí,
`int#medir`) y se pasan como cualquier función. Cero opcodes, runtime intacto.

La síntesis es **recursiva**: si el interior fuera a su vez un impl genérico acotado —`Caja<Caja<int>>`—
el diccionario capturado sería *otro* closure. La recursión sigue la estructura del tipo.

```text
Caja<Caja<int>>   →   fn(d) { Caja#medir(d, «el dict de Caja<int>») }
                                              │
                                              └─ fn(e) { Caja#medir(e, int#medir) }
```

## Un detalle de plomería: renumerar los fn-exprs

El intérprete y la VM identifican cada función anónima por un `id` denso (`0..N`). Como el
*lowering* **inyecta** estos closures sintéticos después de parsear, sus ids llegan provisionales;
una pasada final (`renumber_fn_exprs`) recorre el AST y los reasigna densos. Es el precio de
generar funciones nuevas tras el parser —pequeño y contenido—.

## Runtime: sin cambios (otra vez)

Como M9.1 (funciones mangladas), M9.2 (diccionarios) y M9.3 (structs sintetizados / closures),
M9.2b **no toca los motores**: los diccionarios anidados son closures, y los closures existen
desde M4. El oráculo VM↔intérprete —incluido el modo estrés del GC, que aquí importa porque los
closures son objetos del heap— sigue valiendo sin tocar `vm.rs`.

La lección se repite con fuerza: cuando las capas están bien puestas —genéricos con inferencia
(M6), funciones de primera clase y closures (M4), el paso de diccionarios (M9.2)—, la feature
"siguiente" que sonaba grande (impls genéricos) resulta, sobre todo, **composición de lo que ya
había**. El único ingrediente realmente nuevo fue *un closure que captura un diccionario*.
