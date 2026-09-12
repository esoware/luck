---
name: release-publish
description: Brings the tree to a provably releasable state and produces the dependency-ordered publish checklist for the human to run (cargo publish is denied to agents). Use when asked to cut a release, publish luck, ship a version, tag a release, or package the VS Code extension.
---

# Release and publish

`cargo publish` is intentionally denied to agents in this repo. Your job is
to bring the tree to a provably releasable state and hand the human an exact,
dependency-ordered command list. Never attempt to publish, yank, or push tags
yourself.

## 1. Pre-flight (all must be green)

```sh
git status                     # clean tree, on main
cargo fmt --all -- --check
just lint
just test
just doctest
cargo build --workspace --release
```

Verify the lockstep version was bumped
(`.agents/skills/bump-versions/SKILL.md`): every publishable crate and the
VS Code extension share one version. If `cargo metadata` shows any
publishable crate at a different version, stop.

Registry naming (decided 2026-07, do not revisit): the crate name `luck` is
taken on crates.io, so the facade publishes as package `luck_lua` with
`[lib] name = "luck"`, so users still write `use luck::...`. All `luck_*`
crates publish under their real names; `luck_cli` keeps its `luck` binary.

## 2. Dependency-ordered publish list

Crates must publish in dependency order (path deps must already exist on the
registry at the required version). Derive the current order from reality.
Dependencies change, so never reuse a remembered order:

```sh
cargo metadata --format-version=1 --no-deps
```

Topologically sort the publishable crates by their `luck*` dependencies.
Versions move in lockstep, so every publishable crate ships every release.
No skipping. For each crate the human runs `cargo publish -p <crate>` and
waits for the registry to index before the next dependent.

## 3. Tag

After publishing: annotated `git tag v<workspace-version>`, push the tag.
The VS Code build workflow triggers on `vscode-v*` tags, so also tag
`vscode-v<workspace-version>` when the extension should ship.

## 4. VS Code extension

`editors/vscode/` shares the workspace version. Before tagging a `vscode-v*`
release, confirm the schema is current: `just test -p luck_core`.

## 5. Hand-off format

End with one copy-pasteable block: the ordered `cargo publish -p ...`
commands, then the tag commands, and nothing else.
