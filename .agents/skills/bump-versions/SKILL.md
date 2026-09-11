---
name: bump-versions
description: Applies the workspace-wide lockstep version bump (single shared 0.x version for every publishable crate and the VS Code extension). Use when asked to bump versions, prep a release, or determine what a version should be.
---

# Bump the workspace version

Luck versions in **lockstep**: every publishable crate and the VS Code
extension share ONE version number and move together. There is no per-crate
bumping and no dependency fan-out analysis.

## Where the version lives

- `[workspace.package] version` in the root `Cargo.toml`, inherited by
  every crate via `version.workspace = true`.
- Every `luck*` entry in `[workspace.dependencies]` in the root `Cargo.toml`
  carries the same `version = "..."` (required for publishing path deps).
- `editors/vscode/package.json` and the two root `version` fields in its
  `package-lock.json`. The extension version must match the
  `serverInfo.version` the LSP reports.

`luck_testgen` and `luck_benchmark` are internal (`publish = false`) and stay
at `0.0.0` forever. Never bump them.

## Choosing the bump (0.x rules)

One bump per release, decided by the most severe change anywhere in the
workspace since the last release tag
(`git log $(git tag --sort=-v:refname | head -1)..HEAD`, or the whole history
if untagged):

- Breaking public-API or behavior change anywhere -> **minor** (0.X+1.0).
- Features, new rules/transforms/options, non-breaking additions ->
  **minor** (0.X+1.0).
- Bug fixes only -> **patch** (0.X.Y+1).

In 0.x, cargo treats minor as the compatibility major, so breaking and
additive changes both land on minor; patch is reserved for pure-fix releases.
If one crate had a feature, the whole workspace gets the minor.

## Verify

```sh
cargo metadata --no-deps --format-version=1   # every luck crate shows the new version
cargo check --workspace                       # manifests resolve
```

Any publishable crate reporting a different version means a spot was missed.
