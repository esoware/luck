# luck_bundler

Bundles a multi-file Luau project into a single file with no runtime
dependencies.

Starting from an entry file, the bundler follows every `require`, resolves it
with `luck_resolver`, and writes one file in which each module is wrapped in a
loader that behaves like Luau's own `require`. A module runs the first time
it's required, and later requires get the cached result.

Modules are read and parsed in parallel on rayon's pool. Every path written
into the bundle is relative to the project root, so the output never contains
paths from the machine that built it. `bundle_minified` minifies each module's
tree directly, without parsing the bundle again.

A `require` whose argument isn't a string literal can't be resolved at build
time, and `DynamicRequire` decides whether that's an error, a warning, or
allowed.

## Usage

The bundler parses modules on rayon's pool, so give its threads
`luck_ast::MIN_STACK_BYTES` of stack:

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

* `bundle`, which bundles every module an entry file reaches into a `Bundle`.
* `bundle_minified`, which does the same and minifies it with a
  `luck_minifier::TransformConfig`, into a `MinifiedBundle`.
* `BundleOptions`, the project root, the `DynamicRequire` setting, and the
  function that reads `.config.luau` aliases.
* `DependencyGraph`, every module, the entry, and a topological order, built by
  `DependencyGraph::build`.
* `ModuleInfo`, `ModuleId`, and `Dependency`, one module and what it requires.
* `LineMapEntry`, the range of bundle lines that holds one module.

## Cargo features

* `serde` - Derives `Deserialize` for `DynamicRequire`, in the shape a
  `luck.json` config gives it.
* `schemars` - Derives `JsonSchema` for `DynamicRequire`. Enables `serde`.
