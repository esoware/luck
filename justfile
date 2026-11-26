#!/usr/bin/env -S just --justfile

set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]
set shell := ["bash", "-cu"]

_default:
  @just --list -u

alias r := ready
alias t := test
alias f := fmt

# Fetch the Luau submodule, and install the extension's packages, cargo-nextest, cargo-insta, cargo-deny, cargo-shear, and zizmor
init:
  git submodule update --init
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

# Run all tests, or narrow them with nextest args like -p luck_parser if_expression
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

# Run clippy and fail on any warning
lint *args:
  cargo clippy --workspace --all-targets --all-features --keep-going --config "build.warnings='deny'" {{args}}

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

# Regenerate roblox_api.toml, roblox_enums.toml, and roblox.luau from the live Roblox API dump and Creator Documentation, then the stdlib catalog
roblox-api:
  cargo test -p luck_stdlib regenerate_roblox -- --ignored
  just stdlib

# Regenerate the committed minsize.snap after the minifier or the benchmark files change
minsize:
  cargo test -p luck_benchmark --test it regenerate_minsize -- --ignored

# Rewrite the committed allocs_*.yaml snapshots after a stage changes, or with `rules`, write the gitignored per-rule linter report
allocs *args:
  cargo run -p luck_allocs --profile allocs -- {{args}}

# Move the Luau submodule that luck_type_checker builds to a release tag, such as `just luau 0.749`
luau tag:
  git -C crates/luck_type_checker/luau fetch --depth 1 origin tag {{tag}}
  git -C crates/luck_type_checker/luau checkout {{tag}}

# Run Luau's official conformance tests through every stage and the reference interpreter, and rewrite the committed snapshots, or with `--filter strings`, print the failures of the files whose name holds it
conformance *args:
  cargo run -p luck_conformance --profile conformance -- {{args}}
