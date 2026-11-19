# luck_minifier

The Lua and Luau minifier, which rewrites a tree in place in the caller's arena and prints it with `luck_codegen`.

## Overview

The work runs in three phases, each once:

1. **Binding.** One walk resolves every name to its binding with the target's scoping rules and counts each binding's references. It writes the binding into the name, so later phases tell two same-named locals apart however the tree changes.
2. **Compression.** One in-place walk runs every rewrite as it leaves each node. Each rewrite keeps the counts accurate, so one walk reaches the fixpoint. It walks again only when it exposes a rewrite too late in the walk to make it.
3. **Renaming.** Every local gets the shortest name its scopes allow.

Then `luck_codegen::compact` prints the tree once, every literal in the shortest spelling of its value. Every rewrite assumes a metamethod can run wherever Lua allows one, so only literal arithmetic folds, and folding works on the values the parser decoded and reproduces the target's float arithmetic exactly. The output parses, and minifying it again changes nothing. The minifier keeps a shebang and Luau `--!` hot comments on top and drops every other comment.

## Features

`TransformConfig` turns each rewrite on or off. All are on by default but `rename_globals`.

- `remove_dead_code`: constant branches, loops that never run or run once, unused locals, and statements that do nothing.
- `simplify_statements`: unwraps `do` blocks that scope nothing, merges nested `if`s, and turns `if ... return` runs into `and`/`or`.
- `fold_constants`: evaluates operators over literals, a string's length included.
- `inline_locals`: replaces a local read once with its value, when that's a literal expression that can't raise an error.
- `merge_locals`: joins neighboring `local`s, and neighboring assignments to locals, into one statement.
- `simplify_indexes`: `t["x"]` to `t.x`, and `{[1] = a, [2] = b}` to `{a, b}`, however the key is spelled.
- `simplify_parens`: drops parentheses that change neither grouping nor how many values an expression yields.
- `lift_locals`: hoists a function's declarations to one `local` at its top.
- `rename_locals`: gives locals the shortest names.
- `rename_globals`: renames globals the file defines too. It's off by default, because other chunks and `_G["name"]` lookups still use the old names, and it needs `rename_locals`.

The `serde` feature derives `Deserialize` for `TransformConfig`, the shape of `luck.json`'s `transforms`, and `schemars` derives `JsonSchema` too.

## Usage

- `minify(allocator, source, version, &config)` parses into the arena and returns the minified source, or the parse errors.
- `minify_chunk(allocator, &mut chunk, &config)` minifies a tree the caller holds, in the version its `Chunk` records. It fails only on a tree nested past `luck_ast::MAX_NESTING_DEPTH`, and leaves the shebang and hot comments to the caller.

```rust
use luck_allocator::Allocator;
use luck_minifier::TransformConfig;
use luck_token::LuaVersion;

let allocator = Allocator::new();
let config = TransformConfig::default();
let output = luck_minifier::minify(&allocator, "return 1 + 2", LuaVersion::Lua54, &config);
assert_eq!(output.as_deref(), Ok("return 3"));
```
