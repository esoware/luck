# luck_lsp

Language Server Protocol implementation for the luck Lua/Luau toolchain.

## Capabilities

Text-document features:
- text document sync (open / change / save / close), incremental
- push diagnostics on open / change / save (`luck_linter`)
- formatting + range formatting (`luck_formatter`)
- hover with stdlib signature, deprecation, must-use / pure / Roblox markers
- completion (stdlib globals + namespace members + scope-visible locals + keywords)
- signature help (active-parameter tracking, typed parameter labels)
- document symbols (outline view for functions, methods, locals) and
  workspace symbols (substring search over open documents' outlines)
- go-to-definition (local symbols via the scope tree; `require("path")`
  strings jump to the resolved module file)
- find references and rename (local symbols only; rename refuses globals,
  stdlib names, and any new name that would change meaning)
- code actions: per-diagnostic auto-fix, `source.fixAll.luck`,
  "disable rule for this line"
- semantic tokens (full document and range)
- document highlights (occurrences of the symbol under cursor)
- folding ranges (block constructs)
- selection ranges (smart-expand selection)
- document links (clickable `require()` paths, resolved through
  `luck_resolver` so Lua search paths, Luau relative / `@alias` imports, and
  the `init` parent-parent rule all match the bundler)

Custom requests:
- `luck/syntaxTree` returns a debug AST dump for the requested document
- `luck/fixAllWorkspace` returns a server-computed WorkspaceEdit applying
  every available fix across every open document

## Config

The server reads `luck.json` from any parent directory of an
opened file to pick up `target`, `format`, `lint`, and `search_paths`
settings. With no config it falls back to Lua 5.4 (or Luau if the file
extension is `.luau`), the formatter defaults, linting off, and the default Lua
search paths. Document-link resolution uses the config root as the Lua template
base, or the requiring file's own directory when no config is found.

## Build

`luck_lsp` is a library crate with no binary of its own (`[lib]` only, no
`[[bin]]` in its `Cargo.toml`). The server is served by the `luck_cli`
binary's `lsp` subcommand, which calls `luck_lsp::serve_stdio` or
`luck_lsp::serve_socket`.

```sh
cargo build -p luck_cli --release
```

## Transports

```sh
luck lsp                    # stdio (the default, and what every editor uses)
luck lsp --socket 9257      # TCP on 127.0.0.1:9257, useful for debugging
```

## Editor integration

### VS Code

Use the bundled `luck.luck` extension under `editors/vscode/`. It ships the
server binary, wires every capability above, and registers commands for
restart, show output, view syntax tree, and apply-all-fixes.

### Neovim (nvim-lspconfig)

```lua
require("lspconfig.configs").luck_lsp = {
  default_config = {
    cmd = { "luck", "lsp" },
    filetypes = { "lua", "luau" },
    root_dir = require("lspconfig.util").root_pattern("luck.json", ".luaurc", ".git"),
    single_file_support = true,
  },
}
require("lspconfig").luck_lsp.setup({})
```

Format-on-save is then a one-liner with `vim.lsp.buf.format` in a
`BufWritePre` autocommand.
