# luck_codegen

Code generation from Lua ASTs back to source text. Supports Lua 5.1-5.5 and Luau.

## Overview

`luck_codegen` is the luck toolchain's reverse parser. It walks an AST and emits minimal valid source code. It is source-independent: identifiers, numbers, strings, and Luau types all come from token-carried values on the AST, not from slicing the original source. The minifier prints its output through this crate, and so can any tool that builds or rewrites an AST.

## Key features

- **Compact output.** Minimal valid code. It strips comments, keeps whitespace to a minimum, and inserts separators only where ambiguity requires them. Output is idempotent: `compact(compact(x)) == compact(x)`.
- **Source-independent.** Leaf text comes from the tokens stored on AST nodes, so the printer needs no access to the original bytes. Luau type annotations come from walking the real `Type` AST, not from re-slicing source spans.
- **Token-merge disambiguation.** The separator system inserts a space wherever two adjacent emitted pieces would otherwise merge into a different token, covering cases like `--` (comment start), `//` (floor division), `[[` (long-string open), `..`/`...` (concat/vararg), and `<<`/`>>`/`>=`.
- **Statement boundary disambiguation.** A statement that starts with `(` gets a semicolon before it unless it opens its block, so the parser can't re-read it as a call continuing the previous statement.
- **Bounded depth.** Both entry points return a `SourceError` for a tree nested past `luck_ast::MAX_NESTING_DEPTH`, which a synthetic or transformed AST can be. The parser would refuse that output, and the printer stops descending at the cap, so its stack stays bounded on a tree of any depth.
- **Prints a tree as itself or refuses it.** Every leaf's text comes through `luck_ast`'s `*_spelling` and `check_*` functions, the same ones the formatter uses, so the two printers accept and refuse the same trees. A leaf no source spells where it sits, such as a keyword or `Eof` token where a name belongs, number or string text that isn't one token of its kind, `typeof` where a Luau type name starts, an interpolated string whose segments aren't shaped as the lexer reads them, or an error node, makes both entry points return a `SourceError` at that leaf instead of output that would read back as another tree or not at all. The printer doesn't know the target dialect, so a leaf passes when it reads back in some dialect, as the name `goto` does.

## Architecture

### Compact printer

`compact.rs` holds the two entry points, the tree-walking printer behind them, and its spacing primitives. The emitters live in `compact/`, split by node family as the parser's grammar is: `stmt.rs` (blocks and statements), `declaration.rs` (function, `local`, `type`, and `global` declarations), `expr.rs`, `function.rs` (the bodies, parameters, and attributes that function statements and expressions share), and `types.rs` (Luau types). Each AST variant has an emitter that calls the spacing primitives and recurses into children, counting the levels `luck_ast::Visitor::descend` defines as it goes. The printer builds output into a `luck_token::CodeBuffer` (a byte buffer with an ASCII fast path). `compact(block)` starts with no reserved capacity, and `compact_with_capacity(block, capacity)` takes a byte estimate, which is dropped if the allocator can't meet it. Fixpoint loops pass the previous output's length. Source length can badly over-reserve for comment-heavy input, and transformed or synthetic ASTs can emit more bytes than their original source.

### Separator logic

`separator.rs` classifies each emitted piece as a one-byte `PieceClass` (word, number, `-`, `/`, `[`, `.`, `..`, `<`, `>`, `;`, or other), and the printer keeps the previous piece's class instead of a cloned token. `needs_space` answers whether the next piece, given its first byte and class, would merge with the previous one into something else: two words merging into one identifier, a number swallowing a following `.`, `--` forming a comment, `//` forming floor division, `[[` opening a long string, `..`/`...` colliding with concat or vararg, and `<<`/`>>`/`>=` forming shift or comparison tokens. The rules don't depend on the Lua version, so a space that only some versions need goes in for all of them.

## Testing

Public-API tests live in `tests/it/` as a single binary. Every exact-output case goes through `assert_compacts_to`, which also checks that the output re-parses and compacts to itself. `compact.rs` pins the output per construct and the refusal of a misplaced token and an error node, `separator.rs` pins where the printer inserts a space or `;` and where it glues pieces together, and `fixtures.rs` checks that every shared fixture's compact output reparses. `src/separator.rs` additionally keeps inline white-box tests of the merge rules at the bottom of the file.
