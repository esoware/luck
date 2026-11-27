# luck

A Luau parser, formatter, linter, minifier, bundler, and type checker in one
crate.

This crate re-exports the luck crates as modules, so a build tool or editor can
depend on one crate instead of tracking each `luck_*` crate. It's published as
`luck_luau` but imported as `luck`. The lexer, parser, and syntax tree are
always included, and every other stage is behind its own feature. The language
server and the CLI aren't included.

Every tree lives in an `allocator::Allocator` that you own. Calling `reset`
frees everything in it at once and keeps the memory for the next file. For
threads, `allocator::AllocatorPool` lends out arenas that reset when they're
returned.

## Usage

This example needs the `formatter` and `minifier` features:

```rust
use luck::allocator::Allocator;
use luck::formatter::FormatOptions;
use luck::minifier::TransformConfig;

let allocator = Allocator::new();
let formatted = luck::formatter::format(&allocator, b"local x=1", &FormatOptions::default());
assert_eq!(formatted.output, b"local x = 1\n");

let minified = luck::minifier::minify(&allocator, b"return 1 + 2", &TransformConfig::default());
assert_eq!(minified.as_deref(), Ok(&b"return 3"[..]));
```

## Modules

| Module | Crate | Feature |
| --- | --- | --- |
| `allocator` | `luck_allocator` | always |
| `token` | `luck_token` | always |
| `lexer` | `luck_lexer` | always |
| `ast` | `luck_ast` | always |
| `parser` | `luck_parser` | always |
| `diagnostics` | `luck_diagnostics` | always |
| `codegen` | `luck_codegen` | `codegen` |
| `resolver` | `luck_resolver` | `resolver` |
| `bundler` | `luck_bundler` | `bundler` |
| `minifier` | `luck_minifier` | `minifier` |
| `formatter` | `luck_formatter` | `formatter` |
| `semantic` | `luck_semantic` | `semantic` |
| `stdlib` | `luck_stdlib` | `stdlib` |
| `linter` | `luck_linter` | `linter` |
| `type_checker` | `luck_type_checker` | `type_checker` |
| `config` | `luck_config` | `config` |

`VERSION` is the toolchain's version.

## Cargo features

No feature is on by default.

* `full` - Enables every feature below.
* `codegen` - The compact printer.
* `resolver` - Module resolution for `require`.
* `bundler` - Single-file bundling. Enables `resolver` and `minifier`.
* `minifier` - The minifier.
* `formatter` - The formatter.
* `semantic` - Scope, symbol, and reference analysis.
* `stdlib` - Luau's and Roblox's standard libraries. Enables `semantic`.
* `linter` - The linter. Enables `stdlib`.
* `type_checker` - Luau's own type checker. Building it needs clang.
* `config` - `luck.json` loading. Enables `bundler`, `minifier`, `formatter`,
  and `linter`.
