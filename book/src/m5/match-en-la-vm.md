# match en la máquina virtual

El intérprete ejecuta `match` recorriendo brazos y comparando etiquetas. La VM no tiene
ese lujo: trabaja con bytecode plano y una pila. M5.3 **baja** `match` a una secuencia
de instrucciones —y con eso los dos motores vuelven a coincidir bajo el oráculo—.

## El plan: una cadena de decisión

Un `match` se compila a una **cadena de pruebas**: probar la etiqueta del primer brazo;
si no casa, saltar al siguiente; si casa, ligar su payload, ejecutar el cuerpo y saltar
al final. Es la misma forma que una cascada de `if`/`else if`, reusando los saltos
(`Jump`/`JumpIfFalse`) que la VM ya tenía desde M2.

Tres instrucciones nuevas hacen el trabajo específico de enums:

- `EnumTagEq(tag)` — saca un enum de la pila y empuja un bool: ¿es esta la variante?
- `GetEnumField(i)` — saca un enum y empuja el valor en la posición `i` de su payload.
- `MatchFail` — un *trap* defensivo para el "ningún brazo casó". Inalcanzable, por la
  exhaustividad que el checker ya probó; está por si un bug del compilador lo alcanzara.

## El escrutinio, en un local temporal

Hay una decisión clave en cómo se compila. El escrutinio se evalúa **una sola vez** y
se guarda en un **local temporal** (lo llamamos `$match`, un nombre que ningún
identificador válido puede chocar). ¿Por qué un local, y no dejarlo en la pila?

Porque se necesita **muchas veces**: para leer su etiqueta en cada brazo, y para
extraer cada campo de su payload. Tenerlo en un slot permite leerlo con `GetLocal`
cuantas veces haga falta, sin reevaluar la expresión (que podría tener efectos) y sin
malabares de pila.

Y hay un segundo motivo, más sutil: **el GC**. Mientras el `match` corre, el escrutinio
debe seguir vivo —su payload se está extrayendo—. Guardado en un local del marco, es
una **raíz**: el recolector lo ve y no lo libera. Si se quedara flotando en una
variable temporal de Rust, sería invisible para el marcado. La lección de los *puntos
seguros* de M4.3 vuelve a pagar.

Así se compila cada brazo de variante:

```text
GetLocal($match)      ; el escrutinio
EnumTagEq(tag)        ; ¿es esta variante? -> bool
JumpIfFalse(siguiente); si no, al siguiente brazo
Pop                   ; descartar el bool
GetLocal($match)      ; para cada sub-binding:
GetEnumField(0)       ;   extraer payload[0]
InitLocal(slot)       ;   ligarlo a una local
... (cuerpo del brazo) ...
Jump(fin)
siguiente:
Pop                   ; descartar el bool
... (siguiente brazo) ...
fin:
```

## Ligar el payload reusa el boxing de M4

Cuando un brazo liga `Cons(h, t)`, `h` y `t` son locales nuevas —y se declaran con el
mismo `InitLocal` que usa un `let`—. Esto no es un detalle menor: si una **closure**
dentro del cuerpo del brazo captura `h`, su slot debe **boxearse** (vivir en una
celda), exactamente como en M4.2.

```rust
fn sumador(e: E) -> fn(int) -> int {
    match (e) {
        E.A(n) => fn(x: int) -> int { x + n },   // la closure captura n, ligado por el patrón
        E.C    => fn(x: int) -> int { x },
    }
}
```

Que `InitLocal` ya supiera boxear los slots capturados significó que ligar un patrón
*es* declarar una local, sin código especial. Una pieza de M4 encaja en M5 sin
retoques —la señal de que la abstracción estaba bien puesta—.

## El oráculo vuelve

Mientras `match` solo vivía en el intérprete (M5.2), el oráculo no podía cubrirlo: no
había con qué comparar. Con M5.3, los programas con `match` corren en los **dos**
motores, y los tests `oracle_*` exigen que el resultado coincida —recorrer una lista,
seleccionar un brazo, ligar y recurrir—.

La prueba más exigente combina `match` con el **modo estrés del GC**: recolectar en
*cada* punto seguro mientras se recorre una lista recursiva. Si el escrutinio en el
local temporal, o un campo de payload recién extraído, no estuvieran rooteados, un
valor vivo se liberaría y el resultado cambiaría o reventaría. Que coincida con el
intérprete, recolección tras recolección, es la prueba de que las raíces de `match`
están completas.

## M5 completo

raylang ya tiene las dos mitades del álgebra de tipos —productos (structs) y sumas
(enums)— y la forma de consumir las sumas con seguridad: `match` exhaustivo. Dos
motores que coinciden, uno trazando con `Rc` y otro con su recolector, verificados en
cada `cargo test`. El siguiente hito, **M6**, añade **genéricos**: y con ellos, los
enums de M5 se vuelven `Option<T>` y `Result<T, E>` —el sistema de errores como
valores que da nombre al norte del lenguaje—.

> Código: `src/bytecode.rs` (`EnumTagEq`, `GetEnumField`, `MatchFail`), `src/compiler.rs`
> (`emit_match`: el local temporal, la cadena de decisión, el boxing de los bindings),
> `src/vm.rs` (la ejecución de los tres opcodes, el oráculo en modo estrés).
