# Architecture

luck is a workspace of small crates around one syntax tree. This file covers the crates and their dependencies, the path a source file takes through them, and the rules every crate follows. Each crate's README and docs cover how it works inside.

## Crates

| Crate | Role | Depends on |
| --- | --- | --- |
| `luck_allocator` | The bump arena trees live in | |
| `luck_token` | Spans, tokens, `LuaVersion`, `SourceError`, and the literal and line-break rules every stage shares | |
| `luck_core` | `LuaTarget`, the dialect a user names, and `Diagnostic` | token |
| `luck_lexer` | Source to tokens and comments | token |
| `luck_ast_macros` | The `Ast` derive, which marks a type as part of the tree and generates nothing | |
| `luck_ast` | The tree, the support code generated from it, `Synth`, and the nesting cap | allocator, token, ast_macros |
| `luck_parser` | Source to a tree, through the lexer | allocator, token, lexer, ast |
| `luck_codegen` | A tree to the shortest source that parses back to it | token, ast |
| `luck_formatter` | A tree to formatted source | allocator, token, lexer, ast, parser |
| `luck_minifier` | Rewrites a tree to be small, then prints it | allocator, token, ast, parser, codegen |
| `luck_semantic` | Scopes, symbols, references, and the stdlib catalogs | token, ast |
| `luck_linter` | Rules over a tree and its analysis, with fixes | allocator, token, core, ast, parser, semantic |
| `luck_resolver` | `require` strings to files, and the `FileSystem` files are read through | token |
| `luck_bundler` | A project to one file | allocator, token, core, lexer, ast, parser, minifier, semantic, resolver |
| `luck_config` | A `luck.json` to one validated project, which gives each file its dialect and options | token, core, formatter, minifier, linter, resolver, bundler |
| `luck_lsp` | The language server | allocator, token, core, ast, parser, formatter, semantic, linter, resolver, config |
| `luck_cli` | The `luck` binary | allocator, token, core, ast, parser, formatter, minifier, linter, resolver, bundler, config, lsp |
| `luck` | The facade, published as `luck_lua`, which re-exports each stage behind a feature | every crate above but lsp and cli |
| `luck_ast_tools` | Unpublished: writes `luck_ast`'s visitors, node kinds, span accessors, content equality, builder, and layout assertions from the types that derive `Ast` | |
| `luck_testgen` | Unpublished: generated programs, stack-test inputs, and a round-trip tree comparison for other crates' tests | token, ast |
| `luck_benchmark_files` | Unpublished: the pinned files and projects the benches and allocation snapshots measure | allocator, token, core, parser |
| `luck_benchmark` | Unpublished: a bench per stage | |
| `luck_allocs` | Unpublished: committed allocation counts for each stage, behind `luck_allocator`'s `track_allocations` feature | allocator, lexer, ast, parser, codegen, formatter, minifier, semantic, linter, benchmark_files |
| `luck_conformance` | Unpublished: each dialect's official test suite through every stage and the reference interpreter, with committed results | allocator, token, core, ast, parser, codegen, formatter, minifier, linter, testgen |

"Depends on" leaves out dev-dependencies and the `luck_` prefix. `luck_codegen`, `luck_formatter`, and `luck_minifier` depend on nothing that analyzes, resolves, bundles, or configures. No tool depends on `luck_config`, which depends on them. The VS Code extension in `editors/vscode` holds no language logic. It starts `luck lsp`, and everything that has to understand Lua runs in `luck_lsp`.

## The pipeline

1. The caller owns an arena and the source text, and both outlive the tree.
2. `luck_parser` pulls tokens from `luck_lexer` one at a time and builds a `Block` in the arena. A token holds no text, and the lexer keeps comments in a separate list. The tree's leaves borrow their text from the source, and the parser gives each name a `NameIndex` in reading order. The `ParseResult` holds the tree, the comments, every error, and the `TreeCounts` that size the analysis tables.
3. A stage takes the tree. `luck_codegen` prints it compactly, `luck_minifier` rewrites it in place and prints it with `luck_codegen`, and `luck_formatter` lays it out with its comments. `luck_semantic` resolves its names in one walk and matches globals against a stdlib catalog. `luck_linter` builds its own table of the tree's nodes during that walk and runs its rules over both.
4. `luck_bundler` parses every module an entry reaches, on one thread per arena. It finds their requires with `luck_semantic`, resolves them with `luck_resolver`, and writes one file whose lazy loader works like the target's own `require`. For a minified bundle, it builds one tree from the modules' trees and minifies that, so no text is parsed twice.
5. `luck_cli` and `luck_lsp` drive the stages. They load each `luck.json` once into a `luck_config::Project`, ask it for each file's dialect, lint settings, format options, and module resolution, and own the arenas.

## Arenas

Every tree lives in a `luck_allocator::Allocator` the caller owns, and every entry point that parses or builds nodes takes it as its first argument. Nothing in an arena is dropped on its own. A tree is freed all at once when its arena is reset or dropped, and `reset` keeps the memory for the next file. An arena serves one thread at a time, but a finished tree is `Send` and `Sync`, can hold nodes from several arenas that outlive it, and can be read from any thread. That rules out per-node interior mutability, so a name's `NameIndex` is a plain field. The parser writes it, and a pass that holds the tree mutably, such as the minifier, rewrites it.

Three crates manage arenas:

- `luck_cli` runs `lint`, `fmt`, and `check` on rayon's pool. Each file gets an arena from an `AllocatorPool`, and the CLI resets it as soon as the file's work is done.
- `luck_bundler` takes a slice of arenas, one per discovery thread, the caller's thread included. Each module's source and tree stay in the arena of the thread that read it, and the result borrows none of them.
- `luck_lsp` copies each version of an open document into an arena of its own and shares the parse and analysis there read-only across threads. Once nothing holds a replaced version, the server empties its arena and reuses it.

## Dialects

Three types describe a dialect:

- `LuaVersion`, in `luck_token`, is the syntax: Lua 5.1 to 5.5 or Luau. Code checks each syntax difference with a `has_<feature>` predicate.
- `StdlibEnvironment`, in `luck_token`, is the standard library, standalone or Roblox, which only Luau tells apart.
- `LuaTarget`, in `luck_core`, is the dialect a user names, one of seven, and converts to the other two with `lua_version()` and `stdlib_environment()`.

Each entry point converts a `LuaTarget` once. The parser, codegen, formatter, and minifier read only `LuaVersion`, so `-t roblox` and `-t luau` minify and format identically. Semantic analysis, the linter, and the language server read the environment too.

## Comments live outside the tree

The tree holds no comments. The lexer puts each in a sorted list beside the tokens, as `Leading` or `Trailing`, with the start of the token it attaches to, and whatever needs comments reads that list:

- The formatter places each comment at the nearest node boundary, as `oxc_formatter` does. A node takes the comments before it as leading ones and those after it on its line as trailing ones, a container takes those before its closer, and a block those between its statements. A comment that ends its line in the source ends it in the output, so the next format reads it where this one put it. Only `format off` regions and statements under `ignore` go out as written. A tree built in code carries `SyntheticComment`s, anchored to its statements' and blocks' spans, instead.
- The linter reads its suppression directives from the list, and the formatter its `format off`, `format on`, and `ignore` directives.
- Codegen prints no comments, and the minifier keeps only a shebang and Luau's `--!` hot comments, which it writes above the code.

So no rewrite has to carry comments along.

## The nesting cap and the stack

Every traversal recurses on the thread's stack, once per level of nesting, so luck caps the depth of a tree rather than rewrite every walk as a loop. `luck_ast::MAX_NESTING_DEPTH` is 2,001 levels in every dialect, counted the way `Visit::descend` counts them. That is enough to read any construct nested inside itself as deep as the reference Lua and Luau parsers do. A run of suffixes, or of binary operators at one precedence, is one node and one level however long it gets, so a long `a.b.c` or `a + b + c` never nears the cap.

- The parser refuses the node that would pass the cap with one error that ends the parse, so every tree it returns, with errors or without, fits.
- `minify_block` and `format_block` return an error for a deeper tree. A minifier rewrite that nests an expression deeper checks that it still fits, and the bundler reports E013 where a module's wrapper would push it past the cap. `compact` takes only trees its callers keep within the cap. So no stage writes output the parser would refuse.
- `luck_ast::MIN_STACK_BYTES`, 16 MiB, is the stack any stage needs on a tree at the cap. The CLI runs each command on a thread of that size and gives rayon's pool and the language server's tokio threads the same. The bundler starts its discovery threads with it, and code that embeds luck gives it to its own threads. A stack is reserved address space, and only the pages a thread touches take memory.

## Output guarantees

- `minify(minify(x)) == minify(x)` and `format(format(x)) == format(x)`, and both outputs parse with no errors. Tests check both.
- A formatted or minified program ends as the original does. `luck_conformance` runs the official Lua and Luau suites both ways in the reference interpreters, and lists each test that reads what a printer changes, such as its own line numbers.
- Purity analysis assumes metamethods. A variable read, an index, arithmetic, a comparison, or a concatenation can each call one, so only literal arithmetic is pure. `#"str"` is never folded, because escape sequences make its raw length unreliable.
- Constant folding reproduces the target's float arithmetic exactly, so it never fuses or reorders an operation.
- Emitters never read source text. Codegen and the formatter read it only for trivia and verbatim regions, and work without it, so a tree built in code still prints.
- A bundle names no host path. Every path it carries is relative to the project root.

## Design decisions

- **Hand-written lexer, parser, and printers.** luck has no parser dependency, so it controls version gating and error recovery itself.
- **`Span` is `u32`.** That halves every span and caps a source at 4 GiB, which the lexer reports as one error.
- **A token holds no text, and there is no global string interner.** `Token::text` slices the source, and an interner's lock would serialize rayon's workers.
- **Leaves hold their text as written, and the printers print it unchecked.** The parser builds only leaves the lexer read, and luck's own rewrites build none the lexer would read differently, so a check would only cost time. Code that builds a tree is responsible for its leaves.
- **Luau types are a tree.** `luck_ast::Type` holds them as nodes, not text, so every consumer gets their structure.
- **The tree's support code is generated from its types, as oxc's is.** `luck_ast_tools` reads the types that derive `Ast` and writes `Visit` and `VisitMut`, with a hook for every type in source order, `AstKind`, `GetSpan`, `ContentEq`, the builder, and a test that pins every type's size, so a new node or field is one edit and `just ast`. The levels the nesting cap counts are marked on the types with `#[visit(level)]`. Every tree is built through the generated constructors, by the parser, by the minifier's and bundler's rewrites, and by `Synth`, which adds only policy on top. Code that compares code compares trees with `ContentEq`, which leaves out spans, rather than source text.
- **A fix has a kind, and only a fix that keeps what the code does runs unasked.** `--fix` and the editor's fix-all apply `Fix`es, round after round until none is left. A `Suggestion`, such as renaming an unused variable to `_x`, is an editor quick fix, or `--fix-suggestions` applies it. The linter builds only the kinds its host applies, and drops any fix whose edits would delete a comment.
- **Errors are data, and one diagnostic scheme reports them.** A lex, parse, or format error is a `SourceError`, and a `require` that doesn't resolve gets a `ResolveError`. An error that reaches a user is a `Diagnostic` from a numbered constructor in `luck_core`, such as `Diagnostic::e008` for a parse failure, and a lint diagnostic is named after its rule. Only the caller that reports an error builds its diagnostic. The CLI exits 0 on success, 1 when it found problems, and 2 on a usage or config error.
- **The core crates make no OS calls.** `luck_token`, `luck_lexer`, `luck_ast`, `luck_parser`, `luck_codegen`, `luck_core`, `luck_formatter`, and `luck_minifier` take text or a tree and return text, a tree, or errors. Reading files and resolving paths start at `luck_resolver`, whose resolver reads through a `FileSystem` trait, so a caller such as the language server can resolve against files it holds in memory.
- **Paths stay paths until they're written out.** Inside luck a path is a `Path`, canonical where it names a module, so a name that isn't Unicode survives. It becomes text only in output, in the forward-slash form `luck_resolver::to_slash_lossy` gives, and a bundle carries only project-relative paths.
- **One validated project per `luck.json`.** A file's settings come from the nearest `luck.json` alone, which takes nothing from a config above it unless it `extends` that config. It loads once, through its `extends` chain, into a `luck_config::Project`, which every command and the language server read, so `build`, `bundle`, `graph`, `lint`, and the editor resolve modules, pick dialects, and run rules the same way. Everything is checked at load, and a problem is one E014 diagnostic placed where it is in the file at fault. The CLI exits 2 on it, and the language server shows it, publishes it on that file, and uses the defaults until it's fixed. Command-line flags layer over the project, and win over the config.
- **Each tool defines its own config types.** A tool's option types derive their `luck.json` form behind its `serde` feature, which `luck_config` turns on, and check their own values as they deserialize, so the linter rejects a rule name it doesn't have. The extension's schema is generated from these types.
- **Per-file settings layer.** A file's lint settings are the `lint` section, then the `overrides` its path matches, in order, as oxlint's are. Its format options are the defaults, then `.editorconfig`, then the `format` section, then the matching `overrides`.
- **A file's dialect comes from its extension and the config.** A `.luau` file takes the `luau` key and any other file the `lua` key, and either key can name any dialect, so a Roblox project that keeps Luau in `.lua` files sets `"lua": "roblox"`. The CLI and the language server both choose through `Project::target_for`.
