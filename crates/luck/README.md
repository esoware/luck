# luck

Umbrella crate re-exporting the luck toolchain as a single dependency.

## Overview

`luck` is the facade. Add it once instead of tracking each `luck_*` crate
individually. The crate carries no logic of its own — everything is a
re-export of the underlying `luck_*` crates.

The parse core — `token`, `lexer`, `ast`, `parser`, `core` — is always
present. Every downstream stage is behind its own Cargo feature, off by
default; the `full` feature turns all of them on.

## Re-exports

| Path | Source | Feature |
|------|--------|---------|
| `luck::token` | [`luck_token`](../luck_token) | always on |
| `luck::lexer` | [`luck_lexer`](../luck_lexer) | always on |
| `luck::ast` | [`luck_ast`](../luck_ast) | always on |
| `luck::parser` | [`luck_parser`](../luck_parser) | always on |
| `luck::core` | [`luck_core`](../luck_core) | always on |
| `luck::codegen` | [`luck_codegen`](../luck_codegen) | `codegen` |
| `luck::resolver` | [`luck_resolver`](../luck_resolver) | `resolver` |
| `luck::bundler` | [`luck_bundler`](../luck_bundler) | `bundler` (pulls in `resolver`) |
| `luck::minifier` | [`luck_minifier`](../luck_minifier) | `minifier` |
| `luck::formatter` | [`luck_formatter`](../luck_formatter) | `formatter` |
| `luck::semantic` | [`luck_semantic`](../luck_semantic) | `semantic` |
| `luck::linter` | [`luck_linter`](../luck_linter) | `linter` (pulls in `semantic`) |
| `luck::VERSION` | Crate version string | always on |

## Usage

The package publishes as `luck-lua` (the `luck` name on crates.io
belongs to an unrelated project), but the library it ships is named
`luck`, so imports are unaffected. Enable the features for the stages
you need, or `full` for all of them:

```toml
[dependencies]
luck-lua = { version = "0.1", features = ["bundler", "minifier"] }
```

```rust
use luck::bundler;
use luck::minifier;

let bundled = bundler::bundle(&entry_path, target, &search_paths, &project_root)?;
let minified = minifier::minify(&bundled.output, target, &Default::default(), "bundle.lua")?;
```

The underlying crates remain published and can still be depended on individually if you only need one piece.
