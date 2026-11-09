# luck

Umbrella crate re-exporting the luck toolchain as a single dependency.

## Overview

Add `luck` once instead of tracking each `luck_*` crate. It has no logic of its own. Every module is a re-export of the matching `luck_*` crate.

The parse core (`token`, `lexer`, `ast`, `parser`, and `core`) is always present. Every other stage sits behind its own Cargo feature, off by default, and the `full` feature turns all of them on.

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

The package publishes as `luck_lua` (the `luck` name on crates.io belongs to an unrelated project), but the library it ships is named `luck`, so code still imports `luck::...`. Enable the features for the stages you need, or `full` for all of them:

```toml
[dependencies]
luck = { package = "luck_lua", version = "0.4", features = ["bundler", "minifier"] }
```

```rust
use luck::bundler;
use luck::minifier;

let options = bundler::BundleOptions {
    version: target.lua_version(),
    search_paths: &search_paths,
    project_root: &project_root,
    dynamic_require,
};
let bundled = bundler::bundle(&entry_path, &options)?;
let minified = minifier::minify(&bundled.output, target.lua_version(), &Default::default())?;
```

The underlying crates stay published, so you can depend on a single one if that's all you need.
