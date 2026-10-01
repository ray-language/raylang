# Editores

Extensiones oficiales sobre el LSP de raylang (`ray lsp`): diagnósticos en vivo, autocompletado,
hover, ir a definición, renombrar y formateo.

| Editor | Dónde |
|---|---|
| VS Code | [`vscode/`](vscode/) — publicada en el [marketplace](https://marketplace.visualstudio.com/items?itemName=ray-language.raylang) |
| Sublime Text | [`sublime/`](sublime/) — `Package Control: Install Package → raylang` ([espejo](https://github.com/ray-language/sublime-raylang)) |
| Zed | [`zed/`](zed/) — cómo mantener [zed-raylang](https://github.com/ray-language/zed-raylang) (tree-sitter + LSP) |
| Neovim · Helix | sin extensión: apuntan a `ray lsp` directo (abajo) |

## Neovim

Con el API nativo (`vim.lsp.start`), sin plugins:

```lua
vim.filetype.add({ extension = { ray = "ray" } })
vim.api.nvim_create_autocmd("FileType", {
  pattern = "ray",
  callback = function()
    vim.lsp.start({
      name = "raylang",
      cmd = { "ray", "lsp" },
      root_dir = vim.fs.root(0, { "ray.toml", ".git" }) or vim.fn.getcwd(),
    })
  end,
})
```

Con `nvim-lspconfig`, lo mismo registrando un servidor `raylang` con `cmd = { "ray", "lsp" }` y
`filetypes = { "ray" }`.

## Helix

En `~/.config/helix/languages.toml`:

```toml
[language-server.raylang]
command = "ray"
args = ["lsp"]

[[language]]
name = "raylang"
scope = "source.ray"
file-types = ["ray"]
roots = ["ray.toml"]
comment-token = "//"
indent = { tab-width = 4, unit = "    " }
language-servers = ["raylang"]
```

El formateo va por el LSP (`ray lsp` implementa `textDocument/formatting`); `ray fmt --write src/`
hace lo mismo desde la terminal.

El resaltado de sintaxis en Helix y Zed viene de la gramática
[tree-sitter-raylang](https://github.com/ray-language/tree-sitter-raylang); la de VS Code y
Sublime es la TextMate canónica de [raylang-grammar](https://github.com/ray-language/raylang-grammar).
