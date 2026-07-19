---
name: bump-versions
description: Decides and applies the semver bump for every crate touched by a change, per project policy and the cross-crate fan-out rules. Use after any non-trivial code change in crates/ that will be committed, or when asked to bump versions, prepare for commit, prep a release, or determine what a version should be.
allowed-tools: Read, Edit, Bash(cargo metadata:*), Bash(git diff:*), Bash(git status:*)
---

# Bump crate versions

Luck follows strict semver per crate. Only bump crates whose **behavior**
changed - `cargo fmt`-only edits, comment-only edits, and test-only edits
do not warrant a bump.

## Policy

| Kind of change | Bump |
|---|---|
| Bug fix (no public-API change) | **patch** |
| New feature / new public API surface / new lint rule / new transform / new config field | **minor** |
| Breaking change to public types or behavior | **major** |
| Internal refactor with no observable behavior change | **none** |
| Format / comment / test-only | **none** |

`luck` (the facade) tracks the highest-bumped underlying crate. If you
minor-bumped `luck_minifier` and `luck_linter`, minor-bump `luck` too.

Note the cross-crate couplings that are easy to miss:
- A new minifier transform also adds a `TransformConfig` field ->
  **minor bump `luck_core` too**.
- A new lint rule, format option, or CLI-visible behavior usually means
  the generated schema changed -> `luck_core` and the schema test.

## Read the live versions

Never trust a written snapshot of version numbers - read them:

```sh
cargo metadata --no-deps --format-version=1 | python -c "import json,sys; [print(p['name'], p['version']) for p in json.load(sys.stdin)['packages']]"
```

or grep `version =` in each touched crate's `Cargo.toml`.

## Determine what changed

```sh
git diff --stat
git diff -- crates/<crate>/
```

For each crate with edits beyond `tests/`, comments, or formatting, run
through the policy table.

## Walk the dependency graph

Workspace dependencies in the root `Cargo.toml` are path-based. A minor
or major bump in a foundation crate (`luck_token`, `luck_ast`, `luck_core`)
requires minor bumps in every consumer that exposes the bumped type in
its own public API.

The fan-out for a public-type change in each foundation crate:

| Foundation crate | Public-API consumers |
|---|---|
| `luck_token` | every other crate (re-exports `Span`, `SourceError`, `LuaVersion`) |
| `luck_ast` | `luck_parser`, `luck_codegen`, `luck_minifier`, `luck_formatter`, `luck_linter`, `luck_semantic`, `luck_lsp` |
| `luck_core` | `luck_resolver`, `luck_bundler`, `luck_minifier`, `luck_linter`, `luck_lsp`, `luck_cli`, `luck` |
| `luck_semantic` | `luck_linter`, `luck_lsp` |
| `luck_linter`, `luck_formatter` | `luck_cli`, `luck_lsp` |

For internal-only changes (private impls, helpers) you don't have to
ripple the bump - only when the type or trait was re-exported or changed
its public signature.

## Apply the bump

Each crate's version lives in `crates/<crate>/Cargo.toml` under
`[package]`. Workspace deps in the root `Cargo.toml` are by path, not by
version, so no second edit is needed there.

## Verify

```sh
cargo build --workspace        # sanity
cargo test -p <bumped-crate>   # ensure tests still pass
```

If `cargo build` fails because a dependent crate references a removed or
renamed symbol, you may have a major change disguised as a minor - go
back to the policy table and reclassify.
