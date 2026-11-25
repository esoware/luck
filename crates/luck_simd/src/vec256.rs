#![expect(
    clippy::inline_always,
    reason = "a method that isn't inlined into its AVX2 caller makes each intrinsic a call"
)]

use std::ops::BitAnd;

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{
    __m256i, _mm256_and_si256, _mm256_cmpeq_epi8, _mm256_cmpgt_epi8, _mm256_loadu_si256,
    _mm256_movemask_epi8, _mm256_set1_epi8, _mm256_shuffle_epi8, _mm256_srli_epi16,
};

use crate::Avx2;

#[cfg(target_arch = "x86_64")]
type Inner = __m256i;
#[cfg(not(target_arch = "x86_64"))]
type Inner = [u8; 32];

/// 32 bytes in an AVX2 register. It holds an [`Avx2`], so one exists only
/// where the CPU has AVX2, which makes its methods safe.
#[derive(Debug, Clone, Copy)]
pub struct Vec256 {
    #[cfg_attr(
        not(target_arch = "x86_64"),
        expect(dead_code, reason = "only the AVX2 intrinsics read it")
    )]
    inner: Inner,
    cpu: Avx2,
}

impl Vec256 {
    #[must_use]
    #[inline(always)]
    pub fn read(bytes: &[u8; 32], cpu: Avx2) -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: `bytes` is 32 readable bytes, and the `Avx2` proves the
            // CPU has AVX2.
            let inner = unsafe { _mm256_loadu_si256(bytes.as_ptr().cast()) };
            Self { inner, cpu }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = bytes;
            match cpu.0 {}
        }
    }

    #[must_use]
    #[inline(always)]
    pub fn splat(byte: u8, cpu: Avx2) -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: The `Avx2` proves the CPU has AVX2.
            let inner = unsafe { _mm256_set1_epi8(byte.cast_signed()) };
            Self { inner, cpu }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = byte;
            match cpu.0 {}
        }
    }

    /// `0xFF` in each byte equal to `other`'s, `0x00` in the rest.
    #[must_use]
    #[inline(always)]
    pub fn eq(self, other: Self) -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: The `Avx2` proves the CPU has AVX2.
            let inner = unsafe { _mm256_cmpeq_epi8(self.inner, other.inner) };
            Self { inner, cpu: self.cpu }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = other;
            match self.cpu.0 {}
        }
    }

    /// `0xFF` in each byte greater than `other`'s as an `i8`, `0x00` in the
    /// rest.
    #[must_use]
    #[inline(always)]
    pub fn gt_i8(self, other: Self) -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: The `Avx2` proves the CPU has AVX2.
            let inner = unsafe { _mm256_cmpgt_epi8(self.inner, other.inner) };
            Self { inner, cpu: self.cpu }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = other;
            match self.cpu.0 {}
        }
    }

    /// Each byte's high nibble, in its low four bits.
    #[must_use]
    #[inline(always)]
    pub fn high_nibbles(self) -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: The `Avx2` proves the CPU has AVX2.
            let shifted = unsafe { _mm256_srli_epi16::<4>(self.inner) };
            Self { inner: shifted, cpu: self.cpu } & Self::splat(0x0F, self.cpu)
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            match self.cpu.0 {}
        }
    }

    /// Each byte of `indices` looked up in `self`, a 16-byte table repeated in
    /// both halves. An index with its high bit set reads 0, and any other
    /// reads the entry its low four bits name.
    #[must_use]
    #[inline(always)]
    pub fn lookup(self, indices: Self) -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: The `Avx2` proves the CPU has AVX2.
            let inner = unsafe { _mm256_shuffle_epi8(self.inner, indices.inner) };
            Self { inner, cpu: self.cpu }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = indices;
            match self.cpu.0 {}
        }
    }

    /// The high bit of each byte, byte 0's lowest.
    #[inline(always)]
    pub fn movemask(self) -> u32 {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: The `Avx2` proves the CPU has AVX2.
            unsafe { _mm256_movemask_epi8(self.inner) }.cast_unsigned()
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            match self.cpu.0 {}
        }
    }
}

impl BitAnd for Vec256 {
    type Output = Self;

    #[inline(always)]
    fn bitand(self, other: Self) -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: The `Avx2` proves the CPU has AVX2.
            let inner = unsafe { _mm256_and_si256(self.inner, other.inner) };
            Self { inner, cpu: self.cpu }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = other;
            match self.cpu.0 {}
        }
    }
}
