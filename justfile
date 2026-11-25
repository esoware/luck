#!/usr/bin/env -S just --justfile

set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]
set shell := ["bash", "-cu"]

_default:
  @just --list -u

alias r := ready
alias t := test
alias f := fmt

# Install the extension's packages, cargo-nextest, cargo-insta, cargo-deny, cargo-shear, and zizmor
init:
  npm ci --prefix editors/vscode
  cargo install --locked cargo-nextest@0.9.151 cargo-insta@1.49.0 cargo-deny@0.20.2 cargo-shear@1.14.0 zizmor@1.32.0

# Run fmt, clippy, rustdoc, cargo-shear, zizmor, the extension's type check and oxlint, the tests with the unreferenced-snapshot check, the doctests, and cargo-deny
ready:
  just fmt
  just lint
  cargo doc --workspace --no-deps --document-private-items --keep-going --config "build.warnings='deny'"
  cargo shear
  zizmor --offline --pedantic .github
  npm run --prefix editors/vscode check
  npm run --prefix editors/vscode lint
  just snapshots
  just doctest
  just deny
  git status

# Run all tests, or narrow them with nextest args like -p luck_parser goto_stmt
test *args:
  cargo nextest run {{args}}

# Run all tests and fail on an insta snapshot none of them asserts, such as a deleted fixture's
snapshots:
  cargo insta test --check --fail-fast --unreferenced reject --test-runner nextest --disable-nextest-doctest

# Run the doctests, which nextest skips
doctest:
  cargo test --doc --workspace

# Format Rust and the extension
fmt:
  cargo fmt --all
  npm run --prefix editors/vscode fmt

# Run clippy and fail on any warning, with luck_conformance on its own under Lua 5.4 and Luau, since its dialect features exclude each other
lint *args:
  cargo clippy --workspace --exclude luck_conformance --all-targets --all-features --keep-going --config "build.warnings='deny'" {{args}}
  cargo clippy -p luck_conformance --all-targets --keep-going --config "build.warnings='deny'" {{args}}
  cargo clippy -p luck_conformance --all-targets --no-default-features --features luau --keep-going --config "build.warnings='deny'" {{args}}

# Accept pending insta snapshots, since cargo insta review is interactive
accept:
  cargo insta accept

# Check bans, licenses, and sources
deny:
  cargo deny check bans licenses sources

# Kept out of ready because a new advisory can fail an unchanged commit. CI runs it daily.
# npm audit skips moderate until vscode-languageclient accepts minimatch 11, the only fix for GHSA-g3hv-75wg-m249.

# Check the locked crates and the packages the extension ships against published advisories
advisories:
  cargo deny check advisories
  npm audit --prefix editors/vscode --omit=dev --audit-level=high

# Run the criterion benches, or one stage with --bench parser
bench *args:
  cargo bench -p luck_benchmark {{args}}

# Regenerate luck_ast/src/generated after a type that derives `Ast` changes
ast:
  cargo run -p luck_ast_tools

# Regenerate editors/vscode/schemas/luckrc.schema.json after a config type changes
schema:
  cargo test -p luck_config regenerate_luckrc_schema -- --ignored

# Regenerate luck_stdlib's static stdlib catalog after a stdlib_data file changes
stdlib:
  cargo test -p luck_stdlib regenerate_stdlib_catalog -- --ignored

# Regenerate roblox_api.toml and roblox_enums.toml from the live Roblox API dump, then the stdlib catalog
roblox-api:
  cargo test -p luck_stdlib regenerate_roblox_api -- --ignored
  just stdlib

# Regenerate the committed minsize.snap after the minifier or the benchmark files change
minsize:
  cargo test -p luck_benchmark --test it regenerate_minsize -- --ignored

# Rewrite the committed allocs_*.yaml snapshots after a stage changes, or with `rules`, write the gitignored per-rule linter report
allocs *args:
  cargo run -p luck_allocs --profile allocs -- {{args}}

# Run each dialect's official test suite through every stage and its reference interpreter, and rewrite the committed snapshots, or with `--filter strings`, print the failures of the files whose path holds it
conformance *args:
  cargo run -p luck_conformance --profile conformance --no-default-features --features lua51 -- {{args}}
  cargo run -p luck_conformance --profile conformance --no-default-features --features lua52 -- {{args}}
  cargo run -p luck_conformance --profile conformance --no-default-features --features lua53 -- {{args}}
  cargo run -p luck_conformance --profile conformance --no-default-features --features lua54 -- {{args}}
  cargo run -p luck_conformance --profile conformance --no-default-features --features lua55 -- {{args}}
  cargo run -p luck_conformance --profile conformance --no-default-features --features luau -- {{args}}
