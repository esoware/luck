---
name: bump-versions
description: Applies the workspace-wide lockstep version bump (single shared 0.x version for every crate and the VS Code extension). Use when asked to bump versions, prepare for commit, prep a release, or determine what a version should be.
allowed-tools: Read, Edit, Bash(cargo metadata:*), Bash(cargo check:*), Bash(git diff:*), Bash(git status:*), Bash(git log:*), Bash(git tag:*)
---

# Bump the workspace version

Luck versions in **lockstep**: every publishable crate and the VS Code
extension share ONE version number and move together. There is no
per-crate bumping, no per-crate policy table, and no dependency fan-out
analysis - that scheme is dead.

## Where the version lives

The single source of truth is the root `Cargo.toml`:

- `[workspace.package] version = "X.Y.Z"` - inherited by every crate
  via `version.workspace = true`.
- The 15 `luck*` entries in `[workspace.dependencies]` each carry
  `version = "X.Y.Z"` (required for publishing path deps).
- `editors/vscode/package.json` (and its `package-lock.json` root
  entries) carry the same number, so the extension version always
  matches the `serverInfo.version` the LSP reports.

`luck_testgen` and `luck_benchmark` are internal (`publish = false`)
and stay at `0.0.0` forever - never bump them.

## Choosing the bump (0.x rules)

One bump per release, decided by the most severe change anywhere in the
workspace since the last release tag (`git log $(git tag --sort=-v:refname | head -1)..HEAD`
or the whole history if untagged):

| Most severe change in the release | Bump |
|---|---|
| Any breaking public-API or behavior change, anywhere | **minor** (0.X+1.0) |
| Features, new rules/transforms/options, non-breaking API additions | **minor** (0.X+1.0) |
| Bug fixes only | **patch** (0.X.Y+1) |

In 0.x land cargo treats the minor as the compatibility major, so
breaking and additive changes both land on minor; patch is reserved for
pure-fix releases. Do not agonize per crate - if one crate had a
feature, the whole workspace gets the minor.

## Apply

Edit the version string in exactly these places, all to the same value:

1. `[workspace.package] version` in the root `Cargo.toml`.
2. Every `version = "..."` inside the `luck*` `[workspace.dependencies]`
   entries in the root `Cargo.toml` (one file, one pass).
3. `"version"` in `editors/vscode/package.json` and the two root
   `version` fields in `editors/vscode/package-lock.json`.

## Verify

```sh
cargo metadata --no-deps --format-version=1   # every luck crate shows the new version
cargo check --workspace                       # manifests resolve
```

Every publishable crate must report the identical version; any
divergence means a spot was missed.
