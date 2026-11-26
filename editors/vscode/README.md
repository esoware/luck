# Luck for VS Code

Luau language support powered by the luck toolchain:

- **Diagnostics** from `luck_linter` and Luau's own type checker on every change, with the rules `luck lint` runs and the definitions `luck check` loads on the same project, each linking to its rule, with the places it's about as related locations, and unused code faded and deprecated code struck through, a `require` that doesn't resolve shown where it's named, as `luck build` reports it, and a `luck.json` that doesn't load shown as an error on the file
- **Formatting** + range formatting from `luck_formatter`
- **Hover** with the type at the cursor as Luau prints it, and the documentation a definition file has of what it names, Roblox's API and Luau's library included
- **Completion** from Luau's own autocomplete, with each entry's type, deprecation, and documentation
- **Signature help** from the callee's type, with active-parameter tracking
- **Go to definition**, find references, and rename for locals, and across the workspace for the members of a module's table, such as `util.greet` from `local util = require("./util")`, `require` paths jump to the module file, and a global, type, or member a definition file declares jumps to its declaration, in Luau's and Roblox's definitions too, which open read-only
- **Document and workspace symbols**
- **Code actions**: a quick fix per fix or suggestion, `source.fixAll.luck` for every fix that keeps what the code does (suggestions, such as renaming an unused variable to `_x`, stay quick fixes), "disable rule for this statement"
- **JS/TS-style highlighting**: variables, globals, properties, functions, types, and word operators use standard theme scopes, and semantic tokens add scope-aware classification and stdlib deprecation strikethrough
- **Document highlights**, folding ranges, selection ranges, document links
- **Snippets** for common Luau patterns
- **TextMate grammar** for Luau (type annotations, string interpolation, attributes, generics, and definition files' `declare` statements)
- Status bar item, dedicated output channels, restart-server command

Supports Luau, standalone or with the Roblox API. Every `.luau` file is read as Luau, and so is every `.lua` file unless `luck.treatLuaFilesAsLuau` is off.

## Quick start

1. Install the extension.
2. Open a `.luau` or `.lua` file. Parse errors show up right away.
3. Format with `Shift+Alt+F` or enable `editor.formatOnSave`.
4. Configure the definitions your Luau runs with, such as Roblox's API, formatter style, and lint rules by running `Luck: Create luck.json` from the command palette.

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

- Formatter style, lint rules, the definitions (such as `@roblox`), and globals are all read from `luck.json` / `.editorconfig` by the server. There are no editor settings for them.
- `luck.treatLuaFilesAsLuau` (on by default) opens `.lua` files as Luau. Turn it off to leave them in VS Code's Lua mode, untouched by Luck.
- `luck.server.path` is an explicit path to the `luck` binary, overriding the bundled one.
- `luck.trace.server` traces LSP messages for debugging.
