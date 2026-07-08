# panic, assert y el runner de pruebas

Un compilador escrito en raylang necesita dos cosas que hasta M13 faltaban: una forma de **abortar
con un mensaje** cuando se alcanza un estado imposible (`panic`), y una forma de **comprobar
invariantes** en las pruebas (`assert`/`assert_eq`). M13.2 las añade, y de paso mejora el runner de
`@test` para que una prueba que falla no se lleve por delante a las demás.

## `panic`: el único toque de runtime

Casi todo en M13.2 se construye **en raylang**, sobre una sola pieza nueva del runtime: el builtin
`panic(msg)`. Aborta la ejecución con `msg` en la posición de la llamada.

```rust
panic("estado imposible: la tabla debería tener la clave");
```

Como cualquier builtin tras la limpieza L1, es una fila en la tabla más un opcode y una impl por
motor. El intérprete lo intercepta en `eval_call` y devuelve `Flow::Error`; la VM lo baja al opcode
`Panic`. Ambos motores producen **el mismo mensaje** en la misma posición, así que el oráculo
(`panic_y_assert_falla_oraculo`) los cuadra.

### `panic` diverge

Una llamada a `panic` no retorna nunca. Eso importa para el análisis de tipos: una rama que termina
en `panic` **cede su tipo a la otra**, igual que una que termina en `return`.

```rust
fn cabeza(xs: [int]) -> int {
    if (len(xs) > 0) { xs[0] } else { panic("lista vacía") }
    // el `if` tipa como `int`: la rama `else` diverge y no impone su tipo
}
```

El checker ya hacía esto para `return` (análisis de divergencia, M1); M13.2 extiende
`expr_diverges` para reconocer también `panic`. Esta sola regla es lo que hace usable a `panic`
dentro de expresiones, y la explotará a fondo el compilador auto-alojado (M14), lleno de ramas
"esto no puede pasar".

## `assert` y `assert_eq`: en el propio lenguaje

Con `panic` como cimiento, las aserciones son **funciones del prelude escritas en raylang** —cero
runtime nuevo—.

```rust
fn assert(cond: bool) {
    if (!cond) { panic("aserción falló"); }
}

fn assert_eq<T: Eq + Show>(a: T, b: T) {
    if (!a.eq(b)) {
        panic("assert_eq falló: " + a.show() + " != " + b.show());
    }
}
```

`assert_eq` es genérica con dos bounds: `T: Eq` para comparar (`.eq()`) y `T: Show` para
mostrar el mensaje (`.show()`). Los bounds se bajan al paso de diccionarios de M9.2, así que esto
es front-end puro sobre `panic`. Para que funcione con primitivos, el prelude trae los `impl Eq` e
`impl Show` de `int`/`float`/`bool`/`string`/`char` que `assert_eq` necesita.

**No hay sobrecarga** en raylang, así que no existe `assert(cond, msg)`. El menú es deliberado:
`assert(cond)` (mensaje genérico), `assert_eq(a, b)` (mensaje detallado con ambos valores) y, para un
mensaje a medida, `panic("…")` directo.

## El runner de `@test`, aislado por prueba

La anotación `@test` y su runner existían desde M10.1, pero con una debilidad: todas las pruebas se
sintetizaban en un único `main`, así que **un `panic` en una abortaba la batería entera**. Con
aserciones que abortan, eso ya no vale.

El runner (`test_runner.rs`, un cliente externo que no toca el core) se rehízo:

- **Aislamiento por prueba.** Cada `@test` corre en **su propia ejecución** del intérprete: se clona
  el programa base y se sintetiza un `main` que llama solo a esa prueba. Un `panic` o aserción que
  falla aborta *esa* ejecución y se captura su mensaje; las demás siguen.
- **`@test` admite `() -> unit`** además de `() -> bool`. El checker relaja la firma; el runner lee
  el tipo del AST: una prueba `bool` pasa si devuelve `true`; una `unit` pasa si no dispara ninguna
  aserción.
- **Reporte por prueba + resumen**, y el **código de salida = número de fallos** (compatible con lo
  anterior).
- **Filtro por subcadena** del nombre: `raylang --test archivo.ray patron`.

```rust
@test
fn suma_conmuta() {
    assert_eq(2 + 3, 3 + 2);
    assert(1 + 1 == 2);
}
```

Una prueba `unit` como esta pasa si llega al final sin abortar. Si `assert_eq` falla, solo cae esta
prueba —con su mensaje— y el runner sigue con las demás.

> **Por qué importa.** `panic` da el "esto no puede pasar" que puebla un compilador; `assert`/
> `assert_eq` y el runner aislado dan la red de seguridad para escribirlo con confianza. Las tres
> son, de nuevo, una mínima pieza de runtime (`panic`) y mucho raylang encima.
