/// A fixed number of bits, all clear until set, one bit apiece.
#[derive(Debug, Clone, Default)]
pub struct BitSet {
    words: Vec<u64>,
}

impl BitSet {
    /// `bit_count` bits, all clear.
    #[must_use]
    pub fn new(bit_count: usize) -> Self {
        Self { words: vec![0; bit_count.div_ceil(64)] }
    }

    /// Whether `bit` is set, which a bit past the end never is.
    #[inline]
    #[must_use]
    pub fn contains(&self, bit: usize) -> bool {
        self.words.get(bit >> 6u32).is_some_and(|&word| (word >> (bit & 63)) & 1 != 0)
    }

    /// Sets `bit`, and ignores a bit past the end.
    #[inline]
    pub fn insert(&mut self, bit: usize) {
        if let Some(word) = self.words.get_mut(bit >> 6u32) {
            *word |= 1u64 << (bit & 63);
        }
    }
}
