# luck_testgen

Internal and unpublished: the test inputs other luck crates' tests share.

- `generate_runtime_safe(seed, version, statement_budget)` writes a deterministic random program that runs without error and prints what it computes, so an original and a transformed program can be compared by their output.
- `generate_full` gives up running safely to reach the whole grammar, for parse and round-trip tests.
- `sources_at_the_nesting_cap`, `sources_past_the_nesting_cap`, `broken_sources`, and `deep_and_long_sources` are inputs at and far past `luck_ast::MAX_NESTING_DEPTH`, and `on_the_smallest_stack` runs a stage over them on a thread of exactly `luck_ast::MIN_STACK_BYTES`.
- `collect_fixture_files` and `detect_version` walk the shared fixtures in `tests/fixtures/`.
- `tree_shape` prints a tree without what a printer may change, such as spans, quotes, and parentheses that change nothing, so a round-trip test can check that printed output parses back to the same tree.

Crates take it as a dev-dependency. Its own tests round-trip generated programs through the parser, formatter, and minifier:

```sh
just test -p luck_testgen
```
