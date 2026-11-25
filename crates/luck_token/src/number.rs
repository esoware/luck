use std::fmt::{self, Write as _};
use std::iter;

#[expect(
    clippy::cast_precision_loss,
    reason = "the C cast of an unsigned 64-bit integer to double, which rounds to the nearest"
)]
const fn unsigned_to_f64(value: u64) -> f64 {
    value as f64
}

/// 2^64, which Luau's `strtoull` saturates a hex or binary numeral past 64
/// bits at.
const TWO_TO_64: f64 = 18_446_744_073_709_551_616.0;

/// What a numeral means: its value, and whether that is the integer it
/// spells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Numeral {
    pub value: f64,
    /// False for a numeral spelled as an integer that doesn't read as that
    /// integer: one a double can't hold, or hex or binary digits past 64
    /// bits, which saturate. True for a numeral spelled as a float.
    pub is_exact: bool,
}

/// The number `text` spells, as Luau's parser converts it.
///
/// `text` is a numeral the lexer took, and any other text reads as some
/// number. Every `_` is dropped first. A numeral reads as the nearest double,
/// but hex and binary digits past 64 bits saturate at 2^64.
pub fn read_numeral(text: &str) -> Numeral {
    if let Some(integer) = short_decimal_integer(text.as_bytes()) {
        return Numeral { value: unsigned_to_f64(integer), is_exact: true };
    }
    read_other_numeral(text)
}

/// The integer a decimal of at most 15 digits spells, as most numerals are,
/// which a double holds exactly.
#[inline]
fn short_decimal_integer(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() || digits.len() > 15 {
        return None;
    }
    digits.iter().try_fold(0, |value, &digit| {
        let digit_value = digit.wrapping_sub(b'0');
        (digit_value <= 9).then(|| value * 10 + u64::from(digit_value))
    })
}

/// Out of line, so the short path inlines into its caller alone.
#[cold]
#[inline(never)]
fn read_other_numeral(text: &str) -> Numeral {
    without_separators(text, |separated| match separated.as_bytes() {
        [b'0', b'x' | b'X', digits @ ..] => read_radix(digits, 16),
        [b'0', b'b' | b'B', digits @ ..] => read_radix(digits, 2),
        _ => read_decimal(separated),
    })
}

/// What `read` returns for `text` without its `_`s, copied to the stack
/// unless it's longer than nearly every numeral is.
fn without_separators<T, Read>(text: &str, read: Read) -> T
where
    Read: FnOnce(&str) -> T,
{
    if !text.contains('_') {
        return read(text);
    }
    let mut short = [0; 64];
    if text.len() > short.len() {
        return read(&text.replace('_', ""));
    }
    let mut length = 0;
    for (slot, b) in short.iter_mut().zip(text.bytes().filter(|&b| b != b'_')) {
        *slot = b;
        length += 1;
    }
    read(str::from_utf8(&short[..length]).expect("dropping ASCII `_`s leaves UTF-8"))
}

fn read_decimal(text: &str) -> Numeral {
    let value = text.parse::<f64>().unwrap_or_default();
    let is_float = text.contains(['.', 'e', 'E']);
    let is_exact = is_float || decimal_integer(text.as_bytes()).is_some_and(is_exact_in_double);
    Numeral { value, is_exact }
}

/// `digits` follow the `0x` or `0b` of `radix`, 16 or 2.
fn read_radix(digits: &[u8], radix: u32) -> Numeral {
    match digits_value(digits, radix) {
        Some(integer) => Numeral {
            value: unsigned_to_f64(integer),
            is_exact: is_exact_in_double(u128::from(integer)),
        },
        None => Numeral { value: TWO_TO_64, is_exact: false },
    }
}

/// Whether a double holds `value` exactly, meaning its significant bits fit
/// the 53-bit significand.
const fn is_exact_in_double(value: u128) -> bool {
    // `wrapping_shr` takes zero's 128 trailing zeros as a shift of 0.
    value.wrapping_shr(value.trailing_zeros()) < (1 << 53)
}

/// The integer decimal `digits` spell, or `None` past `u128`.
fn decimal_integer(digits: &[u8]) -> Option<u128> {
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    digits.iter().try_fold(0u128, |value, &digit| {
        value.checked_mul(10)?.checked_add(u128::from(digit - b'0'))
    })
}

/// The integer `digits` spell in `radix`, 2 or 16, or `None` past 64 bits.
fn digits_value(digits: &[u8], radix: u32) -> Option<u64> {
    digits.iter().try_fold(0u64, |value, &digit| {
        let digit_value = char::from(digit).to_digit(radix).unwrap_or_default();
        value.checked_mul(u64::from(radix))?.checked_add(u64::from(digit_value))
    })
}

/// The integer an `i` literal such as `0xFFi` spells, its `_`s dropped.
///
/// `None` when it's past 64 bits: a decimal must fit an `i64`, and hex and
/// binary digits a `u64`, as a bit pattern.
pub fn read_integer(text: &str) -> Option<i64> {
    without_separators(text, |separated| {
        let body = separated.strip_suffix('i').unwrap_or(separated);
        match body.as_bytes() {
            [b'0', b'x' | b'X', digits @ ..] => digits_value(digits, 16).map(u64::cast_signed),
            [b'0', b'b' | b'B', digits @ ..] => digits_value(digits, 2).map(u64::cast_signed),
            digits => i64::try_from(decimal_integer(digits)?).ok(),
        }
    })
}

/// A numeral's spelling, on the stack, since none takes more than 32 bytes.
#[derive(Clone, Copy)]
pub struct NumberSpelling {
    bytes: [u8; 32],
    length: usize,
}

impl NumberSpelling {
    const EMPTY: Self = Self { bytes: [0; 32], length: 0 };

    #[expect(clippy::missing_panics_doc, reason = "a numeral's spelling is ASCII")]
    pub fn as_str(&self) -> &str {
        str::from_utf8(self.as_bytes()).expect("a numeral's spelling is ASCII")
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    const fn push(&mut self, byte: u8) {
        self.bytes[self.length] = byte;
        self.length += 1;
    }

    fn push_all(&mut self, bytes: impl IntoIterator<Item = u8>) {
        for byte in bytes {
            self.push(byte);
        }
    }
}

impl fmt::Write for NumberSpelling {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.length + s.len();
        self.bytes.get_mut(self.length..end).ok_or(fmt::Error)?.copy_from_slice(s.as_bytes());
        self.length = end;
        Ok(())
    }
}

impl fmt::Debug for NumberSpelling {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

/// The shortest numeral that reads back as `value`.
///
/// `value` is one a literal holds: neither negative nor NaN. Infinity is
/// `1e999`, which overflows to it.
///
/// # Panics
///
/// Never, since `{:e}` always writes a mantissa and an integer exponent.
pub fn spell_number(value: f64) -> NumberSpelling {
    let mut spelling = NumberSpelling::EMPTY;
    if value.is_nan() {
        spelling.push_all(*b"(0/0)");
        return spelling;
    }
    let magnitude = value.abs();
    if magnitude == 0.0 {
        spelling.push(b'0');
        return spelling;
    }
    if magnitude.is_infinite() {
        spelling.push_all(*b"1e999");
        return spelling;
    }
    // `{:e}` gives the shortest digits that read back as the value, as
    // `d.ddde-x`.
    let mut scientific = NumberSpelling::EMPTY;
    let _ = write!(scientific, "{magnitude:e}");
    let (mantissa, exponent) =
        scientific.as_str().split_once('e').expect("scientific notation has an exponent");
    let exponent: i64 = exponent.parse().expect("scientific notation has an integer exponent");
    let mut digit_text = NumberSpelling::EMPTY;
    digit_text.push_all(mantissa.bytes().filter(|&b| b != b'.'));
    let digits = digit_text.as_bytes();
    let layout = shortest_layout(magnitude, digits, exponent);
    write_layout(layout, magnitude, digits, exponent, &mut spelling);
    spelling
}

/// The shortest integer literal for `value`, which is in hex when it's
/// negative.
pub fn spell_integer(value: i64) -> NumberSpelling {
    let mut spelling = NumberSpelling::EMPTY;
    let bits = value.cast_unsigned();
    let decimal_length = (value >= 0).then(|| decimal_length(bits));
    // Plain digits win a tie.
    let _ = if decimal_length.is_some_and(|length| length <= hex_length(bits)) {
        write!(spelling, "{value}i")
    } else {
        write!(spelling, "0x{bits:x}i")
    };
    spelling
}

/// How many digits `value` takes in decimal.
fn decimal_length(value: u64) -> usize {
    iter::successors(Some(value), |&rest| (rest >= 10).then_some(rest / 10)).count()
}

/// How many bytes `value` takes as a hex numeral, `0x` included.
fn hex_length(value: u64) -> usize {
    let digit_count = (64 - value.leading_zeros()).div_ceil(4).max(1);
    digit_count as usize + 2
}

/// How many bytes `exponent` takes in decimal, its sign included.
fn exponent_length(exponent: i64) -> usize {
    decimal_length(exponent.unsigned_abs()) + usize::from(exponent < 0)
}

/// The ways [`spell_number`] can lay out a number's digits.
#[derive(Clone, Copy)]
enum Layout {
    /// No exponent: `12300`, `1.23`, or `.00123`.
    Plain,
    /// The first `n` digits before the point, then an exponent: `123e2`,
    /// `1.23e4`, or `.123e5`.
    Exponent(usize),
    /// A hex integer.
    Hex,
}

/// The shortest layout of `digits`, the first worth `10^exponent`. Earlier
/// layouts win a tie: plain, then exponents with the most digits before the
/// point, then hex.
fn shortest_layout(value: f64, digits: &[u8], exponent: i64) -> Layout {
    let count = digits.len();
    let count_wide = i64::try_from(count).expect("a double has at most 17 digits");
    let last_exponent = exponent - count_wide + 1;
    let to_length = |wide: i64| usize::try_from(wide).expect("each branch checks the sign");
    let plain = if last_exponent >= 0 {
        count + to_length(last_exponent)
    } else if exponent >= 0 {
        count + 1
    } else {
        count + to_length(-exponent)
    };
    let mut best = (Layout::Plain, plain);
    for before_point in (0..=count).rev() {
        let shifted =
            exponent + 1 - i64::try_from(before_point).expect("a double has at most 17 digits");
        let length = count + usize::from(before_point < count) + 1 + exponent_length(shifted);
        if length < best.1 {
            best = (Layout::Exponent(before_point), length);
        }
    }
    if let Some(integer) = float_to_u64(value)
        && hex_length(integer) < best.1
    {
        return Layout::Hex;
    }
    best.0
}

/// `value` as an integer, when it is one below 2^64.
fn float_to_u64(value: f64) -> Option<u64> {
    let bits = value.to_bits();
    let biased = i64::try_from(bits >> 52).expect("a sign and an exponent fit 12 bits");
    let significand = (bits & 0x000F_FFFF_FFFF_FFFF) | (1 << 52);
    let shift = biased - 1075;
    if shift >= 0 {
        (shift < 12).then(|| significand << shift)
    } else {
        let right = shift.unsigned_abs();
        (right < 53 && u64::from(significand.trailing_zeros()) >= right)
            .then(|| significand >> right)
    }
}

fn write_layout(
    layout: Layout,
    value: f64,
    digits: &[u8],
    exponent: i64,
    spelling: &mut NumberSpelling,
) {
    let count = i64::try_from(digits.len()).expect("a double has at most 17 digits");
    match layout {
        Layout::Plain => {
            if let Ok(zeros) = usize::try_from(exponent - count + 1) {
                spelling.push_all(digits.iter().copied());
                spelling.push_all(iter::repeat_n(b'0', zeros));
            } else if let Ok(whole) = usize::try_from(exponent + 1) {
                let (whole_digits, fraction_digits) = digits.split_at(whole);
                spelling.push_all(whole_digits.iter().copied());
                spelling.push(b'.');
                spelling.push_all(fraction_digits.iter().copied());
            } else {
                spelling.push(b'.');
                let zeros = usize::try_from(-exponent - 1).expect("the exponent is below -1");
                spelling.push_all(iter::repeat_n(b'0', zeros));
                spelling.push_all(digits.iter().copied());
            }
        }
        Layout::Exponent(before_point) => {
            let (whole_digits, fraction_digits) = digits.split_at(before_point);
            spelling.push_all(whole_digits.iter().copied());
            if !fraction_digits.is_empty() {
                spelling.push(b'.');
                spelling.push_all(fraction_digits.iter().copied());
            }
            let shifted =
                exponent + 1 - i64::try_from(before_point).expect("a double has at most 17 digits");
            let _ = write!(spelling, "e{shifted}");
        }
        Layout::Hex => {
            let integer = float_to_u64(value).expect("only an integer takes the hex layout");
            let _ = write!(spelling, "0x{integer:x}");
        }
    }
}
