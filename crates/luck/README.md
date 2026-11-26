# luck_lua

The whole luck toolchain behind one dependency, for a build tool or editor that would rather not track each `luck_*` crate. The package is `luck_lua` and the library is `luck`, so code imports `luck::`.

## Features

The parsing crates are always present: `luck::allocator`, `luck::token`, `luck::lexer`, `luck::ast`, `luck::parser`, and `luck::diagnostics`. Every other stage is behind a feature of its name, and none is on by default.

| Feature | Adds |
| --- | --- |
| `codegen` | `luck::codegen`, the compact printer |
| `minifier` | `luck::minifier` |
| `formatter` | `luck::formatter` |
| `semantic` | `luck::semantic`, scoping, compile-time checks, the node table, and `require` sites, in one pass |
| `stdlib` | `luck::stdlib`, the standard library from Luau definition files, Luau's with Roblox's API or a host's definitions over it, and what a chunk reaches in it, and `semantic` |
| `linter` | `luck::linter`, and `stdlib` |
| `type_checker` | `luck::type_checker`, Luau's own type checker, built from Luau's C++, which can't share a binary with another build of Luau, such as `mlua`'s |
| `resolver` | `luck::resolver`, module resolution and source reading |
| `bundler` | `luck::bundler`, and `resolver` and `minifier` |
| `config` | `luck::config`, `luck.json` and `.editorconfig` loading, and the stages it configures |
| `full` | All of them |

The language server is not part of it. `luck::VERSION` is the toolchain's version.

## Usage

A tree lives in a `luck::allocator::Allocator` the caller owns, and borrows the source it was parsed from, so both outlive it. Every stage that builds nodes takes the arena first. `reset` frees everything in it at once and keeps the memory, so one arena serves file after file, and threads each take an arena from an `AllocatorPool`. The bundler reads modules on rayon's pool, each into the arena its thread gets from a `ThreadAllocators`.

```rust
use luck::allocator::Allocator;

let mut allocator = Allocator::new();
for source in [&b"local x = 1"[..], b"return x"] {
    let parsed = luck::parser::parse(&allocator, source);
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

let allocator = Allocator::new();
let formatted = luck::formatter::format(&allocator, b"local x=1", &FormatOptions::default());
assert_eq!(formatted.output, b"local x = 1\n");
```
