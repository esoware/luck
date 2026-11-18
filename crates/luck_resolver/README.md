# luck_resolver

Maps the string a `require` call names to the file it loads.

## Overview

- Lua 5.x substitutes the module name, with its dots as slashes, into each search path template, `?.lua` and `?/init.lua` by default, relative to the project root, and takes the first file that exists.
- Luau resolves `./` and `../` paths from the requiring file's directory, and `@alias` prefixes from the `.luaurc` files up the directory tree.

A `Resolver` caches the directory listings it reads and what each require resolved to, and never invalidates them, so make one per build and share it across that build's threads. A require that names no file, or more than one, gets a `Diagnostic` saying why.

This is the lowest luck crate that touches the filesystem, so it also holds what every file-reading consumer shares: `read_source_file`, and the forward-slash path form modules are keyed on, `normalize_path` and `native_path`.

## Usage

- `Resolver::new(version, search_paths, project_root)`, where an empty `search_paths` means the default templates. Luau ignores both.
- `Resolver::resolve(&ResolveRequest)` returns a `ResolvedModule`, its path and any warnings, or the `Diagnostic`.

```rust
use std::path::Path;

use luck_resolver::{ResolveRequest, Resolver};
use luck_token::{LuaVersion, Span};

let search_paths = ["./?.lua".to_owned()];
let resolver = Resolver::new(LuaVersion::Lua54, &search_paths, Path::new("."));
let request = ResolveRequest { module: "foo", from_file: "main.lua", span: Span::new(0, 3) };
match resolver.resolve(&request) {
    Ok(resolved) => println!("{}", resolved.path),
    Err(diag) => println!("{}", diag.message),
}
```
