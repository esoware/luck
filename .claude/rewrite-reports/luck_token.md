# Rewrite report: luck_token

## 1. Diagnosis

`luck_token` was already one of the stronger crates in the workspace: the
literal decode/encode logic, `CodeBuffer` UTF-8-invariant design, token
tables, and the version-predicate family are all well-built and heavily
tested. The rewrite here is deliberately conservative - most of the crate
came out untouched. The concrete defects found:

- **Boolean-blind public API.** `parse_lua_number(text, int_subtype: bool)`
  is the standard's textbook boolean-blindness case. Its three linter call
  sites read `parse_lua_number(&literal.text, true)` - the bare `true` says
  nothing at the call site. The bool encodes a genuine independent choice
  (whether the dialect distinguishes int/float subtypes), not a mere version
  flag: the Roblox rules deliberately pass `true` even though Luau has no
  integer subtype, so switching the parameter to `LuaVersion` would have
  changed folding behavior. That confirmed a two-variant enum, not a version,
  was the right abstraction.
- **Banned identifier names.** `CodeBuffer` stored its buffer as
  `buf: Vec<u8>` and `literal.rs` used a `let mut buf = [0u8; 4]` scratch
  array - `buf` is on CLAUDE.md's explicit ban list.
- **A misleadingly-named closure.** `encode_string_literal`'s inner closure
  was named `pending_digit_follows` - named after a condition, describing
  nothing about what it does (it writes a byte as a decimal escape).
- **Structural inconsistency in file layout.** The crate gave `token`,
  `literal`, `comment`, and `code_buffer` their own files but dumped the two
  headline types - `Span` and the 160-line `LuaVersion` predicate family -
  plus `StdlibEnvironment` and `SourceError` into `lib.rs`. The most-
  referenced types in the whole workspace were the least navigable.
- **Stale README claims.** The README asserted `SourceError` carries
  "optional help text" (it has only span + message) and described
  `CommentKind` as "(line, block, shebang)" when it has four variants
  (`Line`, `SingleLineBlock`, `MultiLineBlock`, `Shebang`).
- **Minor `#[must_use]` gaps**: `TokenKind::is_stat_start` and `is_unary_op`
  were the only predicates in the file lacking the annotation every sibling
  had.

## 2. What changed

- **`Span` moved to `span.rs`, `LuaVersion` + `StdlibEnvironment` to
  `version.rs`.** `lib.rs` is now a clean crate root: docs, module
  declarations, `SourceError` (kept here rather than a 6-line `error.rs`),
  and the re-export surface. All types stay re-exported at the crate root, so
  every external path (`luck_token::Span`, `luck_token::LuaVersion`, ...) is
  unchanged. This makes the crate's file layout consistent - one coherent
  responsibility per file - and puts the highest-traffic types where they are
  found by name.
- **`NumberSubtypes { Unified, IntFloat }` enum replaces the `int_subtype`
  bool** on `parse_lua_number`. Call sites are now self-documenting
  (`parse_lua_number(&literal.text, NumberSubtypes::IntFloat)`). The enum's
  two variants map exactly onto the old `true`/`false`, so folding behavior
  is bit-for-bit preserved (verified by the workspace idempotency/roundtrip
  sweeps).
- **`buf` -> `bytes`** (CodeBuffer field + its outlined `push_slow` param)
  and the scratch `buf` -> `encoded` in `literal.rs`, matching the encoding
  scratch buffer already named `encoded` elsewhere.
- **`pending_digit_follows` -> `push_decimal_escape`.**
- **`#[must_use]`** added to the two token predicates that lacked it.

No hot-path algorithm changed. `TokenKind`/`Token` sizes are unchanged
(the pinned 32/40 size test still passes).

## 3. Cross-crate fallout

One public API changed: `parse_lua_number`'s second parameter went from
`bool` to `NumberSubtypes` (new type, re-exported from the crate root and
from `luck_token::literal`). Downstream updates:

- `luck_minifier/src/expr.rs`: `extract_lua_number` keeps its internal
  `int_subtype: bool` (it is threaded through the minifier's fold/shorten
  transforms and is out of scope to re-plumb); it maps the bool to the enum
  once at the `parse_lua_number` boundary.
- `luck_linter/src/rules/roblox_incorrect_color3_new_bounds.rs`,
  `roblox_manual_fromscale_or_fromoffset.rs`, `roblox_suspicious_udim2_new.rs`:
  the three `parse_lua_number(&literal.text, true)` calls now pass
  `NumberSubtypes::IntFloat`.

Module moves caused zero fallout: no downstream crate imported `Span`/
`LuaVersion`/`StdlibEnvironment`/`SourceError` by submodule path (they use
crate-root re-exports); the four submodules other crates *do* import by path
(`token`, `literal`, `comment`, `code_buffer`) are untouched.

## 4. Docs updated

- `crates/luck_token/README.md`: fixed the false "optional help text" and the
  wrong `CommentKind` variant list; added a bullet documenting the shared
  literal decode/encode surface.
- `CLAUDE.md`: refreshed the `luck_token` architecture-table row (key-entry
  files now point at `span.rs`, `version.rs`, `token.rs`, `literal.rs`,
  `code_buffer.rs`; added "shared literal decode/encode" to the role).
- `.claude/skills/add-lua-version-feature/SKILL.md`: two references to
  `luck_token/src/lib.rs` for the `LuaVersion` predicates repointed to
  `version.rs`.

## 5. Flags

- **`SourceError` is a flat `{ span, message: String }`, not a structured
  typed error.** The Rust standard prefers structured library errors, but
  this is a deliberate, tested, deeply-embedded design point (the whole
  toolchain aliases it as `LexError`/`ParseError`/`FormatError` for one
  rendering path; the richer diagnostic codes live in `luck_core`).
  Restructuring it would be a workspace-wide change well beyond this crate
  and would alter no observable behavior. Left intact by design.
- **The minifier still threads `int_subtype: bool` internally.** That bool is
  legibly named and derived from `version.has_integer_subtype()` at its
  origin; re-plumbing the enum through `fold_constants`/`shorten_numbers`
  would be churn in another crate's owned surface. The blindness that
  mattered - the bare `true` literals - is fixed. Left for the minifier's own
  pass to decide.
- No behaviors were changed. No bugs found in this crate. No other-crate
  quality issues worth flagging beyond the minifier note above.

## 6. Gate results

- `cargo build --workspace` - clean.
- `cargo clippy --workspace --all-targets -- -D warnings` - zero warnings.
- `cargo nextest run --workspace` - 2000 passed, 4 skipped, 0 failed
  (cross-cutting crate, so full workspace run).
- `cargo test --doc -p luck_token` - 1 passed.
- `RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc -p luck_token
  --no-deps` - clean (new intra-doc links resolve).
- `cargo bench -p luck_benchmark --bench lexer -- --baseline pre-rewrite` -
  within noise (improvements or noise-threshold on every input; this crate
  does not touch the lexer path).
- `cargo bench -p luck_benchmark --bench parser -- --baseline pre-rewrite` -
  every input improved or within noise; no regressions.
- Minsize: `parse_lua_number` logic is unchanged (only the parameter type),
  the enum maps exactly onto the old bool, and `minify_is_idempotent_and_
  reparses` passes, so minified output is unchanged by construction. The
  committed `minsize_is_up_to_date` metrics test is corpus-fetch-gated
  (ignored locally) and was not regenerated.
