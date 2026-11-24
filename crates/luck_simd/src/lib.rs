//! # `luck_simd`
//!
//! CPU feature sets, and the SIMD vectors that work with them, as
//! `oxc_simd` has them for oxc, whose design this follows.
//!
//! - [`Avx2`], [`Neon`], and [`Generic`]: a feature set each. Holding one
//!   proves the CPU has it, because only [`DynCpu::detect`] makes one, and a
//!   feature set the platform can't have is an uninhabited type there.
//! - [`DynCpu`]: whichever one this CPU has, chosen at runtime on `x86_64` and
//!   at compile time everywhere else.
//! - [`Vec256`] and [`Vec128`]: 32 bytes in an AVX2 register and 16 in a Neon
//!   one. Each holds its feature set, so its methods are safe.
//!
//! A vector method compiles to its one instruction only in a function that
//! enables the feature, so AVX2 code runs inside [`Avx2::run`]. Neon is part
//! of every aarch64 target, so its code needs no such step.
//!
//! The code assumes little-endian byte order, as every target with these
//! feature sets has.
//!
//! This crate has zero internal dependencies.
//!
//! # Usage
//!
//! ```
//! use luck_simd::{DynCpu, Vec256};
//!
//! let bytes = [b'x'; 32];
//! let count = match DynCpu::detect() {
//!     DynCpu::Avx2(cpu) => cpu.run(|cpu| {
//!         Vec256::read(&bytes, cpu).eq(Vec256::splat(b'x', cpu)).movemask().count_ones()
//!     }),
//!     DynCpu::Neon(_) | DynCpu::Generic(_) => bytes.iter().filter(|&&b| b == b'x').count() as u32,
//! };
//! assert_eq!(count, 32);
//! ```

#![expect(
    unsafe_code,
    reason = "the vectors call intrinsics that the feature set they hold proves safe"
)]

mod cpu;
mod vec128;
mod vec256;

pub use cpu::{Avx2, DynCpu, Generic, Neon};
pub use vec128::Vec128;
pub use vec256::Vec256;
