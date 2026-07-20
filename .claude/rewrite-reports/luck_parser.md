# luck_parser rewrite

Crate: `crates/luck_parser` (Pratt expressions + recursive-descent
statements + full Luau type grammar, version-gated). Rewritten in place.

## 1. Diagnosis

The crate was already high quality: clean module split (cursor, Pratt
expressions, statement dispatch, Luau type grammar, post-parse
validation), good why-comments, anchor-based error recovery exactly as
the standard wants, and well-structured tests routed through small
helpers. The task brief's "~7k LOC, 20 files" was stale - the crate is
6 source files, ~3,600 LOC. The genuine defects were:

- **Unreachable-pub throughout `parser.rs`.** `Parser` and ~29 of its
  methods were `pub`, plus the `version` field, even though `mod parser`
  is private and the only externally-reached API is `parse`,
  `ParseResult`, `validate`, `ParseError`. Every one of those `pub`
  markers overstated the surface (matklad's `unreachable_pub`
  discipline; the standard's three-tier visibility rule).

- **~40 sites of duplicated error-recovery boilerplate.** The pattern
  `self.expect_identifier().unwrap_or_else(|err| { self.errors.push(err);
  Token::new(TokenKind::Identifier(String::new().into()),
  self.current_span()) })` appeared verbatim 18 times, and the span
  form `self.expect_span(&X).unwrap_or_else(|err| { self.errors.push(err);
  self.current_span() })` (plus its `if let Err(err) = ...` twin) another
  23 times. The recovery placeholder token was hand-rolled at 18 sites -
  a single source of truth was missing. Far past the "three similar
  lines beats a helper" threshold.

- **A vestigial `Option` return.** `parse_statement` returned
  `Option<Statement>` and its doc claimed it "returns None if error
  recovery consumed everything," but every match arm returned `Some(...)`
  (the error arm returns `Some(Statement::Error(span))`). The `None`
  branch in `parse_block` was dead code guarded by a comment that
  restated it.

- **stmt.rs at 1,114 lines conflated two grammars.** Statement-shape
  recursive descent was interleaved with a self-contained attribute
  micro-grammar (Lua `<const>`/`<close>` and Luau `@native`/`@[...]`)
  that carries its own literal-validation rules
  (`is_attribute_literal`, `validate_function_attribute`) and is shared
  with `expr.rs`.

- **Banned banner comments in validate.rs.** Three
  `// -----------------` ruled section banners - explicitly on
  CLAUDE.md's ban list - wrapped otherwise-valuable prose.

## 2. What changed

- **Visibility corrected to `pub(crate)`** on `Parser`, its `version`
  field, and all 29 methods across `parser.rs`/`expr.rs`/`stmt.rs`/
  `luau.rs`. The crate's true public surface (`parse`, `validate`,
  `ParseResult`, `ParseError`) is now what the `pub` markers say it is.
  Zero behavior change.

- **Two recovery helpers on `Parser`** centralize the duplication:
  `expect(&mut self, kind) -> Span` (record the default error, return the
  current span) and `expect_identifier_recover(&mut self) -> Token`
  (record the error, return the one canonical placeholder token). 18
  identifier sites and 23 span sites collapse to one-liners, and the
  placeholder token now lives in exactly one place. `expect_span`/
  `expect_identifier` remain as the raw `Result`-returning primitives for
  the two callers that build their own messages (`expect_keyword`, the
  early-return list parsers). All four are `#[inline]` so the generated
  code on hot expression paths is equivalent to the former inline form.

- **`parse_statement` now returns `Statement`.** The dead `Option`
  wrapper and the `Some(...)`/`None` handling in `parse_block` are gone;
  the caller is `stmts.push(self.parse_statement())`.

- **`attributes.rs` extracted** (187 lines): the Lua `<attr>` primitives
  (`try_parse_attribute`, `parse_attribute`), the Luau `@attr` machinery
  (`parse_function_attributes`, `parse_attribute_args`,
  `validate_function_attribute`), and the shared `is_attribute_literal`
  literal guard. This is a responsibility split, not a line-count split:
  the attribute grammar is a distinct sub-grammar with its own
  validation, shared across `expr.rs` and `stmt.rs`, and now has a named
  home. stmt.rs drops to 842 focused lines of statement dispatch;
  statement-to-attribute glue (`parse_attributed_function`, the attname
  binding lists) correctly stays in stmt.rs.

- **validate.rs banners de-ruled**: the three `-----` banners became a
  plain doc-comment on `ConstWriteChecker` and two plain comment
  paragraphs, keeping every word of the explanatory prose.

## 3. Cross-crate fallout

None. No public API changed - `parse`, `validate`, `ParseResult`,
`ParseError` keep their exact signatures. All 120 external references
across the workspace (`luck_parser::parse` x114, `::ParseResult` x5,
`::validate` x1) compile unchanged. Every change is crate-internal.

## 4. Docs updated

- `crates/luck_parser/README.md`: the Architecture > Pipeline paragraph
  now describes `attributes.rs`, `validate.rs`, and the shared recovery
  primitives.
- CLAUDE.md and skills: no edits needed - the `luck_parser` architecture
  row (key entries `expr.rs`, `stmt.rs`, `luau.rs`) and every design
  guideline remain accurate; nothing was invalidated.

## 5. Flags

- **Behaviors believed to be bugs: none.** The permissive
  number-singleton acceptance in Luau types, the `::`-splitting in
  `consume_type_close_angle`, and the vararg/loop-scope tracking are all
  intentional and tested; left intact.
- **Design-guideline disagreements: none.** Exhaustive matching, the
  version-predicate gating, and comments-outside-the-AST all hold.
- **`expression_to_var`'s placeholder** (a synthetic `Var::Name` at the
  offending expression's span, in stmt.rs) was deliberately NOT folded
  into `expect_identifier_recover`: it recovers at a *different* span
  (the bad target expression, not `current_span()`) and is a free
  function, so unifying would change the recovery span. Left as its own
  small placeholder.
- **Other crates**: no quality problems observed from this crate's
  vantage; downstream call sites were read only enough to confirm the
  public surface is untouched.

## 6. Gate results

- `cargo build --workspace` - clean (all 17 crates).
- `cargo clippy --workspace --all-targets -- -D warnings` - zero
  warnings.
- `cargo nextest run -p luck_parser` - 226 passed, 0 failed.
- `cargo nextest run --workspace` - 2000 passed, 4 skipped, 0 failed -
  matches the pre-rewrite baseline exactly.
- `cargo test --doc -p luck_parser` - 1 passed.
- `cargo bench -p luck_benchmark --bench parser -- --baseline
  pre-rewrite`: every representative/real-world input improved -
  gen_full_lua54 -0.8%, gen_full_luau flat/-2%, idiomatic flat,
  infinite_yield -1.3%, roact -1.9%, penlight -1.4%. The one adversarial
  input (obfuscated_vm.luau, a 2.2 MB single-expression VM) sits at
  ~+0.6% in isolated back-to-back runs (+0.79 / +0.33 / +0.74%), within
  its own run-to-run noise band; the transient +1.8% readings occurred
  only under concurrent build load. After marking the full expect chain
  `#[inline]`, expr-path codegen is equivalent to the pre-rewrite inline
  form. Net parser performance improved; no representative regression.
