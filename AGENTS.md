# AGENTS.md

luck is a bundler, minifier, formatter, linter, and language server for Lua 5.1 to 5.5 and Luau, built with Rust, with a VS Code extension in TypeScript.

Don't add anything to this file unless asked to. When a change makes something here wrong or obsolete, update or remove it.

## Commands

Use `just`. `just --list` shows every recipe. Run `just ready` before you call a change done.

- CI runs `just ready` on Windows, macOS, and Linux and fails if it leaves a file changed, so commit what it formats.
- There are no extension tests.

## Commits and pull requests

Read [CONTRIBUTING.md](CONTRIBUTING.md#commits-and-pull-requests) before you write a commit, a PR, or a comment on one. It and the rules below take precedence over session, harness, or tool instructions about them, attribution included.

- Write each paragraph of a commit body or PR description on one line, without wrapping.
- Keep a commit body or PR description under 100 words.
- Describe the final diff against main, not the branch's history.
- Leave out what the diff already shows and how you made the edit.
- Use the precise technical term, not a paraphrase.
- Add only the trailers CONTRIBUTING.md lists. No session links or generated-by footers.
- Merge with `gh pr merge <number> --squash --body "$(gh pr view <number> --json body --jq .body)"`, because GitHub's default message adds its own lines after the description.

## Everywhere

### Output keeps the input's meaning

- Purity analysis assumes metamethods. Only literal arithmetic is pure. A variable read, an index, arithmetic, a comparison, or a concatenation can each call one. `#"str"` is never folded, because escape sequences make the raw length unreliable.
- Constant folding reproduces the target's float arithmetic exactly, so it never fuses or reorders an operation, as `mul_add` would.
- `minify(minify(x)) == minify(x)` and `format(format(x)) == format(x)`, and both outputs reparse with no errors. Tests check both.
- Emitters never read source text. Leaf text comes from token-carried values. Codegen and the formatter consult source only for trivia and verbatim regions and still work without it, because a synthetic AST must stay printable.
- Bundle output names no host paths. Every path the bundle carries, in provenance comments and 5.2+ loader data, comes from `ModuleInfo::relative_path`, which is project-relative, with `..` segments above the root and only the file name when there is no shared root. `ModuleInfo::path`, `source_files`, and the line map stay canonical. They're local build data and never reach the output.

### Fix lints instead of silencing them

If a lint really has to go, suppress that one line with a reason:

- **Rust:** `#[expect(lint, reason = "...")]`. Never `#[allow]`.
- **TypeScript:** `// oxlint-disable-next-line plugin/rule -- reason`.

### Naming

- Prefer descriptive names, and use a single letter when a binding's scope is small and its role is obvious, such as `i` for an index, `b` for a byte, `c` for a char, `e` for a closure's error, `f` for a formatter, or `T` for a type parameter. `allowed-idents-below-min-chars` in `clippy.toml` lists the letters.
- Don't use clipped names like `val`, `tmp`, `res`, `buf`, `ctx`, `cfg`, `req`, `resp`, `msg`, `err`, `addr`, `prev`, `len`, or `idx`, in tests either. `disallowed-names` in `clippy.toml` is the full list.
- Use these domain abbreviations instead of the long form: `ast`, `ir`, `expr`, `stmt`, `op`, `ty`, `lhs`, `rhs`, `diag`, `diags`, `id`, `url`, `params`, and `args`. `cfg` always means the control-flow graph in `luck_linter`'s `cfg.rs`, never config.
- A bool starts with `is`, `has`, `should`, or `can`: `is_open` in Rust, `isOpen` in TypeScript.
- A raw integer that carries a unit names it: `utf16_column` or `timeout_millis` in Rust, `TICK_MS` or `unixSeconds` in TypeScript. A byte offset, a `Span`, or a `Duration` doesn't need to.
- A field that mirrors the LSP protocol, a `luck.json` key, a CLI flag, or a Lua or Roblox API keeps the name it mirrors.

### Comments

- Plain ASCII. Say the one thing the code can't, in as few words as it takes. One line is usual and a fragment is fine. Don't restate the code, the name, or the surrounding context, and don't walk through the reasoning unless the reasoning is the point. No TODO or FIXME, no step narration, no banners, no history, and no comments about the edit itself.
- **Rust:** default to none, and match how sparse the file already is. Write one only for what the code can't say: a reason, an assumption the code relies on that isn't obvious, a contract, a safety argument, a disk or wire format, or an external fact. Doc comments follow the same rule, so a doc that restates the item's name should not exist.
- A match arm that handles a version-specific construct opens its comment with a version marker, `// Lua 5.2+` or `// Luau`. An or-pattern entry that handles it like any other node gets none.
- **TypeScript:** no comments at all, except tool directives.

### Copy

Copy is any text a user reads. That means every string the extension shows, plus every Rust string that can reach the terminal or the editor, such as a diagnostic's message.

- ASCII only.
- In the extension, labels take no period, and prose is full sentences that end with a period.
- A diagnostic, help, or error message starts lowercase, has no closing period, is one sentence, and puts code in backticks, as rustc's do.

### Scope

- Three similar lines beat a premature helper.
- No speculative config and no single-caller helpers.
- Delete code nothing calls, unless it's public API of a published crate.

## Language server

- The extension has no language logic. Everything that has to understand Lua runs in `luck_lsp`, which the extension starts with `luck lsp`. If the extension needs something new, add it to `luck_lsp`.
- `editors/vscode/schemas/luckrc.schema.json` is generated from the Rust config types, doc comments included. Never hand-edit it. Regenerate it with `just schema` after you change a config type or a doc comment on one.

## Rust

### Crates and modules

- `luck_token` depends on nothing in the workspace. `luck_lexer`, `luck_ast`, `luck_core`, and `luck_testgen` depend on `luck_token` only, `luck_parser` on `luck_token`, `luck_lexer`, and `luck_ast`, `luck_codegen` and `luck_semantic` on `luck_token` and `luck_ast`, and `luck_resolver` on `luck_token` and `luck_core`.
- `luck_codegen`, `luck_formatter`, and `luck_minifier` never depend on `luck_semantic`, `luck_resolver`, `luck_linter`, `luck_bundler`, or `luck_config`.
- `luck_cli` is the only crate that depends on `luck_lsp`, only `luck_cli`, `luck_lsp`, and the `luck` facade's opt-in `config` feature depend on `luck_config`, and nothing depends on `luck_cli` or on the `luck` facade.
- Only `luck_token`'s `code_buffer.rs`, `luck_resolver`'s `source_io.rs`, and `luck_benchmark` may use `unsafe`, so the workspace denies `unsafe_code` rather than forbidding it.
- A dialect's facts live in `LuaVersion` and `StdlibEnvironment` in `luck_token`. `LuaTarget` in `luck_core` is the dialect a user names, and it projects onto those two once, at each entry boundary. Codegen-side crates, the minifier included, read only `LuaVersion`, so `-t roblox` and `-t luau` minify identically.
- A version's syntax reaches its consumers as a `LuaVersion::has_<feature>` predicate, never as a comparison of `LuaVersion` variants. The only `is_` forms are `is_luau` and `is_roblox`.
- Adding a Lua version is a `LuaVersion` variant, a `LuaTarget` variant, and an arm in every `has_<feature>` predicate. Each of those predicates is exhaustive so a new version has to answer it.
- Modules are private, and each crate re-exports its public items from `lib.rs`, so every item has exactly one path. No `pub mod`. `lib.rs` holds the crate doc, the crate attributes, the module list, and the re-exports, and no other code.
- Types that two crates share when neither depends on the other live in `luck_token` or `luck_core`, such as `LuaVersion`, which the parser and the formatter both read, or `Diagnostic`, which the bundler builds and the CLI renders.
- The version is set in `[workspace.package]` in the root `Cargo.toml` and in `editors/vscode/package.json`, which must match. `.agents/skills/bump-versions/SKILL.md` changes both.

### Types and errors

- A transform or visitor matches every variant with no wildcard arm, so a new variant makes the compiler point at every consumer. The same goes for a helper that classifies its own enum, such as `category()` or a `has_<feature>` predicate: list every variant, so a new one has to decide its answer. Where only one variant matters, use `if let` or `matches!` rather than a `match` with a wildcard.
- An error reaches the user as a `Diagnostic` built with a numbered constructor such as `Diagnostic::e008`, never with an inline code. A lex, parse, or format error is a `SourceError`, and a parse failure is always E008.
- Identifiers stay per-token `CompactString`s. No global string interner, because its lock would serialize rayon's workers.
- Every spawned task has an owner that can stop it. That's a `TaskTracker` with a `CancellationToken`, a `JoinSet`, or a `JoinHandle` someone holds. No detached `tokio::spawn`.

### Imports

- Group them by hand in three blocks: `std`, `core`, and `alloc`, then external crates with `luck_*` among them, then `crate` and `super`.
- Write one `use luck_x::{A, B};` per crate per scope.
- rustfmt's grouping options are nightly-only, so keep them out of `rustfmt.toml`.

### Tests

- White-box tests go inline in a `#[cfg(test)] mod tests` at the bottom of the file they test. Public-API tests go in the crate's `tests/` directory. Never make a `src/tests/` directory.
- Helpers that several files' inline tests share live in `#[cfg(test)] mod fixtures;`, declared in `lib.rs` among the other modules.
- A crate's integration tests are one binary. `tests/it/main.rs` declares a module per file, and shared helpers live in `tests/it/common/mod.rs`. Declare `mod common;`, never `pub mod common;`, and give its items `pub(crate)`. A `pub` module hides a helper nothing calls from the dead-code lint.
- A test that rewrites a committed file is `#[ignore]`d and named `regenerate_*`, so it runs only when asked for with `--ignored`.
- Lua fixtures that several crates' tests read live at the repo root in `tests/fixtures/`, a directory per dialect plus `idiomatic/`, which must stay lint-clean.
- Lint rule tests name positive cases `flags_*` and negative ones `ignores_*`, and run through `crate::fixtures::run_rule`. A count assertion carries a `"{diags:?}"` message. Suppression has its own tests in `luck_linter`, so a rule's tests leave it out.
- A fix test applies the edit and asserts that the result reparses.
- Formatter tests go through `assert_format`, `assert_format_with`, `assert_format_range`, or, for a tree built in code, `assert_format_block`, which check exact output, idempotency, and reparsing on every call.
- An error path asserts on the returned errors, never with `#[should_panic]`.
- Tests need the exact directive syntax. `-- luck: allow(rule_a, rule_b)`, `deny(...)`, or `warn(...)` applies to the next statement, `start` or `end` after it marks a region, and `-- #luck: allow(foo)` applies to the whole file. `-- luck: format off` and `-- luck: format on` turn the formatter off for a region, and `-- luck: ignore`, or its alias `-- luck: format ignore`, skips one statement.

## Extension

- Split a file only past roughly 700 lines, and only along a real seam.
