use luck_simd::{Avx2, Neon, Vec128, Vec256};
use luck_token::LuaVersion;

pub(crate) const WINDOW_BYTES: usize = 64;

/// A window's bytes and 8 more, which the keyword lookup of a name that ends
/// near the window's end reads past it.
pub(crate) type WindowBytes = [u8; WINDOW_BYTES + 8];

/// The classes of a window's bytes, a bit per byte, byte 0's lowest. A byte
/// past the end of the source is zero, which is in no class.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Planes {
    pub(crate) whitespace: u64,
    /// ASCII letters, digits, and `_`.
    pub(crate) name: u64,
    pub(crate) digit: u64,
    /// LF, and CR where it ends a line.
    pub(crate) line_break: u64,
    /// The bytes a one-byte token is spelled with in this version.
    pub(crate) punct: u64,
    /// The bytes that can make one punctuation byte part of a longer token
    /// when they follow it: `-`, `.`, `/`, `:`, `<`, `=`, `>`, and `[`.
    pub(crate) continuation: u64,
    pub(crate) dot: u64,
}

/// A SIMD feature set's way to classify a window.
pub(crate) trait Classify: Copy {
    fn classify(self, bytes: &[u8; WINDOW_BYTES], tables: &Tables) -> Planes;
}

/// Each class is a rectangle in the table of ASCII bytes, the rows its high
/// nibbles name by the columns its low nibbles do, so looking up a byte's low
/// nibble in one table and its high nibble in another and AND-ing them sets
/// exactly its classes' bits. A class that spans rows with different columns
/// needs a bit per row, as names take four. Each table is repeated twice, for
/// AVX2's lookup in each 16-byte half, and no high nibble of a non-ASCII byte
/// has an entry.
#[derive(Debug)]
pub(crate) struct Tables {
    word_low: [u8; 32],
    word_high: [u8; 32],
    punct_low: [u8; 32],
    punct_high: [u8; 32],
    has_cr_line_break: bool,
}

const DIGIT: u8 = 1 << 0;
const A_TO_O: u8 = 1 << 1;
const UPPER_P_TO_Z_OR_UNDERSCORE: u8 = 1 << 2;
const LOWER_P_TO_Z: u8 = 1 << 3;
const CONTROL_WHITESPACE: u8 = 1 << 4;
const SPACE: u8 = 1 << 5;
/// The top bit, which a movemask reads with no compare.
const CR: u8 = 1 << 7;

const NAME: u8 = DIGIT | A_TO_O | UPPER_P_TO_Z_OR_UNDERSCORE | LOWER_P_TO_Z;
const WHITESPACE: u8 = CONTROL_WHITESPACE | SPACE;

const PUNCT_ROW_2: u8 = 1 << 0;
const PUNCT_ROW_3: u8 = 1 << 1;
const PUNCT_ROW_4: u8 = 1 << 2;
const PUNCT_ROW_5: u8 = 1 << 3;
const PUNCT_ROW_7: u8 = 1 << 4;
const CONTINUATION_ROW_2: u8 = 1 << 5;
const CONTINUATION_ROW_3: u8 = 1 << 6;
const CONTINUATION_ROW_5: u8 = 1 << 7;

const PUNCT: u8 = PUNCT_ROW_2 | PUNCT_ROW_3 | PUNCT_ROW_4 | PUNCT_ROW_5 | PUNCT_ROW_7;
const CONTINUATION: u8 = CONTINUATION_ROW_2 | CONTINUATION_ROW_3 | CONTINUATION_ROW_5;

const fn word_bits(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => DIGIT,
        b'A'..=b'O' | b'a'..=b'o' => A_TO_O,
        b'P'..=b'Z' | b'_' => UPPER_P_TO_Z_OR_UNDERSCORE,
        b'p'..=b'z' => LOWER_P_TO_Z,
        b'\r' => CONTROL_WHITESPACE | CR,
        b'\t' | b'\n' | 0x0B | 0x0C => CONTROL_WHITESPACE,
        b' ' => SPACE,
        _ => 0,
    }
}

/// Whether `version`'s lexer reads `byte` alone as a one-byte token when
/// no continuation follows it, as `lex_operator` and `next_token` do.
pub(crate) const fn is_punct(byte: u8, version: LuaVersion) -> bool {
    match byte {
        b'#' | b'%' | b'(' | b')' | b'*' | b'+' | b',' | b'-' | b'.' | b'/' | b':' | b';'
        | b'<' | b'=' | b'>' | b'[' | b']' | b'^' | b'{' | b'}' => true,
        b'&' | b'|' => version.has_bitwise_ops() || version.has_type_syntax(),
        b'~' => version.has_bitwise_ops(),
        b'?' => version.has_type_syntax(),
        b'@' => version.has_function_attributes(),
        _ => false,
    }
}

const fn punct_bits(byte: u8, version: LuaVersion) -> u8 {
    let punct = if is_punct(byte, version) {
        match byte >> 4 {
            2 => PUNCT_ROW_2,
            3 => PUNCT_ROW_3,
            4 => PUNCT_ROW_4,
            5 => PUNCT_ROW_5,
            _ => PUNCT_ROW_7,
        }
    } else {
        0
    };
    let continuation = match byte {
        b'-' | b'.' | b'/' => CONTINUATION_ROW_2,
        b':' | b'<' | b'=' | b'>' => CONTINUATION_ROW_3,
        b'[' => CONTINUATION_ROW_5,
        _ => 0,
    };
    punct | continuation
}

impl Tables {
    pub(crate) const fn of(version: LuaVersion) -> &'static Self {
        &TABLES[version as usize]
    }

    const fn build(version: LuaVersion) -> Self {
        let mut tables = Self {
            word_low: [0; 32],
            word_high: [0; 32],
            punct_low: [0; 32],
            punct_high: [0; 32],
            has_cr_line_break: version.has_lfcr_line_break(),
        };
        let mut byte = 0;
        while byte < 0x80 {
            let (low, high) = ((byte & 0x0F) as usize, (byte >> 4) as usize);
            let word = word_bits(byte);
            let punct = punct_bits(byte, version);
            tables.word_low[low] |= word;
            tables.word_low[low + 16] |= word;
            tables.word_high[high] |= word;
            tables.word_high[high + 16] |= word;
            tables.punct_low[low] |= punct;
            tables.punct_low[low + 16] |= punct;
            tables.punct_high[high] |= punct;
            tables.punct_high[high + 16] |= punct;
            byte += 1;
        }
        tables
    }
}

/// In [`LuaVersion::ALL`]'s order.
static TABLES: [Tables; 6] = [
    Tables::build(LuaVersion::Lua51),
    Tables::build(LuaVersion::Lua52),
    Tables::build(LuaVersion::Lua53),
    Tables::build(LuaVersion::Lua54),
    Tables::build(LuaVersion::Lua55),
    Tables::build(LuaVersion::Luau),
];

impl Classify for Avx2 {
    #[expect(clippy::inline_always, reason = "inlined into `Avx2::run`, which enables AVX2")]
    #[inline(always)]
    fn classify(self, bytes: &[u8; WINDOW_BYTES], tables: &Tables) -> Planes {
        let (halves, _) = bytes.as_chunks::<32>();
        let low = classify_avx2_half(&halves[0], tables, self);
        let high = classify_avx2_half(&halves[1], tables, self);
        let join = |index: usize| u64::from(low[index]) | (u64::from(high[index]) << 32);
        let line_break =
            if tables.has_cr_line_break { join(PLANE_LF) | join(PLANE_CR) } else { join(PLANE_LF) };
        Planes {
            whitespace: join(PLANE_WHITESPACE),
            name: join(PLANE_NAME),
            digit: join(PLANE_DIGIT),
            line_break,
            punct: join(PLANE_PUNCT),
            continuation: join(PLANE_CONTINUATION),
            dot: join(PLANE_DOT),
        }
    }
}

const PLANE_WHITESPACE: usize = 0;
const PLANE_NAME: usize = 1;
const PLANE_DIGIT: usize = 2;
const PLANE_LF: usize = 3;
const PLANE_CR: usize = 4;
const PLANE_PUNCT: usize = 5;
const PLANE_CONTINUATION: usize = 6;
const PLANE_DOT: usize = 7;
const PLANES: usize = 8;

// The AVX2 kernel spells out each half, and builds no array with `map`,
// which is std code compiled without AVX2 that could leave each vector
// method a call.
#[expect(clippy::inline_always, reason = "inlined into `Avx2::run`, which enables AVX2")]
#[inline(always)]
fn classify_avx2_half(half: &[u8; 32], tables: &Tables, cpu: Avx2) -> [u32; PLANES] {
    let v = Vec256::read(half, cpu);
    let high_nibbles = v.high_nibbles();
    // A byte with its top bit set looks up 0 in a low table, so a non-ASCII
    // byte has no class.
    let word = Vec256::read(&tables.word_low, cpu).lookup(v)
        & Vec256::read(&tables.word_high, cpu).lookup(high_nibbles);
    let punct = Vec256::read(&tables.punct_low, cpu).lookup(v)
        & Vec256::read(&tables.punct_high, cpu).lookup(high_nibbles);
    let zero = Vec256::splat(0, cpu);
    let has =
        |classes: Vec256, bits: u8| (classes & Vec256::splat(bits, cpu)).gt_i8(zero).movemask();
    [
        has(word, WHITESPACE),
        has(word, NAME),
        has(word, DIGIT),
        v.eq(Vec256::splat(b'\n', cpu)).movemask(),
        word.movemask(),
        has(punct, PUNCT),
        !(punct & Vec256::splat(CONTINUATION, cpu)).eq(zero).movemask(),
        v.eq(Vec256::splat(b'.', cpu)).movemask(),
    ]
}

impl Classify for Neon {
    #[expect(clippy::inline_always, reason = "inlined into the lexer's batch loop")]
    #[inline(always)]
    fn classify(self, bytes: &[u8; WINDOW_BYTES], tables: &Tables) -> Planes {
        let (quarters, _) = bytes.as_chunks::<16>();
        let [first, second, third, fourth] =
            [&quarters[0], &quarters[1], &quarters[2], &quarters[3]]
                .map(|quarter| classify_neon_quarter(quarter, tables, self));
        let plane = |index: usize| {
            Vec128::bitmask([first[index], second[index], third[index], fourth[index]])
        };
        let line_break = if tables.has_cr_line_break {
            plane(PLANE_LF) | plane(PLANE_CR)
        } else {
            plane(PLANE_LF)
        };
        Planes {
            whitespace: plane(PLANE_WHITESPACE),
            name: plane(PLANE_NAME),
            digit: plane(PLANE_DIGIT),
            line_break,
            punct: plane(PLANE_PUNCT),
            continuation: plane(PLANE_CONTINUATION),
            dot: plane(PLANE_DOT),
        }
    }
}

#[expect(clippy::inline_always, reason = "inlined into the lexer's batch loop")]
#[inline(always)]
fn classify_neon_quarter(quarter: &[u8; 16], tables: &Tables, cpu: Neon) -> [Vec128; PLANES] {
    let v = Vec128::read(quarter, cpu);
    let low_nibbles = v & Vec128::splat(0x0F, cpu);
    let high_nibbles = v.high_nibbles();
    let table = |bytes: &[u8; 32]| {
        let (first, _) = bytes.split_first_chunk::<16>().expect("32 bytes");
        Vec128::read(first, cpu)
    };
    // A non-ASCII byte's high nibble has no entry, so it has no class.
    let word =
        table(&tables.word_low).lookup(low_nibbles) & table(&tables.word_high).lookup(high_nibbles);
    let punct = table(&tables.punct_low).lookup(low_nibbles)
        & table(&tables.punct_high).lookup(high_nibbles);
    let has = |classes: Vec128, bits: u8| classes.test(Vec128::splat(bits, cpu));
    [
        has(word, WHITESPACE),
        has(word, NAME),
        has(word, DIGIT),
        v.eq(Vec128::splat(b'\n', cpu)),
        has(word, CR),
        has(punct, PUNCT),
        has(punct, CONTINUATION),
        v.eq(Vec128::splat(b'.', cpu)),
    ]
}

#[cfg(test)]
mod tests {
    use luck_simd::DynCpu;

    use super::*;

    fn classify(bytes: &[u8; WINDOW_BYTES], version: LuaVersion, cpu: DynCpu) -> Option<Planes> {
        let tables = Tables::of(version);
        match cpu {
            DynCpu::Avx2(cpu) => Some(cpu.run(|cpu| cpu.classify(bytes, tables))),
            DynCpu::Neon(cpu) => Some(cpu.classify(bytes, tables)),
            DynCpu::Generic(_) => None,
        }
    }

    /// Every byte value at every position of a window, beside every other,
    /// so a kernel that mixed up lanes or halves would show.
    #[test]
    fn classify_matches_each_byte_class_at_every_position() {
        for cpu in DynCpu::available() {
            for version in LuaVersion::ALL {
                for rotation in 0..WINDOW_BYTES {
                    for block in 0..4 {
                        let bytes: [u8; WINDOW_BYTES] = std::array::from_fn(|i| {
                            let value = (i + rotation) % WINDOW_BYTES + block * WINDOW_BYTES;
                            u8::try_from(value).expect("under 256")
                        });
                        let Some(planes) = classify(&bytes, version, cpu) else {
                            continue;
                        };
                        for (i, &b) in bytes.iter().enumerate() {
                            let bit = |plane: u64| (plane >> i) & 1 == 1;
                            let context = format!("{cpu:?} {version:?} {b:#04x}");
                            let is_whitespace =
                                matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C);
                            let is_line_break =
                                b == b'\n' || (version.has_lfcr_line_break() && b == b'\r');
                            let is_continuation =
                                matches!(b, b'-' | b'.' | b'/' | b':' | b'<' | b'=' | b'>' | b'[');
                            assert_eq!(bit(planes.whitespace), is_whitespace, "{context}");
                            assert_eq!(
                                bit(planes.name),
                                b.is_ascii_alphanumeric() || b == b'_',
                                "{context}"
                            );
                            assert_eq!(bit(planes.digit), b.is_ascii_digit(), "{context}");
                            assert_eq!(bit(planes.line_break), is_line_break, "{context}");
                            assert_eq!(bit(planes.punct), is_punct(b, version), "{context}");
                            assert_eq!(bit(planes.continuation), is_continuation, "{context}");
                            assert_eq!(bit(planes.dot), b == b'.', "{context}");
                        }
                    }
                }
            }
        }
    }
}
