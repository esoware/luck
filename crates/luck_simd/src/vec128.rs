#![expect(
    clippy::inline_always,
    reason = "a method that isn't inlined into its caller makes each intrinsic a call"
)]

use std::ops::BitAnd;

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::{
    uint8x16_t, vandq_u8, vceqq_u8, vdupq_n_u8, vgetq_lane_u64, vld1q_u8, vpaddq_u8, vqtbl1q_u8,
    vreinterpretq_u64_u8, vshrq_n_u8, vtstq_u8,
};

use crate::Neon;

#[cfg(target_arch = "aarch64")]
type Inner = uint8x16_t;
#[cfg(not(target_arch = "aarch64"))]
type Inner = [u8; 16];

/// Each lane's bit in the byte that [`Vec128::bitmask`] sums it into.
static LANE_BITS: [u8; 16] = [1, 2, 4, 8, 16, 32, 64, 128, 1, 2, 4, 8, 16, 32, 64, 128];

/// 16 bytes in a Neon register. It holds a [`Neon`], so one exists only
/// where the CPU has Neon, which makes its methods safe.
#[derive(Debug, Clone, Copy)]
pub struct Vec128 {
    #[cfg_attr(
        not(target_arch = "aarch64"),
        expect(dead_code, reason = "only the Neon intrinsics read it")
    )]
    inner: Inner,
    cpu: Neon,
}

impl Vec128 {
    #[must_use]
    #[inline(always)]
    pub fn read(bytes: &[u8; 16], cpu: Neon) -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            // SAFETY: `bytes` is 16 readable bytes, and the `Neon` proves the
            // CPU has Neon.
            let inner = unsafe { vld1q_u8(bytes.as_ptr()) };
            Self { inner, cpu }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = bytes;
            match cpu.0 {}
        }
    }

    #[must_use]
    #[inline(always)]
    pub fn splat(byte: u8, cpu: Neon) -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            // SAFETY: The `Neon` proves the CPU has Neon.
            let inner = unsafe { vdupq_n_u8(byte) };
            Self { inner, cpu }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = byte;
            match cpu.0 {}
        }
    }

    /// `0xFF` in each byte equal to `other`'s, `0x00` in the rest.
    #[must_use]
    #[inline(always)]
    pub fn eq(self, other: Self) -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            // SAFETY: The `Neon` proves the CPU has Neon.
            let inner = unsafe { vceqq_u8(self.inner, other.inner) };
            Self { inner, cpu: self.cpu }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = other;
            match self.cpu.0 {}
        }
    }

    /// `0xFF` in each byte that shares a set bit with `other`'s, `0x00` in
    /// the rest.
    #[must_use]
    #[inline(always)]
    pub fn test(self, other: Self) -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            // SAFETY: The `Neon` proves the CPU has Neon.
            let inner = unsafe { vtstq_u8(self.inner, other.inner) };
            Self { inner, cpu: self.cpu }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = other;
            match self.cpu.0 {}
        }
    }

    /// Each byte's high nibble, in its low four bits.
    #[must_use]
    #[inline(always)]
    pub fn high_nibbles(self) -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            // SAFETY: The `Neon` proves the CPU has Neon.
            let inner = unsafe { vshrq_n_u8::<4>(self.inner) };
            Self { inner, cpu: self.cpu }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            match self.cpu.0 {}
        }
    }

    /// Each byte of `indices` looked up in `self`, a 16-byte table. An index
    /// of 16 or more reads 0.
    #[must_use]
    #[inline(always)]
    pub fn lookup(self, indices: Self) -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            // SAFETY: The `Neon` proves the CPU has Neon.
            let inner = unsafe { vqtbl1q_u8(self.inner, indices.inner) };
            Self { inner, cpu: self.cpu }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = indices;
            match self.cpu.0 {}
        }
    }

    /// Bit `16 * i + j` set where byte `j` of `vectors[i]` is set. Every byte
    /// must be `0x00` or `0xFF`, which Neon's compares give, so the sums that
    /// fold the lanes together never carry.
    #[inline(always)]
    pub fn bitmask(vectors: [Self; 4]) -> u64 {
        let [first, second, third, fourth] = vectors;
        #[cfg(target_arch = "aarch64")]
        {
            let lane_bits = Self::read(&LANE_BITS, first.cpu);
            let low = (first & lane_bits).pairwise_add(second & lane_bits);
            let high = (third & lane_bits).pairwise_add(fourth & lane_bits);
            let quarters = low.pairwise_add(high);
            quarters.pairwise_add(quarters).low_u64()
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = (second, third, fourth, &LANE_BITS);
            match first.cpu.0 {}
        }
    }

    /// The sums of adjacent byte pairs, `self`'s then `other`'s.
    #[cfg(target_arch = "aarch64")]
    #[inline(always)]
    fn pairwise_add(self, other: Self) -> Self {
        // SAFETY: The `Neon` proves the CPU has Neon.
        let inner = unsafe { vpaddq_u8(self.inner, other.inner) };
        Self { inner, cpu: self.cpu }
    }

    #[cfg(target_arch = "aarch64")]
    #[inline(always)]
    fn low_u64(self) -> u64 {
        // SAFETY: The `Neon` proves the CPU has Neon.
        let lanes = unsafe { vreinterpretq_u64_u8(self.inner) };
        // SAFETY: The `Neon` proves the CPU has Neon.
        unsafe { vgetq_lane_u64::<0>(lanes) }
    }
}

impl BitAnd for Vec128 {
    type Output = Self;

    #[inline(always)]
    fn bitand(self, other: Self) -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            // SAFETY: The `Neon` proves the CPU has Neon.
            let inner = unsafe { vandq_u8(self.inner, other.inner) };
            Self { inner, cpu: self.cpu }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = other;
            match self.cpu.0 {}
        }
    }
}
