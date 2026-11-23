# luck_lua

The whole luck toolchain behind one dependency, for a build tool or editor that would rather not track each `luck_*` crate. The package is `luck_lua` and the library is `luck`, so code imports `luck::`.

## Features

The parsing crates are always present: `luck::allocator`, `luck::token`, `luck::lexer`, `luck::ast`, `luck::parser`, `luck::core`, and `luck::diagnostics`. Every other stage is behind a feature of its name, and none is on by default.

| Feature | Adds |
| --- | --- |
| `codegen` | `luck::codegen`, the compact printer |
| `minifier` | `luck::minifier` |
| `formatter` | `luck::formatter` |
| `semantic` | `luck::semantic`, scope analysis and the stdlib catalogs |
| `linter` | `luck::linter`, and `semantic` |
| `resolver` | `luck::resolver`, module resolution and source reading |
| `bundler` | `luck::bundler`, and `resolver` and `minifier` |
| `config` | `luck::config`, `luck.json` and `.editorconfig` loading, and the stages it configures |
| `full` | All of them |

The language server is not part of it. `luck::VERSION` is the toolchain's version.

## Usage

A tree lives in a `luck::allocator::Allocator` the caller owns, and borrows the source it was parsed from, so both outlive it. Every stage that builds nodes takes the arena first. `reset` frees everything in it at once and keeps the memory, so one arena serves file after file, and threads each take an arena from an `AllocatorPool`. The bundler reads modules on rayon's pool, each into the arena its thread gets from a `ThreadAllocators`.

```rust
use luck::allocator::Allocator;
use luck::token::LuaVersion;

let mut allocator = Allocator::new();
for source in ["local x = 1", "return x"] {
    let parsed = luck::parser::parse(&allocator, source, LuaVersion::Lua54);
    assert!(parsed.errors.is_empty());
    // The parse borrows the arena, so it goes first.
    drop(parsed);
    allocator.reset();
}
```

With the `formatter` feature:

```rust
use luck::allocator::Allocator;
use luck::formatter::FormatOptions;
use luck::token::LuaVersion;

let allocator = Allocator::new();
let formatted =
    luck::formatter::format(&allocator, "local x=1", LuaVersion::Lua54, &FormatOptions::default());
assert_eq!(formatted.output, "local x = 1\n");
```
