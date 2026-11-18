# luck_parser

The parser for Lua 5.1 to 5.5 and Luau, which builds a `luck_ast` tree in an arena the caller owns.

## Overview

Statements are parsed by recursive descent and expressions by Pratt parsing, along with the whole Luau type grammar, and every construct is gated on the target `LuaVersion`. The parser pulls tokens from `luck_lexer` one at a time, with one token of lookahead, and puts every node in the arena. Names and literals borrow their text from the source. As it goes, it numbers each name it reads and counts what the tree holds, for `luck_semantic`.

An error doesn't stop the parse: it recovers and reports every error in one pass, sorted by position. Two limits end it early instead, with one error each: a source over 4 GiB, and nesting past `luck_ast::MAX_NESTING_DEPTH`. So every tree it returns, with errors or without, can be walked on a thread with `luck_ast::MIN_STACK_BYTES`.

## Usage

- `parse(allocator, source, version)` returns a `ParseResult`: the source, the `Block`, the comments, the errors, the nesting depth, and the `TreeCounts`.
- `parse_with_tokens` also returns every token it read, so a host that needs both lexes the source once.
- `validate` runs the compile-time checks the parse skips: writes to read-only bindings, names no Lua 5.5 `global` declaration covers, `goto` and label resolution, and Luau's `continue` and `until` rule. Run it on a parse with no errors.
- `ParseError` is `luck_token::SourceError`.

```rust
use luck_allocator::Allocator;
use luck_token::LuaVersion;

let allocator = Allocator::new();
let result = luck_parser::parse(&allocator, "local x = 1 + 2", LuaVersion::Lua54);
assert!(result.errors.is_empty());
assert_eq!(result.block.stmts.len(), 1);
```
