use std::{
    marker::PhantomData,
    panic::{self, UnwindSafe},
};

use quickcheck::{Gen, QuickCheck, Testable};

/// Runs `property` on the same cases every run, whatever quickcheck's environment variables say.
pub(crate) fn check<A: Testable>(property: A) {
    QuickCheck::new()
        .rng(Gen::from_size_and_seed(100, 0))
        .tests(100)
        .max_tests(10_000)
        .min_tests_passed(0)
        .quickcheck(property);
}

/// The message `run` panics with.
pub(crate) fn panic_message(run: impl FnOnce() + UnwindSafe) -> String {
    let payload = panic::catch_unwind(run).expect_err("it panics");
    match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => {
            payload.downcast_ref::<&str>().map(|message| (*message).to_owned()).unwrap()
        }
    }
}

/// Whether a concrete `T` is `Send` and `Sync`, as `AutoTraits::<T>::IS_SEND` and `IS_SYNC` say.
/// A path names an inherent constant before a trait's, and each inherent one exists only where its
/// bound holds, so [`Fallback`]'s `false` answers only where the bound fails.
pub(crate) struct AutoTraits<T: ?Sized>(PhantomData<T>);

pub(crate) trait Fallback {
    const IS_SEND: bool = false;
    const IS_SYNC: bool = false;
}

impl<T: ?Sized> Fallback for AutoTraits<T> {}

impl<T: ?Sized + Send> AutoTraits<T> {
    pub(crate) const IS_SEND: bool = true;
}

impl<T: ?Sized + Sync> AutoTraits<T> {
    pub(crate) const IS_SYNC: bool = true;
}
