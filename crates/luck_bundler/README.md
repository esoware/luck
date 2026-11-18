# luck_bundler

Bundles a multi-file Lua or Luau project into one self-contained file.

## Overview

Discovery starts at the entry file. The bundler reads and parses each module and finds its `require` calls with `luck_semantic`, so only a call that reaches the global `require` counts. It resolves each require string with `luck_resolver`. It reads modules on one thread per arena the caller passes, the caller's own thread included, and their trees stay in those arenas. It numbers the modules in require order in a `DependencyGraph`, so the result is the same however many threads read them, and reports a cycle as a warning.

The bundle opens with a small loader, and each module registers as a function that runs on its first `require`. The loader works like the target's own `require`. On Lua, `package.loaded` and `package.preload` work as they do there, and on Luau the loader caches a module by file, and the module must return exactly one value. Every name the bundler generates shares a prefix no module's source contains, and every path in the output is relative to the project root.

`bundle` splices the loader into each module's text, so a line in the bundle maps back to a line of its module. `bundle_minified` instead builds the bundle as one tree from the modules' own parses and minifies it with `luck_minifier`, so no text is parsed twice.

## Features

- `DynamicRequire` decides what a `require` whose argument isn't a string literal does: fail the bundle (E002), stay for the runtime with a warning (W007), which is the default, or stay silently.
- The `serde` feature derives `Deserialize` for `DynamicRequire`, the shape `luck.json` gives it, and `schemars` derives `JsonSchema` too.

## Usage

- `bundle(&mut arenas, entry, &options)` returns a `BundleResult`: the output, the warnings, the bundled files, and the line map. It needs at least one arena, and the result borrows none, so they can be reset right after.
- `bundle_minified(&mut arenas, entry, &options, &transforms)` returns a `MinifiedBundle`.
- `BundleOptions` holds the target version, the Lua search paths, the project root, and the `DynamicRequire` mode.
- `DependencyGraph::build` builds the graph alone.

```rust,no_run
use std::iter;
use std::path::Path;

use luck_allocator::Allocator;
use luck_bundler::{BundleOptions, DynamicRequire};
use luck_token::LuaVersion;

let mut arenas: Vec<Allocator> = iter::repeat_with(Allocator::new).take(8).collect();
let options = BundleOptions {
    version: LuaVersion::Lua54,
    search_paths: &[],
    project_root: Path::new("."),
    dynamic_require: DynamicRequire::default(),
};
let bundle = luck_bundler::bundle(&mut arenas, Path::new("main.lua"), &options).unwrap();
assert!(!bundle.output.is_empty());
```
