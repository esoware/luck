# luck_testgen

Test helpers shared across luck's crates.

* `generate_runtime_safe` and `generate_full_grammar` generate random Luau
  programs from a seed. The same seed gives the same program on every
  platform.
* `sources_at_the_nesting_cap`, `sources_past_the_nesting_cap`,
  `deep_and_long_sources`, `broken_sources`, and `nested` build sources that
  nest deep.
* `on_the_smallest_stack` and `with_the_smallest_stack` run code on the
  smallest stack luck gives its threads.
* `source_fixtures`, `definition_fixtures`, and `fixture_files` walk the shared
  `tests/fixtures` tree.
* `tree_shape` checks that a printer kept a tree's meaning.
