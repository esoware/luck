# luck_simd

Safe SIMD vectors for AVX2 and Neon, gated on CPU feature detection.

## Overview

Having a value of a feature set type, such as `Avx2`, proves the CPU supports it. Vector methods take one, so they're safe to call.

`DynCpu::detect` checks for AVX2 at runtime on `x86_64`. Neon is always present on aarch64.

Put AVX2 code inside `Avx2::run` so each vector method compiles to one instruction. The code assumes little-endian byte order.

Based on `oxc_simd`.

## Usage

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

- `DynCpu` is whichever feature set this CPU has.
- `Avx2`, `Neon`, and `Generic` are the feature sets.
- `Vec256` is 32 bytes in an AVX2 register.
- `Vec128` is 16 bytes in a Neon register.
