# luck_minifier

A Luau minifier that strips types, folds constants, removes dead code, and renames locals.

## Overview

Compression passes repeat until one changes nothing, then the tree is printed once with `luck_codegen`. Based on `oxc_minifier`.

Every rewrite assumes a metamethod can run wherever Luau allows one, so it never drops code that might call one. `minify(minify(x)) == minify(x)`, and tests check it.

## Usage

```rust
use luck_allocator::Allocator;
use luck_minifier::TransformConfig;

let allocator = Allocator::new();
let config = TransformConfig::default();
let output = luck_minifier::minify(&allocator, b"return 1 + 2", &config);
assert_eq!(output.as_deref(), Ok(&b"return 3"[..]));
```

## API

- `minify` parses a source and minifies it.
- `minify_chunk` minifies a tree you already have, into a buffer you pass.
- `TransformConfig` turns each transform on or off.

## Transforms

All of them except `rename_globals` are on by default.

| Field | What it does |
| --- | --- |
| `strip_types` | removes type annotations, casts, and `type` declarations |
| `remove_dead_code` | removes unreachable code and unused locals |
| `simplify_statements` | simplifies `if` and `return` statements |
| `fold_constants` | evaluates constant expressions |
| `inline_locals` | inlines locals used once |
| `merge_locals` | merges neighboring `local` statements |
| `simplify_indexes` | turns `t["x"]` into `t.x` |
| `simplify_parens` | removes parentheses that change nothing |
| `lift_locals` | hoists each function's declarations into one `local` |
| `rename_locals` | gives locals the shortest names their scopes allow |
| `rename_globals` | also renames the globals a file defines, which breaks other chunks that read them |

## Cargo features

- `serde` derives `Deserialize` for `TransformConfig`, in the shape `luck.json` uses.
- `schemars` derives `JsonSchema` too, and enables `serde`.
