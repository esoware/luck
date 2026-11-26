# luck_conformance

Runs Luau's official conformance tests through every luck stage and records the results as snapshots.

```sh
just conformance                   # rewrite every snapshot
just conformance --filter strings  # print failures for files whose name contains "strings"
```

Each stage writes a snapshot to `snapshots/`:

- `parser` checks that luck parses every file Luau compiles, and rejects every file it rejects.
- `codegen` checks that compact output parses back to the same tree and prints the same again.
- `formatter` checks that formatted output parses back to the same tree and formats the same again.
- `minifier` checks that minified output parses and minifies the same again.
- `runtime` checks that the original, formatted, and minified programs end the same way when run in Luau.
- `linter` records every rule's diagnostics.

The reference interpreter is mlua's vendored Luau. Ported from oxc's `tasks/coverage`.
