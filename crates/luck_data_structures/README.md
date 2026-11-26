# luck_data_structures

Small data structures shared across luck's crates, such as the output buffer the printers write to.

Nothing here knows about Luau.

## Usage

```rust
use luck_data_structures::CodeBuffer;

let mut output = CodeBuffer::with_capacity(16);
output.print_bytes(b"local");
output.print_byte(b' ');
output.print_bytes(b"x");
assert_eq!(output.into_bytes(), b"local x");
```

## API

- `CodeBuffer` is the byte buffer the printers write to.
- `BitSet` is a fixed-size set of bits.
