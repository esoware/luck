# Architecture

luck is a workspace of small crates around one syntax tree. This file is the map: the crates and how they depend on each other, the path a source file takes, and the ideas every crate relies on. How each crate works inside is in its README and its docs.

## Crates

| Crate | Role | Depends on |
| --- | --- | --- |
| `luck_allocator` | The bump arena trees live in | |
| `luck_token` | Spans, tokens, `LuaVersion`, `SourceError`, and the literal and line-break rules every stage shares | |
| `luck_core` | `LuaTarget`, the dialect a user names, and `Diagnostic` | token |
| `luck_lexer` | Source to tokens and comments | token |
| `luck_ast` | The tree, its visitors, `Synth`, and the nesting cap | allocator, token |
| `luck_parser` | Source to a tree, through the lexer | allocator, token, lexer, ast |
| `luck_codegen` | A tree to the shortest source that reads back as it | token, ast |
| `luck_formatter` | A tree to formatted source | allocator, token, lexer, ast, parser |
| `luck_minifier` | Rewrites a tree to be small, then prints it | allocator, token, ast, parser, codegen |
| `luck_semantic` | Scopes, symbols, references, and the stdlib catalogs | token, ast |
| `luck_linter` | Rules over a tree and its analysis, with fixes | allocator, token, core, ast, parser, semantic |
| `luck_resolver` | `require` strings to files, and reading them | token, core |
| `luck_bundler` | A project to one file | allocator, token, core, lexer, ast, parser, minifier, semantic, resolver |
| `luck_config` | `luck.json` and `.editorconfig` to each tool's options | core, formatter, minifier, linter, bundler |
| `luck_lsp` | The language server | allocator, token, core, ast, parser, formatter, semantic, linter, resolver, config |
| `luck_cli` | The `luck` binary | allocator, token, core, ast, parser, formatter, minifier, linter, resolver, bundler, config, lsp |
| `luck` | The facade, published as `luck_lua`, with each stage behind a feature | every crate above but lsp and cli |
| `luck_testgen` | Unpublished: generated programs and stack-test inputs for other crates' tests | token, ast |
| `luck_benchmark` | Unpublished: a bench per stage | allocator, token, core, parser |

"Depends on" lists normal dependencies, without the `luck_` prefix. `luck_codegen`, `luck_formatter`, and `luck_minifier` depend on nothing that analyzes, resolves, bundles, or configures. No tool depends on `luck_config`, which depends on them. The VS Code extension in `editors/vscode` holds no language logic: it starts `luck lsp`, and everything that has to understand Lua runs in `luck_lsp`.

## The pipeline

1. The caller owns an arena and the source text, and both outlive the tree.
2. `luck_parser` pulls tokens from `luck_lexer` one at a time and builds a `Block` in the arena. A token holds no text, and the lexer sets comments aside in a list of their own. The tree's leaves borrow their text from the source, and each name gets a `NameIndex` in reading order. The `ParseResult` holds the tree, the comments, every error, and the `TreeCounts` that size the analysis tables.
3. A stage takes the tree. `luck_codegen` prints it compactly, `luck_minifier` rewrites it in place and prints it with `luck_codegen`, and `luck_formatter` lays it out with its comments. `luck_semantic` walks it once to resolve its names, read against a stdlib catalog, and `luck_linter` builds its own table of the tree's nodes in that walk and runs its rules over both.
4. `luck_bundler` parses every module an entry reaches, on one thread per arena, finds their requires with `luck_semantic`, resolves them with `luck_resolver`, and writes one file whose lazy loader mirrors the target's own `require`. A minified bundle goes from the modules' trees straight to `luck_minifier`.
5. `luck_cli` and `luck_lsp` are the hosts. They read `luck.json` through `luck_config`, pick each file's dialect, own the arenas, and drive the stages.

## Arenas

Every tree lives in a `luck_allocator::Allocator` the caller owns, and every entry point that parses or builds nodes takes it as its first argument. Nothing in an arena is dropped on its own: a tree is freed all at once when its arena is reset or dropped, and `reset` keeps the memory for the next file. An arena serves one thread at a time, but a finished tree is `Send` and `Sync`, can hold nodes from several arenas that outlive it, and is read from any thread. That rules out per-node interior mutability, so a name's `NameIndex` is a plain field, which the parser writes and a pass that holds the tree mutably, as the minifier does, rewrites.

The hosts own the arenas:

- `luck_cli` runs `lint`, `fmt`, and `check` on rayon's pool, each file in an arena from an `AllocatorPool`, reset as soon as the file's work is done.
- `luck_bundler` takes a slice of arenas, one per discovery thread, the caller's thread included. Each module's source and tree stay in the arena of the thread that read it, and the result borrows none of them.
- `luck_lsp` copies each version of an open document into an arena of its own, reusing the emptied arena of the version it replaced once nothing holds that, and shares the parse and analysis there read-only across threads.

## The three target axes

- `LuaVersion`, in `luck_token`, is the syntax: Lua 5.1 to 5.5 or Luau. Each syntax difference reaches its consumers as a `has_<feature>` predicate.
- `StdlibEnvironment`, in `luck_token`, is the standard library, standalone or Roblox, which only Luau tells apart.
- `LuaTarget`, in `luck_core`, is the dialect a user names, one of seven, and projects onto the other two with `lua_version()` and `stdlib_environment()`.

The projection happens once, at each entry boundary. The parser, codegen, formatter, and minifier read only `LuaVersion`, so `-t roblox` and `-t luau` minify and format identically. Semantic analysis, the linter, and the language server read the environment too.

## Comments live outside the tree

The tree holds no comments. The lexer puts each in a sorted list beside the tokens, as `Leading` or `Trailing`, with the start of the token it attaches to, and whoever needs comments reads that list:

- The formatter places each comment by byte position and keeps it between the same tokens, apart from separators such as `;`. A statement holding a comment it can't place that way is copied as written. A tree built in code carries `SyntheticComment`s, anchored to its nodes' spans, instead.
- The linter reads its suppression directives there, and the formatter its `format off`, `format on`, and `ignore` directives.
- Codegen prints no comments, and the minifier keeps only a shebang and Luau's `--!` hot comments, which it writes above the code.

So no rewrite has to carry comments along.

## The nesting cap and the stack

Every traversal recurses on the plain stack, once per level of nesting, so luck caps the depth of a tree rather than rewrite every walk as a loop. `luck_ast::MAX_NESTING_DEPTH` is 2,001 levels in every dialect, counted as `Visitor::descend` counts them, which reads any construct nested on itself as deep as the reference Lua and Luau parsers do. A run of suffixes, or of binary operators at one precedence, is one node and one level however long it gets, so a long `a.b.c` or `a + b + c` never nears the cap.

- The parser refuses the node that would pass the cap with one error that ends the parse, so every tree it returns, with errors or without, fits.
- `minify_block` and `format_block` return an error for a deeper tree, a minifier rewrite that nests an expression deeper checks that it still fits, and the bundler reports E013 where a module's wrapper would push it past the cap. `compact` takes only trees its callers keep within it. So no stage writes output the parser would refuse.
- `luck_ast::MIN_STACK_BYTES`, 16 MiB, is the stack any stage needs on a tree at the cap. The CLI runs each command on a thread of that size and builds rayon's pool and the language server's tokio threads with it, the bundler starts its discovery threads with it, and an embedder gives it to its own threads. A stack is reserved address space, and only the pages a thread touches take memory.

## What the output keeps

- `minify(minify(x)) == minify(x)` and `format(format(x)) == format(x)`, and both outputs parse with no errors. Tests check both.
- Purity analysis assumes metamethods. A variable read, an index, arithmetic, a comparison, or a concatenation can each call one, so only literal arithmetic is pure, and `#"str"` is never folded, since escapes make the raw length unreliable.
- Constant folding reproduces the target's float arithmetic exactly, so it never fuses or reorders an operation.
- Emitters never read source text. Codegen and the formatter read the source only for trivia and verbatim regions and work without it, so a tree built in code prints.
- A bundle names no host path. Every path it carries is relative to the project root.

## Design decisions

- **Hand-written lexer, parser, and printers.** There is no parser dependency, so version gating and error recovery are luck's to control.
- **`Span` is `u32`.** That halves every span and caps a source at 4 GiB, which the lexer reports as one error.
- **A token holds no text, and there is no global string interner.** `Token::text` slices the source, and an interner's lock would serialize rayon's workers.
- **Leaves hold their text as written, and the printers print it unchecked.** The parser builds only leaves the lexer read, and luck's own rewrites build none the lexer would read differently, so a check would only cost time. A tree built in code answers for its own leaves.
- **Luau types are a tree**, `luck_ast::Type`, not text, so every consumer gets their structure.
- **One error type and one diagnostic scheme.** A lex, parse, or format error is a `SourceError`. An error that reaches a user is a `Diagnostic` from a numbered constructor in `luck_core`, such as `Diagnostic::e008` for a parse failure, and a lint diagnostic is named after its rule. The CLI exits 0 on success, 1 when it found problems, and 2 on a usage or config error.
- **The core crates make no OS calls.** `luck_token`, `luck_lexer`, `luck_ast`, `luck_parser`, `luck_codegen`, `luck_core`, `luck_formatter`, and `luck_minifier` take text or a tree and return text, a tree, or errors. Reading files and resolving paths start at `luck_resolver`.
- **Each tool owns the shape of its config.** A tool's option types derive their `luck.json` shape behind its `serde` feature, which `luck_config` turns on. The nearest `luck.json` governs a file whole, taking nothing from one above it unless it `extends` it. Format options layer defaults, then `.editorconfig`, then `luck.json`. An unknown key is an error, a rule name is checked against the linter's rules, and the extension's schema is generated from these types.
- **A file's dialect comes from its extension and the config.** A `.luau` file takes the `luau` key and any other file the `lua` key, and either key can name any dialect, so a Roblox tree that keeps Luau in `.lua` files sets `"lua": "roblox"`. The CLI and the language server both choose through `luck_config::DialectKey`.
