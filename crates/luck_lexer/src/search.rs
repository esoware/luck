#[repr(C, align(64))]
pub(crate) struct ByteMatchTable([bool; 256]);

impl ByteMatchTable {
    pub(crate) const fn new(table: [bool; 256]) -> Self {
        Self(table)
    }

    #[inline]
    #[expect(
        clippy::indexing_slicing,
        reason = "a u8 can't index past 256 entries, and this is the per-byte scan"
    )]
    fn matches(&self, byte: u8) -> bool {
        self.0[usize::from(byte)]
    }
}

/// `byte_match_table!(|b| ...)` evaluates the predicate for every byte
/// value at compile time.
macro_rules! byte_match_table {
    (|$byte:ident| $test:expr) => {{
        #[expect(clippy::as_conversions, clippy::cast_possible_truncation, clippy::indexing_slicing, reason = "runs at compile time with index < 256, and the checked conversions aren't const")]
        const TABLE: [bool; 256] = {
            let mut table = [false; 256];
            let mut i = 0;
            while i < 256 {
                let $byte = i as u8;
                table[i] = $test;
                i += 1;
            }
            table
        };
        $crate::search::ByteMatchTable::new(TABLE)
    }};
}
pub(crate) use byte_match_table;

/// Offset of the first byte matching `table`, or `bytes.len()` if none match.
///
/// Most runs are short, so the first 8 bytes are checked one at a time and
/// only longer runs pay for batches. Each batch is tested whole before the
/// match is located, which keeps long runs branch-light.
#[inline]
pub(crate) fn find_match(bytes: &[u8], table: &ByteMatchTable) -> usize {
    const SCALAR_PREFIX: usize = 8;
    const BATCH_SIZE: usize = 32;
    let (prefix, mut remaining) = bytes.split_at(bytes.len().min(SCALAR_PREFIX));
    if let Some(offset) = prefix.iter().position(|&b| table.matches(b)) {
        return offset;
    }
    while let Some((batch, after_batch)) = remaining.split_first_chunk::<BATCH_SIZE>() {
        let mut has_match = false;
        for &b in batch {
            has_match |= table.matches(b);
        }
        if has_match {
            break;
        }
        remaining = after_batch;
    }
    let tail_length = remaining.iter().take_while(|&&b| !table.matches(b)).count();
    bytes.len().saturating_sub(remaining.len()).saturating_add(tail_length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_match_returns_the_first_stop_across_prefix_and_batch_boundaries() {
        let mut stops = [false; 256];
        stops[usize::from(b'!')] = true;
        let stop_table = ByteMatchTable::new(stops);
        for length in 0..=100 {
            let run = vec![b'a'; length];
            assert_eq!(find_match(&run, &stop_table), length, "no stop in {length}");
            for stop in 0..length {
                let mut bytes = run.clone();
                bytes[stop] = b'!';
                assert_eq!(find_match(&bytes, &stop_table), stop, "stop {stop} of {length}");
                bytes[stop..].fill(b'!');
                assert_eq!(find_match(&bytes, &stop_table), stop, "stops from {stop} of {length}");
            }
        }
    }
}
