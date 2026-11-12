---
name: add-lsp-provider
description: Adds or extends a language-server feature in luck_lsp through the provider/declare/handler/capability four-site pattern, whether that is hover, completion, references, rename, semantic tokens, code actions, or any new capability. Use when asked to add LSP or editor-feature support, or for any edit under crates/luck_lsp/src/providers/.
---

# Add an LSP provider

luck_lsp is a library-only tower-lsp-server backend served via `luck lsp`. Every feature follows the same four-site pattern. Miss the last site and the handler is dead code:

1. **Provider module** (`src/providers/<feature>.rs`): a pure function taking the document state pieces it needs and returning the LSP response type. Keep tokio and `tower_lsp_server::Client` out of providers so they stay testable. Read a neighboring provider of similar shape first (`hover.rs` for cursor-position features, `document_symbol.rs` for whole-document, `code_action.rs` for lint-coupled).
2. **Declare** it in `providers.rs` as a `pub(crate) mod`. Providers aren't public API. Only `Backend`, `Notifier`, and the `serve_*` entry points are.
3. **Backend handler** in `backend.rs`: snapshot the document, call the provider, map errors. Mirror a neighboring handler for the lock/snapshot pattern, and don't hold the documents lock across heavy work.
4. **Capability registration** in `initialize()`. This is the step that gets forgotten, and without it clients never call the handler.

Provider correctness rules:

- LSP positions are UTF-16 line/character. Convert at the boundary with `DocumentState`'s `offset`, `position`, and `range` (`src/document.rs`, backed by `LineIndex` in `src/line_index.rs`), never byte arithmetic on protocol positions.
- Reuse `DocumentState`'s cached parse/analysis. Re-running `parse`/`analyze` per keystroke is this crate's known performance trap.
- Name resolution goes through `luck_semantic`'s scope tree, not text matching.
- Reuse span/walk helpers from `providers/cursor.rs` before writing a new giant match.

## Test

Integration test in `crates/luck_lsp/tests/` using the existing harness (build the server, `did_open` a fixture, call the trait method, assert), or a unit test in the provider's own `mod tests` on a `DocumentState` when the case needs a target the server can't pick from a file name, such as Roblox. Cover a hit, a miss (cursor on empty space), and a multi-byte/UTF-16 case (emoji or CJK before the target).

## Gate and bump

`just lint && just test -p luck_lsp`. New capability = minor bump: follow `.agents/skills/bump-versions/SKILL.md`. Check `editors/vscode/` only if the extension must advertise or configure the feature.
