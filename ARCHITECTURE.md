# Architecture

Core pipeline:

```
luck_token -> luck_lexer -> luck_ast -> luck_parser -> luck_codegen
```

Everything else consumes the parsed AST. `luck_minifier`, `luck_formatter`,
and `luck_linter` transform or check it, with `luck_semantic` supplying scope
analysis to the linter and LSP; `luck_resolver` resolves require paths and
feeds `luck_bundler`. On top sit `luck` (facade re-exports), `luck_lsp`
(language server), and `luck_cli`, which drives both. `luck_testgen` and
`luck_benchmark` are unpublished internal harnesses, one for generated test
programs and one for benches.

## Crates

| Crate | Role | Key entry |
|-------|------|-----------|
| `luck_token` | Spans, `LuaVersion`, `StdlibEnvironment`, `SourceError`, shared literal decode/encode, `CompactString` storage, `CodeBuffer` byte-level output builder | `span.rs`, `version.rs`, `token.rs`, `literal.rs`, `code_buffer.rs` |
| `luck_lexer` | Single-pass tokenizer; comments emitted separately; memchr + byte-table batched scanning | `lexer.rs`, `search.rs` |
| `luck_ast` | `Expression`/`Statement`/`Type` <=64 B, `Visitor`, `AstTransform`, `synth` builder (dummy-span AST construction), `NodeType`/`NodeKind`/`AstTypesBitset` for node-table dispatch | `expr.rs`, `stmt.rs`, `types.rs`, `transform.rs`, `synth.rs`, `node.rs` |
| `luck_parser` | Pratt expressions + recursive-descent statements + full Luau type grammar, version-gated | `expr.rs`, `stmt.rs`, `luau.rs` |
| `luck_codegen` | Compact printer (ambiguity cases live in `separator.rs` + its tests) | `compact.rs`, `separator.rs` |
| `luck_core` | `LuaTarget`, typed config, `TransformConfig`, diagnostic codes, schemars schema, `source_io` (SIMD-validated file reads) | `config.rs`, `diagnostics.rs`, `format_options.rs` |
| `luck_resolver` | Lua search paths, Luau `@aliases`, `.luaurc` chain | `lib.rs`, `luau.rs` |
| `luck_bundler` | Scope-aware require extraction (via `luck_semantic`), dep graph with cycle detection, version-exact lazy loader emit + line maps, collision-proof `__luck` prefix, runtime-resolved `require(expr)` kept via the loader's dynamic entry point (`dynamic_require`) | `graph.rs`, `emitter.rs`, `module.rs` |
| `luck_minifier` | AST transform pipeline; passes individually gated by `TransformConfig` flags | `lib.rs` `minify()`, `transforms/` |
| `luck_formatter` | Wadler-style engine: `Format` trait + combinator IR, AST-in `format_block` formats synthetic ASTs (no source needed), idempotency invariant | `ir.rs`, `printer.rs`, `format_*.rs`, `comments.rs` |
| `luck_linter` | `Rule`/`NodeRule` traits + `LintContext`, stateless rules in a static `RULES` registry, node-type-bucketed single-pass bus, suppressions, `--fix` | `rules/`, `rule.rs`, `bus.rs` |
| `luck_semantic` | Scope tree, refs (R/W/RW), upvalues; typed `NonZeroU32` ids; flat node table with parent links; per-environment stdlib catalog TOMLs (5.1-5.5, luau, luau_roblox) with overloads, shapes, deprecations; generated Roblox API data | `builder.rs`, `stdlib_model.rs`, `resolve.rs` |
| `luck_lsp` | Library-only LSP backend (no binary); served via `luck lsp` | `backend.rs`, `serve.rs`, `providers/` |
| `luck_cli` | Flat Clap commands, one module per command; rayon-parallel lint/fmt/check; ariadne rendering; `ExitCode` 0/1/2; 16 MB-stack worker thread | `args.rs`, `commands/`, `render.rs` |
| `luck` | Facade re-exports (no logic); publishes as package `luck-lua` with `[lib] name = "luck"`, so imports stay `luck::` | `lib.rs` |
| `luck_testgen` | Internal (`publish = false`): deterministic program generators (runtime-safe and full-grammar/parse-only) + round-trip property tests | `src/lib.rs`, `src/full.rs` |
| `luck_benchmark` | Internal (`publish = false`): per-stage criterion benches run on CodSpeed; corpus cached in gitignored `corpus/`; committed `minsize.snap` size tracking | `benches/`, `src/corpus.rs`, `tests/metrics.rs` |

## The three target axes

Never conflated:

- `LuaVersion` (luck_token): syntax only, covering 5.1-5.5 and Luau. Parser,
  codegen, formatter, and minifier key off this.
- `StdlibEnvironment` (luck_token): `Standalone` vs `Roblox`. Only meaningful
  for Luau; semantic, linter, and LSP filter on it.
- `LuaTarget` (luck_core): the user-facing dialect (7 variants incl.
  `LuauRoblox`). Projects to the two axes via `lua_version()` and
  `stdlib_environment()`.

The split happens once at each entry boundary; downstream never re-derives
it. Codegen-side crates take `LuaVersion` only. Roblox and standalone share
syntax, so `-t roblox` and `-t luau` minify identically. That is correct, not
a bug.

## Configuration

One typed source of truth: `luck.json`, discovered by walking up from cwd
for project/path commands, or from the input directory for one-shot
`minify`/`bundle` commands (cwd for stdin). `-c/--config` overrides
discovery; explicit transform flags layer over configured transforms, not
fresh defaults. All config types live in `luck_core` and deserialize with
`deny_unknown_fields`, so unknown keys and invalid enum values are hard
errors. Targets are per-extension via the `lua`/`luau` keys, and either key
may name any dialect. Extension and dialect are independent, so a Roblox or
Rojo tree that keeps Luau in `.lua` files sets `"lua": "roblox"`.
`extends`/`include`/`exclude`/`root` shape the project. A bool flag on
`TransformConfig` gates each minifier pass. Schemars generates the VS Code
schema (`editors/vscode/schemas/luckrc.schema.json`) from the Rust types; a
drift test fails if it is stale. Format-option precedence: defaults <
`.editorconfig` < `luck.json` `format`.

## Diagnostics

One scheme: codes live in `luck_core::diagnostics::errors`; consumers build
them with the `Span`-accepting `error_at`/`warning_at` constructors, never
inline literal codes. Parse failures are always E008. Lint diagnostics render
with the rule name as the code; the driver stamps category and severity from
the rule's `category()` and the resolved severity. The CLI exits 0 (success),
1 (problems found), or 2 (usage/config error).

## Design decisions

- **Hand-written lexer/parser/codegen.** No external parser dependency, and
  version gating and error recovery are easier to control by hand.
- **`Span` is `u32`, not `usize`.** Halves AST bytes, at the price of a 4 GB
  file cap nobody hits.
- **Enum size budget <=64 bytes** for `Expression`, `Statement`, and `Type`
  (boxed large variants), enforced by `luck_ast` tests.
- **Comments live outside the AST** in a sorted `Vec<Comment>` with
  `Leading`/`Trailing` classification and `attached_to`; the formatter
  additionally accepts node-anchored `SyntheticComment`s for generated ASTs.
- **Emitters never read source text.** Codegen and formatter leaf text comes
  from token-carried values. They consult source only for trivia fidelity and
  verbatim regions, and every such path degrades gracefully when source is
  absent. That is what makes synthetic ASTs printable.
- **Luau types are a real AST** (`luck_ast::types::Type`), not opaque type
  text, so every consumer gets structure for free.
- **One error type: `SourceError`.** `LexError`, `ParseError`, `FormatError`
  are aliases, keeping one rendering path for all diagnostics.
- **Purity analysis assumes metamethods.** Variable reads, indexing,
  arithmetic, comparison, and concat may all invoke metamethods; only literal
  arithmetic is pure, and `#"str"` is never folded (escape sequences make raw
  length unreliable).
- **Parser depth is a host resource budget, not a VM limit.** The 256-entry
  recursion cap is independent of dialect; target compilers have their own
  configurable limits. Failed depth entries do not change the active count.
- **Interior comments must not relocate.** List-shaped emitters (table
  constructors, call argument lists) claim the comments written between their
  items and break one item per line to host them; a statement header emits a
  single-line block comment stalled before its `then`/`do` inline. Each claims
  only what its own span covers, so none drags a comment across its opening
  delimiter. Anything unplaceable falls back to the smallest enclosing sourced
  statement, re-indented to its block but otherwise emitted as written, and
  never split at a newline inside a long string or long comment, where the
  newline is content. A comment the emitters moved crosses nothing but
  whitespace and the separators the formatter writes at its own discretion
  (`;`, `,`). Verified formatting aligns the two token streams, skipping the
  parens only one side has, then checks every comment's text and its position
  among the tokens both sides keep, alongside AST equivalence and idempotency.
- **Idempotency and re-parseability are tested guarantees.**
  `minify(minify(x)) == minify(x)`, `format(format(x)) == format(x)`, and
  both outputs parse with zero errors.
- **No global string interner.** Identifiers stay per-token `CompactString`;
  a shared interner's lock serializes rayon workers and has halved CPU
  utilization in comparable tools.
