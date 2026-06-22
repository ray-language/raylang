# Funciones genéricas e inferencia

Una función genérica declara sus **parámetros de tipo** entre `<…>` tras el nombre, y
los usa como si fueran tipos cualesquiera:

```rust
fn identidad<T>(x: T) -> T { x }
fn aplicar<T, U>(f: fn(T) -> U, x: T) -> U { f(x) }
fn ultimo<T>(xs: [T]) -> T { xs[len(xs) - 1] }
```

Dentro del cuerpo, `T` es un tipo **opaco**: el código no puede suponer nada sobre él.
Eso limita lo que puede hacer —no se puede `x == y` sobre dos `T`, porque un `T`
podría ser una función o un enum no comparable— y es exactamente lo correcto para
genéricos **no acotados** (sin *traits*/bounds): si no sabes qué es `T`, solo puedes
moverlo, guardarlo o pasarlo.

## `Type::Var`: un parámetro de tipo es un tipo

El tipo `Type`, pensado para crecer desde M1, gana una variante: `Type::Var(String)`,
una `T`. Es opaca —dos `Var` solo son iguales si tienen el mismo nombre—.

Aquí reaparece un patrón de M5. El parser no sabe qué identificadores son parámetros
de tipo, así que produce `Type::Struct("T")` para cualquier nombre en posición de
tipo. El checker lo **reclasifica**: si el nombre está en ámbito como parámetro de
tipo, `resolve_type` lo convierte en `Var`; si es un enum, en `Enum`; si no, se queda
`Struct`. Los parámetros de tipo de la función en curso viven en un conjunto
(`self.type_params`) que se llena al registrar su firma y al verificar su cuerpo.

## Sustitución: instanciar un genérico

La primera operación nueva es la **sustitución**: reemplazar cada `Var` por un tipo
concreto, según un mapa `σ` (sigma). Instanciar el retorno `[U]` de `aplicar` con
`σ = {U ↦ string}` da `[string]`. Es mecánico —se recorre el tipo cambiando las `Var`
que aparezcan en `σ`— y es cómo el checker pasa de "la firma genérica" a "el tipo de
esta llamada concreta".

## Unificación: la inferencia

La segunda operación es la **unificación**, y es el corazón de la inferencia. Al
verificar `aplicar(doble, 21)`:

- la firma dice que los parámetros son `fn(T) -> U` y `T`;
- los argumentos tienen tipos `fn(int) -> int` y `int`;
- **unificar** los primeros, `fn(T) -> U` contra `fn(int) -> int`, liga `T = int` y
  `U = int`; unificar los segundos, `T` contra `int`, confirma `T = int`.

Con `σ = {T ↦ int, U ↦ int}` completo, el tipo del resultado es `subst([U]…)` —en este
caso `subst(U, σ) = int`—. Si dos argumentos exigen valores distintos para el mismo
parámetro (`par(1, true)` querría `T = int` y `T = bool`), la unificación lo detecta y
da un error claro: *"'T' no puede ser int y bool a la vez"*.

> **Una unificación asimétrica.** Hay una sutileza que vale la pena ver. La `unify` de
> raylang **no** es la unificación simétrica de los libros de texto: distingue de qué
> lado viene cada tipo. Los `Var` del **parámetro** (la firma de la función llamada)
> son las **incógnitas** a resolver: se ligan. Los `Var` del **argumento** (que pueden
> venir de la función que estamos compilando, si ella también es genérica) son
> **rígidos**: son constantes opacas, no se tocan. Así, dentro de `fn f<T>(...)`, pasar
> tu `T` a `identidad` infiere correctamente sin "adivinar" qué es tu `T`. Esta
> asimetría es justo lo que separa "inferir los genéricos de la función que llamo" de
> "resolver un sistema de ecuaciones global" (eso sería la inferencia de M8).

## El runtime no se entera

Y aquí está la recompensa del borrado de tipos: **nada de esto llega al runtime**. El
intérprete y la VM ejecutan `identidad(5)` e `identidad(true)` con el mismo código de
siempre —una llamada a función que mueve un valor—. Los tests-oráculo de M6.1 pasan
**sin tocar** `interpreter.rs` ni `vm.rs`: la prueba de que los genéricos viven, de
principio a fin, en el type checker.

Dos límites cierran M6.1, y los dos se levantan en M6.2: un parámetro de tipo que los
argumentos no fijan da error (todavía no hay tipo esperado), y una función genérica no
puede tomarse como **valor** (su tipo no es un `fn(...)` concreto; hay que llamarla por
nombre).

> Código: `src/ast.rs` (`Type::Var`, `Function.type_params`), `src/checker.rs`
> (`resolve_type`, `subst`, `unify`, `check_generic_call`). El runtime, intacto.
