<div align="center">

<a href="https://github.com/esoware/luck">
  <img src="assets/banner.png" alt="Luck" width="640">
</a>

[![MIT licensed][license-badge]][license-url] [![Luau][luau-badge]][luau-url] [![Rust][rust-badge]][rust-url] [![CodSpeed][codspeed-badge]][codspeed-url]

</div>

Luck is a Luau toolchain written in Rust: formatter, linter, language server, parser, bundler, and minifier behind one CLI and one config file. It covers standalone Luau and Luau on Roblox. Multi-file projects in, a single file out.

The lexer, parser, AST, and code generator are hand-written with no external parser dependency, and every tool works on the same syntax tree.

## Quick start

```sh
cargo install luck_cli
```

```sh
luck init          # scaffold a luck.json
luck build         # bundle the project into a single file
luck fmt src/      # format
luck lint src/     # lint (add --fix to auto-fix)
luck check src/    # report syntax and compile errors
```

Configuration lives in `luck.json`. `lint`, `fmt`, `check`, and the language server give each file the nearest one above it, `build` takes the one above the working directory, and `bundle`, `graph`, and `minify` the one above their input. Build entries, minifier passes, format options, lint rule settings, and the Luau variant all come from it, and `overrides` change the lint and format settings of the files their globs match. Every `.lua` and `.luau` file is Luau, `variant` picks the standard library they run against, `"standalone"` or `"roblox"`, and `std` names std definition files, TOML that describes a host's globals, fields, and functions, such as Lune's, in the [format](crates/luck_stdlib/README.md#std-definitions) of luck's own standard library catalogs, which the linter and the language server read as they read the built-in ones. A mistake in it is reported where it is, at load. See the schema shipped with the [VS Code extension](editors/vscode).

## Tools

- **Bundler.** Resolves Luau's string requires, relative paths, `@aliases`, and `.luaurc` chains, then emits one self-contained file with no external loader or runtime library.
- **Minifier.** Eleven AST transforms that rewrite the tree in place, in three phases: Luau type stripping, compression passes that repeat until one changes nothing, and renaming, each reading the scopes and references semantic analysis finds. Dead code removal, constant folding, and local renaming do most of the work. Config flags turn each transform on and off, and every transform assumes metamethods can fire.
- **Formatter.** Prettier-style layout that breaks on line width and handles the full Luau type grammar. `format(format(x)) == format(x)` and the output re-parses, both enforced by tests.
- **Linter.** 64 rules across correctness, suspicious, style, and performance categories, with inline suppressions and `--fix`.
- **Language server.** Hover, completions, diagnostics, go-to-definition, references, rename, and semantic tokens, with a module's members followed across the workspace, served by `luck lsp` and consumed by the [VS Code extension](editors/vscode).

Every tool is also a library crate (`luck_parser`, `luck_formatter`, `luck_linter`, ...). The `luck` facade crate, published as `luck_lua`, re-exports all of them except the language server, so you can build on the same pieces the CLI uses.

## Contribute

Issues and pull requests are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) covers setup and pull requests, and `AGENTS.md` lists the conventions and invariants the code follows.

## License

Luck is free and open-source software licensed under the [MIT License](LICENSE).

[license-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[license-url]: LICENSE
[luau-badge]: https://img.shields.io/badge/Luau-standalone%20%7C%20Roblox-00A2FF
[luau-url]: https://luau.org
[rust-badge]: https://img.shields.io/badge/Rust-1.99%2B-orange?logo=rust
[rust-url]: https://www.rust-lang.org
[codspeed-badge]: https://img.shields.io/endpoint?url=https://codspeed.io/badge.json
[codspeed-url]: https://app.codspeed.io/esoware/luck?utm_source=badge
