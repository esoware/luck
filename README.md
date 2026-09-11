<div align="center">

<a href="https://github.com/esoware/luck">
  <img src="assets/banner.png" alt="Luck" width="640">
</a>

[![MIT licensed][license-badge]][license-url]
[![Lua][lua-badge]][lua-url]
[![Rust][rust-badge]][rust-url]
[![CodSpeed][codspeed-badge]][codspeed-url]

</div>

Luck is a Lua toolchain written in Rust: formatter, linter, language server, parser, bundler, and minifier behind one CLI and one config file. It covers Lua 5.1 through 5.5 and Luau, standalone and Roblox. Multi-file projects in, a single file out.

The lexer, parser, AST, and code generator are hand-written, with no external parser dependency, so every tool reads the same syntax tree.

## Quick start

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

Configuration lives in a single `luck.json`, discovered by walking up from the working directory. Lint suppressions, formatter toggles, and per-extension targets all come from it. The `lua` and `luau` keys each name any dialect, so a Roblox or Rojo tree that keeps Luau in `.lua` files sets `"lua": "roblox"`. See the schema shipped with the [VS Code extension](editors/vscode).

## Tools

- **Bundler.** Resolves `require` calls across Lua search paths, Luau relative imports, `@aliases`, and `.luaurc` chains, then emits one self-contained file with no external loader or runtime library.
- **Minifier.** A twelve-pass AST pipeline. Dead code removal, constant folding, and local renaming do most of the work; every pass toggles on its own, and all of them assume metamethods can fire.
- **Formatter.** Prettier-style layout that breaks on line width and handles the full Luau type grammar. `format(format(x)) == format(x)` and the output re-parses, both enforced by tests.
- **Linter.** 65 rules across correctness, suspicious, style, and performance categories, with inline suppressions and `--fix`.
- **Language server.** Hover, completions, diagnostics, go-to-definition, rename, and semantic tokens, served by `luck lsp` and consumed by the [VS Code extension](editors/vscode).

Every tool is also a library crate (`luck_parser`, `luck_formatter`, `luck_linter`, ...) re-exported through the `luck` facade crate, so you can build on the same pieces the CLI uses.

## Contribute

Issues and pull requests are welcome. `ARCHITECTURE.md` documents the pipeline, crate layout, and design decisions. `AGENTS.md` holds the conventions and invariants the codebase holds itself to.

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
