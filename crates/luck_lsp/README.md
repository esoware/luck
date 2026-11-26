# luck_lsp

A Luau language server, built on luck's linter and formatter and Luau's own type checker.

## Overview

It runs as `luck lsp` and speaks the Language Server Protocol over stdio, or TCP for debugging. Hover, completion, and signature help come from Luau's types, including docs from definition files.

It also supports:

- lint and type error diagnostics, plus requires that don't resolve
- formatting, including ranges
- go-to-definition, references, rename, and document and workspace symbols, including module members across files
- go-to-definition into Luau's and Roblox's definition files, shown as `luck:` documents
- code actions for fixes, `source.fixAll.luck`, and disabling a rule
- semantic tokens, highlights, folding ranges, selection ranges, and document links
- UTF-8, UTF-16, and UTF-32 positions

Custom methods are `luck/syntaxTree`, `luck/fixAllWorkspace`, and the notification `luck/didChangeFiles`. If the client passes a config in `initializationOptions`, the server reads only the files the client sends it.

## Architecture

A main loop reads messages in order and never touches a syntax tree. Requests run on a worker pool.

Each document version is parsed once, in its own arena, and an edit cancels linting of the old version. Idle workers index the rest of the workspace in the background.

## API

- `serve_stdio` serves over stdio, as `luck lsp` does.
- `serve_socket` serves over TCP, for debugging.
- `serve` serves over any `lsp_server` connection.
