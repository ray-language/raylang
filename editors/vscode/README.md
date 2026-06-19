# Extensión de VSCode para raylang

Resaltado de sintaxis (syntax highlighting) para archivos `.ray`.

Esto es **solo coloreado**: clasifica palabras clave, tipos, literales, comentarios,
operadores, definiciones y llamadas a funciones para que el tema de color de tu
editor las pinte. No valida tipos ni detecta errores — esa parte (diagnostics en
vivo) llegará con el **Language Server** (ver `IDEAS.md`, sección de tooling/LSP).

## Qué colorea

| Categoría | Ejemplos | Scope TextMate |
|-----------|----------|----------------|
| Palabras clave de control | `if else while return` | `keyword.control` |
| Declaración / storage | `let var fn` | `storage.type` |
| Tipos primitivos | `int float bool string` | `support.type.primitive` |
| Booleanos | `true false` | `constant.language.boolean` |
| Números | `42`, `3.14` | `constant.numeric` |
| Cadenas y escapes | `"hola\n"` | `string.quoted.double` |
| Comentarios | `// ...` | `comment.line` |
| Builtin | `print(...)` | `support.function.builtin` |
| Definición de función | `fn fib` | `entity.name.function` |
| Llamadas | `fib(...)` | `entity.name.function` |
| Operadores | `+ - == && ->` ... | `keyword.operator` |

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
ln -s "$(pwd)" ~/.vscode/extensions/raylang-0.1.0

# Luego: recarga VSCode (Cmd/Ctrl+Shift+P → "Developer: Reload Window")
```

Abre cualquier archivo `.ray` (por ejemplo `examples/fib.ray`) y deberías ver el
coloreado. Si VSCode no lo reconoce, comprueba el selector de lenguaje (abajo a la
derecha) y elige "raylang".

## Empaquetar (opcional)

Para generar un `.vsix` instalable o publicarlo:

```sh
npm install -g @vscode/vsce
vsce package        # genera raylang-0.1.0.vsix
# Instalar el .vsix: Cmd/Ctrl+Shift+P → "Extensions: Install from VSIX..."
```

## Probar la gramática

VSCode trae una herramienta para inspeccionar qué scope recibe cada token:
`Cmd/Ctrl+Shift+P` → **"Developer: Inspect Editor Tokens and Scopes"**. Útil si
algún elemento no se colorea como esperas.
