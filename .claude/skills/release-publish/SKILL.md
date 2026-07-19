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

Also verify versions were bumped for every crate whose behavior changed
since the last tag (`/bump-versions`), and that `luck` (facade) tracks
the highest bump.

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
luck
luck_cli
```

Verify the order is still correct against reality before emitting it
(dependencies change):

```sh
cargo metadata --format-version=1 --no-deps
```

Only include crates whose version changed. For each, the human runs
`cargo publish -p <crate>` and waits for the registry to index before
the next dependent.

## 3. Tag

After publishing: `git tag v<luck_cli-version>` (annotated), push the
tag. The VS Code build workflow (`.github/workflows/vscode.yml`)
triggers on `vscode-v*` tags - tag `vscode-v<extension-version>` only
when the extension itself should ship.

## 4. VS Code extension

`editors/vscode/` versions independently. Before tagging a `vscode-v*`
release: bump `editors/vscode/package.json` version, confirm the schema
(`schemas/luckrc.schema.json`) is current (`cargo test -p luck_core`),
and confirm the extension's declared CLI compatibility matches the
published `luck_cli`.

## 5. Hand-off format

End by giving the human one copy-pasteable block: the ordered
`cargo publish -p ...` commands, the tag commands, and nothing else.
State explicitly which crates are being skipped (unchanged) so the
omission is visibly deliberate.
