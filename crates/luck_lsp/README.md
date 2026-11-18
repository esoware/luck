# luck_lsp

The language server for Lua and Luau, which `luck lsp` serves and the VS Code extension starts.

## Overview

The server copies each version of an open document into an arena of its own and parses and analyzes it once. Every request reads that version's tree and analysis, shared read-only across threads. Once nothing holds a replaced version, the server empties its arena for the next version to parse into, and closing the document frees it. A document's settings come from the `luck_config::Project` of the nearest `luck.json`, so the editor runs the rules `luck lint` runs, and from `.editorconfig`. The server caches each project until a file it read, or a `.editorconfig` or `.luaurc`, changes. A `luck.json` that doesn't load is shown as an error and published as a diagnostic on the file at fault, and its documents use the defaults until it's fixed.

## Features

- Lint diagnostics on open, change, and save, from `luck_linter`.
- Formatting and range formatting, from `luck_formatter`.
- Hover, completion, signature help, go to definition, references, rename, document and workspace symbols, semantic tokens, document highlights, folding ranges, selection ranges, and document links.
- Code actions: each diagnostic's fix, `source.fixAll.luck`, and comments that disable a rule.
- Custom requests: `luck/syntaxTree` and `luck/fixAllWorkspace`.

## Usage

- `serve_stdio()` serves over stdin and stdout, and `serve_socket(port)` over TCP, for debugging.
- `Backend` is the server, and `Notifier` the part of the client it talks to.

Requests run on the thread that polls the server, on tokio's blocking pool, and, for a large file's lint, on rayon's pool, so each needs `luck_ast::MIN_STACK_BYTES` of stack:

```rust,no_run
use std::thread;

use luck_ast::MIN_STACK_BYTES;

let runtime = tokio::runtime::Builder::new_multi_thread()
    .thread_stack_size(MIN_STACK_BYTES)
    .enable_all()
    .build()
    .expect("a runtime");
let server = thread::Builder::new()
    .stack_size(MIN_STACK_BYTES)
    .spawn(move || runtime.block_on(luck_lsp::serve_stdio()))
    .expect("a server thread");
server.join().expect("the server doesn't panic");
```
