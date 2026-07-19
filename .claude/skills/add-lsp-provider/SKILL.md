---
name: add-lsp-provider
description: Adds or extends a language-server feature in luck_lsp - hover, completion, go-to-definition, references, rename, semantic tokens, inlay hints, code actions, folding, or any new capability - through the provider/export/handler/capability four-site pattern. Use when asked to add LSP support for X, implement an editor feature, or for any edit under crates/luck_lsp/src/providers/.
argument-hint: <feature-name>
allowed-tools: Read, Edit, Write, Grep, Glob, Bash(cargo:*)
---

# Add an LSP provider

luck_lsp is a library-only tower-lsp backend served via `luck lsp`. Every
feature follows the same four-site pattern: provider module -> mod.rs
export -> backend handler -> capability registration. Miss the capability
registration and clients never call the handler.

Before writing anything, read one existing provider of similar shape
(`hover.rs` for cursor-position features, `document_symbol.rs` for
whole-document features, `code_action.rs` for lint-coupled features).

## Layout

| Where | What |
|---|---|
| `crates/luck_lsp/src/providers/` | one module per feature; `pub mod` in `providers/mod.rs` |
| `crates/luck_lsp/src/providers/cursor.rs` | shared cursor/AST-position helpers |
| `crates/luck_lsp/src/backend.rs` | `Backend` state, `LanguageServer` trait impl, capability registration in `initialize` |
| `crates/luck_lsp/src/line_index.rs` | UTF-16 <-> byte offset conversion - **all** protocol positions go through this |
| `crates/luck_lsp/src/config.rs` | luck.json / editorconfig discovery + caching |
| `crates/luck_lsp/tests/integration.rs` | harness: drives the real `LanguageServer` trait with `CapturedNotifier` |

## Steps

### 1. Provider module

`crates/luck_lsp/src/providers/$ARGUMENTS.rs` - a pure function taking
the document state pieces it needs (text, `LineIndex`, cached
`ParseResult`, target) and returning the LSP response type. Keep tokio
and `tower_lsp::Client` out of providers; they stay testable as plain
functions.

Rules that keep providers correct:

- **Positions**: LSP positions are UTF-16 line/character. Convert at the
  boundary via `LineIndex` - never do byte arithmetic on protocol
  positions, never pass byte spans to the client.
- **Reuse cached work**: `DocumentState` caches the parse (and any other
  analysis added since). Never re-run `luck_parser::parse` or
  `luck_semantic::analyze*` inside a provider if the state already has
  it - redundant per-keystroke analysis is this crate's known perf trap.
- **Name resolution goes through `luck_semantic`'s scope tree**, not
  text matching - a name-string match across the file conflates
  unrelated locals.
- **Exhaustive matches** over `Expression`/`Statement` (hard invariant
  3) - use span/walk helpers from `cursor.rs` instead of writing a new
  giant match if one exists.

### 2. Export

Add `pub mod $ARGUMENTS;` to `providers/mod.rs`.

### 3. Backend handler

Implement the `LanguageServer` trait method in `backend.rs`: snapshot the
document, call the provider, map errors to `JsonRpcResult`. Mirror a
neighboring handler for the lock/snapshot pattern (don't hold the
documents lock across heavy work).

### 4. Register the capability

In `initialize()` in `backend.rs`, add the matching
`ServerCapabilities` field. This is the step that gets forgotten -
without it, the handler is dead code from the client's point of view.
If the capability has options (trigger characters, token legends), keep
them next to the provider and re-export.

### 5. Test

Add an integration test in `crates/luck_lsp/tests/integration.rs` using
the existing harness (build the server, `did_open` a fixture document,
call the trait method directly, assert on the response). Cover: a hit, a
miss (cursor on empty space), and a multi-byte/UTF-16 case (emoji or CJK
before the target).

### 6. Gate

```sh
cargo clippy -p luck_lsp --all-targets -- -D warnings
cargo test -p luck_lsp
```

### 7. Version bump

Minor bump `luck_lsp` (new capability = new feature). Use `/bump-versions`.
If the VS Code extension needs to advertise or configure the feature,
check `editors/vscode/`.
