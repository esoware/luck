# luck

Lua/Luau bundler, minifier, formatter, linter, and language server.
Multi-file Lua in, single bundled file out. Supports Lua 5.1, 5.2, 5.3,
5.4, 5.5, and Luau (standalone and Roblox).

Rust workspace, edition 2024, MSRV 1.85. Parser, lexer, AST, codegen are
all hand-written - no external parser dependency.

## Commands

```sh
cargo build --workspace                                # build all crates
cargo nextest run --workspace                          # run all tests (preferred runner; plain `cargo test` also works)
cargo nextest run -p <crate>                           # test one crate
cargo nextest run -p luck_parser goto_statement        # run one test by name
cargo test --doc --workspace                           # doctests (nextest does not run them)
cargo fmt --all                                        # format
cargo clippy --workspace --all-targets -- -D warnings  # must be zero warnings
cargo insta accept                                     # accept snapshots (non-interactive; `review` is for humans)
cargo bench -p luck_benchmark                          # criterion benches, one per stage (--bench lexer|parser|semantic|linter|codegen|formatter|minifier)
cargo test -p luck_core regenerate_luckrc_schema -- --ignored  # regen VS Code schema after config changes
```

## Architecture

Core pipeline: `luck_token` -> `luck_lexer` -> `luck_ast` ->
`luck_parser` -> `luck_codegen`. Everything else consumes the parsed
AST: `luck_minifier`, `luck_formatter`, and `luck_linter` transform or
check it (`luck_semantic` supplies scope analysis to the linter and
LSP); `luck_resolver` resolves require paths
and feeds `luck_bundler`. On top sit `luck` (facade re-exports),
`luck_lsp` (language server), and `luck_cli`, which drives both.
`luck_testgen` is an unpublished internal test harness and
`luck_benchmark` the unpublished bench harness.

| Crate | Role | Key entry |
|-------|------|-----------|
| `luck_token` | Spans, `LuaVersion`, `StdlibEnvironment`, `SourceError`, `CompactString` storage | `lib.rs` |
| `luck_lexer` | Single-pass tokenizer; comments emitted separately | `lexer.rs` |
| `luck_ast` | `Expression`/`Statement`/`Type` <=64 B, `Visitor`, `AstTransform`, `synth` builder (dummy-span AST construction for programmatic/decompiler use) | `expr.rs`, `stmt.rs`, `types.rs`, `transform.rs`, `synth.rs` |
| `luck_parser` | Pratt expressions + recursive-descent statements + full Luau type grammar, version-gated | `expr.rs`, `stmt.rs`, `luau.rs` |
| `luck_codegen` | Compact printer (ambiguity cases live in `separator.rs` + its tests) | `compact.rs`, `separator.rs` |
| `luck_core` | `LuaTarget`, typed config + `FormatOptions`/`LintConfig` enums, `TransformConfig`, diagnostics E001-E012/W001-W004, schemars schema | `config.rs`, `diagnostics.rs`, `format_options.rs` |
| `luck_resolver` | Lua search paths, Luau `@aliases`, `.luaurc` chain | `lib.rs`, `luau.rs` |
| `luck_bundler` | Require validation, dep graph (cycle detection), lazy memoizing loader emit + line maps, `ModuleId`/`ModuleInfo`, `insta` snapshots | `graph.rs`, `emitter.rs`, `module.rs` |
| `luck_minifier` | 12-transform pipeline; passes gated by `TransformConfig` flags | `lib.rs` `minify()`, `transforms/` |
| `luck_formatter` | oxc-style engine: `Format` trait + combinator IR (`BestFitting`, group-id conditionals), AST-in `format_block` formats synthetic ASTs (no source needed), idempotency invariant | `ir.rs`, `printer.rs`, `format_*.rs`, `comments.rs` |
| `luck_linter` | `Rule`/`NodeRule` traits + `LintContext`, 63 stateless rules in a static `RULES` registry, single-pass bus for node-local rules, suppressions, `--fix` | `rules/`, `rule.rs`, `bus.rs` |
| `luck_semantic` | Scope tree, refs (R/W/RW), upvalues, version- & environment-aware stdlib | `builder.rs`, `stdlib_model.rs` |
| `luck_lsp` | Library-only LSP backend (no binary); served via `luck lsp` | `backend.rs`, `serve.rs`, `providers/` |
| `luck_cli` | Flat Clap commands (incl. `lsp`); rayon-parallel lint/fmt/check; ariadne diagnostic rendering; `ExitCode` 0/1/2; 16 MB-stack worker thread | `cli.rs`, `render.rs`, `main.rs` |
| `luck` | Facade re-exports (no logic) | `lib.rs` |
| `luck_testgen` | Internal harness (`publish = false`): deterministic program generator, round-trip property tests | `src/lib.rs` |
| `luck_benchmark` | Internal (`publish = false`): per-stage criterion benches (oxc-style), run on CodSpeed in CI | `benches/`, `src/corpus.rs` |

Version bump policy per crate lives in `/bump-versions` - use the skill,
don't guess. `editors/vscode/` holds the VS Code extension; its config
schema (`editors/vscode/schemas/luckrc.schema.json`) is **generated** -
see below.

## Where tests live

Standard Rust layout everywhere: white-box tests of crate internals go
in an inline `#[cfg(test)] mod tests` at the bottom of the source file
that owns them; tests of the public API go in the crate's `tests/` dir,
as a single binary (`tests/it/main.rs` with submodules, shared helpers
in `tests/it/common/mod.rs`) where the suite is large. Never `src/tests/`
directories.

| Crate | Layout |
|---|---|
| `luck_parser`, `luck_lexer`, `luck_codegen`, `luck_formatter` | `tests/it/` binary (formatter and codegen also keep inline white-box tests in `src/*.rs`) |
| `luck_linter` | inline `#[cfg(test)]` per rule file (shared `src/test_support.rs` helper) + `src/lib.rs` driver tests + `tests/idiomatic_fixtures.rs` |
| `luck_minifier`, `luck_bundler`, `luck_semantic`, `luck_lsp`, `luck_cli` | inline unit tests + `tests/` integration (bundler uses `insta` snapshots; CLI spawns the real binary via `assert_cmd`) |
| every published crate | one `# Usage` doctest in `lib.rs` module docs |
| shared fixtures | `tests/fixtures/{lua51,...,lua55,luau,idiomatic}/` (repo root); parsed by the parser fixture sweep, bundled by bundler tests, `idiomatic/` must stay lint-clean (`luck_linter/tests/idiomatic_fixtures.rs`), all seed the nightly fuzz corpus |

Test conventions:

- Lint-rule tests: positive cases `flags_*`, negative cases `ignores_*`;
  build diagnostics through `crate::test_support::run_rule`; count
  assertions carry a `"{diags:?}"` failure message. Suppression is
  tested centrally, not per rule (see the add-lint-rule skill).
- Fix tests apply the edit and assert the result re-parses.
- Formatter tests go through `assert_format`/`assert_format_with`
  (idempotency + reparse are checked on every call).
- Error paths assert on returned `errors`, never `#[should_panic]`.

## Targets, config & diagnostics

**Three axes, never conflated:**

- `LuaVersion` (luck_token) - *syntax* only: 5.1-5.5, Luau. Parser, codegen,
  formatter, and minifier key off this.
- `StdlibEnvironment` (luck_token) - *stdlib environment*: `Standalone` vs
  `Roblox`. Only meaningful for Luau; semantic, linter, and LSP filter on it.
- `LuaTarget` (luck_core) - the user-facing dialect (7 variants incl.
  `LuauRoblox`). Projects to the two axes via `lua_version()` and
  `stdlib_environment()`. The split happens **once** at each entry boundary;
  downstream never re-derives it. Codegen-side crates take `LuaVersion` only -
  Roblox and standalone share syntax, so `-t roblox` and `-t luau` minify
  identically (correct, not a bug).

**Config is one typed source of truth.** `luck.json` only (discovered by
walking up from cwd; `-c/--config` overrides). The format-option enums,
`LintConfig`/`RuleSetting`/`Category`, and `DiagnosticSeverity` all live in
`luck_core` and deserialize with `#[serde(deny_unknown_fields)]` - unknown
keys **and** invalid enum values are hard errors, not silent drops. Targets are
per-extension via the `lua`/`luau` keys; `extends`/`include`/`exclude`/`root`
shape the project. Minifier passes are individually gated by bool flags on
`TransformConfig` (luck_core) - a new transform means a new flag **and** a
schema regen. The VS Code schema is **generated** from the Rust types with
`schemars` (a test fails if `luckrc.schema.json` drifts) - never hand-edit it;
regen with the command in the Commands section.
Format-option precedence: defaults < `.editorconfig` < `luck.json` `format`.

**Diagnostics are one scheme.** Codes live in `luck_core::diagnostics::errors`
(E001-E012, W001-W004); build them with the `Span`-accepting `error_at`/
`warning_at` constructors - never inline literal codes in consumers. Parse
failures are always E008. Lint diagnostics render with the rule name as the
code; the driver stamps category/severity from the rule's `category()` and the
resolved severity (rules don't restate them). The CLI returns an `ExitCode`:
0 success, 1 problems found, 2 usage/config error.

## Source directives

Users control luck from comments; tests need the exact syntax:

- Lint suppression: `-- luck: allow(rule_a, rule_b)` / `deny(...)` / `warn(...)`,
  applying to the next statement. Add `start` / `end` after the list for a
  region (`-- luck: allow(foo) start`). File-level form: `-- #luck: allow(foo)`.
- Formatter: `-- luck: format off` / `-- luck: format on` disable a region;
  `-- luck: ignore` (alias `-- luck: format ignore`) skips one statement.

## Hard invariants

These hold across every crate. Violating any of them is a correctness bug.

1. **`SourceError` is the single error type.** `LexError`, `ParseError`,
   `FormatError` are type aliases for it in their respective crates.
2. **`Span` is `u32`, not `usize`** - 4 GB file cap, half the AST bytes.
3. **`Expression` and `Statement` are not `#[non_exhaustive]`.** Every
   `match` must be exhaustive. Missing arms = silent skipped subtrees in
   transforms. No `_ => ...` catch-alls in transforms or visitors.
4. **Enum size budget <=64 bytes** for `Expression`, `Statement`, and
   `Type`, enforced by `luck_ast` tests. Box large variants.
5. **Comments are not in the AST.** They live in a sorted `Vec<Comment>`
   with `Leading`/`Trailing` classification (standalone = leading with
   `preceded_by_newline`) and `attached_to`. The formatter additionally
   accepts node-anchored `SyntheticComment`s (`luck_ast::synth`) for
   generated ASTs.
6. **Metamethod-safe purity.** `is_pure_expression(_, allow_var_reads=true)`
   rejects variable arithmetic. Only literal arithmetic is pure; variable
   reads, indexing, arithmetic, comparison, and concat may all metamethod.
7. **Idempotency.** `minify(minify(x)) == minify(x)` and
   `format(format(x)) == format(x)` are hard invariants - enforced by tests.
8. **Re-parseability.** Minified and formatted output must `parse()` with
   zero errors.
9. **`#"str"` must not be folded.** Escape sequences make raw length
   unreliable.
10. **Version gating goes through `LuaVersion::has_<feature>` predicates**
    (`has_goto`, `has_floor_div`, `has_compound_assignment`, ...; the only
    `is_` forms are `is_luau`/`is_roblox`), never direct variant comparison.
    Predicates are named by feature, not version.
11. **Emitters never read source text.** Codegen and formatter leaf text
    comes from token-carried `TokenKind` values (`compact.rs` `token_text`,
    formatter `tokens.rs`); source is consulted only for trivia fidelity
    (blank lines, comment gaps) and verbatim regions, and every such path
    must degrade gracefully when source is absent (synthetic ASTs).
12. **Luau types are a real AST** (`luck_ast::types::Type`), parsed by
    `luck_parser/src/luau.rs`. Never store or re-tokenize opaque type
    text.
13. **No global string interner.** Identifiers stay per-token
    `CompactString`; a shared interner's lock serializes rayon workers
    (oxc removed theirs after CPU utilization halved). Reject "intern
    all identifiers" proposals unless the interner is per-thread.

## Skills

Task workflows live in `.claude/skills/` and load on demand (their
descriptions are already in your context). When a task matches one -
new lint rule, minifier transform, formatter change, version feature,
LSP provider, config option, version bump, release - follow the skill
rather than working from memory; each encodes the registration steps
and test requirements that are easy to miss.

## Style

### Naming

Full words. No single-letter vars (except `i`/`idx` loop counters, `_`
discards). No `val`/`tmp`/`res`/`buf`/`str`. Domain abbreviations
**required**, not optional: write `stmt` not `statement`, `expr` not
`expression`, `ast`/`ir`/`span`/`lhs`/`rhs`/`args`/`params`.
Three grandfathered domain terms: the formatter's `Ctx` threading
struct (and `ctx` params), the linter's `LintContext` (and its `ctx`
params in `Rule::check`/`NodeRule` hooks), and `cfg` meaning
control-flow graph in `luck_linter/src/cfg.rs`. Everywhere else
`ctx`/`cfg` stay banned - and never `cfg` for "config". Bools start
with `is_`/`has_`/`should_`/`can_`. Collections are plural.

### Comments

Default to **no comment**. Most code in this repo is uncommented and
should stay that way; match the sparse density of the file you are in.
Before writing a comment, apply this test: *does it state something a
competent reader cannot get from the code itself* - a why, a
non-obvious invariant, an external fact, a workaround's cause? If not,
don't write it. Never write a comment to describe your change,
justify it to a reviewer, or describe the previous state of that code; 
that belongs in the commit message.

Required comments (the only two kinds that are mandatory):

- Version markers on match arms: `// Lua 5.2+`, `// Luau`.
- A why-comment when code looks wrong but isn't (e.g. an intentionally
  duplicated branch, an ordering constraint).

Banned outright:

- Restating the code: `// walk the block`, `// return the result`.
- Step narration and section banners: `// Step 1:`, `// --- helpers ---`.
- Filler doc comments that add nothing over the signature.
- `TODO`/`FIXME`/`HACK` - fix it or file an issue instead.
- Comments about the edit itself: `// changed to use X`, `// now handles Y`.
- Massive bloaty banner comments, eg. `---------something---------`

Formatting rules for every comment you do write:

- **Plain ASCII only.** No em/en dashes, curly quotes, ellipsis
  characters, arrows, or any other typographic Unicode - use `-`,
  straight quotes, `...`, `->` spelled in ASCII, or rephrase with a
  comma/colon/parentheses.
- Complete sentences in sentence case, `//` style, ending punctuation
  matching the file's convention. No fragments, no trailing filler.

```rust
// Good: says what the code cannot.
// Shadowed names resolve to the innermost scope, so a flat
// name->value map would miscompile here; walk the scope tree instead.

// Bad: narration, filler, typographic Unicode.
// Loop over the statements — for each one, check if it's unused…
```

### Scope

No overengineering. Three similar lines beats a premature helper.
No `unwrap()` on parser/lexer input - produce `SourceError`.
Write the minimum that solves the task: no speculative config, no
"future-proofing" abstractions, no helpers with one caller.

## Pre-commit gate

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p <crate-you-touched>
```

Only run the full workspace test if you touched something cross-cutting.
The PostToolUse hook auto-runs `cargo fmt` after every `.rs` edit, so
that step is usually already done.

## Final rule

The user's word is final and absolute. If the user contradicts anything
in this file, follow the user.
