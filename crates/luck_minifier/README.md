# luck_minifier

A Luau minifier that strips types, folds constants, removes dead code, and
renames locals.

Minification rewrites the syntax tree in place. Types go first, then
compression passes such as constant folding, dead code removal, and inlining
repeat until one changes nothing. Each function's declarations are then
hoisted into a single `local`, and the passes run again. Last, locals get the
shortest names their scopes allow, and the tree is printed once.

Every rewrite assumes a metamethod can run wherever Luau allows one, so the
output behaves like the input even when tables have metatables. Minifying
already minified code changes nothing. The design follows `oxc_minifier`.

## Usage

This example minifies a constant expression:

```rust
use luck_allocator::Allocator;
use luck_minifier::TransformConfig;

let allocator = Allocator::new();
let config = TransformConfig::default();
let output = luck_minifier::minify(&allocator, b"return 1 + 2", &config);
assert_eq!(output.as_deref(), Ok(&b"return 3"[..]));
```

`minify` parses the source itself, `minify_parsed` minifies a parse you
already have, and `minify_chunk` minifies a tree you already have.

## Transforms

`TransformConfig` turns each transform on or off. All of them except
`rename_globals` are on by default.

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

* `serde` - Derives `Deserialize` for `TransformConfig`, in the shape a
  `luck.json` config gives it.
* `schemars` - Derives `JsonSchema` for `TransformConfig`. Enables `serde`.
