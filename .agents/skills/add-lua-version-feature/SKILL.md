---
name: add-lua-version-feature
description: Adds support for a syntactic feature introduced in a specific Lua version or Luau across predicate, lexer, parser, fixtures, and downstream consumers. Use when asked to support Lua 5.x or Luau syntax, implement a version-gated feature, or change LuaVersion predicates.
---

# Add a Lua version feature

A version-gated feature crosses the token -> lexer -> parser -> AST ->
fixture -> downstream-consumer boundary. Skip a step and the feature silently
fails for users on other versions, or parses and then panics in codegen.
Work through this checklist in order:

1. **Predicate.** Add `has_<feature>` to `LuaVersion`
   (`luck_token/src/version.rs`), matching the existing family. Name it by
   the feature, not the version, so future Lua releases inherit it without
   code edits. Never add new `is_*` feature predicates (only
   `is_luau`/`is_roblox` exist), and never gate on an unrelated predicate
   that happens to have the right version set. Add a new one even if its
   body is identical to an existing one.
2. **Lexer gating** (only if the feature has new tokens). On unsupported
   versions, fall back to producing the pre-feature tokens so the parser
   emits a normal grammar error. Never panic.
3. **Parser gating** through the predicate, never a direct variant
   comparison. Statement grammar in `stmt.rs`, expressions in `expr.rs`,
   Luau type grammar in `luau.rs`.
4. **Version comments** on every downstream match arm handling the new AST
   variant (`// Lua 5.3+`, `// Luau`).
5. **Parser tests** in the per-version files
   (`luck_parser/src/tests/lua5x.rs`): the lowest supporting version parses
   it with zero errors; one version below rejects it with >=1 error.
6. **Fixture** under `tests/fixtures/<version>/` (repo root). Parser and
   bundler integration tests pick it up automatically; confirm
   `detect_version` maps the fixture directory.
7. **Downstream consumer walk.** Exhaustive matching means the compiler
   points at every site. Do not silence it with `_ =>` arms. Ask per crate:
   codegen (does the compact printer emit it? new separator-test case if
   tokens can merge wrongly), minifier (do transforms preserve it? attributes
   must block removal/lifting), formatter (layout + idempotency), semantic
   (binding kinds, new scopes), linter (rules that must know it), lsp
   (span-walking providers).
8. **Gate.** `just lint && just test`, workspace-wide, since this is a
   cross-cutting change.
9. **Bump.** Minor for every crate that gained behavior; token/lexer/parser
   always, others only if touched. Follow
   `.agents/skills/bump-versions/SKILL.md`.
