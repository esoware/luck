# luck_data_structures

Data structures that luck's crates share and that know nothing of Luau, as `oxc_data_structures` holds oxc's.

## Usage

- `CodeBuffer`: the output buffer `luck_codegen` and `luck_formatter` write into. It holds bytes, as a source is, so a printer copies a string or a comment that isn't UTF-8 as it was written, and it copies a short piece, as nearly every piece of code is, without a `memcpy` call.
- `BitSet`: a fixed number of bits, one apiece, as `luck_minifier` marks the references a pass drops.

```rust
use luck_data_structures::CodeBuffer;

let mut output = CodeBuffer::with_capacity(16);
output.print_bytes(b"local");
output.print_byte(b' ');
output.print_bytes(b"x");
assert_eq!(output.into_bytes(), b"local x");
```
