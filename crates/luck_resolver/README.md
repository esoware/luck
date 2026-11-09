# luck_resolver

Module resolution for `require()` calls across Lua 5.x and Luau.

## Overview

The resolver maps the string argument of a `require()` call to a filesystem path. The two language flavors take different approaches: Lua uses template-based search paths, Luau uses explicit relative paths with optional alias prefixes. This crate implements both, and on Luau it reports an ambiguity diagnostic when more than one candidate exists.

## API

A `Resolver` serves one build. It takes the `LuaVersion`, which picks the Lua 5.x or Luau scheme, and the Lua search paths and project root, which Luau ignores. On Luau it owns the `.luaurc` alias cache built up during resolution. Each `require()` is described by a `ResolveRequest` (the require string, the requiring file, and the call span) and resolved to a `ResolvedModule` (a normalized path plus any warnings):

```rust
let mut resolver = Resolver::new(version, &search_paths, project_root);
let resolved = resolver.resolve(&ResolveRequest { /* ... */ })?;
```

The cache lives on the resolver, not in global state, and is never invalidated. Create a fresh `Resolver` per build, as the bundler does once per `DependencyGraph::build`, and stale alias data drops with it.

`luck_core::normalize_path` gives the form every resolved path takes: canonical when the file exists, with forward slashes and no Windows extended-length prefix. The bundler keys modules on it, and the requiring file passed in a `ResolveRequest` must use it too. `luck_core::native_path` turns such a path back into the platform's own separators for filesystem calls.

## Key features

- **Lua 5.x template paths.** Substitutes the module name, with dots turned into slashes, into template strings (`./?.lua`, `./lib/?/init.lua`, ...) and returns the first hit. An empty list means `luck_core::DEFAULT_SEARCH_PATHS`, so builds and editor links read `"search_paths": []` the same way.
- **Luau relative imports.** `./module`, `../module`, with init-file resolution and `.luau` / `.lua` extension probing. A require is walked one component at a time, as Luau's navigator does.
- **Luau aliases.** `@utils` and similar prefixes resolve through `.luaurc` files discovered up the directory tree, with the closest definition winning. An alias may name another alias, as in `"outer": "@inner/sub"`.
- **`@self`** is a built-in Luau alias resolving to the current file's own directory, including for init files, with no `.luaurc` entry needed. A `.luaurc` alias named `self` can't override it and raises W004.
- **Ambiguity detection.** When both `.luau` and `.lua` exist, both `init.luau` and `init.lua` exist, or a file exists beside a same-named directory, the resolver reports E007 instead of silently picking one. That applies to every component of a require and to the requiring module itself.

## Architecture

### Lua 5.x resolution

`lua.rs` implements the template resolver. For `require("foo.bar")`, the resolver:

1. Replaces every `.` in the require string with `/`, turning `foo.bar` into `foo/bar`, and drops any leading `/` that produces so `.foo` stays under the project root.
2. Substitutes the result for every `?` in each template.
3. Probes each candidate on disk in order, relative to the project root, and the first existing file wins.

### Luau resolution

`luau.rs` implements the relative-import and alias resolver, and `luaurc.rs` the `.luaurc` chain behind the aliases.

**Relative paths.** `./module` and `../module` resolve from the requiring file's directory. Each component is probed as `name.luau`, `name.lua`, and `name/` with an `init.luau` or `init.lua`, and at most one of the file and the directory may exist. The last component must name a module. `init` can't be named as a file and `.config` can't be named at all. Empty and `.` components are skipped, and `\` reads as `/`.

**Init file rule.** When the requiring file is itself an `init.lua` or `init.luau`, relative paths resolve from the parent's parent directory, the one containing the folder that holds the init file, and the `.luaurc` search starts there too, so the `.luaurc` beside an init file doesn't apply to it. This matches Roblox's resolver semantics.

**Alias prefixes.** Aliases like `@utils` map to directories defined in `.luaurc` files, and a relative target resolves against the directory of the `.luaurc` that declares it. The resolver walks upward from the requiring module's parent, reading each directory's `.luaurc` and caching its `aliases`. The reader follows Luau's own grammar: quoted keys, `//` and `--` comments, trailing commas, raw bytes rather than UTF-8, and a repeated key that merges. When multiple `.luaurc` files define the same alias, the closest one wins. Alias matching is ASCII case-insensitive, and when one file declares names that are equal after lowercasing, the last one wins, as in Luau. A file that doesn't parse, or that declares an invalid alias name, reports E004 for any alias that would have to be found at or above it. A merged alias map is cached only for directories that actually make alias requests, and each ancestor caches just its own raw table. Both caches live as long as the resolver, and a missing or malformed `.luaurc` is cached too.

### Extension preference

Neither extension is preferred. The resolver uses whichever of `.luau` and `.lua` exists, and when both exist for the same require string it reports E007 instead of choosing.
