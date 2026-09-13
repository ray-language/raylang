# Extensión de Zed para raylang (repos externos)

> A diferencia de VSCode y Sublime, la extensión de Zed **no vive en este repo**: Zed exige un
> repositorio por extensión y otro por gramática tree-sitter. Este directorio existe para que un
> cambio en el lenguaje **recuerde** actualizar esa extensión, y para documentar cómo se hace.

| Pieza | Repositorio | Clon local habitual | Qué contiene |
|-------|-------------|---------------------|--------------|
| Gramática | [`ray-language/tree-sitter-raylang`](https://github.com/ray-language/tree-sitter-raylang) | `~/Dev/Rayala/labs/tree-sitter-raylang` | `grammar.js` (la gramática), `queries/highlights.scm` (captures estilo nvim), `test/corpus/*.txt` (árboles esperados) |
| Extensión | [`ray-language/zed-raylang`](https://github.com/ray-language/zed-raylang) | `~/Dev/Rayala/labs/zed-raylang` | `extension.toml` (versión + `rev` de la gramática), `languages/raylang/{highlights,brackets,indents,outline}.scm` (captures del tema de Zed), `src/` (Rust: arranca `ray lsp`) |
| Registro | [`zed-industries/extensions`](https://github.com/zed-industries/extensions) — PR [#7361](https://github.com/zed-industries/extensions/pull/7361) | fork `roberto-ayala/extensions`, rama `add-raylang`, clon `~/Dev/Rayala/labs/zed-extensions-fork` | `extensions.toml` (entrada `[raylang]` con `version`) + submódulo `extensions/raylang` |

Hasta que Zed fusione ese PR, la extensión se instala como **dev extension**: en Zed,
`zed: install dev extension` y elegir el clon de `zed-raylang`.

## Cuándo hay que tocarla

Cada vez que el lenguaje gane o cambie **sintaxis**: palabras clave, literales, escapes, tipos
primitivos, formas nuevas de declaración (`extern … blocking`, `if let`, sufijos `u8`…). Las
gramáticas de VSCode y Sublime son expresiones regulares y se arreglan en este repo (con las
aserciones de `editors/sublime/tests/syntax_test_raylang.ray`); la de Zed es un **parser**: si no
conoce la construcción nueva, no da error visible — la parsea como identificadores y simplemente no
la colorea (así pasó con `break`/`continue` hasta la 0.1.2). Lista de comprobación cuando cambie
la SPEC:

1. **`tree-sitter-raylang/grammar.js`**: añadir la regla (mirar `_statement`, `_expression`,
   `_type`, `integer_literal`, `escape_sequence`).
2. **`test/corpus/*.txt`**: un caso con el árbol esperado (el formato: título entre `===`, código,
   `---`, S-expresión). `npx tree-sitter test` muestra el diff si el árbol no coincide.
3. **`queries/highlights.scm`** (tree-sitter, captures nvim: `@keyword.repeat`, `@comment.documentation`…)
   **y** `zed-raylang/languages/raylang/highlights.scm` (captures de Zed: `@keyword`, `@comment.doc`…):
   las dos, porque son listas distintas.
4. Verificar sin instalar nada en Zed:

   ```sh
   cd ~/Dev/Rayala/labs/tree-sitter-raylang
   npx tree-sitter generate && npx tree-sitter test
   npx tree-sitter parse  prueba.ray                      # sin ERROR/MISSING en el árbol
   npx tree-sitter query  queries/highlights.scm prueba.ray
   npx tree-sitter query  ../zed-raylang/languages/raylang/highlights.scm prueba.ray
   ```

   (`tree-sitter` avisa de que no hay `parser directories` configurados: es inofensivo para
   `parse`/`query` dentro del directorio de la gramática.)

## Cómo se publica

1. Commit y push en `tree-sitter-raylang` (incluye `src/parser.c` generado: Zed compila desde el
   repo, sin `npm`). Anota el sha.
2. En `zed-raylang/extension.toml`: `rev = "<sha>"` bajo `[grammars.raylang]` y sube `version`
   (semver propio de la extensión: 0.1.2 = gramática de raylang 1.20.0). Commit y push.
3. Registro: en el clon del fork, `git -C extensions/raylang fetch origin <sha-de-zed-raylang> &&
   git -C extensions/raylang checkout <sha>`, sube `version` en la entrada `[raylang]` de
   `extensions.toml`, commit `Update raylang extension to vX.Y.Z` y push a la rama `add-raylang`:
   el PR #7361 se actualiza solo. Cuando el PR esté fusionado, las versiones siguientes van como
   PRs nuevos contra `zed-industries/extensions` con el mismo contenido (submódulo + versión).

Es un push a repos ajenos a este: al hacerlo desde una sesión automatizada, avisar antes.

## Qué NO hay que sincronizar

- La lista de **builtins** de las gramáticas de VSCode/Sublime (M244): las queries de Zed no tienen
  lista; toda llamada va a `@function` por estructura.
- El LSP: `src/` de la extensión solo localiza `ray` en el PATH y lanza `ray lsp`; las capacidades
  nuevas del servidor llegan solas.

## Historial

- 0.1.0 (ago 2026): primera versión; PR #7361 al registro.
- 0.1.1 (12 sep 2026): sufijos `u8`/`u32`/`u64` en los literales enteros.
- 0.1.2 (12 sep 2026): `break`/`continue` como sentencias; `///` como comentario de documentación.
