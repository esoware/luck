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
- document symbols (outline view for functions, methods, locals)
- code actions: per-diagnostic auto-fix, `source.fixAll.luck`,
  "disable rule for this line"
- semantic tokens (full document)
- inlay hints (parameter-name hints for stdlib calls)
- document highlights (occurrences of the symbol under cursor)
- folding ranges (block constructs)
- selection ranges (smart-expand selection)
- document links (clickable `require()` paths)

Custom requests:
- `luck/syntaxTree` — debug AST dump for the requested document
- `luck/fixAllWorkspace` — server-computed WorkspaceEdit applying every
  available fix across every open document

## Config

The server reads `luck.json` from any parent directory of an
opened file to pick up `target` and `format` settings. With no config it falls
back to Lua 5.4 (or Luau if the file extension is `.luau`) and the formatter
defaults.

## Build

```sh
cargo build -p luck_lsp --release
```

The binary lands at `target/release/luck_lsp` (or `luck_lsp.exe` on Windows).

## Transports

```sh
luck_lsp                    # stdio (default — what every editor uses)
luck_lsp --stdio            # explicit stdio
luck_lsp --socket 9257      # TCP on 127.0.0.1:9257 — useful for debugging
```

## Editor integration

### VS Code

Use the bundled `luck.luck` extension under `editors/vscode/` — it ships the
server binary, wires every capability above, and registers commands for
restart / show output / view syntax tree / apply-all-fixes / etc.

### Neovim (nvim-lspconfig)

```lua
require("lspconfig.configs").luck_lsp = {
  default_config = {
    cmd = { "luck_lsp" },
    filetypes = { "lua", "luau" },
    root_dir = require("lspconfig.util").root_pattern("luck.json", ".luaurc", ".git"),
    single_file_support = true,
  },
}
require("lspconfig").luck_lsp.setup({})
```

Format-on-save is then a one-liner with `vim.lsp.buf.format` in a
`BufWritePre` autocommand.
