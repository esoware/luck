# luck_resolver

Resolves Luau `require` paths to files, including relative paths and `.luaurc`
aliases.

Resolution follows Luau's require-by-string rules. A path like `./module` or
`../module` is relative to the requiring file, and an `@alias` prefix comes
from the `.luaurc` and `.config.luau` files in the directories above it. A
require that matches no file, or more than one, gets a `ResolveError` saying
which.

`Resolver` works on the real file system, while `ResolverGeneric` takes any
`FileSystem`, so it can resolve against files that only exist in memory, such
as an editor's unsaved buffers. Directory listings, aliases, and file probes
are cached and shared across threads until `clear_cache`.

A `.config.luau` is Luau code, so reading its aliases means running it. The
resolver leaves that to a `LuauConfigAliases` function you pass in, such as
`luck_type_checker::luau_config_aliases`.

## Usage

This example resolves `./foo` from `main.luau`:

```rust,no_run
use std::path::Path;

use luck_resolver::Resolver;

let resolver = Resolver::new();
let resolution = resolver.resolve(Path::new("main.luau"), "./foo").unwrap();
assert!(resolution.path.ends_with("foo.luau"));
```

## API

* `Resolver` and `ResolverGeneric`, with `resolve`, `with_luau_config`, and
  `clear_cache`.
* `Resolution`, the resolved path, and whether `@self` shadowed a `.luaurc`
  alias.
* `ResolveError`, with `to_diagnostic` to report it.
* `LuauConfigAliases`, the function type that reads a `.config.luau`'s aliases.
* `FileSystem`, the file system interface, with `DirEntry` and `FileMetadata`,
  and `FileSystemOs`, the operating system's.
* `lexically_normalized`, which resolves `.` and `..` without touching the
  disk.
* `to_slash_lossy`, which writes a path with forward slashes.
* `is_definition_file`, which checks for a `.d.luau` file name.
