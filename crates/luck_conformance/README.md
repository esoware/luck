# luck_conformance

Runs Luau's official conformance tests through every stage of luck.

`just conformance` runs every test file and rewrites the committed snapshots in
`snapshots/`. With `--filter strings`, it checks only the files whose name
contains `strings`, prints each failure in full, and writes no snapshot. The
reference interpreter is mlua's vendored build of Luau.

Each stage has its own snapshot:

* `parser` - luck parses every file Luau compiles, and rejects every file it
  rejects.
* `codegen` - Compact output parses back to the same tree, and prints the same
  again.
* `formatter` - Formatted output parses back to the same tree, and formats the
  same again.
* `minifier` - Minified output parses, and minifies the same again.
* `runtime` - The original, formatted, and minified programs end the same way
  in Luau.
* `linter` - Every rule's diagnostics.

Ported from oxc's `tasks/coverage`.
