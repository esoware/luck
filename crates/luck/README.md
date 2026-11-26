# luck

The full luck toolchain in one crate: a parser, formatter, linter, minifier, bundler, and type checker for Luau.

## Overview

It re-exports every luck crate as a module, except the language server and the CLI. The package is published as `luck_lua`, but you import it as `luck`.

The parser and what it needs are always included. Everything else is behind a feature.

## Usage

```sh
cargo add luck_lua --features formatter,minifier
```

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

`VERSION` is the toolchain version.

## Arenas

Every tree lives in an `allocator::Allocator` that you own. Call `reset` to free everything in it and reuse the memory for the next file. For threads, take arenas from an `AllocatorPool`.

## Cargo features

- `full` enables everything below.
- `codegen` adds the compact printer.
- `resolver` adds module resolution.
- `bundler` adds bundling, and enables `resolver` and `minifier`.
- `minifier` adds the minifier.
- `formatter` adds the formatter.
- `semantic` adds scope and symbol analysis.
- `stdlib` adds the standard library definitions, and enables `semantic`.
- `linter` adds the linter, and enables `stdlib`.
- `type_checker` adds Luau's type checker. Building it needs clang.
- `config` adds `luck.json` loading, and enables `bundler`, `minifier`, `formatter`, and `linter`.
