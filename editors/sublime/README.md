# Paquete de Sublime Text 4 para raylang

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
   selector de sintaxis (o **View → Syntax → raylang**).

No hay que compilar nada: el `.sublime-syntax` es declarativo (a diferencia del cliente de
VSCode, que sí es TypeScript a compilar).

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

3. Abre **Preferences → Package Settings → LSP → Settings** y añade el cliente de raylang
   dentro de `clients`:

   ```jsonc
   {
     "clients": {
       "raylang": {
         "enabled": true,
         // Si 'raylang' no está en el PATH, pon la ruta absoluta al binario:
         //   "command": ["/ruta/a/target/release/raylang", "--lsp"],
         "command": ["raylang", "--lsp"],
         "selector": "source.raylang"
       }
     }
   }
   ```

4. Reabre un `.ray`. El paquete LSP lanza `raylang --lsp` y subraya los errores que el checker
   reporta (un error a la vez: el compilador es *fail-fast*; al corregirlo aparece el siguiente).

> **Por qué solo configuración.** El protocolo (LSP/JSON-RPC) lo implementa por un lado nuestro
> binario (`raylang --lsp`, sin dependencias) y por otro el paquete LSP de Sublime. El paquete de
> aquí solo aporta el coloreado y le dice a LSP *cómo arrancar* el servidor. Es la misma idea que
> en Neovim/Helix; VSCode es el único que necesita compilar un cliente propio porque su API de
> LSP es una librería de extensión, no un paquete aparte.

## Alcance

Diagnósticos (M10.2). Hover e ir-a-definición quedan para un futuro M10.2b (exigirían exponer una
API de tipos del checker y un índice de símbolos).
