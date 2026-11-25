# luck_conformance

Internal and unpublished: runs Luau's official conformance tests through every stage, and commits what it finds as one snapshot per stage, `snapshots/{stage}.snap`. Ported from oxc's `tasks/coverage`.

```sh
just conformance                    # the whole suite, rewriting the snapshots
just conformance --filter strings   # print the failures of the files whose name holds `strings`
```

## The suite

Luau's conformance tests, pinned in `src/suite.rs` to the release of the Luau that mlua vendors. The first run downloads them into the gitignored `cache/` directory.

## The checks

For every file the reference interpreter compiles:

- `parser`: luck parses it. For a file the interpreter rejects, such as one that uses syntax still behind a flag, luck has to reject it too.
- `codegen`: the compact output parses back to the same tree and prints as itself.
- `formatter`: the formatted output parses back to the same tree and formats as itself.
- `minifier`: the minified output parses and minifies as itself.
- `runtime`: the original, formatted, and minified programs end the same way in the reference interpreter, returning the same values.
- `linter`: every rule's diagnostics.

Trees compare with `luck_testgen::tree_shape`.

## The reference interpreter

It is mlua's vendored build of Luau, which the harness checks is the release the suite is pinned to. Each program runs in a child process of this binary, so a test that exits, loops, changes the locale, or crashes takes nothing else down, and one that runs past a minute is killed.

The child sets up what Luau's `runConformance` does: its `loadstring`, its native-code helpers as they answer without native code, and globals of the test's own, resumed as a thread. Luau's flags for what luck reads ahead of the pinned release, exported values and conditional bindings, are on, and so is the one for the `buffer` vector functions `buffers.luau` calls.

## Status

`src/status.rs` lists, each with why:

- the files the harness can't run, such as a test that calls functions its runner registers in C++
- the files whose formatted or minified program can't end the way the original does, such as one that reads its own line numbers or its locals' names

A file on the second list that ends the same fails, so the list can't go stale.
