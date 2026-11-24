/// An xorshift64* generator: deterministic, dependency-free, and the same on
/// every platform. Only reproducibility matters here, not speed or
/// statistical quality.
#[derive(Debug)]
pub(crate) struct Rng {
    state: u64,
}

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        // A zero state stays zero under xorshift, so seed 0 draws like seed 1.
        Self { state: seed.max(1) }
    }

    const fn next(&mut self) -> u64 {
        let mut state = self.state;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.state = state;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A uniform value in `0..bound`, or zero when `bound` is zero. The draw
    /// stays in `u64` so a seed generates the same program on every platform.
    pub(crate) fn below(&mut self, bound: usize) -> usize {
        let draw = self.next();
        if bound == 0 {
            return 0;
        }
        usize::try_from(draw % bound as u64).expect("the remainder is below `bound`")
    }

    pub(crate) fn one_through(&mut self, bound: usize) -> usize {
        self.below(bound) + 1
    }

    /// A uniform element of `items`.
    pub(crate) fn pick<T>(&mut self, items: &[T]) -> T
    where
        T: Clone,
    {
        let index = self.below(items.len());
        items[index].clone()
    }

    pub(crate) fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

#[cfg(test)]
mod tests {
    use std::array;

    use super::*;

    #[test]
    fn seed_zero_draws_like_seed_one() {
        let mut zero = Rng::new(0);
        let mut one = Rng::new(1);
        for _ in 0..8 {
            assert_eq!(zero.next(), one.next());
        }
    }

    // Every generated program, and so minsize.snap, rests on this sequence.
    #[test]
    fn draws_for_a_seed_never_change() {
        let mut rng = Rng::new(42);
        let draws: [usize; 6] = array::from_fn(|_| rng.below(1000));
        assert_eq!(draws, [600, 498, 846, 735, 678, 875]);
    }
}
