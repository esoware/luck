# Luck for VS Code

Lua/Luau language support powered by the luck toolchain:

- **Diagnostics** from `luck_linter` on every change (lint rules are opt-in through `luck.json`, and parse errors always show)
- **Formatting** + range formatting from `luck_formatter`
- **Hover** with stdlib signatures, deprecation, and must-use / pure markers
- **Completion** of stdlib globals, namespace members, locals, keywords, and constant string arguments such as Roblox service names
- **Signature help** with active-parameter tracking
- **Go to definition**, find references, and rename for locals, and `require` paths jump to the module file
- **Document and workspace symbols**
- **Code actions**: per-fix quickfix, `source.fixAll.luck`, "disable rule for this line"
- **JS/TS-style highlighting** for Lua and Luau: variables, globals, properties, functions, types, and word operators use standard theme scopes, and semantic tokens add scope-aware classification and stdlib deprecation strikethrough
- **Document highlights**, folding ranges, selection ranges, document links
- **Snippets** for common Lua patterns
- **TextMate grammar** for both Lua and Luau (type annotations, string interpolation, attributes, generics)
- Status bar item, dedicated output channels, restart-server command

Supports Lua 5.1, 5.2, 5.3, 5.4, 5.5, and Luau, standalone or with the Roblox API.

## Quick start

1. Install the extension.
2. Open a `.lua` or `.luau` file. Parse errors show up right away.
3. Format with `Shift+Alt+F` or enable `editor.formatOnSave`.
4. Configure target version, formatter style, and lint rules by running `Luck: Create luck.json` from the command palette.

## Commands

All commands are under the `Luck:` prefix in the command palette:

- Restart / Start / Stop Server
- Show Server Output, Show Extension Output, Show LSP Trace
- Show Server Version
- Format Document, Format Selection
- Apply All Fixes (File), Apply All Fixes (Workspace)
- Create luck.json
- Download Server Binary
- View Syntax Tree (Debug)
- Copy Debug Info, Report an Issue
- Open Getting Started Walkthrough

## Configuration highlights

- Formatter style, lint rules, target version, and globals are all read from `luck.json` / `.editorconfig` by the server (`.lua` files follow the `lua` key and `.luau` files the `luau` key). There are no editor settings for them.
- `luck.server.path` is an explicit path to the `luck` binary, overriding the bundled one.
- `luck.trace.server` traces LSP messages for debugging.
