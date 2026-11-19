# luck_parser

The parser for Lua 5.1 to 5.5 and Luau, which builds a `luck_ast` tree in an arena the caller owns.

## Overview

The parser reads statements by recursive descent and expressions by Pratt parsing, covers the whole Luau type grammar, and gates every construct on the target `LuaVersion`. It pulls tokens from `luck_lexer` one at a time, with one token of lookahead, and puts every node in the arena. Names borrow their text from the source. It decodes each literal once, with the one numeral and escape reader in `luck_token`, so a literal holds its value beside its spelling, and a string borrows its value from the source too unless an escape or a line break had to be rewritten. As it goes, it numbers each name it reads and counts what the tree holds, for `luck_semantic`.

An error doesn't stop the parse. The parser recovers and reports every error in one pass, sorted by position. Two limits end it early instead, with one error each: a source over 4 GiB, and nesting past `luck_ast::MAX_NESTING_DEPTH`. So every tree it returns, with errors or without, can be walked on a thread with `luck_ast::MIN_STACK_BYTES`.

## Usage

- `parse(allocator, source, version)` returns a `ParseResult`: the source, the `Chunk`, which records `version` and holds the tree, the comments, the errors, the lenient escapes the lexer noted, the nesting depth, and the `TreeCounts`.
- `parse_with_tokens` also returns every token it read, so a host that needs both lexes the source once.
- `validate` runs the compile-time checks the parse skips: writes to read-only bindings, names no Lua 5.5 `global` declaration covers, `goto` and label resolution, and Luau's `continue` and `until` rule. Run it on a parse with no errors.
- `ParseError` is `luck_token::SourceError`.

Every dialect's source reads into the same shape: a `break`, a `continue`, or a `return` is a statement like any other, and a `;` is no node. A statement the grammar takes only at the end of its block but that has others after it is an error, and stays where it was written.

```rust
use luck_allocator::Allocator;
use luck_token::LuaVersion;

let allocator = Allocator::new();
let result = luck_parser::parse(&allocator, "local x = 1 + 2", LuaVersion::Lua54);
assert!(result.errors.is_empty());
assert_eq!(result.chunk.block.stmts.len(), 1);
```
