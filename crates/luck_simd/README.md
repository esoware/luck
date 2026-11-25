# luck_simd

CPU feature sets, and the SIMD vectors that work with them, for luck's lexer. It began as a port of `oxc_simd`, whose notice is in `LICENSE`.

## Usage

- `Avx2`, `Neon`, and `Generic`: a feature set each. Holding one proves the CPU has it, because only `DynCpu::detect` makes one, and a feature set a platform can't have is an uninhabited type there, so the code that handles it compiles away.
- `DynCpu`: the best feature set this CPU has, checked at runtime on x86_64 and known at compile time everywhere else. `DynCpu::available` lists each one the CPU has, so a test can run its code under every one.
- `Vec256` and `Vec128`: 32 bytes in an AVX2 register and 16 in a Neon one. Each holds its feature set, so its methods are safe to call.

An AVX2 intrinsic compiles to its one instruction only inside a function that enables AVX2, so AVX2 code runs in a closure passed to `Avx2::run`. Neon is part of every aarch64 target, so its code needs no such step.

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