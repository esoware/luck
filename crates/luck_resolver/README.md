# luck_resolver

Module resolution for `require()` calls across Lua 5.x and Luau.

## Overview

The resolver maps the string argument of a `require()` call to a filesystem path. The two language flavors take different approaches: Lua uses template-based search paths, Luau uses explicit relative paths with optional alias prefixes. This crate implements both and emits an ambiguity diagnostic when more than one valid candidate exists.

## Key Features

- **Lua 5.x template paths** — substitutes the dotted module name into template strings (`./?.lua`, `./lib/?/init.lua`, …) and returns the first hit.
- **Luau relative imports** — `./module`, `../module`, with init-file resolution and `.luau` / `.lua` extension probing.
- **Luau aliases** — `@utils` and similar prefixes resolve through `.luaurc` files discovered up the directory tree, with the closest definition winning.
- **`@self`** — built-in Luau alias resolving to the current file's directory (or parent's parent for init files), without needing a `.luaurc` entry.
- **Ambiguity detection** — when both `.luau` and `.lua` exist, or both a file and `dir/init.luau` exist, the resolver emits diagnostic E007 rather than silently picking one.

## Architecture

### Lua 5.x Resolution

`lib.rs` implements the template resolver. For `require("foo.bar")`, the resolver:

1. Replaces every `.` in the require string with the OS path separator (`foo.bar` → `foo/bar`).
2. Substitutes the result into each template's `?` placeholder.
3. Probes each candidate on disk in order; the first existing file wins.

### Luau Resolution

`luau.rs` implements the relative-import and alias resolver.

**Relative paths** — `./module` and `../module` resolve from the requiring file's directory, probing extensions in order: `.luau` then `.lua`. If a path resolves to a directory, `init.luau` and `init.lua` inside it are tried.

**Init file rule** — when the requiring file is itself an `init.lua` or `init.luau`, relative paths resolve from the parent's parent directory (the directory containing the folder that holds the init file). This matches Roblox's resolver semantics.

**Alias prefixes** — aliases like `@utils` map to directories defined in `.luaurc` files. The resolver walks upward from the requiring file, discovering and caching `.luaurc` files per directory. When multiple `.luaurc` files define the same alias, the closest one wins. Alias matching is case-insensitive.

### Extension Preference

When only one extension exists, `.luau` is preferred over `.lua`. When both exist for the same require string, the resolver flags ambiguity rather than choosing.
