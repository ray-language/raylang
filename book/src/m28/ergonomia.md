# Ergonomía del lenguaje II

M27 pulió la comodidad del día a día —tuplas, `for`, interpolación, casts—. M28 sube un peldaño: no
persigue escribir *menos*, sino **abstraer mejor**. Sus tres piezas se apoyan todas en los traits de M9,
y las tres nacen del mismo dolor: escribir tanta criptografía en raylang (M20, M30…) reveló los patrones
que el lenguaje aún obligaba a repetir a mano. Como en M27, casi todo es **azúcar de front-end** que se
baja a algo que el runtime ya sabía hacer; el intérprete y la VM apenas se enteran.

## M28.1 — Sobrecarga de operadores

Un `struct Vec2 { x: int, y: int }` no podía sumarse con `+`: `a + b` estaba *special-cased* para
primitivos, y para todo lo demás había que escribir `a.add(b)` o una función libre. La solución es la
clásica: los operadores son **métodos de trait**. El prelude define `Add`/`Sub`/`Mul`/`Div` (con método
`add`/`sub`/… que devuelve `Self`) y `Neg` para el `-` unario; un tipo que los implemente gana el operador.

```raylang
impl Add for Vec2 {
    fn add(self, otro: Vec2) -> Vec2 { Vec2 { x: self.x + otro.x, y: self.y + otro.y } }
}
// ahora `a + b` funciona
```

Lo bonito es cuánto **reusa**. No hay opcode nuevo ni cambio en los motores: es **erasure**, apoyado en la
misma bajada por posición que ya usaban UFCS y los métodos de trait (M7, M9). En `check_binary`, cuando el
camino built-in falla y **ambos operandos son el mismo tipo de usuario** que implementa el trait del
operador, se registra el sitio `(línea, col, "Add")` y el retorno es `Self`. Luego una pasada
`lower_operators` (antes de `lower_ufcs`) reescribe el `Binary` a la llamada ordinaria `Vec2#add(a, b)` —a
la función manglada que M9 ya había inyectado—. El runtime nunca ve un `+` sobre un `Vec2`.

El gotcha vive en la **clave del sitio**. En `a + b + c`, los dos `+` comparten el mismo `(línea, col)` en
el AST (es el mismo operador). Por eso la clave lleva el **nombre del trait**, no solo la posición: mismo
operador ⇒ mismo método, así que la colisión de posiciones es inocua. Comparación (`==`, `<`) e impls
genéricos de operador quedan **diferidos**: los aritméticos concretos son el caso útil, y los primitivos
siguen por el camino built-in de siempre.

## M28.2 — `?` que convierte el error

El operador `?` de M6.3 propagaba un `Result<T, E>` tal cual: si tu función devuelve `Result<T, MiError>`
pero llamas a algo que devuelve `Result<T, string>`, el `?` no compilaba. La consecuencia era fea: las
librerías arrastraban `string` como tipo de error en todas partes, en vez de tener su propio enum. Lo que
queremos es lo de Rust: si existe `From<E1> for E2`, el `?` **convierte** automáticamente.

Para eso hizo falta un habilitador de peso: **parámetros de tipo en traits**. `From<S>` es el primer trait
con `<…>` del proyecto (`TraitDef.type_params`, `ImplBlock.trait_args`). Vive en el prelude:

```raylang
trait From<S> { fn desde(origen: S) -> Self; }

impl From<string> for MiError { fn desde(o: string) -> MiError { MiError { msg: o } } }
```

Dos detalles de diseño. El método se llama `desde`, no `from`: `from` ya es palabra clave (del `from M
import …`). Y `desde` **no tiene `self`** —es un método asociado, se invoca por el tipo—, como el `from`
de Rust.

De nuevo, **front-end puro**. En el paso 0c el método se inyecta como función libre manglada **por origen**
(`MiError#desde#string`), para que varios `impl From<…> for MiError` no colisionen entre sí. Cuando
`check_try` ve que el error del `Result` (E1) difiere del retorno de la función (E2) pero hay `impl
From<E1> for E2`, registra el sitio, y `lower_try_conversions` reescribe ese `expr?` a un `match` explícito:

```raylang
match (expr) {
    Result.Ok($to)  => $to,
    Result.Err($te) => { return Result.Err(MiError#desde#string($te)); },
}
```

Es decir, se desazucara a `match` + `return` + construcción de enum + una llamada —todo lo que el runtime
ya tenía—. El `?` **sin** conversión sigue siendo el nodo `Try` nativo de M6.3. Advertencia de alcance: los
parámetros de tipo en traits solo tienen semántica para `From`/`?`; sus otros usos (bounds, `dyn`, despacho
`.metodo()` con `<…>`) se **aceptan sintácticamente pero se difieren**, igual que `Into`, las cadenas de
conversión y `From` entre módulos.

## M28.3 — Enteros con tamaño

Este es el más invasivo: toca todo el modelo numérico. El motivo se ve en cualquier hash escrito en
raylang. SHA-256 trabaja con palabras de 32 bits, pero el `int` de raylang tiene 64, así que el código está
plagado de `& 0xFFFFFFFF` tras cada operación que pueda desbordar. Es ruido que oscurece el algoritmo.

Las **decisiones se fijaron con el usuario**, deliberadamente acotadas para no volverse research-grade:
solo `u8`/`u32`/`u64` (el `int` sigue siendo `i64`), aritmética con **wrapping** dentro del ancho, y
conversión **solo con `as`** —sin promoción implícita ni mezclas de anchos—.

En **M28.3a** entra el núcleo. `Type::UInt(ancho)` con keywords `u8`/`u32`/`u64`; en runtime, `Value::UInt(u64,
u8)` (y su gemelo `HeapValue`): un escalar inline —como `char`, sin tocar el GC— que **lleva su ancho** para
saber por dónde envolver. Helpers `uint_mask`/`make_uint` enmascaran al ancho, y con eso el wrapping sale
solo. La aritmética, las bitops y la comparación **sin signo** exigen **el mismo ancho** en ambos operandos;
`as` convierte entre `int`, cualquier `uint` y `float`. Como ambos motores comparten la máscara, el oráculo
`uint_oraculo` queda verde. El ejemplo `examples/types/enteros.ray` reescribe FNV-1a en `u32` sin un solo
`& 0xFFFFFFFF`.

Pero M28.3a aún obligaba a escribir `5 as u8` para cada literal, lo que es su propio ruido. **M28.3b** lo
cura con el **literal entero polimórfico**: un literal adopta el ancho del **contexto**.

```raylang
let x: u8 = 5;        // 5 toma u8 del tipo esperado
let y = x + 100;      // 100 se cede a u8 porque el otro operando es u8
```

La cesión se propaga por el tipo esperado (anotación, argumento, elemento de arreglo) o por el **operando**
de un operador, recursivamente en aritmética y bitops (`200 + 100` con esperado `u8` empuja a los dos
literales). No es promoción: solo el **literal** se convierte —un `x: int` jamás se promociona a `u8`—, y un
literal fuera de rango es error (`el literal 300 no cabe en u8`). El mecanismo es el de siempre:
`check_expr_expected` registra el sitio y `lower_uint_literals` envuelve el literal en un `Cast` al ancho,
reusando el `as` de M28.3a. Runtime intacto.

---

La lección de M28.3 la cobra M30. ChaCha20, Poly1305, SHA-512: escritos en `u32`/`u64` limpios, su código es
**idéntico al pseudocódigo del RFC**, sin el enmascarado a mano que plagaba SHA-256. Y es la moraleja del
hito entero, otra vez la de M7 y M27: la abstracción que hace el lenguaje *sentirse* completo —operadores
propios, `?` que convierte, enteros del ancho justo— se paga casi toda en el front-end. Solo los enteros con
tamaño, que de verdad cambian cómo se guardan los bits, bajaron al motor.
