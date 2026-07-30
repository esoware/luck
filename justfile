#!/usr/bin/env -S just --justfile

set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]
set shell := ["bash", "-cu"]

_default:
  @just --list -u

alias r := ready
alias t := test
alias f := fmt

# Install the cargo tools the recipes below use
init:
  cargo install cargo-nextest cargo-insta --locked

# Run the same checks as CI (fmt, clippy, tests, doctests)
ready:
  cargo fmt --all
  just lint
  just test
  just doctest
  git status

# Build all crates
build *args:
  cargo build --workspace {{args}}

# Run all tests; scope with e.g. `just test -p luck_parser goto_statement`
test *args:
  cargo nextest run --workspace {{args}}

# Run doctests (nextest does not run them)
doctest:
  cargo test --doc --workspace

# Format all code
fmt:
  cargo fmt --all

# Clippy with zero warnings allowed
lint *args:
  cargo clippy --workspace --all-targets {{args}} -- -D warnings

# Accept pending insta snapshots (non-interactive; `cargo insta review` is for humans)
accept:
  cargo insta accept

# Criterion benches; one stage with e.g. `just bench --bench parser`
bench *args:
  cargo bench -p luck_benchmark {{args}}

# Regenerate the VS Code luck.json schema after config type changes
schema:
  cargo test -p luck_core regenerate_luckrc_schema -- --ignored

# Regenerate roblox_api.toml + roblox_enums.toml from the live Roblox API dump
roblox-api:
  cargo test -p luck_semantic regenerate_roblox_api -- --ignored

# Regenerate the committed minsize.snap after minifier/corpus changes
minsize:
  cargo test -p luck_benchmark --test metrics regenerate_minsize -- --ignored

# Run a fuzz target (nightly): `just fuzz fuzz_parser`
fuzz target *args:
  cargo +nightly fuzz run {{target}} --fuzz-dir fuzz {{args}}
