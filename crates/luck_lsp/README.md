# luck_lsp

The language server for Lua and Luau, which `luck lsp` serves and the VS Code extension starts.

## Overview

Each version of an open document is copied into an arena of its own, parsed, and analyzed once, and every request reads that version's tree and analysis, shared read-only across threads. A replaced version's arena, once nothing holds it, is emptied for the next version to parse into, and a close frees it. A document's settings come from the nearest `luck.json` and `.editorconfig`, through `luck_config`, and are cached until a watched config file changes. Linting is opt-in: without a `lint` section, only parse errors show.

## Features

- Lint diagnostics on open, change, and save, from `luck_linter`.
- Formatting and range formatting, from `luck_formatter`.
- Hover, completion, signature help, go to definition, references, rename, document and workspace symbols, semantic tokens, document highlights, folding ranges, selection ranges, and document links.
- Code actions: each diagnostic's fix, `source.fixAll.luck`, and comments that disable a rule.
- Custom requests: `luck/syntaxTree` and `luck/fixAllWorkspace`.

## Public surface

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
