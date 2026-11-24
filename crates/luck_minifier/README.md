# luck_minifier

The Lua and Luau minifier, which rewrites a tree in place in the caller's arena and prints it with `luck_codegen`, as `oxc_minifier` minifies JavaScript.

## Overview

The work runs in three phases:

1. **Type stripping.** A Luau tree loses its annotations, casts, instantiations, and `type` declarations before anything else reads it, as `oxc_transformer` strips TypeScript's. A cast over a call or `...` leaves the parentheses that truncate it.
2. **Compression.** The tree's bindings come from `luck_semantic`'s scoping, which every name finds its symbol or reference in by its index. Passes of an in-place walk run every rewrite as they leave each node, until a pass changes nothing, as `oxc_minifier`'s peephole loop runs. A rewrite that drops code marks the references in it, so every decision reads the tree as it stands, and at the end of each pass the symbols' lists lose the dropped references. Once the passes settle, each function's declarations lift to one `local` at its top, and since that reshapes the bindings, the scoping is built again and the passes run on what lifting left. Debug builds check the lists against a fresh analysis after every round.
3. **Renaming.** Every local gets the shortest name its scopes allow, from a fresh analysis of the tree, as `oxc_mangler` names them.

Then `luck_codegen::compact_into` prints the tree once, after the header, with `LiteralSpelling::Shortest`, every literal in the shortest spelling of its value. A rewrite asks `luck_value` what an expression evaluates to, whether it's truthy, and whether evaluating it can do anything, which assumes a metamethod can run wherever Lua allows one and a global read can run `__index`, and evaluates in the target's exact arithmetic. The output parses, and minifying it again changes nothing. The minifier keeps a shebang and Luau `--!` hot comments on top and drops every other comment.

## Features

`TransformConfig` turns each rewrite on or off. All are on by default but `rename_globals`.

- `strip_types`: Luau types go before the other passes run. Types never run, so this changes no behavior, but a typed library turns it off to keep them.
- `remove_dead_code`: constant branches, loops that never run or run once, unused locals, and statements that do nothing.
- `simplify_statements`: unwraps `do` blocks that scope nothing, merges nested `if`s, and turns `if ... return` runs into `and`/`or`.
- `fold_constants`: evaluates operators whose operands evaluate, a string's length and bitwise operators included, and drops the operands of an `and` or `or` that never run. A number folds only to a literal no longer than what it replaces, so `1/3` and `1/0` stay.
- `inline_locals`: replaces a local read once with its value, when that's the same wherever it runs: it reads no variable, builds no table or closure, and can't raise an error.
- `merge_locals`: joins neighboring `local`s, and neighboring assignments to locals, into one statement.
- `simplify_indexes`: `t["x"]` to `t.x`, and `{[1] = a, [2] = b}` to `{a, b}`, however the key is spelled.
- `simplify_parens`: drops parentheses that change nothing: around a call or `...` wherever only its first value counts, as in `if (f()) then` or `{(f()), x}`, around a lone string or table argument, and around a run in a run at its level where they group the same, as in `(a + b) + c`.
- `lift_locals`: hoists a function's declarations to one `local` at its top.
- `rename_locals`: gives locals the shortest names.
- `rename_globals`: renames globals the file defines too. It's off by default, because other chunks and `_G["name"]` lookups still use the old names, and it needs `rename_locals`.

The `serde` feature derives `Deserialize` for `TransformConfig`, the shape of `luck.json`'s `transforms`, and `schemars` derives `JsonSchema` too.

## Usage

- `minify(allocator, source, version, &config)` parses into the arena and returns the minified source, or the parse errors. The source and the output are bytes, as Lua reads them: a string spelled as written keeps its bytes, and the output is UTF-8 whenever the source is.
- `minify_chunk(allocator, &mut chunk, &config, output)` minifies a tree the caller holds, in the version its `Chunk` records, and writes it after the bytes `output` holds, where the caller puts the shebang and hot comments a tree can't. It numbers the tree's names first, so a tree that joins several parses, or builds nodes in code, minifies too. It fails only on a tree nested past `luck_ast::MAX_NESTING_DEPTH`.

```rust
use luck_allocator::Allocator;
use luck_minifier::TransformConfig;
use luck_token::LuaVersion;

let allocator = Allocator::new();
let config = TransformConfig::default();
let output = luck_minifier::minify(&allocator, b"return 1 + 2", LuaVersion::Lua54, &config);
assert_eq!(output.as_deref(), Ok(&b"return 3"[..]));
```
