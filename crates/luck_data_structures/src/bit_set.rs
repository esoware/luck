/// A fixed number of bits, all clear until set, one bit apiece.
#[derive(Debug, Clone, Default)]
pub struct BitSet {
    words: Box<[u64]>,
}

impl BitSet {
    /// `bit_count` bits, all clear.
    pub fn new(bit_count: usize) -> Self {
        Self { words: vec![0; bit_count.div_ceil(64)].into_boxed_slice() }
    }

    /// Whether `bit` is set, which a bit past the end never is.
    #[inline]
    pub fn contains(&self, bit: usize) -> bool {
        self.words.get(bit >> 6).is_some_and(|&word| (word >> (bit & 63)) & 1 != 0)
    }

    /// Sets `bit`, and returns whether it was clear, as `HashSet::insert`
    /// says whether the value was new. A bit past the end stays clear, and
    /// inserting it returns false.
    #[inline]
    pub fn insert(&mut self, bit: usize) -> bool {
        let Some(word) = self.words.get_mut(bit >> 6) else {
            return false;
        };
        let mask = 1 << (bit & 63);
        let was_clear = *word & mask == 0;
        *word |= mask;
        was_clear
    }
}
