# luck_ast rewrite report

## 1. Diagnosis

`luck_ast` is the AST foundation: `Expression`/`Statement`/`Type` under a
64-byte budget, the `Visitor`/`AstTransform` traversal pair, the `synth`
builder, and the `NodeType`/`NodeKind`/`AstTypesBitset` node tables. Read in
full, it is one of the *excellent* crates in the workspace: the enums are
exhaustive by design, the visitor/transform are textbook, the node-table
dispatch is const-friendly and pinned by tests, and the synth surface is
carefully documented (spans, guarantees, caller obligations). It is also the
most heavily depended-upon crate in the tree - its public modules
(`expr`, `stmt`, `types`, `shared`, `node`, `synth`, `transform`, `visitor`,
`query`) are named by path across 100+ files.

That combination - already-good plus load-bearing - means the disciplined
move is a light touch on genuine defects, not a rewrite for fingerprints.
The concrete defects found:

- **`builder.rs` is misnamed.** It contains no builders: only the
  `Punctuated<T>` inherent-method block and the family of `span()`
  accessors. The README itself had to apologize for the name ("there are no
  fluent `with_*` constructors"). The module is declared `pub mod builder`
  but is never referenced by path anywhere in the workspace (inherent
  methods reach their types without it), so the name is pure noise.
- **The README was stale in three places.** It documented a `ContainedSpan`
  type that does not exist anywhere in the crate; it claimed `Punctuated<T>`
  "preserves separator tokens" when the type does the exact opposite (a
  `Vec<T>` plus a `has_trailing_separator` bool, with separators implied by
  position and *no* tokens or spans stored); and it described the
  now-removed `builder.rs`.

Everything else - the 64-byte budget, the exhaustive matches, the visitor
recursion, the 2,310-line `synth.rs` - was judged correct and left intact
(see What changed for the synth.rs judgment call).

## 2. What changed

- **Renamed `builder.rs` -> `span.rs` and split it by responsibility.** The
  `Punctuated<T>` inherent impl moved next to its own struct definition in
  `shared.rs` (where it belongs - the methods are the type's own API, not a
  separate "builder"). The `span()` accessor family (`Statement`,
  `LastStatement`, `Expression`, `Var`, `Field`, `Type`, `TypeField`) stayed
  together in the new `span.rs`, which is now honestly named for its one
  responsibility: span extraction as a single greppable surface. The old
  file's misleading name is gone and with it the README apology it forced.
  No behavior, signature, or memory layout changed - this is pure code
  motion of inherent impls within the crate.

- **README rewritten to match reality**: dropped `ContainedSpan`, corrected
  the `Punctuated<T>` description to state that separators are implied and
  unstored, and replaced the `builder.rs` sentence with an accurate note on
  where `span()` accessors and `Punctuated` helpers now live.

### synth.rs judgment (explicitly requested)

`synth.rs` is 2,310 lines but ~800 of those are tests; the ~1,500 lines of
implementation are a flat list of tiny `impl Synth` constructor methods plus
four small escaping helpers. This is **one coherent surface, not a structure
failure**: it is a single builder type whose value is discoverability -
every constructor for the AST lives on one `Synth`. Splitting it into
`synth/literals.rs`, `synth/stmts.rs`, `synth/types.rs` would fragment one
`impl` block across files and make the API *harder* to navigate, buying
nothing. Left as is, deliberately.

## 3. Cross-crate fallout

None. The only structural change (`builder.rs` -> `span.rs`) touched a module
that no other crate names by path, and it moved inherent methods without
altering any signature. The full workspace builds unchanged. No downstream
crate required edits.

## 4. Docs updated

- `crates/luck_ast/README.md` - the three stale statements above.
- `crates/luck_ast/src/span.rs` - new module doc for the span accessors.
- `CLAUDE.md` - no change needed: the `luck_ast` architecture row lists
  `expr.rs`/`stmt.rs`/`types.rs`/`transform.rs`/`synth.rs`/`node.rs` and
  never referenced `builder.rs`.
- No skill under `.claude/skills/` references the renamed module.

## 5. Flags

- **`Punctuated::last_item` (left intact).** It is the odd member of an
  otherwise clean API - `first()` pairs naturally with `last()`, and
  `last_item` breaks that symmetry and the slice/`Vec` convention. Renaming
  to `last()` would be a genuine, if cosmetic, improvement, but it ripples to
  ~15 call sites across parser tests, the minifier, and the linter for zero
  behavioral gain. Deliberately left alone to avoid churn in files outside
  this crate's focus; flagged here rather than changed.
- **No behaviors believed to be bugs.** The precedence/prefix-wrapping logic
  in `synth`, the `saturating_sub(3)` `end`-keyword span heuristic, and the
  permissive numeric-singleton acceptance in `Type` are all intentional and
  documented; left intact.
- **No design guideline disagreements.** The 64-byte budget, exhaustive
  matching, and comments-outside-the-AST rules all hold and are enforced by
  tests that still pass.
- **Other crates**: no quality problems observed from this crate's vantage;
  I only read downstream call sites enough to confirm the module surface is
  stable.

## 6. Gate results

- `cargo build -p luck_ast` - clean.
- `cargo build --workspace` - clean (all 17 crates).
- `cargo clippy --workspace --all-targets -- -D warnings` - zero warnings.
- `cargo nextest run -p luck_ast` - 59 passed, 0 failed. The
  `ast_node_layouts` and `ast_enum_sizes` pins pass unchanged, proving
  byte-identical memory layout.
- `cargo test --doc -p luck_ast` - 2 passed.
- `cargo nextest run --workspace` - 2000 passed, 4 skipped, 0 failed -
  matching the pre-rewrite baseline exactly.
- Benches vs `pre-rewrite` baseline (`synth`, `codegen`, `parser`): all
  deltas within +/-2%, symmetric in both directions (e.g. synth/codegen
  -3.3%, synth/build +2.2%, parser mostly p > 0.05). Consistent with
  criterion microbenchmark noise, not regression - expected, since the change
  is intra-crate code motion that cannot alter generated code or layout.
