# Architecture

Core pipeline:

```
luck_token -> luck_lexer -> luck_ast -> luck_parser -> luck_codegen
```

Everything else consumes the parsed AST. `luck_minifier`, `luck_formatter`, and `luck_linter` transform or check it, and `luck_semantic` supplies scope analysis to the linter, the LSP, and the bundler. `luck_resolver` resolves require paths for `luck_bundler` and the LSP's document links. On top sit `luck` (facade re-exports), `luck_lsp` (language server), and `luck_cli`, which drives the stage crates directly and serves `luck_lsp`. `luck_testgen` and `luck_benchmark` are unpublished internal harnesses, one for generated test programs and one for benches.

## Crates

| Crate | Role | Key entry |
|-------|------|-----------|
| `luck_token` | Spans, `LuaVersion`, `StdlibEnvironment`, `SourceError`, shared literal decode/encode, `CompactString` storage, `CodeBuffer` byte-level output builder | `span.rs`, `version.rs`, `token.rs`, `op.rs`, `number.rs`, `string.rs`, `code_buffer.rs` |
| `luck_lexer` | Single-pass tokenizer, comments emitted separately, memchr + byte-table batched scanning | `lexer.rs`, `search.rs` |
| `luck_ast` | `Expression`/`Statement`/`Type` <=64 B, `Visitor`, `Transform`, `Synth` builder (source-free AST construction) | `expr.rs`, `stmt.rs`, `types.rs`, `transform.rs`, `synth.rs` |
| `luck_parser` | Pratt expressions + recursive-descent statements + full Luau type grammar, version-gated | `parser.rs`, `parser/expr.rs`, `parser/stmt.rs`, `parser/types.rs` |
| `luck_codegen` | Compact printer (ambiguity cases live in `separator.rs` + its tests) | `compact.rs`, `separator.rs` |
| `luck_core` | `LuaTarget`, typed config, `TransformConfig`, diagnostic codes, schemars schema, `read_source_file` (SIMD-validated file reads), `normalize_path`/`native_path` (the forward-slash path form modules are keyed on) | `config.rs`, `diagnostic/codes.rs`, `lua_target.rs` |
| `luck_resolver` | Lua search paths, Luau `@aliases`, `.luaurc` chain | `resolver.rs`, `lua.rs`, `luau.rs`, `luaurc.rs` |
| `luck_bundler` | Scope-aware require extraction (via `luck_semantic`), dep graph with cycle detection, version-exact lazy loader emit + line maps, collision-proof `__luck` prefix, runtime-resolved `require(expr)` kept via the loader's dynamic entry point (`dynamic_require`) | `require_extraction.rs`, `graph.rs`, `emitter.rs`, `loader.rs`, `module.rs` |
| `luck_minifier` | AST transform pipeline, with `TransformConfig` flags turning passes on and off | `pipeline.rs` `minify()`, `transforms/` |
| `luck_formatter` | Wadler-style engine: `Format` trait + combinator IR, AST-in `format_block` formats synthetic ASTs (no source needed), idempotency invariant | `ir.rs`, `printer.rs`, `format_*.rs`, `comments.rs` |
| `luck_linter` | `Rule` identity trait, `WholeRule`/`NodeRule` checking traits + `LintContext`, stateless rules in a static `RULES` registry reporting `Finding`s the driver turns into `LintDiagnostic`s, `NodeType`-bucketed single-pass bus over a flat pre-order node table, suppressions, `--fix` | `rules/`, `rule.rs`, `bus.rs`, `nodes.rs`, `node_type.rs` |
| `luck_semantic` | Scope tree, refs (R/W/RW), upvalues, typed `NonZeroU32` ids, per-environment stdlib catalog TOMLs (5.1-5.5, luau, luau_roblox) with overloads, shapes, and deprecations, and generated Roblox API data | `analysis.rs`, `builder.rs`, `stdlib_model.rs`, `resolve.rs` |
| `luck_lsp` | Library-only LSP backend (no binary), served by `luck lsp` | `backend.rs`, `transport.rs`, `providers/` |
| `luck_cli` | Binary only: flat Clap commands, one module per command, rayon-parallel lint/fmt/check, ariadne rendering, exit codes 0/1/2, 16 MB-stack worker thread | `main.rs`, `cli.rs`, `commands/`, `render.rs` |
| `luck` | Facade re-exports (no logic), published as package `luck_lua` with `[lib] name = "luck"`, so imports stay `luck::` | `lib.rs` |
| `luck_testgen` | Internal (`publish = false`): deterministic program generators (runtime-safe and full-grammar/parse-only), round-trip property tests, and the shared fixture walker | `src/runtime.rs`, `src/full.rs`, `src/fixtures.rs`, `tests/it/roundtrip.rs` |
| `luck_benchmark` | Internal (`publish = false`): per-stage criterion benches run on CodSpeed, corpus cached in gitignored `corpus/`, committed `minsize.snap` size tracking | `benches/`, `src/corpus.rs`, `tests/it/metrics.rs` |

## The three target axes

Never conflated:

- `LuaVersion` (luck_token): syntax only, covering 5.1-5.5 and Luau. Parser, codegen, formatter, and minifier key off this.
- `StdlibEnvironment` (luck_token): `Standalone` vs `Roblox`. Only meaningful for Luau. Semantic, linter, and LSP filter on it.
- `LuaTarget` (luck_core): the user-facing dialect (7 variants incl. `LuauRoblox`). Projects to the two axes via `lua_version()` and `stdlib_environment()`.

The split happens once at each entry boundary. Codegen-side crates, the minifier included, read only the syntax axis, and Roblox and standalone Luau share syntax, so `-t roblox` and `-t luau` minify identically. That's correct, not a bug.

## Configuration

One typed source of truth: `luck.json`, discovered by walking up from cwd for project/path commands, from the input directory for one-shot `minify`/`bundle`/`graph` commands (cwd for stdin), and from each open document's directory in the LSP. `-c/--config` overrides discovery, and explicit transform flags layer over the configured transforms, not fresh defaults. All config types live in `luck_core` and deserialize with `deny_unknown_fields`, so unknown keys and invalid enum values are hard errors. Targets are per-extension via the `lua`/`luau` keys, and either key may name any dialect. `DialectKey` picks the key for a path and holds its default, so the CLI and the LSP choose a file's dialect the same way. Extension and dialect are independent, so a Roblox or Rojo tree that keeps Luau in `.lua` files sets `"lua": "roblox"`. `extends`/`include`/`exclude`/`root` shape the project. Bool flags on `TransformConfig` turn minifier passes on and off. Schemars generates the VS Code schema (`editors/vscode/schemas/luckrc.schema.json`) from the Rust types, and a drift test fails if it's stale. Format-option precedence: defaults < `.editorconfig` < `luck.json` `format`, layered for both the CLI and the LSP by `resolved_format_config`.

## Diagnostics

One scheme: codes live in `luck_core`'s `diagnostic/codes.rs` as numbered `Diagnostic` constructors (`Diagnostic::e008`), the only way to build a coded diagnostic, so consumers never inline literal codes. Parse failures are always E008. Lint diagnostics render with the rule name as the code. Rules report bare findings, and the driver stamps each with the rule's name, its `category()`, and the resolved severity. The CLI exits 0 (success), 1 (problems found), or 2 (usage/config error).

## Design decisions

- **Hand-written lexer/parser/codegen.** No external parser dependency, and version gating and error recovery are easier to control by hand.
- **`Span` is `u32`, not `usize`.** Halves the size of every span, at the price of a 4 GB file cap nobody hits.
- **Enum size budget <=64 bytes** for `Expression`, `Statement`, and `Type` (boxed large variants), enforced by `luck_ast` tests.
- **Comments live outside the AST** in a sorted `Vec<Comment>` with `Leading`/`Trailing` classification and `attached_to`. The formatter also accepts node-anchored `SyntheticComment`s for generated ASTs.
- **Emitters never read source text.** Codegen and formatter leaf text comes from token-carried values. They consult source only for trivia fidelity and verbatim regions, and every such path degrades gracefully when source is absent, so synthetic ASTs stay printable.
- **Luau types are a real AST** (`luck_ast::Type`), not opaque type text, so every consumer gets structure for free.
- **One error type: `SourceError`.** `LexError`, `ParseError`, `FormatError` are aliases, keeping one rendering path for all diagnostics.
- **Purity analysis assumes metamethods.** Variable reads, indexing, arithmetic, comparison, and concat may all invoke metamethods. Only literal arithmetic is pure, and `#"str"` is never folded (escape sequences make raw length unreliable).
- **Parser depth is a host resource budget, not a VM limit.** The 256-level nesting cap is the same for every dialect, and target compilers enforce their own limits. An entry refused at the cap leaves the depth count unchanged.
- **Interior comments must not relocate.** List-shaped emitters (table constructors, call argument lists) claim the comments written between their items and break one item per line to hold them. A statement header keeps a single-line block comment that sits before its `then` or `do` inline (`if cond --[[ why ]] then`). Each emitter claims only what its own span covers, so none drags a comment across its opening delimiter. Anything unplaceable falls back to the smallest enclosing sourced statement, which is re-indented to its block but otherwise emitted as written, and never split at a newline inside a long string or long comment, where the newline is content. A moved comment crosses nothing but whitespace and the separators the formatter writes at its own discretion (`;`, `,`). `luck fmt --verify` checks AST equivalence, idempotency, and each comment's text and position among the tokens both sides keep. It aligns the two token streams, skipping parens only one side has, up to the first other mismatch, such as a leading `|` the formatter adds to or drops from a Luau union. Past that point, comments are checked by text and order only.
- **Idempotency and re-parseability are tested guarantees.** `minify(minify(x)) == minify(x)`, `format(format(x)) == format(x)`, and both outputs parse with zero errors.
- **No global string interner.** Identifiers stay per-token `CompactString`. A shared interner's lock serializes rayon workers and has halved CPU utilization in comparable tools.
