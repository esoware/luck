# luck_data_structures

Small data structures for code printers.

`CodeBuffer` is a growable byte buffer that a printer writes source code into,
and `BitSet` is a fixed number of bits. Neither knows anything about Luau.

## Usage

```rust
use luck_data_structures::CodeBuffer;

let mut output = CodeBuffer::with_capacity(16);
output.print_bytes(b"local");
output.print_byte(b' ');
output.print_bytes(b"x");
assert_eq!(output.into_bytes(), b"local x");
```
