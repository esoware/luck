# luck_lsp

The language server for Lua and Luau, which `luck lsp` serves and the VS Code extension starts.

## Overview

One main loop takes the client's messages in order and touches no tree, as rust-analyzer's does. It applies each notification as it arrives and hands every request to a pool of its own workers, with the version of each document it reads as it stood then. A request waits in a queue ahead of the diagnostics, so a lint of every open document doesn't keep a hover waiting.

Each version of an open document keeps its text in an `Arc<str>`, and the first job that needs it parses and analyzes it in an arena of its own, held with its parse and analysis through `self_cell`, with no `unsafe`. A job that needs the same version meanwhile waits for that one parse. Lints are built once per version too, and publishing, code actions, and fix-all share them. An edit or a close stops the lint of the version it replaces between rules, the main loop drops any answer or diagnostics that went stale, and a request the client cancels before it starts never runs. Once nothing holds a replaced version, its arena empties into a spare slot the next version parses into, and a close frees it.

Every provider reads that one analysis. A request at a position finds the innermost node there in `luck_semantic`'s node table, with one binary search, and walks up through its parents, and a name finds the local it declares or refers to by its symbol and reference, so shadowed names never mix. Links come from the analysis's `require` sites, the ones a build resolves.

A document's settings come from the `luck_config::Project` of the nearest `luck.json`, so the editor runs the rules `luck lint` runs, and from `.editorconfig`. A worker finds them the first time a document is analyzed, since that reads the disk. The server caches each project until a file it read, or a `.editorconfig`, changes. A `luck.json` that doesn't load is shown as an error and published as a diagnostic on the file at fault, and its documents use the defaults until it's fixed. Each project keeps one resolver for its Lua files and one for its Luau files, which a `.luaurc` edited, or a module created or deleted, clears.

Positions count UTF-8 bytes when the client offers them, since a column then costs nothing, or else UTF-32 characters or UTF-16 units, in the order the client prefers.

## Features

- Lint diagnostics on open and change, from `luck_linter`, each with its labels as related locations, its help and note after its message, a link to its rule, and the `Unnecessary` or `Deprecated` tag where it has one.
- Formatting and range formatting, from `luck_formatter`.
- Hover, completion, signature help, go to definition, references, rename, document and workspace symbols, semantic tokens, document highlights, folding ranges, selection ranges, and document links.
- Code actions: each diagnostic's fix or suggestion, `source.fixAll.luck`, which applies the fixes and leaves the suggestions, and comments that disable a rule.
- Custom requests: `luck/syntaxTree` and `luck/fixAllWorkspace`.

## Usage

- `serve_stdio()` serves over stdin and stdout, `serve_socket(port)` over TCP, for debugging, and `serve(connection)` over any `lsp_server::Connection`, such as the in-memory pair the tests drive it through.
- Each returns an error when the session doesn't end as LSP says it should, such as with `exit` before `shutdown`, which the CLI turns into exit code 1.

The server starts its workers with `luck_ast::MIN_STACK_BYTES` of stack. The linter runs a large file's rules on rayon's global pool, so a host builds that pool with `ThreadPoolBuilder::stack_size(MIN_STACK_BYTES)` before it serves, as `luck lsp` does.
