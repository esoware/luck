# Rewrite report: luck_core

## 1. Diagnosis

`luck_core` is the config + diagnostics hub (~2.3k LOC). Most of it was
already in good shape: `types.rs`, `format_options.rs`, `transform_config.rs`,
`editorconfig.rs`, `source_io.rs`, and `diagnostics.rs` are each one coherent
concern, well tested, and cleanly written. Per the mandate, a file that is
already excellent should come out untouched - those six did.

The one real structural offender was `config.rs` at 1,211 lines. Its ~555
lines of source mixed four genuinely different concerns at different
abstraction levels in a single flat file:

- pure serde config **types** plus their `extends`/profile **merge** logic
  (value-to-value, no IO),
- **filesystem loading**: JSON5 parsing, the recursive `extends` chain with
  cycle detection and `root`-boundary security, upward discovery, `.luaurc`,
- **build-target resolution**: `BuildConfig` + entry/output path arithmetic
  and profile application,
- **glob project filtering**: `ProjectFilter`.

Reading or changing any one of these meant scrolling past the other three,
and the ~650-line inline test block sat on top of all of it. This was the
prime structure candidate the crate had.

Stale docs: the `README.md` claimed `luck_core` owned `ModuleId`/`ModuleInfo`
(they live in `luck_bundler`) and that it rendered diagnostics "via ariadne"
(that is the CLI). The `add-config-option` skill's layout table pointed at
`config.rs` for types/discovery.

## 2. What changed

Split `config.rs` into a `config/` directory using the module-root pattern
(CLAUDE.md prefers named module files over `mod.rs`; `config.rs` stays as the
module root that declares submodules and re-exports):

- `config/schema.rs` - the deserialized `luck.json` surface (`LuckConfig`,
  `FormatConfig`, `LintConfig`, `RuleSetting`, `EntryConfig`,
  `ProfileOverrides`), target-resolution methods, and `merge_onto`/
  `merge_format`/`merge_lint`.
- `config/load.rs` - `parse_luck_config`, the `extends` chain (`load_with_extends`
  + inner), `discover_config`, and `.luaurc` (`LuauRc`, `parse_luaurc`).
- `config/resolve.rs` - `BuildConfig`, `DEFAULT_SEARCH_PATHS`,
  `resolve_build_config`.
- `config/filter.rs` - `ProjectFilter` + its glob building, with the two
  `ProjectFilter` tests moved next to it.

`config.rs` is now a thin module root: doc, `mod`/`pub use` re-exports,
`CONFIG_FILE_NAME`, and the crate's config public-API test suite (which
naturally aggregates at the module that re-exports that surface). Every
name stays reachable at `luck_core::config::*`, so the public surface is
byte-identical. Each source file is now independently comprehensible: pure
data vs IO vs path arithmetic vs glob matching no longer interleave.

The other six source files were left untouched (correct outcome, not
neglect). Diagnostic message text was deliberately not touched - it is
observable output.

## 3. Cross-crate fallout

None. The split is behind unchanged re-export paths:
- All `luck_core::config::{ProjectFilter, LuckConfig, FormatConfig,
  parse_luck_config, load_with_extends, discover_config, BuildConfig,
  resolve_build_config, DEFAULT_SEARCH_PATHS, LuauRc, parse_luaurc, ...}`
  paths resolve exactly as before.
- `merge_format` stays `pub(crate)`, re-exported at `config::` for the one
  internal consumer (`editorconfig.rs`).
- `lib.rs`'s `pub use config::{LintConfig, RuleSetting}` is unaffected.

`cargo build --workspace` and the full `cargo nextest run --workspace` both
pass without touching any downstream crate.

## 4. Docs updated

- `crates/luck_core/README.md` - removed the stale `ModuleId`/`ModuleInfo`
  claim and the "renders via ariadne" line; documented the `config`
  submodule structure, the two target axes, format-option ownership,
  `source_io`, and the schema-regen command.
- `.claude/skills/add-config-option/SKILL.md` - layout table now points at
  `config/schema.rs`, `config/load.rs`, and `config/resolve.rs` instead of
  the old monolithic `config.rs`.
- `CLAUDE.md` architecture table needs no change: `config.rs` is still a
  valid key entry (now the module root).

## 5. Flags

- **`Diagnostic` stores `Range<usize>` + `String` file_path** while the rest
  of the workspace uses `luck_token::Span` (u32). `error_at`/`warning_at`
  accept a `Span` and convert, but the numbered `errors::eNNN` constructors
  and the bundler call sites all thread `Range<usize>`. Left intact: ariadne
  wants `Range<usize>` at the render boundary, migrating would ripple through
  bundler/resolver/render with no observable benefit, and it is not clearly
  wrong. Noting the inconsistency only.
- **`W004` is defined before `W003`** in `diagnostics::errors` (out of numeric
  order in an otherwise-sequential set). Left alone to avoid churning an
  untouched, behavior-carrying file for a cosmetic reorder.
- **No design guideline disagreements.** The split follows the CLAUDE.md
  module-organization and "file size is a symptom" guidance directly.
- Other crates: not surveyed beyond the downstream config/diagnostic call
  sites, which were clean.

## 6. Gate results

- `cargo build --workspace` - clean.
- `cargo clippy -p luck_core --all-targets -- -D warnings` - clean.
- `cargo clippy --workspace --all-targets -- -D warnings` - clean.
- `cargo nextest run -p luck_core` - 65 passed, 1 skipped (the `--ignored`
  schema regen test).
- `cargo test --doc -p luck_core` - 1 passed.
- `cargo nextest run --workspace` - 2000 passed, 4 skipped, 0 failed
  (matches the pre-rewrite baseline exactly).
- Perf/minsize: no hot-path, minifier, or codegen code touched; config is a
  cold path, so no bench or `minsize.snap` impact. The `luckrc.schema.json`
  drift test passes (schema unchanged).
