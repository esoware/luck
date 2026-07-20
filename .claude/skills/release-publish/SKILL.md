---
name: release-publish
description: Brings the tree to a provably releasable state and produces the dependency-ordered publish checklist for the human to run (cargo publish is deny-listed for agents). Use when asked to cut a release, publish luck, ship a new version, tag a release, prepare a release checklist, or package the VS Code extension.
allowed-tools: Read, Edit, Write, Grep, Glob, Bash(cargo:*), Bash(git status:*), Bash(git diff:*), Bash(git log:*), Bash(git tag:*)
---

# Release & publish

`cargo publish` is intentionally denied to agents in this repo. Your job
is to bring the tree to a provably releasable state and hand the human
an exact, dependency-ordered command list. Never attempt to publish,
yank, or push tags yourself.

## 1. Pre-flight (all must be green)

```sh
git status                     # clean tree, on main
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

Also verify the lockstep workspace version was bumped (`/bump-versions`):
every publishable crate and the VS Code extension share ONE version,
inherited from `[workspace.package]`. If `cargo metadata` shows any
publishable crate at a different version, stop - a spot was missed.

Registry naming (decided 2026-07): the crate name `luck` is taken on
crates.io by an unrelated project, so the facade publishes as the
package `luck-lua` with `[lib] name = "luck"` - users still write
`use luck::...`. All `luck_*` crates publish under their real names,
and `luck_cli` keeps its `luck` binary. Do not revisit this decision.

## 2. Dependency-ordered publish list

Crates must publish in dependency order (path deps must already exist on
the registry at the required version). The order for this workspace:

```
luck_token
luck_lexer
luck_ast
luck_parser
luck_codegen
luck_core
luck_semantic
luck_resolver
luck_bundler
luck_minifier
luck_formatter
luck_linter
luck_lsp
luck-lua
luck_cli
```

Verify the order is still correct against reality before emitting it
(dependencies change):

```sh
cargo metadata --format-version=1 --no-deps
```

Versions move in lockstep, so every publishable crate ships every
release - the full ordered list above, no skipping. For each, the human
runs `cargo publish -p <crate>` and waits for the registry to index
before the next dependent.

## 3. Tag

After publishing: `git tag v<workspace-version>` (annotated), push the
tag. The VS Code build workflow (`.github/workflows/vscode.yml`)
triggers on `vscode-v*` tags - tag `vscode-v<workspace-version>` when
the extension should ship (its version is the same workspace number).

## 4. VS Code extension

`editors/vscode/` shares the workspace version (kept in sync by
`/bump-versions`; the LSP's reported `serverInfo.version` is the same
number, so the extension UI and the extension version always agree).
Before tagging a `vscode-v*` release: confirm the schema
(`schemas/luckrc.schema.json`) is current (`cargo test -p luck_core`).

## 5. Hand-off format

End by giving the human one copy-pasteable block: the ordered
`cargo publish -p ...` commands, the tag commands, and nothing else.
State explicitly which crates are being skipped (unchanged) so the
omission is visibly deliberate.
