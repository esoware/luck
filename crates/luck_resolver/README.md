# luck_resolver

Maps the string a `require` call names to the file it loads, as Luau's require by string does.

## Overview

A require resolves `./` and `../` paths from the requiring file's directory, and `@alias` prefixes from the configs up the directory tree, each directory's `.luaurc` or `.config.luau`. The path it ends at is a module when it names a `.luau` or `.lua` file, or a directory with an `init.luau` or `init.lua`, and a require that starts with none of `./`, `../`, and `@` names no module. Nor does a definition file, which `is_definition_file` tells by its name ending in `.d.luau`.

A `.config.luau` is Luau's config written in Luau, which returns its keys in a `luau` table, so only running it in Luau's sandbox says what aliases it declares. This crate runs no Luau, so a resolver reads them through the `LuauConfigAliases` its caller gives it with `with_luau_config`, such as `luck_type_checker::luau_config_aliases`, Luau's own Config library, and without one, a `.config.luau` doesn't load. As in Luau, a config that doesn't load, or a directory with both a `.luaurc` and a `.config.luau`, hides every alias declared above it.

A resolver reads through a `FileSystem`, so a caller can resolve against files only it holds, such as an editor's unsaved buffers. `Resolver` is the one on the OS's own file system, `FileSystemOs`. It lists each directory a require looks in once and caches the listings, the aliases of each config, and what each module path holds, which its threads share, until `clear_cache` drops them all. A require that names no file, or more than one, or a definition file, gets a `ResolveError` saying why, which `to_diagnostic` turns into the E004 or E007 that `luck build` and the language server report, and a resolution's `to_diagnostic` gives the W004 they warn with when a `@self` shadows a `.luaurc` alias.

It is the lowest luck crate that reads the file system, so the crates above it read Luau sources and canonicalize paths through `FileSystemOs`, with `canonicalize_existing` for a path not all on disk, so every canonical path in luck has one form. Paths stay `Path`s, a module's canonical, `lexically_normalized` resolves a path's `.` and `..` without reading the disk, and `to_slash_lossy` gives the forward-slash form luck writes a path out in.

## Usage

- `Resolver::new()` on the OS's file system, or `ResolverGeneric::new_with_file_system(file_system)` on any other, and `with_luau_config(aliases)` to read each `.config.luau` with `aliases`.
- `resolve(from_file, module)` returns a `Resolution`, the module's canonical path and whether a `.luaurc` alias named `self` was shadowed, whose `to_diagnostic(span)` gives the W004 that warns of it, or the `ResolveError`, whose `to_diagnostic(span, module)` reports it at the require string.
- `clear_cache()` forgets everything read so far, so the next resolution sees the file system as it is then.

```rust
use std::path::Path;

use luck_resolver::Resolver;

let resolver = Resolver::new();
match resolver.resolve(Path::new("main.luau"), "./foo") {
    Ok(resolution) => println!("{}", resolution.path.display()),
    Err(error) => println!("{error:?}"),
}
```
