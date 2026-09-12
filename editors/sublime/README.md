# Paquete de Sublime Text 4 para raylang

> **Espejo publicado**: [`ray-language/sublime-raylang`](https://github.com/ray-language/sublime-raylang)
> (tag `1.0.0`), [aceptado en Package Control](https://github.com/sublimehq/package_control_channel/pull/9534)
> (sep 2026): Package Control → *Install Package* → `raylang`. Para desarrollo, clona el espejo en
> `Preferences → Browse Packages…` como `raylang` (o la copia manual de abajo).

Soporte para archivos `.ray` en **Sublime Text 4**: **resaltado de sintaxis** + **diagnósticos
en vivo** (vía el Language Server `raylang --lsp`).

Es el equivalente a la extensión de VSCode (`editors/vscode/`), con la misma división:

| Pieza | Archivo | Qué hace |
|-------|---------|----------|
| Coloreado | `raylang.sublime-syntax` | clasifica tokens (palabras clave, tipos, literales…) con los **mismos scopes** que la gramática de VSCode |
| Comentarios | `Comments.tmPreferences` | hace que `Ctrl/Cmd+/` inserte `//` |
| Diagnósticos | *(config del paquete LSP)* | conecta `raylang --lsp`; ver abajo |

El coloreado es **estático** (no entiende el programa); la validación real —errores de léxico,
sintaxis y tipos subrayados mientras escribes— la da el **Language Server**, igual que en los
demás editores. La lógica vive toda en el binario de Rust; aquí no se duplica nada.

## Instalación del coloreado

Sublime carga los paquetes desde su carpeta `Packages`. Copia (o enlaza) esta carpeta ahí con el
nombre `raylang`:

1. En Sublime: **Preferences → Browse Packages…** (abre la carpeta `Packages`).
2. Enlaza esta carpeta dentro, como `raylang`:

   ```sh
   # macOS (ajusta la ruta de Packages si difiere)
   ln -s "$(pwd)" ~/"Library/Application Support/Sublime Text/Packages/raylang"

   # Linux
   ln -s "$(pwd)" ~/.config/sublime-text/Packages/raylang
   ```

3. Abre cualquier `.ray`. Abajo a la derecha debería decir **raylang**; si no, elígelo en el
   selector de sintaxis (o **View → Syntax → raylang**). Los templates compilados (`*.ray.html`,
   M55) usan la sintaxis **raylang template** (HTML + los delimitadores `{{ }}`/`{% %}` con la
   expresión coloreada como raylang) y reciben del mismo LSP los diagnósticos del template y,
   dentro de `{{ }}`/`{% %}`: autocompletado (params tipados, variables de `for`, y tras un `.`
   los miembros del receptor), hover con el tipo real, ir-a-definición, signature help,
   find-references/rename/highlight, outline y snippets de bloque (`for`/`if` insertan el bloque
   entero con placeholders).

No hay que compilar nada: el `.sublime-syntax` es declarativo (a diferencia del cliente de
VSCode, que sí es TypeScript a compilar).

## Tests de la gramática (`tests/syntax_test_raylang.ray`)

La gramática tiene sus aserciones en el formato estándar de Sublime: cada línea de código va
seguida de comentarios `// ^^^ scope` que dicen qué scope se espera en esas columnas (M243, 881
aserciones sobre todos los contextos: comentarios y `///`, strings con escapes e interpolación,
templates, bytes, chars, anotaciones, definiciones, `impl … for`, `extern … blocking`, genéricos,
`match`, `if let`, literales con base y sufijo, exponentes, builtins, pipelines, operadores). El
archivo es además un programa raylang válido: el CI lo compila y corre
(`tests/sublime_syntax_test.rs`), comprueba que cada scope aseverado exista en las dos gramáticas
(Sublime y VSCode) y que toda palabra clave del lexer tenga regla.

Para ejecutar las aserciones de verdad:

- **En Sublime**: abre el archivo con el paquete instalado como `raylang` y lanza **Build**
  (`Ctrl/Cmd+B`); el panel lista cada aserción que falla con su línea y columna.
- **Con el tokenizador de ray-sublime** (el editor escrito en raylang, que suma este archivo a su
  corpus de cobertura): `ray run tools/syntax_tests.ray raylang v` desde su repo.

Al tocar la gramática, añade la aserción primero y haz que pase; al añadir sintaxis al lenguaje,
añade su línea aquí. La gramática de VSCode (`editors/vscode/syntaxes/raylang.tmLanguage.json`)
recibe el mismo cambio en tándem: comparten scopes.

## Diagnósticos en vivo (Language Server)

Sublime Text 4 **no trae LSP de fábrica**: usa el paquete **LSP** de
[sublimelsp](https://github.com/sublimelsp/LSP), instalable desde **Package Control**. Una vez
instalado, se le declara el servidor de raylang. Es análogo a Neovim/Helix: solo configuración,
sin compilar un cliente.

1. **Compila el binario** de raylang y ponlo en el PATH (o anota su ruta):

   ```sh
   cargo build --release      # genera ./target/release/raylang
   ```

2. Instala el paquete **LSP** (Package Control → *Install Package* → `LSP`).

3. Declara el servidor de raylang. **Desde LSP 2.13 los clientes viven en
   `Packages/User/LanguageServers.sublime-settings`** (Preferences → Package Settings → LSP →
   Language Servers); en versiones anteriores iban dentro de `"clients"` en
   `LSP.sublime-settings` (LSP migra solo el formato viejo). En el archivo nuevo el cliente va
   al NIVEL SUPERIOR, sin envoltorio `clients`:

   ```jsonc
   {
     "raylang": {
       "enabled": true,
       // ⚠️ Usa la RUTA ABSOLUTA al binario, no solo "ray"/"raylang". Sublime es una app
       // de GUI y en macOS/Linux las apps de GUI NO heredan el PATH de tu shell (arrancan
       // con un PATH mínimo del sistema), así que un comando a secas falla con
       // "[Errno 2] No such file or directory: 'ray'" aunque tu terminal sí lo encuentre.
       "command": ["/Users/TU_USUARIO/.local/bin/ray", "lsp"],
       // El instalador (curl|sh) deja el binario ahí; si compilaste a mano, apunta a
       // ".../target/release/ray". Un símlink estable evita tener que reeditar tras recompilar.
       // El selector cubre los .ray Y los templates compilados .ray.html (M55).
       "selector": "source.raylang | text.html.raylang"
     }
   }
   ```

   > Si algo no arranca, `LSP: Troubleshoot Server Configuration` muestra el selector y el
   > comando EFECTIVOS — si no coinciden con lo que escribiste, hay una config vieja migrada
   > en `LanguageServers.sublime-settings` pisando la tuya.

4. Reabre un `.ray`. El paquete LSP lanza `ray lsp` y subraya los errores que el checker
   reporta (un error a la vez: el compilador es *fail-fast*; al corregirlo aparece el siguiente).

> **Por qué solo configuración.** El protocolo (LSP/JSON-RPC) lo implementa por un lado nuestro
> binario (`raylang --lsp`, sin dependencias) y por otro el paquete LSP de Sublime. El paquete de
> aquí solo aporta el coloreado y le dice a LSP *cómo arrancar* el servidor. Es la misma idea que
> en Neovim/Helix; VSCode es el único que necesita compilar un cliente propio porque su API de
> LSP es una librería de extensión, no un paquete aparte.

## Alcance

El servidor implementa el núcleo del protocolo: **diagnósticos** en vivo, **hover**,
**ir-a-definición** (cruza archivos), **find-references**, **rename** (seguro, cruza archivos),
**completado**, **signature help**, **formateo** del documento (el mismo de `ray fmt`),
**outline de símbolos** (*Goto Symbol in File*) y **resaltado de ocurrencias** del símbolo bajo el
cursor. El paquete LSP de Sublime los expone sin configuración extra una vez declarado el cliente
(`K` para hover, "Goto Definition", "Goto Symbol", "LSP: Format File", etc.).
