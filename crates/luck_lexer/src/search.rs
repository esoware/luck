#[repr(C, align(64))]
pub(crate) struct ByteMatchTable([bool; 256]);

impl ByteMatchTable {
    pub(crate) const fn new(table: [bool; 256]) -> Self {
        Self(table)
    }

    #[inline]
    fn matches(&self, byte: u8) -> bool {
        self.0[usize::from(byte)]
    }
}

/// `byte_match_table!(|b| ...)` evaluates the predicate for every byte
/// value at compile time.
macro_rules! byte_match_table {
    (|$byte:ident| $test:expr) => {{
        #[expect(
            clippy::cast_possible_truncation,
            reason = "`i` stays below 256, and `u8::try_from` isn't const"
        )]
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
    bytes.len() - remaining.len() + tail_length
}

static NOT_NAME_CONTINUE: ByteMatchTable =
    byte_match_table!(|b| !(b.is_ascii_alphanumeric() || b == b'_'));

// The word scans below test 8 bytes at once, each answer in its byte's high
// bit, so a short run ends without a loop exit to mispredict.

const HIGH_BITS: u64 = 0x8080_8080_8080_8080;
const LOW_BITS: u64 = 0x7F7F_7F7F_7F7F_7F7F;

/// `chunk` with its first byte lowest, so the first set high bit marks the
/// first byte that answers.
#[inline]
const fn word(chunk: [u8; 8]) -> u64 {
    u64::from_le_bytes(chunk)
}

/// The index of the byte whose high bit is the lowest one set in `answers`.
#[inline]
const fn first_answer(answers: u64) -> usize {
    (answers.trailing_zeros() >> 3) as usize
}

/// The high bit of each byte of `word` that equals `byte`.
///
/// Adding 0x7F to the low 7 bits of a byte sets its high bit unless they are
/// all clear, and never carries into the next byte.
#[inline]
fn bytes_equal(word: u64, byte: u8) -> u64 {
    let differs = word ^ u64::from(byte).wrapping_mul(0x0101_0101_0101_0101);
    !(((differs & LOW_BITS).wrapping_add(LOW_BITS)) | differs) & HIGH_BITS
}

/// The high bit of each byte of `ascii` that lies in `low..=high`.
///
/// Each bound repeats in every byte, and every byte of `ascii` is under 0x80,
/// so setting its high bit keeps each subtraction from borrowing across bytes.
#[inline]
const fn bytes_in_range(ascii: u64, low: u64, high: u64) -> u64 {
    ((ascii | HIGH_BITS).wrapping_sub(low)) & (high.wrapping_add(HIGH_BITS).wrapping_sub(ascii))
}

/// The high bit of each byte of `word` that continues a name: an ASCII letter,
/// digit, or `_`.
#[inline]
const fn name_bytes(word: u64) -> u64 {
    let ascii = word & LOW_BITS;
    let digits = bytes_in_range(ascii, 0x3030_3030_3030_3030, 0x3939_3939_3939_3939);
    let folded = ascii | 0x2020_2020_2020_2020;
    let letters = bytes_in_range(folded, 0x6161_6161_6161_6161, 0x7A7A_7A7A_7A7A_7A7A);
    let underscores = !(ascii ^ 0x5F5F_5F5F_5F5F_5F5F).wrapping_add(LOW_BITS);
    (digits | letters | underscores) & !word & HIGH_BITS
}

/// How many bytes of name open `bytes`. Every byte it counts is ASCII, which
/// [`Cursor::eat_name`](crate::cursor::Cursor::eat_name) relies on.
#[inline]
pub(crate) fn name_length(bytes: &[u8]) -> usize {
    let mut remaining = bytes;
    while let Some((&chunk, after_chunk)) = remaining.split_first_chunk::<8>() {
        let stops = !name_bytes(word(chunk)) & HIGH_BITS;
        if stops != 0 {
            return bytes.len() - remaining.len() + first_answer(stops);
        }
        remaining = after_chunk;
    }
    bytes.len() - remaining.len() + find_match(remaining, &NOT_NAME_CONTINUE)
}

/// How many bytes open `bytes` before a `quote`, a `\`, or a line break, the
/// bytes that end a run of a short string's body.
///
/// Most strings close within 32 bytes, which words scan without a call, and
/// `memchr` takes any longer run.
#[inline]
pub(crate) fn short_string_length(bytes: &[u8], quote: u8) -> usize {
    const WORD_SCAN_LENGTH: usize = 32;
    let mut length = 0;
    while length < WORD_SCAN_LENGTH
        && let Some(&chunk) = bytes[length..].first_chunk::<8>()
    {
        let word = word(chunk);
        let stops = bytes_equal(word, quote)
            | bytes_equal(word, b'\\')
            | bytes_equal(word, b'\n')
            | bytes_equal(word, b'\r');
        if stops != 0 {
            return length + first_answer(stops);
        }
        length += 8;
    }
    let rest = &bytes[length..];
    // Stopping at LF bounds an unterminated string to its line, and the
    // second scan finds any CR before that stop.
    let end = memchr::memchr3(quote, b'\\', b'\n', rest).unwrap_or(rest.len());
    let (line, _) = rest.split_at(end);
    length + memchr::memchr(b'\r', line).unwrap_or(end)
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

    /// A borrow or carry between bytes would let one byte change its
    /// neighbor's answer, so every pair of bytes is tried side by side.
    #[test]
    fn word_tests_answer_for_each_byte_whatever_its_neighbor() {
        for low in 0..=u8::MAX {
            for high in 0..=u8::MAX {
                for position in 0..7 {
                    let mut bytes = [b'a'; 8];
                    bytes[position] = low;
                    bytes[position + 1] = high;
                    let names = name_bytes(word(bytes));
                    let quotes = bytes_equal(word(bytes), b'"');
                    for (i, &b) in bytes.iter().enumerate() {
                        let bit = 0x80 << (8 * i);
                        let is_name = b.is_ascii_alphanumeric() || b == b'_';
                        assert_eq!(names & bit != 0, is_name, "{bytes:?} at {i}");
                        assert_eq!(quotes & bit != 0, b == b'"', "{bytes:?} at {i}");
                    }
                }
            }
        }
    }

    #[test]
    fn name_length_stops_at_the_first_byte_no_name_continues_with() {
        for length in 0..=40 {
            let name = vec![b'x'; length];
            assert_eq!(name_length(&name), length, "no stop in {length}");
            for stop in 0..length {
                for stop_byte in [b' ', b'.', b'(', 0x80, b'^', b'`', b'@', b'[', b'{', b'/', b':']
                {
                    let mut bytes = name.clone();
                    bytes[stop] = stop_byte;
                    assert_eq!(name_length(&bytes), stop, "{stop_byte} at {stop} of {length}");
                }
            }
        }
    }

    #[test]
    fn short_string_length_stops_at_the_first_quote_escape_or_line_break() {
        for length in 0..=80 {
            let body = vec![b'\''; length];
            assert_eq!(short_string_length(&body, b'"'), length, "no stop in {length}");
            for stop in 0..length {
                for &stop_byte in b"\"\\\n\r" {
                    let mut bytes = body.clone();
                    bytes[stop] = stop_byte;
                    assert_eq!(
                        short_string_length(&bytes, b'"'),
                        stop,
                        "{stop_byte} at {stop} of {length}"
                    );
                    bytes[stop..].fill(stop_byte);
                    assert_eq!(short_string_length(&bytes, b'"'), stop, "run of {stop_byte}");
                }
            }
            // A CR past an LF is past the line, and one before it ends the run.
            for (first, second) in [(b'\n', b'\r'), (b'\r', b'\n')] {
                let mut bytes = body.clone();
                bytes.extend([first, b'x', second]);
                assert_eq!(short_string_length(&bytes, b'"'), length, "{first} then {second}");
            }
        }
    }
}
