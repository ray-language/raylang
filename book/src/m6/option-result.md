# Option, Result y el operador `?`

Con genéricos y enums, raylang ya tiene todo lo necesario para su modelo de errores.
No hace falta añadir nada al *lenguaje*: basta con dos tipos de **librería** y un
operador que los haga ergonómicos.

## El prelude: errores como librería

`Option<T>` y `Result<T, E>` no están incrustados en el compilador. Son enums
genéricos **escritos en raylang**:

```rust
enum Option<T> { Some(T), None }
enum Result<T, E> { Ok(T), Err(E) }
```

Viven en un *prelude* (`src/prelude.rs`): una cadena de fuente que se parsea una vez y
cuyos enums se **inyectan** al inicio de la verificación, antes del programa del
usuario. A partir de ahí, el checker, el intérprete y la VM los tratan como enums
genéricos cualesquiera —no hay reglas especiales para ellos—.

> **Por qué librería y no incrustados.** Que `Option`/`Result` salgan del mismo
> mecanismo que cualquier `enum Caja<T>` no es pereza: es la prueba de que el mecanismo
> es **general**. El usuario podría definir su propio `Either<A, B>`, o un
> `Validado<T>`, con exactamente el mismo poder. El lenguaje provee la maquinaria
> (genéricos + enums + `match`); la librería elige los tipos. Lo único que el
> compilador conoce por nombre es el operador `?` —y solo para saber qué desempaquetar—.

`Option<T>` modela la **ausencia** de un valor (lo que en otros lenguajes sería `null`,
pero tipado: el checker te obliga a tratar el caso `None`). `Result<T, E>` modela una
operación que **puede fallar**, llevando el valor (`Ok`) o el error (`Err`).

## El operador `?`: propagar sin anidar

Consumir un `Result` con `match` en cada paso es correcto pero tedioso: tres
operaciones falibles encadenadas serían tres `match` anidados. El operador `?` lo
colapsa:

```rust
fn evaluar(a: int, b: int, c: int) -> Result<int, string> {
    let p: int = dividir(a, b)?;   // si Err -> retorna ese Err; si Ok(v) -> p = v
    let q: int = dividir(p, c)?;
    Result.Ok(q + 1)
}
```

`expr?` es un operador **postfijo**. Su semántica:

- si el valor es `Ok(v)` o `Some(v)`, la expresión vale `v` (desempaqueta);
- si es `Err(e)` o `None`, la función **retorna** ese valor inmediatamente.

El checker valida la coherencia: la función que usa `?` debe declarar un retorno
**compatible** con lo que `?` propagaría —`Result<_, E>` con la misma `E` para un `?`
sobre `Result`, o `Option<_>` para uno sobre `Option`—. Olvidarlo es un error de tipos,
no una sorpresa en ejecución.

## El borrado de tipos hace trivial la propagación

Hay un detalle elegante en cómo `?` propaga un error. Cuando `evaluar` hace
`dividir(a, b)?` y el resultado es `Err("...")`, ¿qué valor retorna? El **mismo**
`Err`. Y funciona aunque el tipo de retorno de `evaluar` tenga otro parámetro `Ok` que
el de `dividir`: con borrado de tipos, un `Err(e)` **no guarda** sus argumentos de
tipo en runtime —es, literalmente, la variante `Err` con su payload—. Propagar el error
es devolverlo tal cual. El borrado de tipos, que en M6.1 y M6.2 nos ahorró tocar el
runtime, aquí vuelve a pagar.

## Ejecución nativa, reusando lo que ya había

`?` es el **único** toque de runtime de todo M6. ¿Por qué no se puede *desugar* a un
`match`, como se resolvieron otras construcciones? Porque su rama de error hace un
`return`, y en raylang `return` es una **sentencia**, no una expresión: no cabe como
cuerpo de un brazo de `match`. Así que cada motor lo ejecuta directamente:

- El **intérprete** reusa su señal `Flow::Return` —la misma con la que M1 implementó
  `return`—: en `Err`/`None`, devuelve el valor como un retorno de la función.
- La **VM** lo baja a bytecode reusando los opcodes de enum de M5.3: guarda el valor en
  un local temporal, comprueba con `EnumTagEq(0)` si es el caso de éxito (`Ok`/`Some`
  son la primera variante, *tag* 0); si lo es, extrae el payload con `GetEnumField(0)`;
  si no, emite un `Return`. **Ningún opcode nuevo**: las piezas de M5.3 encajaban.

Que `?` no necesitara ni un *desugaring* artificial ni instrucciones nuevas —solo
recombinar `Flow::Return` y los opcodes de enum— es la señal de que las capas
anteriores estaban bien puestas.

## M6 completo

raylang cumple su norte: un lenguaje **sin `null`**, donde la ausencia de valor y la
posibilidad de fallo son tipos —`Option<T>`, `Result<T, E>`— que el compilador obliga a
tratar, y donde `?` hace que propagarlos sea ergonómico. Todo el polimorfismo vive en
el type checker; los dos motores coinciden sin saber qué es `T`. El siguiente hito,
**M7**, suma azúcar sobre la llamada —UFCS (`s.trim()` ≡ `trim(s)`) y pipelines
(`x |> f(a)`)— y una stdlib que aproveche estos cimientos.

> Código: `src/prelude.rs` (Option/Result y su inyección), `src/checker.rs`
> (`check_try`), `src/interpreter.rs` (`Flow::Return` en `Try`), `src/compiler.rs`
> (`emit_try`: temp local + opcodes de enum + `Return`).
