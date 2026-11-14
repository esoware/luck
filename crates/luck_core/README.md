# luck_core

Dialect targets and diagnostics shared across the luck toolchain.

## Overview

`luck_core` sits one level above `luck_token` and holds the types that crates share when neither depends on the other: the dialect a user names and diagnostics. It touches no files and depends on nothing but `luck_token`, so it links anywhere the core pipeline does. It has no configuration types. Each tool owns its options, and `luck_config` reads `luck.json` into them.

## Key features

- **Build targets.** `LuaTarget` enumerates the supported flavors (Lua 5.1 through 5.5, standalone Luau, and Roblox Luau) and parses from common string forms (`"lua54"`, `"5.4"`, `"luau"`, `"roblox"`). It projects onto the two independent axes the rest of the workspace keys off: `lua_version()` (syntax) and `stdlib_environment()` (stdlib).
- **Diagnostics.** `Diagnostic` carries a code, message, severity, file path, span, and help text. Its numbered `E002`-`E013` / `W001`-`W007` constructors, such as `Diagnostic::e008`, live in `diagnostic/codes.rs` and are the only way to build one with an `E` or `W` code.

Every item is exported once, from the crate root.

## Architecture

### Build targets

`LuaTarget` is the configuration-layer view of a dialect. It maps to `luck_token::LuaVersion` for the parser, lexer, codegen, and formatter, and to `luck_token::StdlibEnvironment` for the semantic layer, linter, and LSP. The split happens once at each entry boundary.

### Diagnostics

`Diagnostic` carries a code, message, severity, file path, source `Span`, and optional help text. Consumers never inline literal codes. They call the numbered constructors in `diagnostic/codes.rs`, such as `Diagnostic::e004`. Only a lint diagnostic, whose code is its rule's name, is built field by field. Codes marked "not emitted" have no constructor, and their numbers stay reserved:

| Code | Meaning |
|------|---------|
| E001 | require not at top of file (not emitted) |
| E002 | require argument not a string literal (only in `dynamic_require: "error"`) |
| E003 | require not assigned to local (not emitted) |
| E004 | module not found (lists searched paths) |
| E005 | circular dependency (not emitted, since cycles warn with W003) |
| E006 | write to `package.loaded` (Luau targets) |
| E007 | ambiguous module resolution |
| E008 | parse error |
| E009 | too many modules (bundle module-count limit exceeded) |
| E010 | cannot read file (IO error) |
| E011 | entry file not found (not emitted, since a missing entry reports E010) |
| E012 | file too large (byte-length limit exceeded) |
| E013 | construct the bundle's module wrapper would nest past the nesting cap |
| W001 | duplicate require |
| W002 | top-level vararg in bundled module (not emitted) |
| W003 | circular dependency between lazily-loaded modules |
| W004 | `.luaurc` alias `self` shadowed by built-in `@self` |
| W005 | require call not bundled (a call that doesn't reach the global `require`, or the global `require` used other than in a direct call) |
| W006 | Luau hot comment has no effect in a bundle |
| W007 | require argument not a string literal, so the call resolves at runtime |
