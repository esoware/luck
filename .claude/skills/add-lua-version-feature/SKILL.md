---
name: add-lua-version-feature
description: Adds support for a syntactic feature introduced in a specific Lua version or Luau - goto/labels (5.2+), integer division and bitwise ops (5.3+), const/close attributes (5.4+), generalized iteration (5.5+), Luau extensions (interpolated strings, type casts, compound assignment, attributes) - across predicate, lexer, parser, fixtures, and downstream consumers. Use when asked to support Lua 5.x syntax, implement a version-gated feature, or change LuaVersion predicates.
argument-hint: <feature-name>
allowed-tools: Read, Edit, Write, Grep, Glob, Bash(cargo:*)
---

# Add a Lua version feature

A version-gated feature crosses the token -> lexer -> parser -> AST ->
fixture -> downstream-consumer boundary. Skip a step and the feature
silently fails for users on other versions, or worse, succeeds and
panics in codegen.

Copy this checklist into your response and check items off as you go:

```
- [ ] 1. has_<feature> predicate in luck_token
- [ ] 2. Lexer gating (only if new tokens)
- [ ] 3. Parser gating through the predicate
- [ ] 4. Version comments on downstream match arms
- [ ] 5. Parser tests: lowest supporting version parses, one below rejects
- [ ] 6. Fixture under tests/fixtures/<version>/
- [ ] 7. Downstream consumer walk (codegen/minifier/formatter/semantic/linter/lsp)
- [ ] 8. Workspace clippy + tests green
- [ ] 9. Version bumps via /bump-versions
```

## Layout

| Where | What |
|---|---|
| `crates/luck_token/src/version.rs` | `LuaVersion` + feature predicates |
| `crates/luck_lexer/src/{lexer,number,string}.rs` | lexer gating |
| `crates/luck_parser/src/expr.rs` | Pratt expression parser |
| `crates/luck_parser/src/stmt.rs` | recursive-descent statement parser |
| `crates/luck_parser/src/luau.rs` | Luau type grammar parser (`parse_type`, generic lists, `>>`/`>=` splitting) |
| `crates/luck_parser/src/tests/{lua51,lua52,lua53,lua54,lua55,luau}.rs` | per-version unit tests |
| `tests/fixtures/<version>/` | shared fixtures (used by parser + bundler) |

## Steps

### 1. Add the predicate

In `crates/luck_token/src/version.rs`. The naming convention is
**`has_<feature>`** - match the existing family (`has_goto`,
`has_floor_div`, `has_bitwise_ops`, `has_compound_assignment`, ...).
The only `is_` predicates are the dialect checks `is_luau`/`is_roblox`;
do not add new `is_*` feature predicates.

```rust
impl LuaVersion {
    pub fn has_$ARGUMENTS(self) -> bool {
        matches!(self, LuaVersion::Lua54 | LuaVersion::Lua55 | LuaVersion::Luau)
    }
}
```

Name the predicate by the **feature**, not the version. Readers should
see *what* is gated, not just *when* it landed. Future Lua releases
inherit the feature without code edits.

### 2. Gate the lexer (only if the feature has new tokens)

```rust
if self.version.has_$ARGUMENTS() && self.peek() == '!' {
    return self.lex_$ARGUMENTS();
}
```

On unsupported versions, fall back to producing the pre-feature tokens.
The parser will then emit a normal grammar error - never panic.

### 3. Gate the parser

```rust
if self.version.has_$ARGUMENTS() && self.peek_is_$ARGUMENTS_start() {
    return self.parse_$ARGUMENTS();
}
```

Always go through the predicate, never `LuaVersion::Lua54` directly.
Never gate on an unrelated predicate that happens to have the right
version set - add the new predicate even if its `matches!` body is
identical to an existing one.

### 4. Annotate downstream match arms

Every `match` arm that handles the new AST variant gets a short version
comment (`// Lua 5.3+`, `// Luau`). Future maintainers walking
transforms, visitors, codegen, and formatter need to know which version
introduced each variant.

### 5. Parser tests

In the lowest version that supports the feature
(`crates/luck_parser/src/tests/lua5x.rs`):

```rust
#[test]
fn lua5x_parses_$ARGUMENTS() { /* expect zero errors */ }

#[test]
fn lua5x_minus_one_rejects_$ARGUMENTS() { /* expect >=1 error */ }
```

If the feature affects round-tripping (new tokens, new statement shapes),
also confirm the fixture round-trip tests in
`crates/luck_parser/tests/integration.rs` cover the fixture you add -
check that its `detect_version` maps your fixture directory.

### 6. Fixtures

Add a fixture under `tests/fixtures/<version>/`. Bundler and parser
integration tests pick it up automatically.

### 7. Walk the downstream consumers

Exhaustive matching means the compiler tells you every site that needs
updating - **don't add catch-all `_ => ...` arms** to silence it.

| Crate | Question |
|---|---|
| `luck_codegen` | Does the compact printer emit the new syntax? Add a separator-test case if tokens can now merge wrongly. |
| `luck_minifier` | Do transforms preserve / understand it? Add a minifier metamethod test if the feature involves operators. Attributes (`<const>`/`<close>`) must block removal and lifting; merging is allowed because `AttributedName` pairing carries the attribute along structurally. |
| `luck_formatter` | Does the formatter know how to lay it out? Idempotency must hold for the new construct. |
| `luck_semantic` | Does scope/symbol resolution handle it (e.g., binding kinds, new statement scopes)? |
| `luck_linter` | Does any rule need to know about it (`unused_variable` on `<close>` locals, etc.)? |
| `luck_lsp` | Do span-walking providers (cursor, selection range, semantic tokens) handle the new variant? |

### 8. Gate

```sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### 9. Version bumps

Minor bump every crate that gained behavior. Token / lexer / parser
always bump. Codegen / formatter / minifier / semantic / linter / lsp
bump only if you had to touch them. Use `/bump-versions` to walk the
policy.
