# luck_lsp

A Luau language server, with type-aware hover and completion, lint diagnostics,
and formatting.

The server speaks the Language Server Protocol over stdio, or over TCP for
debugging. Hover, completion, and signature help come from Luau's own type
checker, including documentation from definition files. Diagnostics combine
lint results, requires that don't resolve, and type errors.

Go-to-definition, references, and rename work across files for a module's
members, and go-to-definition can jump into Luau's and Roblox's built-in
definitions, which are served as `luck:` documents. The server also provides
code actions, semantic tokens, highlights, folding and selection ranges,
document links, and document and workspace symbols, with positions in UTF-8,
UTF-16, or UTF-32.

A client that sends a config in `initializationOptions` can serve every file
itself, with nothing read from disk.

## API

* `serve_stdio`, which serves over stdio, as `luck lsp` does.
* `serve_socket`, which serves over TCP, for debugging.
* `serve`, which serves over any `lsp_server` connection.

Beyond the standard protocol, the server handles the `luck/syntaxTree` and
`luck/fixAllWorkspace` requests and the `luck/didChangeFiles` notification,
through which a client pushes the text of files it holds.
