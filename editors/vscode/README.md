# Luck for VS Code

Lua/Luau language support powered by the luck toolchain:

- **Diagnostics** from `luck_linter` on every change
- **Formatting** + range formatting from `luck_formatter`
- **Hover** with stdlib signatures, deprecation, must-use, and Roblox markers
- **Completion** of stdlib globals, namespace members, and visible locals
- **Signature help** with active-parameter tracking
- **Document symbols** outline
- **Code actions**: per-fix quickfix, `source.fixAll.luck`, "disable rule for this line"
- **Semantic highlighting** (deprecated and Roblox-only entries get distinct modifiers)
- **Inlay hints** for stdlib parameters
- **Document highlights**, folding ranges, selection ranges, document links
- **Snippets** for common Lua patterns
- **TextMate grammar** for both Lua and Luau (type annotations, string interpolation, attributes, generics)
- Status bar item, dedicated output channels, restart-server command

Supports Lua 5.1, 5.2, 5.3, 5.4, 5.5, and Luau (with Roblox-runtime additions
tagged separately).

## Quick start

1. Install the extension.
2. Open a `.lua` or `.luau` file. Diagnostics appear automatically.
3. Format with `Shift+Alt+F` or enable `editor.formatOnSave`.
4. Configure target version and formatter style by running
   `Luck: Create luck.json` from the command palette.

## Commands

All commands are under the `Luck:` prefix in the command palette:

- Restart / Start / Stop Server
- Show Server Output, Show Extension Output, Show LSP Trace
- Format Document, Format Selection
- Apply All Fixes (File), Apply All Fixes (Workspace)
- Create luck.json
- View Syntax Tree (debug)
- Copy Debug Info, Report an Issue

## Configuration highlights

- Formatter style, lint rules, target version, and globals are all read
  from `luck.json` / `.editorconfig` by the server (`.luau` files always
  use Luau); there are no editor settings for them.
- `luck.server.path` — explicit path to the `luck` binary (overrides
  the bundled one).
- `luck.trace.server` — trace LSP messages for debugging.
