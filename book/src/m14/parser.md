# El parser auto-alojado

El segundo eslabón: tokens → AST. `selfhost/parser.ray` se alimenta del lexer auto-alojado
(`from lexer import Token, TokKind;`) y reconstruye, en raylang, el descenso recursivo de
`src/parser.rs`. Es la fase más grande del front-end, así que se construyó por capas (núcleo →
datos → tipos → azúcar) hasta cubrir **el lenguaje entero**.

## El AST en raylang

El reto interesante es modelar un **AST mutuamente recursivo** en raylang: un `Expr` lleva un
`EKind`, que a su vez contiene más `Expr`s.

```rust
pub struct Expr { kind: EKind, line: int, col: int }
pub enum EKind {
    EInt(int), EIdent(string),
    EBinary(BinOp, Expr, Expr),   // recursivo
    ECall(Expr, [Expr]),
    EMatch(Expr, [MatchArm]),
    /* … */
}
```

¿No es esto un tamaño infinito? No: `[Expr]` y `Option<Expr>` viven en el **heap** (son referencias),
así que un `Expr` tiene tamaño fijo aunque contenga otros `Expr`. El `struct Parser` se muta por
referencia, igual que el `Lexer`. Un helper `tok_name(k) -> string` da la grafía canónica de cada
token para los mensajes de `check`/`eat`/`expect`, sin números mágicos.

## El oráculo: volcar el AST como S-expression

Un AST no se compara tan fácil como una lista de tokens. La decisión fue **máximo rigor**: volcar el
árbol como una *S-expression* con `@línea:col` en **cada** nodo de expresión y sentencia.

```text
(fn main () int (block
  (let x (int 42)@1:24)@1:20
  (+ (ident x)@1:32 (int 1)@1:36)@1:30))
```

El driver `selfhost/parse_dump.ray` lo imprime; el test (`tests/selfhost_parser.rs`) reconstruye el
mismo formato desde el AST de Rust con `dump_program`. Un detalle clave: el volcado se hace sobre el
**AST crudo**, antes del checker. Por eso un nombre en posición de tipo es `Struct(n, [])` (todavía
no `Enum`/`Var`, eso lo decide el checker) y no hay `EnumLit` (la construcción de enum aún es
`Field`/`Call`). El parser de raylang produce exactamente eso, y cuadra.

## Construido por capas

El parser es grande, así que se hizo en cuatro tandas, cada una con su oráculo:

- **Núcleo** — toda la precedencia de expresiones, sentencias (let/var/assign/return/expr), tipos
  básicos, bloques y funciones de nivel superior. Suficiente para `fib`/`fizzbuzz`.
- **Datos y control** — `struct`/`enum`, literal de struct, funciones anónimas y `match`/patrones.
- **Sistema de tipos** — genéricos `<T: A + B>`, args de tipo, `dyn A + B`, `trait`, `impl`, `self`.
- **Azúcar y módulos** — `?`, pipelines `|>` (desugar puro a `Call`), anotaciones `@`, `pub`,
  `import`/`from import`, y referencias calificadas `M.Tipo`.

Un par de decisiones de fidelidad para que el dump cuadre nodo a nodo: las funciones anónimas llevan
un `id` denso asignado en **pre-orden** (igual que Rust), y el `self` se representa como
`TNamed("Self", [])` y se vuelca `"Self"` (como el `SelfType` de Rust), sin un nodo propio.

## Errores como valores, otra vez

Como el lexer, el parser cierra pasando de `panic` a `Result`:

```rust
pub struct ParseError { msg: string, line: int, col: int }
fn parse(toks: [Token]) -> Result<Program, ParseError> { /* … */ }
```

`expect`/`expect_ident` devuelven `Result`, cada función de parseo propaga con `?`, y los mensajes
son **idénticos** a Rust —incluido "se esperaba una expresión, se encontró `<Debug>`", que reproduce
la representación `Debug` de un `TokenKind` con un helper `tok_debug(k)` (`Semicolon`, `LParen`…)—. El
oráculo cubre así también las **entradas inválidas**.

## El hito de fidelidad

El test fuerte no se queda en snippets: parsea los **35 ejemplos** del repo **más los cuatro fuentes
del self-hosting**, y exige que el AST coincida nodo a nodo —con posiciones— con el de Rust. Es
decir: **el parser se parsea a sí mismo**, idéntico al parser de Rust.

> **Por qué importa.** Con el lexer y el parser auto-alojados verificados contra Rust sobre el código
> real del proyecto (incluido el suyo propio), tenemos la mitad delantera del compilador escrita en
> raylang. Falta darle **significado** a ese AST: el checker.
