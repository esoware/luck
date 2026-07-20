# luck

Umbrella crate re-exporting the luck toolchain as a single dependency.

## Overview

`luck` is the facade. Add it once and you get the whole toolchain — lexer, parser, AST, codegen, resolver, bundler, minifier, formatter, semantic analysis, and linter — under one prefix. The crate carries no logic of its own — everything is a re-export of the underlying `luck_*` crates.

## Re-exports

| Path | Source |
|------|--------|
| `luck::token` | [`luck_token`](../luck_token) |
| `luck::lexer` | [`luck_lexer`](../luck_lexer) |
| `luck::ast` | [`luck_ast`](../luck_ast) |
| `luck::parser` | [`luck_parser`](../luck_parser) |
| `luck::codegen` | [`luck_codegen`](../luck_codegen) |
| `luck::core` | [`luck_core`](../luck_core) |
| `luck::resolver` | [`luck_resolver`](../luck_resolver) |
| `luck::bundler` | [`luck_bundler`](../luck_bundler) |
| `luck::minifier` | [`luck_minifier`](../luck_minifier) |
| `luck::formatter` | [`luck_formatter`](../luck_formatter) |
| `luck::semantic` | [`luck_semantic`](../luck_semantic) |
| `luck::linter` | [`luck_linter`](../luck_linter) |
| `luck::VERSION` | Crate version string |

## Usage

The package publishes as `luck-lua` (the `luck` name on crates.io
belongs to an unrelated project), but the library it ships is named
`luck`, so imports are unaffected:

```toml
[dependencies]
luck-lua = "0.1"
```

```rust
use luck::bundler;
use luck::minifier;

let bundled = bundler::bundle(&entry_path, target, &search_paths, &project_root)?;
let minified = minifier::minify(&bundled.output, target, &Default::default(), "bundle.lua")?;
```

Depend on this crate instead of pulling individual `luck_*` crates directly. The underlying crates remain published and can still be depended on individually if you only need one piece.
