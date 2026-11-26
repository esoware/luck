# luck_resolver

Resolves Luau `require` paths to files, including relative paths and `.luaurc` aliases.

## Overview

It follows Luau's require-by-string rules. `./module` and `../module` are relative to the requiring file, and `@alias` comes from `.luaurc` and `.config.luau` files in parent directories.

`ResolverGeneric` reads through a `FileSystem` trait, so it works on files that only exist in memory. It caches lookups across threads until `clear_cache`.

A `.config.luau` is Luau code, so you pass in a function that runs it, such as `luck_type_checker::luau_config_aliases`.

The luck crates above this one read every file through it.

## Usage

```rust,no_run
use std::path::Path;

use luck_resolver::Resolver;

let resolver = Resolver::new();
let resolution = resolver.resolve(Path::new("main.luau"), "./foo").unwrap();
assert!(resolution.path.ends_with("foo.luau"));
```

## API

- `Resolver` resolves against the OS file system.
- `ResolverGeneric` resolves against any `FileSystem`, with `new_with_file_system`, `with_luau_config`, `resolve`, and `clear_cache`.
- `Resolution` holds the resolved path, and whether `@self` shadowed a `.luaurc` alias.
- `ResolveError` says why a require didn't resolve, and `to_diagnostic` turns it into a diagnostic.
- `LuauConfigAliases` is the function type that reads a `.config.luau`'s aliases.
- `FileSystem` is the file system trait, and `FileSystemOs` is the OS's. `DirEntry` and `FileMetadata` are what it returns.
- `lexically_normalized` resolves `.` and `..` without touching the disk.
- `to_slash_lossy` writes a path with forward slashes.
- `is_definition_file` checks whether a path names a `.d.luau` file.
