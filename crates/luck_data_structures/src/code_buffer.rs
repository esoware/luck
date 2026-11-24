use core::iter;
use core::mem::MaybeUninit;

/// Bytes, as a Lua source is, so a printer copies a string or a comment that
/// isn't UTF-8 as it was written.
#[derive(Debug, Default)]
pub struct CodeBuffer {
    bytes: Vec<u8>,
}

impl CodeBuffer {
    /// The capacity is a hint: one the allocator can't meet is dropped, and
    /// the buffer grows as it's written.
    #[inline]
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let mut bytes = Vec::new();
        let _unused = bytes.try_reserve_exact(capacity);
        Self { bytes }
    }

    #[inline]
    #[must_use]
    pub const fn len(&self) -> usize {
        self.bytes.len()
    }

    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The grow path is outlined so a push with spare capacity stays a load,
    /// store, and increment.
    #[inline]
    pub fn print_byte(&mut self, byte: u8) {
        #[cold]
        #[inline(never)]
        fn push_slow(bytes: &mut Vec<u8>, byte: u8) {
            bytes.push(byte);
        }

        if self.bytes.len() < self.bytes.capacity() {
            self.bytes.push(byte);
        } else {
            push_slow(&mut self.bytes, byte);
        }
    }

    #[inline]
    pub fn print_repeat(&mut self, byte: u8, count: usize) {
        self.bytes.extend(iter::repeat_n(byte, count));
    }

    /// At most 16 bytes, as nearly every piece of code is, are copied as two
    /// overlapping words of one width instead of with a `memcpy` call, and
    /// bytes known at compile time fold to their stores. Any more take a
    /// cold path out of line, which prints about 1% faster than inlining it.
    #[inline]
    #[expect(unsafe_code, reason = "extending the vector calls memcpy for every short piece")]
    pub fn print_bytes(&mut self, source: &[u8]) {
        #[cold]
        #[inline(never)]
        fn print_slow(bytes: &mut Vec<u8>, source: &[u8]) {
            bytes.extend_from_slice(source);
        }

        let count = source.len();
        let length = self.bytes.len();
        if count <= 16
            && let Some(window) = self.bytes.spare_capacity_mut().get_mut(..count)
        {
            match count {
                8.. => copy_overlapping::<8>(source, window),
                4..=7 => copy_overlapping::<4>(source, window),
                2 | 3 => copy_overlapping::<2>(source, window),
                1 => copy_overlapping::<1>(source, window),
                0 => {}
            }
            // SAFETY: The `count` bytes past `length` now hold `source`, so
            // they're initialized, and they were spare capacity, so the sum
            // can't wrap.
            unsafe {
                self.bytes.set_len(length.wrapping_add(count));
            }
        } else {
            print_slow(&mut self.bytes, source);
        }
    }

    /// Appends what `write` appends to the vector it's handed, which is the
    /// buffer itself, so nothing is copied.
    #[inline]
    pub fn print_with<Write>(&mut self, write: Write)
    where
        Write: FnOnce(&mut Vec<u8>),
    {
        write(&mut self.bytes);
    }

    #[inline]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[inline]
    pub fn truncate(&mut self, new_length: usize) {
        self.bytes.truncate(new_length);
    }

    #[inline]
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Writes after the bytes the vector holds, into its capacity.
impl From<Vec<u8>> for CodeBuffer {
    #[inline]
    fn from(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

/// Copies the first and the last `N` bytes of `source` into `window`, which
/// covers all of it when it's `N` to `2 * N` bytes long, as `window` is too.
#[inline]
fn copy_overlapping<const N: usize>(source: &[u8], window: &mut [MaybeUninit<u8>]) {
    if let (Some(head), Some(slot)) = (source.first_chunk::<N>(), window.first_chunk_mut::<N>()) {
        *slot = head.map(MaybeUninit::new);
    }
    if let (Some(tail), Some(slot)) = (source.last_chunk::<N>(), window.last_chunk_mut::<N>()) {
        *slot = tail.map(MaybeUninit::new);
    }
}
