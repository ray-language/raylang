# Traits: comportamiento polimórfico

Hasta M8, en raylang los **datos** viven en `struct` y `enum`, y los "métodos" son
funciones libres que UFCS (M7) deja invocar con sintaxis de punto (`p.norma()`). Pero
falta una pieza del polimorfismo: no hay forma de **programar contra una abstracción**.
No podemos decir "cualquier tipo que sepa *mostrarse*" y escribir código que valga para
todos ellos. Eso es un **trait**: la *interfaz* (Java/Go), la *typeclass* (Haskell), el
*protocol* (Swift) de otros lenguajes.

M9 introduce traits estilo Rust. La decisión, fijada desde el inicio del proyecto, es
clara: **datos en structs/enums, comportamiento en traits**. No clases con herencia.
Composición sobre herencia, despacho estático por defecto, e integración limpia con lo
que ya existe (UFCS y genéricos).

## Qué resuelve un trait

Un `trait` es un **contrato**: un conjunto de firmas de métodos que un tipo debe proveer
para "ser" ese trait. Un `impl Trait for Tipo` cumple el contrato para un tipo concreto.

```rust
trait Mostrable {
    fn show(self) -> string;     // el contrato: una firma, sin cuerpo
}

impl Mostrable for Punto {
    fn show(self) -> string {     // el cumplimiento, para Punto
        "un punto"
    }
}
```

El dato (`Punto`) y el comportamiento (`impl`) viven **separados**. Un mismo tipo puede
implementar varios traits; un trait puede implementarse para tipos que no escribiste tú
(incluso primitivos como `int`). Esa separación es la **primitiva de desacople** del
lenguaje: programar contra un trait, no contra un tipo concreto, es lo que permite
intercambiar implementaciones.

## La descomposición de M9

Traits es un salto grande, así que se construye en tres sub-fases:

- **M9.1 — trait + impl con despacho estático.** El corazón: declarar contratos,
  implementarlos para tipos concretos, y resolver `recv.metodo()` en tiempo de chequeo
  cuando se conoce el tipo del receptor. Es **front-end puro**: el runtime no cambia.
- **M9.2 — bounds de genéricos** (`fn f<T: Mostrable>(x: T)`). Aquí aparece el reto de
  fondo: despachar un método sobre un `T` que en runtime ya no existe (*erasure*).
- **M9.3 — métodos por defecto y trait objects** (despacho dinámico).

## El hilo conductor: despacho estático = elegir en tiempo de chequeo

La idea que hace a M9.1 encajar tan limpio es que el **despacho estático** —elegir la
implementación correcta cuando ya conoces el tipo concreto— es justo lo que raylang **ya
hace** con UFCS. Un trait añade el *contrato* (qué métodos, qué firmas) y la *agrupación*
(varios impls para varios tipos), pero el mecanismo de despacho es el mismo: el checker
sabe que `p` es un `Punto`, busca el `impl`, y reescribe la llamada a una función
ordinaria. El runtime nunca se entera de que hubo un trait.

El polimorfismo *de verdad* —resolver el método sin conocer el tipo concreto— llega con
los bounds (M9.2) y los trait objects (M9.3), y es ahí donde el runtime entrará en juego.
M9.1 se queda, deliberadamente, del lado donde todo se resuelve antes de ejecutar.
