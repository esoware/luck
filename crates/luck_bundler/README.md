# luck_bundler

Bundles a multi-file Lua or Luau project into one self-contained file.

## Overview

Discovery starts at the entry file. The bundler reads each module as bytes, as Lua does, parses it, and reads everything else it needs off one `luck_semantic` pass, which finds its `require` calls, so only a call that reaches the global `require`, by its name or as `_G.require`, counts. It resolves each require string with `luck_resolver`. Each module is a task on rayon's pool, which reads it into the arena its thread gets from the caller's `ThreadAllocators` and resolves its requires. The calling thread alone keeps the graph, and starts a task for each new module a finished one reaches, as `oxc_linter`'s service does. It numbers the modules in require order in a `DependencyGraph`, so the result is the same however many threads read them, and reports a cycle as a warning.

The bundle opens with a small loader, and each module registers as a function that runs on its first `require`. The loader works like the target's own `require`. On Lua, `package.loaded` and `package.preload` work as they do there, and on Luau the loader caches a module by file, and the module must return exactly one value. Every name the bundler generates shares a prefix no module's source contains, and every path in the output is relative to the project root.

`bundle` splices the loader into each module's text, which it copies byte for byte, so a line in the bundle maps back to a line of its module. `bundle_minified` instead builds the bundle as one tree from the modules' own parses and minifies it with `luck_minifier`, so no text is parsed twice.

## Features

- `DynamicRequire` decides what a `require` whose argument isn't a string literal does: fail the bundle (E002), stay for the runtime with a warning (W007), which is the default, or stay silently.
- The `serde` feature derives `Deserialize` for `DynamicRequire`, the shape `luck.json` gives it, and `schemars` derives `JsonSchema` too.

## Usage

- `bundle(&arenas, entry, &options)` returns a `Bundle`: the output, the warnings, the bundled files, and the line map. The result borrows no arena, so `arenas.reset()` can empty them right after.
- `bundle_minified(&arenas, entry, &options, &transforms)` returns a `MinifiedBundle`.
- `BundleOptions` holds the target version, the Lua search paths, the project root, and the `DynamicRequire` mode.
- `DependencyGraph::build` builds the graph alone.

Modules are parsed and analyzed on rayon's threads, so give that pool `luck_ast::MIN_STACK_BYTES` of stack. Called from a thread of the pool, the bundler runs queued modules on it while it waits, so a pool of one thread does all the work on the caller's.

```rust,no_run
use std::path::Path;

use luck_allocator::ThreadAllocators;
use luck_ast::MIN_STACK_BYTES;
use luck_bundler::{BundleOptions, DynamicRequire};
use luck_token::LuaVersion;

rayon::ThreadPoolBuilder::new().stack_size(MIN_STACK_BYTES).build_global().unwrap();
let options = BundleOptions {
    version: LuaVersion::Lua54,
    search_paths: &[],
    project_root: Path::new("."),
    dynamic_require: DynamicRequire::default(),
};
let arenas = ThreadAllocators::new();
let bundle = luck_bundler::bundle(&arenas, Path::new("main.lua"), &options).unwrap();
assert!(!bundle.output.is_empty());
```
