# luck_resolver

Module resolution for `require()` calls across Lua 5.x and Luau.

## Overview

The resolver maps the string argument of a `require()` call to a filesystem path. The two language flavors take different approaches: Lua uses template-based search paths, Luau uses explicit relative paths with optional alias prefixes. This crate implements both and emits an ambiguity diagnostic when more than one valid candidate exists.

## API

A `Resolver` owns the `.luaurc` alias cache built up during Luau resolution. Each `require()` is described by a `ResolveRequest` (the require string, the requiring file, target dialect, Lua search paths, project root, and the call span) and resolved to a `ResolvedModule` (a normalized path plus any warnings):

```rust
let mut resolver = Resolver::new();
let resolved = resolver.resolve(&ResolveRequest { /* ... */ })?;
```

The cache lives on the resolver, not in global state. Create a fresh `Resolver` per build, as the bundler does once per `build_graph`, and stale alias data drops with it. There is no cache to clear by hand.

## Key features

- **Lua 5.x template paths.** Substitutes the dotted module name into template strings (`./?.lua`, `./lib/?/init.lua`, ...) and returns the first hit.
- **Luau relative imports.** `./module`, `../module`, with init-file resolution and `.luau` / `.lua` extension probing.
- **Luau aliases.** `@utils` and similar prefixes resolve through `.luaurc` files discovered up the directory tree, with the closest definition winning.
- **`@self`** is a built-in Luau alias resolving to the current file's own directory, including for init files, with no `.luaurc` entry needed.
- **Ambiguity detection.** When both `.luau` and `.lua` exist, or both a file and `dir/init.luau` exist, the resolver emits diagnostic E007 rather than silently picking one.

## Architecture

### Lua 5.x resolution

`lib.rs` implements the template resolver. For `require("foo.bar")`, the resolver:

1. Replaces every `.` in the require string with `/`, turning `foo.bar` into `foo/bar`.
2. Substitutes the result into each template's `?` placeholder.
3. Probes each candidate on disk in order; the first existing file wins.

### Luau resolution

`luau.rs` implements the relative-import and alias resolver.

**Relative paths.** `./module` and `../module` resolve from the requiring file's directory, probing extensions in order: `.luau` then `.lua`. If a path resolves to a directory, the resolver tries `init.luau` and `init.lua` inside it.

**Init file rule.** When the requiring file is itself an `init.lua` or `init.luau`, relative paths resolve from the parent's parent directory, the one containing the folder that holds the init file. This matches Roblox's resolver semantics.

**Alias prefixes.** Aliases like `@utils` map to directories defined in `.luaurc` files. The resolver walks upward from the requiring file, discovering and caching `.luaurc` files per directory. When multiple `.luaurc` files define the same alias, the closest one wins. Alias matching is case-insensitive. The merged alias map is cached only for directories that actually receive alias requests, not eagerly for every ancestor. Cold requests merge borrowed raw tables without cloning the chain; warm requests, including `@self` shadow warnings, borrow the merged map. Both caches share the resolver's per-build lifetime, including cached missing or malformed files.

### Extension preference

With only one extension present, the resolver prefers `.luau` over `.lua`. When both exist for the same require string, it flags ambiguity rather than choosing.
