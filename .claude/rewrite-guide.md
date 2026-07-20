# luck rewrite standard

This document is the reference standard for the crate-by-crate rewrite of
the luck workspace. You are one agent in that rewrite, assigned one crate.
Read this whole document before touching code. It is a guide to judgment,
not a checklist to grep for: apply what fits your crate, skip what does
not, and let taste decide ties.

## Mandate

The codebase is AI-written with high quality variance: some crates are
excellent, some are visibly uneven, and the goal is to bring everything to
the same high standard. You are expected to rethink your crate from first
principles, not to polish line by line. Nothing structural is off the
table:

- Renaming, splitting, merging, moving, and deleting files and modules.
- Redesigning types, functions, and public APIs - including cross-crate
  APIs, as long as you update every usage site in the workspace so the
  whole workspace builds and passes tests when you are done.
- Rewriting whole subsystems where the design is wrong, not just the code.
- Improving tests structurally (better helpers, better names, less
  brittleness) and adding missing coverage.

Change things because they are better, not to leave fingerprints. A file
that is already excellent should come out untouched or nearly so; a file
that is mediocre should come out unrecognizable. Both outcomes are
success. "No need to do anything for no reason" and "everything may change
if there is a reason" are the same rule.

Restraint has a failure mode too: do not leave a defect you have already
identified unfixed out of churn-aversion. If you noticed it, named it, and
the fix is safe and local (an out-of-order definition, a stutter name, a
dead binding), fix it - "avoiding churn in an otherwise-untouched file" is
not a reason when the diff is small and obviously right. Reserve
"flagged but left alone" for changes that are genuinely risky, genuinely
out of scope (another crate's redesign), or genuinely debatable.

### Fixed points (the only ones)

1. **Observable behavior.** The tool's outputs - parse results, bundled
   output, minified output, formatted output, diagnostics, LSP responses,
   CLI behavior - stay the same unless a test proves the old behavior was
   a bug. If you believe a behavior is wrong, report it; do not silently
   change it.
2. **Test assertions.** You may restructure tests freely, but the facts
   they assert are load-bearing. Never weaken, delete, or invert an
   assertion to make a rewrite pass. A failing test means your rewrite is
   wrong until proven otherwise.
3. **Performance and minsize.** No regressions. The workspace has
   per-stage criterion benches and a committed `minsize.snap`; treat both
   as gates. Perf-motivated ugliness in existing hot paths (fast hashers,
   byte tables, alloc reuse) exists for measured reasons - do not
   "clean it up" into slowness.
4. **The design guidelines in CLAUDE.md.** They are defaults with
   rationale, most backed by tests. If you are convinced one is wrong for
   your crate, flag it in your report with your reasoning instead of
   silently breaking it.

Everything else - structure, naming, APIs, file layout, module
boundaries, internal algorithms, test organization - is yours to change.

## Procedure

1. Read `CLAUDE.md` fully. Then read **every file** in your crate: all of
   `src/`, all of `tests/`, `Cargo.toml`, `README.md`. Do not sample.
2. Form a diagnosis first: what is this crate's job, what would the ideal
   version of it look like, and where does the current code fall short -
   design, API, structure, naming, tests. Write this down for your report.
3. Rewrite. Edit and write files in place (create/delete files as needed,
   but work in the working tree - no scratch copies of the crate).
4. Fix all fallout: if you changed a cross-crate API, update every
   downstream usage site until `cargo build --workspace` succeeds.
5. Update the crate's `README.md` to match reality. If your changes
   invalidate statements in `CLAUDE.md` (architecture table, key entries,
   test layout, design guidelines) or in a skill under `.claude/skills/`,
   update those too - stale docs are a defect you own.
6. Run the gates (below). Iterate until green.
7. Report what you did (see Reporting).

## Gates

Run all of these before you finish; all must pass:

```sh
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p <your-crate>
cargo nextest run -p <each downstream crate you touched>
cargo test --doc -p <your-crate>
```

If you touched anything cross-cutting (luck_token, luck_ast, luck_core, or
any cross-crate API), run the full `cargo nextest run --workspace`.

Snapshot discipline: if bundler/insta snapshots change, read the diff and
confirm every change is an intended consequence of your rewrite before
`cargo insta accept`. A snapshot diff you cannot explain is a bug signal.
If you touched the minifier or codegen, check minsize: run
`cargo test -p luck_benchmark --test metrics` and confirm no size
regression (do not regenerate the snap to hide one).

A PostToolUse hook runs `cargo fmt` after every edit; formatting is
handled for you.

## Rust standard

What follows distills the official Rust API Guidelines, the rustc dev
guide, and the practice of the closest comparable codebases (oxc, biome,
ruff, rust-analyzer, largely via matklad's writing) into the rules that
matter for this workspace. Sources are noted so you can weigh authority.

### API design

- **Conversion naming is a contract** (API Guidelines C-CONV): `as_`
  borrows and is cheap, `to_` produces an owned value and may cost,
  `into_` consumes self. Iterator-producing methods are `iter`/`iter_mut`/
  `into_iter`. A `to_x` that only borrows or an `as_x` that allocates is a
  defect.
- **No boolean blindness.** A call site reading `foo(true, false)` is a
  defect; use a two-variant enum or a small options struct so the meaning
  is legible where it is called. Same for bare magic integers.
- **Newtype indices stay the only way in.** `ScopeId`/`SymbolId`/
  `ReferenceId` style typed ids are the house pattern (same as
  oxc_index); do not introduce raw `usize` indexing into parallel tables
  where a typed id could prevent cross-table mixups.
- **Parameters: concrete `&str`/`&Path` by default.** Reserve
  `impl AsRef<...>` for genuinely polymorphic top-level entry points and
  `impl Into<String>` for functions that store the value. Generics on
  internal APIs buy caller convenience at monomorphization and
  compile-time cost - this workspace does not want that trade.
- **Caller controls allocation** (C-CALLER-CONTROL): take `&T` and borrow
  rather than taking `T` to clone internally; return `Cow<str>` from
  transforms that usually pass input through unchanged rather than
  allocating unconditionally - but only where the path is warm enough to
  matter.
- **Builders over wide constructors** once a type passes roughly four
  configuration axes; never a family of `new_with_x_y` variants.
- **`#[must_use]`** on builders, on pure queries whose result is the whole
  point, and on anything that silently no-ops when dropped. Do not
  blanket-annotate every `-> Self` method.
- **Private fields for invariant-carrying types** (C-STRUCT-PRIVATE): a
  type whose validity depends on an invariant (sorted vec, in-bounds span,
  non-zero id) must not expose the field that could break it; construct
  through validating constructors.
- **`#[non_exhaustive]` by audience**: appropriate on error/config enums
  that cross the published-crate boundary; wrong on AST enums that every
  transform must match exhaustively (this is why `Expression`/`Statement`
  are deliberately exhaustive).
- **Error variants do not stutter** (`SourceError::Parse`, not
  `::ParseError`), and a variant that merely wraps another error with no
  added context (bare `#[from]`) should either gain context (span, path,
  operation) or be questioned. Variant granularity tracks how callers
  recover, not how failures arise internally.
- **Traits: decide sealed vs object-safe deliberately.** The linter's
  dyn-dispatched `Rule` registry requires object safety; document that
  constraint on the trait. Seal traits that are only meant to be
  implemented inside the workspace.
- **No `Deref` to fake inheritance** on domain newtypes; smart pointers
  only.

### Module and crate organization

- **Layered DAG, no upward edges.** Foundation (token) -> ast ->
  parser/semantic -> processing (codegen, minifier, formatter, linter,
  resolver, bundler) -> lsp/cli/facade. A lower crate never names an upper
  one. Context objects wrap downward (LintContext wraps semantic wraps
  ast) instead of upper layers reaching into lower internals - the oxc
  pattern.
- **Visibility is three-tier**: private by default, `pub(crate)` for
  intra-crate sharing, `pub` only for the deliberate cross-crate surface.
  `pub` that no other crate reaches is a defect (matklad's
  `unreachable_pub` discipline). Keep each crate's true public API small
  enough to audit.
- **Named module files over `mod.rs`** for leaf modules; a directory with
  an aggregator only once real submodules exist. Never a directory for a
  single file. (`rules/`, `transforms/`, `providers/` are the justified
  directory cases.)
- **One item per file in registries**: one lint rule, one transform, one
  LSP provider per file, plus exactly one registration edit in the central
  registry (ruff's layout). No second rule squatting in an existing rule
  file.
- **No `utils`/`common`/`helpers` dumping grounds.** Shared code joins the
  most specific domain home (spans in token, diagnostics in core); if a
  helper has no domain home, question whether it deserves to exist.
- **Facade stays logic-free** with explicit named re-exports, never
  `pub use foo::*` - the public surface must be greppable.
- **File size is a symptom, not a rule.** A 2,900-line file with 76
  functions (cli.rs today) is a structure failure; a 900-line file that is
  one coherent algorithm is fine. Split by responsibility, not line count.

### Errors and diagnostics

- **Structured errors in libraries, always.** One `SourceError` type,
  typed fields (spans, found/expected), formatting only at the display
  boundary. `anyhow`-style dynamic errors belong only at the CLI edge, if
  anywhere. Never stringly-typed failures.
- **Panics mean tool bugs.** `unwrap`/`expect`/`panic!` are for internal
  invariant violations only, never reachable from malformed user input;
  user input errors return `SourceError`. An `expect` message states the
  invariant that holds ("peeked statement exists"), not the failure.
- **Diagnostic message style** (rustc/clippy house style): lowercase first
  letter, no trailing period on one-sentence messages, backticks around
  code, problem stated in the first sentence, remedy in a `help`, context
  in a `note` - never a remedy inside a note. Prefer "invalid" over
  "illegal"; imperative suggestions over "did you mean".
- **Spans are as small as possible while still identifying the problem**;
  secondary spans for contributing locations.
- **Codes are minted only when an extended explanation earns them**
  (rustc's rule); build diagnostics only through the `error_at`/
  `warning_at` constructors, never inline literal codes.
- **Fixes carry confidence.** Safe fixes preserve semantics and never
  drop comments (except with a wholly-deleted statement); anything else
  is not auto-applied by `--fix`. If the fix machinery lacks this
  distinction, treat rules whose fixes could change semantics as
  suspect and report it.
- **Parser error recovery is anchor-based**: on error, emit one
  diagnostic, wrap the wreckage in the error node, resync at
  statement-start/`end`-class tokens, and avoid cascading duplicate
  diagnostics from one root cause.

### Testing

- **Purity over layering** (matklad): the pipeline is pure functions, so
  the default test is string-in / value-out with no disk IO, regardless
  of how much of the pipeline it exercises. Fixture-file IO belongs in
  the dedicated sweeps, not in every test.
- **Tests route through one `check`-style helper per subsystem**
  (`run_rule`, `assert_format` are the house instances). Test bodies are
  data: input plus expectation. If your crate's tests each hand-build the
  world, that is rewrite material.
- **Assert on observable output only** - tokens, AST shape, diagnostics,
  emitted text. A test should survive the implementation being swapped
  wholesale. Never assert on private internals.
- **Names describe scenarios, not code paths** (`flags_shadowed_loop_var`,
  not `test_branch_when_scope_depth_gt_1`). Keep the `flags_*`/`ignores_*`
  polarity convention for lint rules.
- **Plain asserts with informative failure messages** (the `"{diags:?}"`
  convention) over assertion DSLs. Error paths assert on returned errors,
  never `#[should_panic]`.
- **Snapshots**: inline expectations for small outputs, file snapshots
  (insta) for large emitted text; never hand-edit a `.snap`; never accept
  a diff you have not read and understood.
- **Property invariants stay continuously enforced**: reparse, roundtrip,
  and idempotency sweeps over the testgen generators are the enforcement
  mechanism for the formatter/minifier guarantees - strengthen them,
  never bypass them.

### Performance with clarity

- **Profile-first culture**: a perf-motivated change to a hot path lands
  with a bench number (`cargo bench -p luck_benchmark`), not a hunch; and
  conversely, cold code is optimized for clarity, full stop.
- **Scale proof burden to regression plausibility.** A change with no
  plausible mechanism for slowdown - a monomorphized generic helper
  replacing hand-duplicated loops, code motion between files, renames,
  visibility changes - does not need to prove itself beyond the bench
  noise floor; demanding that makes all cleanup impossible in perf-gated
  crates. When a change is genuinely suspect, judge it by same-session
  A/B (bench the old and new code back-to-back), never against an
  hours-old saved baseline - wall-clock baselines drift +/-2-4% with
  machine state. CI runs the benches on CodSpeed, which measures
  instruction counts and is immune to that drift; it is the final
  arbiter for close calls.
- **Structural wins over micro-wins**: dispatch shape (bucketed rule bus,
  fixpoint scheduling) is where this workspace gets its speed; keep new
  perf work in that register.
- Cheap idioms with no clarity cost, use freely: `Vec::with_capacity` when
  size is known or estimable, hoist-and-`clear()` buffer reuse in loops,
  `extend` over collect-then-append, `filter_map` over filter+map,
  `write!`/`push_str` into an existing buffer over `format!` in hot
  emission paths, implement `size_hint` on custom iterators.
- **Clones are innocent until profiled guilty.** Do not contort lifetimes
  to shave an unmeasured clone; do remove the ones clippy's
  `redundant_clone` or a profile identifies.
- **Enum size budget interacts with everything**: SmallVec inline
  capacity, new variant fields - run the luck_ast size tests; box rare
  large variants so the common case stays lean.
- **Cache-friendly by construction**: flat tables plus typed ids over
  pointer-chasing node graphs (the existing semantic design; keep new
  graph structures on it).
- **No shared-lock caches in rayon paths** - the interner rule
  generalizes: any "speed up with a global cache" idea must answer for
  contention under parallelism first.

### Comments, naming, scope

CLAUDE.md's Style section is the standard; the essentials that most
rewrites get wrong:

- Default to no comment; a comment must say something the code cannot
  (why, invariant, external fact, workaround cause). Version markers on
  match arms (`// Lua 5.2+`, `// Luau`) are mandatory. Never narrate
  steps, restate code, or describe your edit. Plain ASCII, complete
  sentences.
- Full words, domain abbreviations required (`stmt`, `expr`, `lhs`,
  `span`); no `val`/`tmp`/`res`; bools read as predicates
  (`is_`/`has_`/`should_`/`can_`); collections plural. `ctx`/`cfg` stay
  banned outside the three grandfathered uses.
- No overengineering: no helper with one caller, no speculative
  configuration, no future-proofing abstractions. Three similar lines
  beat a premature helper. This applies to your rewrite itself: do not
  introduce architecture your crate does not need yet.

## TypeScript standard (vscode extension agent only)

- The extension host layer stays a thin client: lifecycle, config
  plumbing, binary discovery, and UI only; all language intelligence
  lives in the Rust server (rust-analyzer/biome discipline).
- Activation is lazy and narrow: minimal `activationEvents`, import-light
  `extension.ts` top, heavy setup deferred to first use.
- The client is one lifecycle state machine (stopped/starting/running/
  restarting) owned by one object with guarded transitions - no scattered
  `client?` null-checks. Client-lifetime disposables are rebuilt per
  restart, extension-lifetime disposables survive it; everything lands in
  `context.subscriptions`, and `deactivate` awaits `client.stop()`.
- Logging through a `LogOutputChannel` (`{ log: true }`) with real levels;
  a `luck.trace.server` setting routes LSP traffic to a separate trace
  channel. `showErrorMessage` is reserved for actionable failures and
  carries action buttons (Open Log / Restart); stack traces go to the log,
  never toasts.
- tsconfig: `strict` plus `noUncheckedIndexedAccess`,
  `exactOptionalPropertyTypes`, `noImplicitReturns`. If build tooling is
  touched: esbuild-bundled single-file output with `external: ['vscode']`
  and a separate `tsc --noEmit` type gate.

## Reporting

Your final report must contain:

1. **Diagnosis**: what was wrong with the crate as found (concrete, with
   the worst offenders named).
2. **What changed**: the redesigns and restructures, and why each is
   better - not a file-by-file changelog.
3. **Cross-crate fallout**: every API you changed and the downstream
   crates you updated for it.
4. **Docs updated**: README/CLAUDE.md/skills touched.
5. **Flags**: behaviors you believe are bugs (left intact), design
   guidelines you disagree with, quality problems you saw in *other*
   crates, and anything you deliberately left alone with the reason.
6. **Gate results**: the exact commands you ran and their outcomes.
