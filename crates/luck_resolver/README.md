# luck_resolver

Maps the string a `require` call names to the file it loads.

## Overview

- Lua 5.x substitutes the module name, with its dots as slashes, into each search path template, `?.lua` and `?/init.lua` by default, relative to the project root, and takes the first file that exists.
- Luau resolves `./` and `../` paths from the requiring file's directory, and `@alias` prefixes from the `.luaurc` files up the directory tree.

A resolver reads through a `FileSystem`, so a caller can resolve against files only it holds, such as an editor's unsaved buffers. `Resolver` is the one on the OS's own file system, `FileSystemOs`. It lists each directory a require looks in once and caches the listings and what each require found, which its threads share, until `clear_cache` drops them all. A require that names no file, or more than one, gets a `ResolveError` saying why, which `to_diagnostic` turns into the E004 or E007 that `luck build` and the language server report, and a resolution's `to_diagnostic` gives the W004 they warn with when a `@self` shadows a `.luaurc` alias.

It is the lowest luck crate that reads the file system, so the crates above it read Lua sources and canonicalize paths through `FileSystemOs`, with `canonicalize_existing` for a path not all on disk, so every canonical path in luck has one form. Paths stay `Path`s, a module's canonical, `lexically_normalized` resolves a path's `.` and `..` without reading the disk, and `to_slash_lossy` gives the forward-slash form luck writes a path out in.

## Usage

- `Resolver::new(options)` on the OS's file system, or `ResolverGeneric::new_with_file_system(file_system, options)` on any other. `ResolveOptions` holds the version, the Lua search paths, where an empty list means the default templates, and the root they start at. Luau ignores both.
- `resolve(from_file, module)` returns a `Resolution`, the module's canonical path and whether a `.luaurc` alias named `self` was shadowed, whose `to_diagnostic(span)` gives the W004 that warns of it, or the `ResolveError`, whose `to_diagnostic(span, module)` reports it at the require string.
- `clear_cache()` forgets everything read so far, so the next resolution sees the file system as it is then.

```rust
use std::path::{Path, PathBuf};

use luck_resolver::{ResolveOptions, Resolver};
use luck_token::LuaVersion;

let resolver = Resolver::new(ResolveOptions {
    version: LuaVersion::Lua54,
    search_paths: vec!["./?.lua".to_owned()],
    root: PathBuf::from("."),
});
match resolver.resolve(Path::new("main.lua"), "foo") {
    Ok(resolution) => println!("{}", resolution.path.display()),
    Err(error) => println!("{error:?}"),
}
```
