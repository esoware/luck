# luck_conformance

Internal and unpublished: runs each dialect's official test suite through every stage, and commits what it finds as one snapshot per stage and dialect in `snapshots/`. Ported from oxc's `tasks/coverage`.

```sh
just conformance                    # every dialect, rewriting the snapshots
just conformance --filter strings   # print the failures of the files whose path holds `strings`
```

## The suites

PUC-Rio's suites for Lua 5.1 to 5.5 and Luau's conformance tests, each pinned in `src/suite.rs` to the release of the interpreter mlua vendors for that dialect. The first run downloads them into the gitignored `cache/` directory.

## The checks

For every file the reference interpreter compiles:

- `parser`: luck parses it. For a file the interpreter rejects, such as one that uses Luau syntax still behind a flag, luck has to reject it too.
- `codegen`: the compact output parses back to the same tree and prints as itself.
- `formatter`: the formatted output parses back to the same tree and formats as itself.
- `minifier`: the minified output parses and minifies as itself.
- `runtime`: the original, formatted, and minified programs end the same way in the reference interpreter, returning the same values.
- `linter`: every rule's diagnostics.

Trees compare with `luck_testgen::tree_shape`.

## The reference interpreter

mlua links one interpreter per build, so the crate builds once per dialect, picked by its `lua51` to `lua55` or `luau` feature. Each program runs in a child process of this binary, so a test that exits, loops, changes the locale, or crashes takes nothing else down, and one that runs past a minute is killed.

The child sets up what the suite's own runner does: PUC-Rio's `all.lua` with `_soft`, `_port`, and `_nomsg` set, and Lua 5.2's `_noposix` and `_nolonglong`, or Luau's `runConformance`, with its `loadstring`, its native-code helpers as they answer without native code, and globals of the test's own, resumed as a thread. Luau's flags for what luck reads ahead of the pinned release, integer literals and exported values, are on.

## Status

`src/status.rs` lists, each with why:

- the files the harness can't run, such as a Luau test that calls functions its runner registers in C++
- the files whose formatted or minified program can't end the way the original does, such as one that reads its own line numbers or its locals' names

A file on the second list that ends the same fails, so the list can't go stale.
