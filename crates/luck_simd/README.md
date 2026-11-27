# luck_simd

Safe SIMD vectors for AVX2 and Neon.

The CPU features a vector needs are encoded as types. Holding a value of `Avx2`
proves the CPU supports AVX2, and the only way to get one is `DynCpu::detect`.
Every vector method takes that value, which is what makes it safe to call.

On `x86_64`, AVX2 is detected at runtime. Neon is part of every aarch64 target,
so there it's known at compile time. AVX2 code has to run inside `Avx2::run`,
because an intrinsic only compiles to its single instruction in a function that
enables the feature. The code assumes little-endian byte order, and its design
follows `oxc_simd`.

## Usage

This example counts the bytes in a 32-byte block that equal `x`, with AVX2 when
the CPU has it:

```rust
use luck_simd::{DynCpu, Vec256};

let bytes = [b'x'; 32];
let count = match DynCpu::detect() {
    DynCpu::Avx2(cpu) => cpu.run(|cpu| {
        Vec256::read(&bytes, cpu).eq(Vec256::splat(b'x', cpu)).movemask().count_ones()
    }),
    DynCpu::Neon(_) | DynCpu::Generic(_) => bytes.iter().filter(|&&b| b == b'x').count() as u32,
};
assert_eq!(count, 32);
```

## API

* `DynCpu`, whichever feature set this CPU has.
* `Avx2`, `Neon`, and `Generic`, the feature sets. One that a platform can
  never have is an uninhabited type there.
* `Vec256`, 32 bytes in an AVX2 register.
* `Vec128`, 16 bytes in a Neon register.
