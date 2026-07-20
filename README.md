<div align="center">

<a href="https://github.com/esoware/luck">
  <img src="assets/banner.png" alt="Luck" width="640">
</a>

[![MIT licensed][license-badge]][license-url]
[![Lua][lua-badge]][lua-url]
[![Rust][rust-badge]][rust-url]
[![CodSpeed][codspeed-badge]][codspeed-url]

</div>

Luck is a collection of high-performance tools for Lua 5.1 through 5.5 & Luau (standalone and Roblox), written in Rust: a formatter, linter, language server, parser, bundler and minifier behind one CLI and one config file. Multi-file projects in, a single file out.

The lexer, parser, AST, and code generator are all hand-written, with no external parser dependency, so every tool works from the same exact syntax tree.

## Quick Start

```sh
cargo install luck_cli
```

```sh
luck init          # scaffold a luck.json
luck build         # bundle the project into a single file
luck fmt src/      # format
luck lint src/     # lint (add --fix to auto-fix)
luck check src/    # everything at once
```

Configuration lives in a single `luck.json`, discovered by walking up from the working directory. Lint suppressions, formatter toggles, and per-dialect targets are all driven from it — see the schema shipped with the [VS Code extension](editors/vscode).

## Tools

- **Bundler** — resolves `require` calls across Lua search paths, Luau relative imports, `@aliases`, and `.luaurc` chains; emits a single self-contained file with no loader or runtime library.
- **Minifier** — a 12-transform AST pipeline (dead code removal, constant folding, variable renaming, and more), each pass individually configurable, all metamethod-safe.
- **Formatter** — Prettier-style, line-width-aware layout with full Luau type annotation support. Formatting is idempotent and output is guaranteed to re-parse.
- **Linter** — 64 rules across correctness, suspicious, style, and performance categories, with inline suppressions and `--fix`.
- **Language server** — hover, completions, diagnostics, and more, served via `luck lsp` and consumed by the [VS Code extension](editors/vscode).

Every tool is also a library crate (`luck_parser`, `luck_formatter`, `luck_linter`, ...) re-exported through the `luck` facade crate, so you can build on the same infrastructure the CLI uses.

## Contribute

Issues and pull requests are welcome. If you want to poke around, `CLAUDE.md` documents the architecture, crate layout, and invariants the codebase holds itself to.

## License

Luck is free and open-source software licensed under the [MIT License](LICENSE).

[license-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[license-url]: LICENSE
[lua-badge]: https://img.shields.io/badge/Lua-5.1%20--%205.5%20%7C%20Luau-2C2D72?logo=lua
[lua-url]: https://www.lua.org
[rust-badge]: https://img.shields.io/badge/Rust-1.88%2B-orange?logo=rust
[rust-url]: https://www.rust-lang.org
[codspeed-badge]: https://img.shields.io/endpoint?url=https://codspeed.io/badge.json
[codspeed-url]: https://app.codspeed.io/esoware/luck?utm_source=badge
