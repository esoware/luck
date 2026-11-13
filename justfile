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

# Run fmt, clippy, rustdoc, the fuzz crate's check, cargo-shear, zizmor, the extension's type check and oxlint, the tests, the doctests, and cargo-deny
ready:
  just fmt
  just lint
  cargo doc --workspace --no-deps --document-private-items --keep-going --config "build.warnings='deny'"
  cargo check --manifest-path fuzz/Cargo.toml
  cargo shear
  zizmor --offline --pedantic .github
  npm run --prefix editors/vscode check
  npm run --prefix editors/vscode lint
  just test
  just doctest
  just deny
  git status

# Run all tests, or narrow them with nextest args like -p luck_parser goto_stmt
test *args:
  cargo nextest run {{args}}

# Run the doctests, which nextest skips
doctest:
  cargo test --doc --workspace

# Format Rust, the fuzz crate, and the extension
fmt:
  cargo fmt --all
  cargo fmt --manifest-path fuzz/Cargo.toml
  npm run --prefix editors/vscode fmt

# Run clippy and fail on any warning
lint *args:
  cargo clippy --workspace --all-targets --all-features --keep-going --config "build.warnings='deny'" {{args}} -- --cap-lints warn

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

# Regenerate editors/vscode/schemas/luckrc.schema.json after a config type changes
schema:
  cargo test -p luck_config regenerate_luckrc_schema -- --ignored

# Regenerate roblox_api.toml and roblox_enums.toml from the live Roblox API dump
roblox-api:
  cargo test -p luck_semantic regenerate_roblox_api -- --ignored

# Regenerate the committed minsize.snap after the minifier or the corpus changes
minsize:
  cargo test -p luck_benchmark --test it regenerate_minsize -- --ignored

# Run a fuzz target on nightly, such as just fuzz fuzz_parser
fuzz target *args:
  cargo +nightly fuzz run {{target}} --fuzz-dir fuzz {{args}}
