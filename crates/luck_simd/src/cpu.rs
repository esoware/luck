//! Each type here can exist only where the CPU has its feature set, so holding
//! one proves the CPU supports it, and the vector types and [`Avx2::run`] rely
//! on that for safety. Each has a private field, and only [`DynCpu::detect`]
//! builds one, after checking the CPU. A type whose feature set is impossible
//! on the platform holds an uninhabited field there, so the compiler drops
//! every arm that handles it.

use std::convert::Infallible;

/// AVX2, with the BMI1, BMI2, and POPCNT that every AVX2 CPU has, on `x86_64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Avx2(pub(crate) Avx2Proof);

#[cfg(target_arch = "x86_64")]
pub(crate) type Avx2Proof = ();
#[cfg(not(target_arch = "x86_64"))]
pub(crate) type Avx2Proof = Infallible;

/// Neon on aarch64, which every aarch64 target with `std` enables, so it
/// needs no runtime check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Neon(pub(crate) NeonProof);

#[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
pub(crate) type NeonProof = ();
#[cfg(not(all(target_arch = "aarch64", target_feature = "neon")))]
pub(crate) type NeonProof = Infallible;

/// No SIMD. Uninhabited where a SIMD feature set is enabled at compile time,
/// so the scalar code isn't in those binaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generic(GenericProof);

#[cfg(not(any(
    all(
        target_arch = "x86_64",
        target_feature = "avx2",
        target_feature = "bmi1",
        target_feature = "bmi2",
        target_feature = "popcnt"
    ),
    all(target_arch = "aarch64", target_feature = "neon")
)))]
type GenericProof = ();
#[cfg(any(
    all(
        target_arch = "x86_64",
        target_feature = "avx2",
        target_feature = "bmi1",
        target_feature = "bmi2",
        target_feature = "popcnt"
    ),
    all(target_arch = "aarch64", target_feature = "neon")
))]
type GenericProof = Infallible;

/// The best feature set this CPU has. Only one variant is inhabited, and the
/// type is zero-sized, everywhere but `x86_64` without AVX2 enabled at compile
/// time, where it's a one-byte choice between [`Avx2`] and [`Generic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynCpu {
    Avx2(Avx2),
    Neon(Neon),
    Generic(Generic),
}

/// A [`Generic`], where one can exist.
#[cfg(not(any(
    all(
        target_arch = "x86_64",
        target_feature = "avx2",
        target_feature = "bmi1",
        target_feature = "bmi2",
        target_feature = "popcnt"
    ),
    all(target_arch = "aarch64", target_feature = "neon")
)))]
const GENERIC: Option<Generic> = Some(Generic(()));
#[cfg(any(
    all(
        target_arch = "x86_64",
        target_feature = "avx2",
        target_feature = "bmi1",
        target_feature = "bmi2",
        target_feature = "popcnt"
    ),
    all(target_arch = "aarch64", target_feature = "neon")
))]
const GENERIC: Option<Generic> = None;

impl DynCpu {
    #[inline]
    pub fn detect() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            // Where the features are enabled at compile time, there's no
            // `Generic` to fall back on, and no need for one.
            if let Some(generic) = GENERIC
                && !(std::arch::is_x86_feature_detected!("avx2")
                    && std::arch::is_x86_feature_detected!("bmi1")
                    && std::arch::is_x86_feature_detected!("bmi2")
                    && std::arch::is_x86_feature_detected!("popcnt"))
            {
                return Self::Generic(generic);
            }
            Self::Avx2(Avx2(()))
        }
        #[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
        {
            Self::Neon(Neon(()))
        }
        #[cfg(not(any(
            target_arch = "x86_64",
            all(target_arch = "aarch64", target_feature = "neon")
        )))]
        {
            Self::Generic(Generic(()))
        }
    }

    /// Every feature set this CPU has, best first, for a test that runs each.
    pub fn available() -> Vec<Self> {
        let best = Self::detect();
        let mut available = vec![best];
        if !matches!(best, Self::Generic(_)) {
            available.extend(GENERIC.map(Self::Generic));
        }
        available
    }
}

impl Avx2 {
    /// Run `f` with AVX2 enabled, so the vector methods it calls inline into
    /// it. Through a plain call, each one would be a call to its intrinsic.
    #[inline]
    pub fn run<R>(self, f: impl FnOnce(Self) -> R) -> R {
        #[cfg(target_arch = "x86_64")]
        {
            #[target_feature(enable = "avx2,bmi1,bmi2,popcnt")]
            #[inline]
            fn with_avx2<R>(cpu: Avx2, f: impl FnOnce(Avx2) -> R) -> R {
                f(cpu)
            }
            // SAFETY: An `Avx2` exists only where the CPU has these features.
            unsafe { with_avx2(self, f) }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = f;
            match self.0 {}
        }
    }
}
