# El LSP: diagnósticos en vivo

La otra mitad de M10 es el **Language Server** (LSP): un programa que le da al editor los
errores del compilador **mientras escribes**, subrayados bajo el código. Lo bonito del LSP es
que se escribe **una vez** y sirve a *todos* los editores que hablen el protocolo —VSCode,
Neovim, Helix…—, en lugar de un plugin distinto por editor.

## Qué es un Language Server, en una frase

El editor y el servidor se hablan por **stdin/stdout** con un protocolo, **LSP**, montado sobre
**JSON-RPC**. El editor avisa "abrí este archivo", "cambió a esto"; el servidor responde
"hay un error de tipos en la línea 3, columna 9". El editor dibuja el subrayado. Eso es todo.

```text
   editor  ──── textDocument/didChange (el texto nuevo) ───▶  raylang --lsp
           ◀─── textDocument/publishDiagnostics (errores) ───
```

## Dos decisiones al arrancar

El LSP traía dos decisiones de enfoque, y las dos se resolvieron *fieles al proyecto*:

1. **Transporte: JSON-RPC a mano.** raylang no tiene **ninguna dependencia de Cargo** —todo es
   `std`—, y no íbamos a romper eso por el LSP. Nada de `lsp-server` ni `tower-lsp` ni `serde`:
   escribimos el *framing* y un mini-JSON nosotros. Es más plomería, pero el punto es
   pedagógico: se *ve* el protocolo por dentro. (En un proyecto de producción usarías un crate
   sin pensarlo; aquí el objetivo es entenderlo.)
2. **Alcance: solo diagnósticos.** `initialize` + abrir/cambiar/cerrar documento →
   publicar errores. Sin *hover* (mostrar el tipo bajo el cursor) ni *ir-a-definición*: ambos
   exigirían exponer una API de tipos del checker (que evitamos ya en el REPL) y un índice de
   símbolos. Quedan para un futuro M10.2b. Los diagnósticos son el 80% del valor.

## Fiel al patrón: un cliente externo

Como el REPL (M8.2) y el runner de `@test` (M10.1), el LSP es un **cliente externo**: vive en
`src/lsp.rs` y usa **solo la API pública** del compilador. **Cero cambios en el núcleo.** Todo
el acoplamiento con el compilador cabe en una función:

```rust
pub fn analizar(src: &str) -> Option<Diag> {
    let tokens = match lexer::lex(src) {
        Ok(t) => t,
        Err(e) => return Some(Diag { line: e.line, col: e.col, message: e.to_string() }),
    };
    let mut program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => return Some(Diag { line: e.line, col: e.col, message: e.to_string() }),
    };
    if let Err(e) = checker::check(&mut program) {
        return Some(Diag { line: e.line, col: e.col, message: e.to_string() });
    }
    None
}
```

Corre el front-end —lexer → parser → checker— **y nada más**: el LSP *no ejecuta* el programa,
solo lo valida. Reusa el `(línea, col)` que toda fase ya reporta (es un principio del proyecto,
no un extra) y el `Display` de cada error. Es la misma información que ves en el terminal, con
otra presentación.

> **Un diagnóstico por documento.** Nuestro compilador es *fail-fast*: en cuanto una fase
> encuentra un error, lo devuelve y para. Así que el LSP publica **un** error a la vez (al
> corregirlo, aparece el siguiente). Reportar *todos* de golpe exigiría que cada fase
> *recolectara* errores y siguiera —un cambio mayor en el compilador—, y queda para el futuro.

## Las tres capas de `src/lsp.rs`

### 1. El mini-JSON (`mod json`)

Un parser de descenso recursivo y un serializador, en `std` puro. No es un JSON de producción
—es justo lo que el LSP intercambia—, pero correcto para ese tráfico, incluido el *unescape* de
`\uXXXX` y las parejas sustitutas UTF-16 (un editor puede enviar emojis en una cadena). El valor
es un enum sencillo:

```rust
pub enum Json { Null, Bool(bool), Num(f64), Str(String), Arr(Vec<Json>), Obj(Vec<(String, Json)>) }
```

### 2. El *framing* LSP

Sobre JSON-RPC, LSP añade una sola cosa: cada mensaje va precedido de cabeceras estilo HTTP.

```text
Content-Length: 124\r\n
\r\n
{"jsonrpc":"2.0","method":"textDocument/didOpen", ... }
```

`read_message` lee las cabeceras hasta la línea en blanco, saca el `Content-Length` y lee
*exactamente* esos bytes. `send` hace lo inverso. Eso es el 100% del transporte.

### 3. El bucle `serve`

Lee un mensaje, lo despacha por su `method`:

| Método | Qué hace |
|--------|----------|
| `initialize` | responde las **capacidades**: `textDocumentSync = 1` (*Full sync*) |
| `textDocument/didOpen` / `didChange` | **analiza** el texto y **publica** diagnósticos |
| `textDocument/didClose` | publica una lista **vacía** (borra los subrayados) |
| `shutdown` / `exit` | termina ordenadamente |
| *(desconocido con `id`)* | error JSON-RPC `-32601` ("método no soportado") |

Con *Full sync* el cliente reenvía el documento **entero** en cada cambio, así que el servidor
**no guarda estado** del documento: cada `didChange` trae todo lo que necesita. Simplísimo.

Una sutileza de coordenadas: nuestras fases dan `(línea, col)` **1-basadas**; LSP las quiere
**0-basadas**. La traducción vive en un solo sitio (`diagnostico_json`), que además subraya
desde la columna del error hasta el final de la línea, para que el *squiggle* se vea.

## Probarlo sin un editor

`serve` es genérico sobre los flujos de entrada/salida, así que se prueba **en memoria** con un
`Cursor` —sin tocar stdin real—: le metes marcos LSP y compruebas el stdout. Y aparte, un test
de subproceso (`tests/lsp_cli.rs`) lanza `raylang --lsp` de verdad y confirma que habla el
protocolo. A mano:

```sh
printf 'Content-Length: 58\r\n\r\n{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | raylang --lsp
```

## Conectarlo a un editor

Como el servidor es un binario que habla LSP por stdin/stdout, cualquier editor lo usa apuntándole
a los archivos `.ray`. En **Neovim** (con `nvim-lspconfig` o el API nativo), un par de líneas
bastan:

```lua
vim.api.nvim_create_autocmd("FileType", {
  pattern = "ray",
  callback = function(args)
    vim.lsp.start({ name = "raylang", cmd = { "raylang", "--lsp" }, root_dir = vim.fn.getcwd() })
  end,
})
```

En **Helix** (`languages.toml`) es igual de breve. Sin npm, sin nada: el binario de siempre con un
modo más.

**VSCode** es el caso pesado. Hasta ahora su extensión era *solo declarativa* (la gramática
TextMate): VSCode leía el `package.json` y coloreaba sin ejecutar código. Para hablar LSP, una
extensión tiene que **ejecutar código** que lance el servidor y traduzca el protocolo a su UI, y
eso se apoya en `vscode-languageclient` —una dependencia de **npm**, del lado del editor; el
binario de raylang sigue sin dependencias—. El cliente cabe en ~25 líneas (`editors/vscode/src/extension.ts`):
arranca `raylang --lsp` por stdio y le declara que se aplica a los documentos `raylang`. Por eso
Neovim/Helix son la vía sin build, y VSCode pide compilar un pequeño cliente TypeScript. Ver
`editors/vscode/README.md` para los pasos.

> **La lección de M10.** Las dos caras del *tooling* comparten una raíz: **reutilizar el
> front-end**. `@derive(Eq)` genera fuente y deja que el compilador la baje; el LSP corre el
> checker y traduce sus errores. Ninguno toca el núcleo. Cuando las fases están bien separadas
> y todo nodo lleva su `(línea, col)`, las herramientas alrededor del lenguaje salen casi gratis.
