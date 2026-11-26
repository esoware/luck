# luck_bundler

Bundles a multi-file Luau project into a single file that runs without any loader or runtime library.

## Overview

Starting from an entry file, it follows every `require` and resolves it with `luck_resolver`. Each module runs on its first `require` and its result is cached, as Luau's `require` does.

Modules are parsed in parallel on rayon's pool. `bundle_minified` minifies each module's tree directly instead of reparsing the bundle. Paths in the output are relative to the project root.

A `require` whose argument isn't a string can't be resolved at build time. `DynamicRequire` decides whether that's an error, a warning, or allowed.

## Usage

The bundler parses modules on rayon's pool, so give its threads `luck_ast::MIN_STACK_BYTES` of stack.

```rust,no_run
use std::path::Path;

use luck_allocator::ThreadAllocators;
use luck_ast::MIN_STACK_BYTES;
use luck_bundler::{BundleOptions, DynamicRequire};

rayon::ThreadPoolBuilder::new().stack_size(MIN_STACK_BYTES).build_global().unwrap();
let options = BundleOptions {
    project_root: Path::new("."),
    dynamic_require: DynamicRequire::default(),
    luau_config: None,
};
let arenas = ThreadAllocators::new();
let bundle = luck_bundler::bundle(&arenas, Path::new("main.luau"), &options).unwrap();
assert!(!bundle.output.is_empty());
```

## API

- `bundle` bundles every module an entry file reaches.
- `bundle_minified` does the same and minifies it with a `TransformConfig`.
- `Bundle` and `MinifiedBundle` hold the output, the warnings, and the list of bundled files.
- `BundleOptions` holds the project root, the `DynamicRequire` setting, and the `.config.luau` reader.
- `DynamicRequire` is `Error`, `Warn`, or `Allow`.
- `DependencyGraph` holds every module, the entry, and a topological order. `build` makes one.
- `ModuleInfo`, `ModuleId`, and `Dependency` describe a module and what it requires.
- `LineMapEntry` maps a range of bundle lines back to its module.

## Cargo features

- `serde` derives `Deserialize` for `DynamicRequire`, in the shape `luck.json` uses.
- `schemars` derives `JsonSchema` too, and enables `serde`.
