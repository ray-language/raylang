# Extensión de VSCode para raylang

Resaltado de sintaxis (syntax highlighting) para archivos `.ray`.

Esto es **solo coloreado**: clasifica palabras clave, tipos, literales, comentarios,
operadores, definiciones y llamadas a funciones para que el tema de color de tu
editor las pinte. No valida tipos ni detecta errores — esa parte (diagnostics en
vivo) llegará con el **Language Server** (ver `IDEAS.md`, sección de tooling/LSP).

## Qué colorea

| Categoría | Ejemplos | Scope TextMate |
|-----------|----------|----------------|
| Palabras clave de control | `if else while return match` | `keyword.control` |
| Declaración / storage | `let var fn struct enum trait impl` | `storage.type` |
| Receptor / tipo propio | `self`, `Self` | `variable.language.self` |
| Tipos primitivos | `int float bool string` | `support.type.primitive` |
| Tipos de usuario | `Punto` (struct), `Figura` (enum) | `entity.name.type` |
| Booleanos | `true false` | `constant.language.boolean` |
| Números | `42`, `3.14` | `constant.numeric` |
| Cadenas y escapes | `"hola\n"` | `string.quoted.double` |
| Comentarios | `// ...` | `comment.line` |
| Builtins y stdlib | `print len push map filter fold` | `support.function.builtin` |
| Definición de struct | `struct Punto` | `entity.name.type` |
| Definición de enum | `enum Figura` | `entity.name.type` |
| Definición de trait | `trait Mostrable` | `entity.name.type` |
| Bloque impl | `impl Mostrable for Punto` | `entity.name.type` + `keyword.control` |
| Variante de enum | `Figura.Circulo`, `Lista.Nil` | `entity.name.type` |
| Definición de función | `fn fib` | `entity.name.function` |
| Función anónima / tipo función | `fn(n: int) -> int`, `fn(int) -> int` | `storage.type` + tipos |
| Llamadas | `fib(...)` | `entity.name.function` |
| Objetivo de pipeline | `doble` en `x \|> doble` | `entity.name.function` |
| Operadores | `+ - == && -> => ? \|>` ... | `keyword.operator` |
| Genéricos | `<T>`, `Caja<int>`, `Par<A, B>` | `entity.name.type` + `keyword.operator` |

> El **tipo función** `fn(...) -> R` y la **función anónima** `fn(n: int) { ... }`
> de M4 no necesitaron reglas nuevas: la `fn` la pinta `storage.type` y sus
> parámetros/retorno caen en las reglas de tipos. Los **nombres de tipo de usuario**
> se reconocen por convención (identificador que empieza en mayúscula), así que tanto
> `struct Punto { ... }` como su uso `Punto { x: 1, y: 2 }` o `: Punto` se colorean.

> **M5 (enums).** `enum Figura { ... }` se colorea como su análogo `struct`. Las
> **variantes** (`Figura.Circulo`, `Lista.Nil`) caen en la misma convención de
> mayúscula, así que se pintan como tipo tanto al construirlas como en (futuro) `match`.
> `match` y `=>` ya se reservan en el lexer; se colorean aunque su sintaxis completa
> llegue en M5.2.

> **M6 (genéricos).** Los **parámetros y argumentos de tipo** no necesitaron reglas
> nuevas: en `fn id<T>`, `enum Caja<T>` o `Caja<int>`, el nombre de tipo (mayúscula o
> primitivo) ya se reconoce y los `<` `>` caen en operadores. Lo único que se añadió
> fue el operador de propagación **`?`** (`dividir(a, b)?`), que ahora se colorea como
> `keyword.operator`. La inferencia de genéricos es cosa del checker, invisible al
> coloreado.

> **M7 (UFCS, pipelines y stdlib).** UFCS (`s.trim()`) no necesita reglas: el `.` ya se
> colorea y el método cae en la regla de llamadas. El **pipeline `|>`** se añadió a los
> operadores. Ambos son azúcar (UFCS en el checker, `|>` en el parser): el coloreado no
> distingue una llamada UFCS/pipeline de una normal, y no falta hacerlo. La **stdlib**
> (`map`/`filter`/`fold`, junto a `len`/`push`) se colorea como `support.function`, igual
> que `print`: aunque `map`/`filter`/`fold` están escritas en raylang (en el prelude), al
> usuario se le presentan como funciones de librería.
>
> **Objetivo de un pipeline desnudo.** En `x |> doble`, `doble` no lleva paréntesis, así
> que la regla general de llamadas (que exige `(`) no lo casa. Una regla específica
> colorea el identificador que sigue a `|>` como función. Excluye los builtins, porque en
> `x |> map(...)` el paréntesis ya los colorea como tales (y así `map` conserva su color
> de builtin). Caso límite: un builtin desnudo tras `|>` (p. ej. `x |> len`) queda sin
> color —raro, porque `filter`/`fold`/`map` siempre llevan argumentos—.

> **M9 (traits).** Se añaden las palabras clave `trait` e `impl` (a `storage.type`) y
> reglas de definición que colorean el **nombre del trait** y, en `impl Trait for Tipo`,
> también el `for` (contextual: no es palabra clave global del lenguaje, solo se colorea
> dentro del `impl`) y el tipo destino. El receptor **`self`** y el tipo **`Self`** se
> pintan como `variable.language`. Los **métodos** dentro de un `impl` caen en las reglas
> ya existentes (definición de función y llamadas); su despacho es cosa del checker,
> invisible al coloreado.

> Nota: la gramática TextMate (`syntaxes/raylang.tmLanguage.json`) es una
> **reescritura en regex** de las reglas léxicas de `DESIGN.md` §3. Es independiente
> de nuestro lexer en Rust; mantener ambos en sincronía es la pequeña deuda
> inevitable del coloreado por-editor. La validación real (LSP) sí reutilizará el
> checker de Rust, una sola vez para todos los editores.

## Instalación (desarrollo local)

La forma más simple, sin empaquetar nada: enlaza esta carpeta dentro del directorio
de extensiones de VSCode y recarga.

```sh
# macOS / Linux
ln -s "$(pwd)" ~/.vscode/extensions/raylang-0.7.0

# Luego: recarga VSCode (Cmd/Ctrl+Shift+P → "Developer: Reload Window")
```

Abre cualquier archivo `.ray` (por ejemplo `examples/fib.ray`) y deberías ver el
coloreado. Si VSCode no lo reconoce, comprueba el selector de lenguaje (abajo a la
derecha) y elige "raylang".

## Empaquetar (opcional)

Para generar un `.vsix` instalable o publicarlo:

```sh
npm install -g @vscode/vsce
vsce package        # genera raylang-0.7.0.vsix
# Instalar el .vsix: Cmd/Ctrl+Shift+P → "Extensions: Install from VSIX..."
```

## Probar la gramática

VSCode trae una herramienta para inspeccionar qué scope recibe cada token:
`Cmd/Ctrl+Shift+P` → **"Developer: Inspect Editor Tokens and Scopes"**. Útil si
algún elemento no se colorea como esperas.
