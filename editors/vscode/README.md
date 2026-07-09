# Extensión de VSCode para raylang

Soporte para archivos `.ray`: **resaltado de sintaxis** + **cliente del Language Server**
(diagnósticos en vivo, M10.2c).

El **coloreado** clasifica palabras clave, tipos, literales, comentarios, operadores,
definiciones y llamadas a funciones para que el tema de color de tu editor las pinte; es
estático (no entiende el programa). La **validación real** —errores de léxico, sintaxis y
tipos subrayados mientras escribes— la da el **Language Server** (`ray lsp`), al que esta
extensión se conecta como cliente (ver "Diagnósticos en vivo" al final).

## Qué colorea

| Categoría | Ejemplos | Scope TextMate |
|-----------|----------|----------------|
| Palabras clave de control | `if else while for in return match import from as` | `keyword.control` |
| Declaración / storage | `let var const fn struct enum trait impl dyn pub` | `storage.type` |
| Receptor / tipo propio | `self`, `Self` | `variable.language.self` |
| Anotaciones | `@test`, `@derive(Eq)` | `storage.modifier.annotation` |
| Tipos primitivos | `int float bool string char bytes u8 u32 u64` | `support.type.primitive` |
| Tipos de usuario | `Punto` (struct), `Figura` (enum) | `entity.name.type` |
| Booleanos | `true false` | `constant.language.boolean` |
| Números | `42`, `3.14` | `constant.numeric` |
| Cadenas y escapes | `"hola\n"`, `"pi = ${x}"` | `string.quoted.double` + `meta.interpolation` |
| Interpolación | `${expr}` dentro de cualquier cadena | `meta.interpolation` + `punctuation.section.interpolation` |
| Literal de bytes | `b"\x00\xff"` | `string.quoted.double.byte` |
| Carácter | `'a'`, `'\n'` | `constant.character` |
| Comentarios | `// ...` | `comment.line` |
| Builtins y stdlib | `print len push map filter fold sort assert spawn send` | `support.function.builtin` |
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
>
> **M9.2 (bounds).** Los *bounds* de genéricos (`fn f<T: Valor + Etiqueta>(...)`) no
> necesitaron reglas nuevas: el `:` y el `+` caen en operadores y los nombres de trait
> (mayúscula) en tipos de usuario. El paso de diccionarios es una reescritura del checker,
> invisible al coloreado.
>
> **M9.3 (defectos y trait objects).** Los **métodos por defecto** (una firma de trait con
> cuerpo) caen en las reglas de función ya existentes. Para los **trait objects** se añade la
> palabra clave **`dyn`** (`dyn Figura`) a `storage.type`; el nombre del trait que la sigue
> (mayúscula) ya se colorea como tipo. El despacho dinámico se realiza como un struct
> sintetizado en el checker, invisible al coloreado.
>
> **M10.1 (anotaciones).** Una anotación `@nombre` (`@test`, `@derive`) se colorea como
> `storage.modifier.annotation`. Los argumentos (`@derive(Eq)`) caen en sus propias reglas
> (el nombre de trait, en mayúscula, como tipo de usuario). El runner `--test` y la
> generación de `@derive` viven en el front-end, invisibles al coloreado.

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
ln -s "$(pwd)" ~/.vscode/extensions/raylang-0.9.0

# Luego: recarga VSCode (Cmd/Ctrl+Shift+P → "Developer: Reload Window")
```

Abre cualquier archivo `.ray` (por ejemplo `examples/fib.ray`) y deberías ver el
coloreado. Si VSCode no lo reconoce, comprueba el selector de lenguaje (abajo a la
derecha) y elige "raylang".

## Empaquetar (opcional)

Para generar un `.vsix` instalable o publicarlo:

```sh
npm install -g @vscode/vsce
vsce package        # genera raylang-0.9.0.vsix
# Instalar el .vsix: Cmd/Ctrl+Shift+P → "Extensions: Install from VSIX..."
```

## Probar la gramática

VSCode trae una herramienta para inspeccionar qué scope recibe cada token:
`Cmd/Ctrl+Shift+P` → **"Developer: Inspect Editor Tokens and Scopes"**. Útil si
algún elemento no se colorea como esperas.

## Diagnósticos en vivo: el Language Server (M10.2)

El coloreado es estático; la **validación real** (errores de léxico, sintaxis y tipos
subrayados mientras escribes) la da el **Language Server**, que reusa el checker de Rust
una sola vez para todos los editores. El servidor es el propio binario de raylang:

```sh
ray lsp              # habla LSP por stdin/stdout hasta recibir 'exit'
```

No tiene dependencias: el *framing* y el JSON están escritos a mano (DESIGN §19.2). Cualquier
editor que hable LSP lo consume apuntándolo a los archivos `.ray`. Para **VSCode**, esta
extensión incluye el **cliente** (`src/extension.ts`, sobre `vscode-languageclient`); para
**Neovim/Helix** basta un par de líneas de config (`cmd = { "ray", "lsp" }`; ver el
capítulo "El LSP" del libro).

### Cómo usarlo en VSCode

1. **Compila el binario** de raylang y ponlo en el PATH (o anota su ruta):

   ```sh
   cargo build --release      # genera ./target/release/ray (y el alias raylang)
   ```

2. **Compila el cliente** de la extensión (TypeScript → JavaScript):

   ```sh
   cd editors/vscode
   npm install                # trae vscode-languageclient (deps de npm, lado del editor)
   npm run compile            # genera out/extension.js
   ```

3. **Enlaza e instala** la extensión (igual que para el coloreado):

   ```sh
   ln -s "$(pwd)" ~/.vscode/extensions/raylang-0.12.0
   # recarga VSCode: Cmd/Ctrl+Shift+P → "Developer: Reload Window"
   ```

4. Abre un `.ray`. Si `ray` no está en el PATH que ve VSCode (p. ej. lanzado desde el
   Dock en macOS), indica su ruta en los ajustes:
   **`raylang.serverPath`** (p. ej. `/ruta/a/target/release/ray`). El ajuste
   **`raylang.enableLsp`** permite desactivar el cliente y quedarse en solo-coloreado.

> El cliente arranca el servidor (`ray lsp`) al abrir el primer `.ray` y conecta sus
> diagnósticos a los subrayados de VSCode. Es el único código JS de la extensión; la lógica de
> análisis vive toda en el binario de Rust (cero duplicación).

> Alcance: **diagnósticos** (un error a la vez, *fail-fast*), **hover** (el tipo bajo el cursor)
> e **ir-a-definición** (M10.2b). El cliente `vscode-languageclient` ya soporta hover y definición
> sin código extra: basta con que el servidor los anuncie. *Completion*, *rename* y *find-references*
> quedan para el futuro.

## Templates compilados (`.ray.html`, M55)

Los archivos `*.ray.html` se colorean como **raylang template** (HTML + los delimitadores
`{{ }}`/`{% %}` con la expresión embebida coloreada como raylang) y reciben **diagnósticos en
vivo** del mismo Language Server: errores del propio template (etiquetas sin cerrar, `params` mal
formados) y errores de tipos del módulo generado, **mapeados a la línea del template** (un typo en
`{{ variable }}` se subraya en el HTML). Dentro de `{{ }}`/`{% %}` hay además **autocompletado**:
los parámetros tipados de la cabecera `{% params %}` (con su tipo), las variables de los
`{% for %}` que encierran el cursor y las palabras clave del template. Genera el módulo con
`ray templ <dir>`.
