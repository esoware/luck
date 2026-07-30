# AGENTS.md

luck is a Lua/Luau bundler, minifier, formatter, linter, and language server:
multi-file Lua in, single bundled file out. Supports Lua 5.1-5.5 and Luau
(standalone and Roblox). Rust workspace, edition 2024, MSRV 1.88; lexer,
parser, AST, and codegen are hand-written - no external parser dependency.

Read `ARCHITECTURE.md` for the pipeline, the crate map, the three target
axes, and design rationale before cross-crate work. Each crate's README
covers its internals.

## Commands

Use `just` (`just --list` shows everything; raw cargo works too):

```sh
just test                                # all tests (cargo-nextest)
just test -p luck_parser goto_statement  # one crate / one test
just doctest                             # doctests (nextest skips them)
just lint                                # clippy, zero warnings allowed
just fmt
just ready                               # fmt + lint + test + doctest (CI mirror)
just accept                              # accept insta snapshots (non-interactive)
just schema                              # regen VS Code luck.json schema after config changes
just roblox-api                          # regen Roblox API data from the live dump
just minsize                             # regen committed minsize.snap
just bench --bench parser                # criterion benches, one per stage
```

Pre-commit gate: `just fmt && just lint`, plus `just test -p <crate-you-touched>`.
Run full `just ready` only for cross-cutting changes. (In Claude Code, a
PostToolUse hook already runs `cargo fmt` after every `.rs` edit.)

## Invariants

Rules that look optional but are backed by tests or hard-won bugs:

- **Three target axes, never conflated**: `LuaVersion` (syntax),
  `StdlibEnvironment` (standalone vs Roblox stdlib), `LuaTarget` (user-facing
  dialect; projects onto the other two once at each entry boundary).
  Codegen-side crates take `LuaVersion` only, so `-t roblox` and `-t luau`
  minify identically - correct, not a bug. Details in `ARCHITECTURE.md`.
- **Version gating goes through `LuaVersion::has_<feature>` predicates**,
  never direct variant comparison (the only `is_` forms are
  `is_luau`/`is_roblox`).
- **Emitters never read source text.** Leaf text comes from token-carried
  values; source is consulted only for trivia fidelity and must degrade
  gracefully when absent - synthetic ASTs must stay printable.
- **Bundle output names no host paths.** Every path the emitted bundle
  carries - provenance comments, 5.2+ loader data - comes from
  `ModuleInfo::relative_path`, which is project-relative (`..` segments above
  the root, file name only when there is no shared root). `ModuleInfo::path`,
  `source_files`, and the line map stay canonical: local build data, never
  emitted.
- **Exhaustive matches in transforms and visitors** - no `_ =>` catch-alls,
  so a new variant makes the compiler point at every consumer.
- **Purity analysis assumes metamethods.** Only literal arithmetic is pure;
  variable reads, indexing, arithmetic, comparison, and concat may all invoke
  metamethods. `#"str"` is never folded.
- **Idempotency is a tested guarantee**: `minify(minify(x)) == minify(x)`,
  `format(format(x)) == format(x)`, and both outputs reparse clean.
- **One error type (`SourceError`), one diagnostic scheme.** Codes live in
  `luck_core::diagnostics::errors`; build with `error_at`/`warning_at`, never
  inline literal codes. Parse failures are always E008.
- **Config is one typed source of truth**: `luck.json`, types in `luck_core`,
  `deny_unknown_fields` everywhere. The VS Code schema is generated - never
  hand-edit `editors/vscode/schemas/luckrc.schema.json`; run `just schema`.
  Format-option precedence: defaults < `.editorconfig` < `luck.json` `format`.
- **No global string interner** - a shared interner's lock serializes rayon
  workers; identifiers stay per-token `CompactString`.

## Source directives (tests need the exact syntax)

- Lint: `-- luck: allow(rule_a, rule_b)` / `deny(...)` / `warn(...)` applies
  to the next statement; append `start` / `end` for a region; file-level form
  is `-- #luck: allow(foo)`.
- Formatter: `-- luck: format off` / `-- luck: format on` disables a region;
  `-- luck: ignore` (alias `-- luck: format ignore`) skips one statement.

## Tests

Standard Rust layout: white-box tests inline in `#[cfg(test)] mod tests` at
the bottom of the owning file; public-API tests in the crate's `tests/` dir
(single binary `tests/it/main.rs` with submodules where the suite is large).
Never `src/tests/` directories. Shared fixtures live at the repo root in
`tests/fixtures/{lua51..lua55,luau,idiomatic}/`; `idiomatic/` must stay
lint-clean.

- Lint rules: positive cases `flags_*`, negatives `ignores_*`, via
  `crate::test_support::run_rule`; count assertions carry a `"{diags:?}"`
  message. Suppression is tested centrally, not per rule.
- Fix tests apply the edit and assert the result re-parses.
- Formatter tests go through `assert_format`/`assert_format_with`
  (idempotency + reparse checked on every call).
- Error paths assert on returned `errors`, never `#[should_panic]`.

## Style

- Naming: full words; no `val`/`tmp`/`res`/`buf`/`str`. Domain abbreviations
  are required, not optional: `stmt`, `expr`, `ast`, `ir`, `span`, `lhs`,
  `rhs`, `args`, `params`. `ctx`/`cfg` are banned except three grandfathered
  uses (formatter `Ctx`, linter `LintContext`, control-flow graph in
  `luck_linter/src/cfg.rs`) - and never `cfg` for "config". Bools start with
  `is_`/`has_`/`should_`/`can_`; collections are plural.
- Comments: default to none - match the sparse density of the file. Write one
  only for what code can't say: a why, a non-obvious invariant, an external
  fact. Version markers on match arms (`// Lua 5.2+`, `// Luau`) are
  mandatory. Plain ASCII, complete sentences. No TODO/FIXME, no step
  narration, no banners, no comments describing the edit itself.
- Scope: no overengineering - three similar lines beat a premature helper; no
  speculative config or single-caller helpers. No `unwrap()` on parser/lexer
  input; produce `SourceError`.

## Workflows

Multi-step task recipes live in `.agents/skills/*/SKILL.md`: adding a lint
rule, formatter rule, minifier transform, config option, LSP provider, or
Lua-version feature, bumping versions, and releasing. When a task matches
one, read and follow it - each encodes registration steps and cross-crate
plumbing that is easy to miss. (`.claude/skills/` holds thin pointer stubs
for Claude Code discovery; always edit the `.agents/skills/` copies.)

Versioning is lockstep: every publishable crate and the VS Code extension
share one workspace version. Follow `.agents/skills/bump-versions/SKILL.md`;
internal harnesses stay at 0.0.0.
