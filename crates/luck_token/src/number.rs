use std::fmt::{self, Write as _};

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

/// Every integer below 2^53 is a double, which no other integer reads as.
const EXACT_INTEGER_LIMIT: u64 = 1 << 53;

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
///
/// Luau drops every `_` before it reads a numeral, so `0_x1` is hex.
#[cold]
#[inline(never)]
fn read_other_numeral(text: &str) -> Numeral {
    let bytes = text.as_bytes();
    let mut significant =
        bytes.iter().enumerate().filter(|&(_, &b)| b != b'_').map(|(index, &b)| (index, b));
    match (significant.next(), significant.next()) {
        (Some((_, b'0')), Some((marker, b'x' | b'X'))) => read_radix(&bytes[marker + 1..], 4),
        (Some((_, b'0')), Some((marker, b'b' | b'B'))) => read_radix(&bytes[marker + 1..], 1),
        _ => without_separators(text, read_decimal),
    }
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
    let is_float = text.bytes().any(|b| matches!(b, b'.' | b'e' | b'E'));
    let is_exact = is_float || decimal_integer(text.as_bytes()).is_some_and(is_exact_in_double);
    Numeral { value, is_exact }
}

/// `digits` follow the `0x` or `0b` of a radix of `bits_per_digit` bits, 4
/// or 1, and may hold `_`s, which it skips.
fn read_radix(digits: &[u8], bits_per_digit: u32) -> Numeral {
    let radix = 1 << bits_per_digit;
    let mut value = 0u64;
    for &digit in digits {
        if digit == b'_' {
            continue;
        }
        // Another digit would shift a set bit out of the 64.
        if value >> (64 - bits_per_digit) != 0 {
            return Numeral { value: TWO_TO_64, is_exact: false };
        }
        let digit_value = char::from(digit).to_digit(radix).unwrap_or_default();
        value = (value << bits_per_digit) | u64::from(digit_value);
    }
    Numeral { value: unsigned_to_f64(value), is_exact: is_exact_in_double(u128::from(value)) }
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

/// A numeral's spelling, on the stack, since none takes more than 32 bytes.
#[derive(Clone, Copy)]
pub struct NumberSpelling {
    bytes: [u8; 32],
    length: u8,
}

impl NumberSpelling {
    const EMPTY: Self = Self { bytes: [0; 32], length: 0 };

    #[expect(clippy::missing_panics_doc, reason = "a numeral's spelling is ASCII")]
    pub fn as_str(&self) -> &str {
        str::from_utf8(self.as_bytes()).expect("a numeral's spelling is ASCII")
    }

    /// The spelling's bytes, which are ASCII.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    /// How many bytes the spelling takes.
    pub fn length(&self) -> usize {
        usize::from(self.length)
    }

    fn push(&mut self, byte: u8) {
        self.bytes[usize::from(self.length)] = byte;
        self.length += 1;
    }

    fn push_all(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.push(byte);
        }
    }

    /// Appends `value` in decimal, which takes `digit_count` digits.
    fn push_digits(&mut self, value: u64, digit_count: usize) {
        let start = usize::from(self.length);
        let end = start + digit_count;
        let mut rest = value;
        for slot in self.bytes[start..end].iter_mut().rev() {
            *slot = b'0' + (rest % 10) as u8;
            rest /= 10;
        }
        self.length = u8::try_from(end).expect("a spelling takes at most 32 bytes");
    }

    fn push_decimal(&mut self, value: u64) {
        self.push_digits(value, decimal_length(value));
    }

    /// Appends `value` as a hex numeral, `0x` included, in lowercase.
    fn push_hex(&mut self, value: u64) {
        self.push_all(b"0x");
        for place in (0..hex_length(value) - 2).rev() {
            let nibble = (value >> (4 * place)) & 0xF;
            self.push(b"0123456789abcdef"[usize::try_from(nibble).unwrap_or_default()]);
        }
    }
}

impl fmt::Write for NumberSpelling {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let start = usize::from(self.length);
        let end = start + s.len();
        self.bytes.get_mut(start..end).ok_or(fmt::Error)?.copy_from_slice(s.as_bytes());
        self.length = u8::try_from(end).map_err(|_| fmt::Error)?;
        Ok(())
    }
}

impl fmt::Debug for NumberSpelling {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

/// The shortest numeral that reads back as `value`, which `raw`, when it's
/// given, is a numeral that reads as.
///
/// `value` is one a literal holds: neither negative nor NaN. Infinity is
/// `1e999`, which overflows to it.
///
/// Most values take no search for their shortest digits. An integer below
/// 2^53 spells shortest with its own, and a decimal `raw` of at most 15
/// significant digits holds those of a value a double holds to full
/// precision, as no other decimal of 15 digits or fewer reads as the same
/// double.
pub fn spell_number(value: f64, raw: Option<&str>) -> NumberSpelling {
    let magnitude = value.abs();
    if let Some(integer) =
        float_to_u64(magnitude).filter(|integer| (1..EXACT_INTEGER_LIMIT).contains(integer))
    {
        return spell_exact_integer(integer);
    }
    let mut spelling = NumberSpelling::EMPTY;
    if value.is_nan() {
        spelling.push_all(b"(0/0)");
    } else if magnitude == 0.0 {
        spelling.push(b'0');
    } else if magnitude.is_infinite() {
        spelling.push_all(b"1e999");
    } else {
        let digits = raw
            .filter(|_| magnitude.is_normal())
            .and_then(|raw| written_digits(raw.as_bytes()))
            .unwrap_or_else(|| searched_digits(magnitude));
        let layout = shortest_layout(magnitude, &digits);
        write_layout(layout, magnitude, &digits, &mut spelling);
    }
    spelling
}

/// The shortest spelling of `integer`, which is below 2^53 and above 0, as
/// [`spell_number`] lays out the shortest digits it searches for.
///
/// Its own digits, without their trailing zeros, are the shortest that read
/// back as it: any other integer as short is another double, and a fraction
/// that rounds to it needs a digit past its last.
fn spell_exact_integer(integer: u64) -> NumberSpelling {
    let mut significand = integer;
    let mut zeros = 0;
    while significand.is_multiple_of(10) {
        significand /= 10;
        zeros += 1;
    }
    let digit_count = decimal_length(significand);
    let plain = digit_count + zeros;
    // Below 2^53, it has at most 15 trailing zeros, one digit or two.
    let exponent = digit_count + 2 + usize::from(zeros >= 10);
    let mut spelling = NumberSpelling::EMPTY;
    if hex_length(integer) < plain.min(exponent) {
        spelling.push_hex(integer);
    } else if exponent < plain {
        spelling.push_digits(significand, digit_count);
        spelling.push(b'e');
        if zeros >= 10 {
            spelling.push(b'1');
        }
        spelling.push(b"0123456789"[zeros % 10]);
    } else {
        spelling.push_digits(integer, plain);
    }
    spelling
}

/// A value's significant digits, which end in no zero, and the power of ten
/// the first is worth.
struct Digits {
    bytes: [u8; 17],
    count: usize,
    exponent: i64,
}

impl Digits {
    const EMPTY: Self = Self { bytes: [0; 17], count: 0, exponent: 0 };

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.count]
    }
}

/// The shortest digits that read back as `magnitude`, finite and above 0.
fn searched_digits(magnitude: f64) -> Digits {
    // `{:e}` gives the shortest digits that read back as the value, as
    // `d.ddde-x`.
    let mut scientific = NumberSpelling::EMPTY;
    let _ = write!(scientific, "{magnitude:e}");
    let (mantissa, exponent) =
        scientific.as_str().split_once('e').expect("scientific notation has an exponent");
    let mut digits = Digits {
        exponent: exponent.parse().expect("scientific notation has an integer exponent"),
        ..Digits::EMPTY
    };
    for b in mantissa.bytes().filter(|&b| b != b'.') {
        digits.bytes[digits.count] = b;
        digits.count += 1;
    }
    digits
}

/// The significant digits of `raw`, a decimal numeral, when there are at
/// most 15 of them, or `None` for any other numeral, such as one with a `_`,
/// which the search spells as well.
fn written_digits(raw: &[u8]) -> Option<Digits> {
    let mantissa_length =
        raw.iter().position(|&b| !(b.is_ascii_digit() || b == b'.')).unwrap_or(raw.len());
    let (mantissa, rest) = raw.split_at(mantissa_length);
    let written_exponent = match rest {
        [] => 0,
        [b'e' | b'E', exponent @ ..] => read_exponent(exponent)?,
        _ => return None,
    };
    let (whole, fraction) = match mantissa.iter().position(|&b| b == b'.') {
        Some(point) => (&mantissa[..point], &mantissa[point + 1..]),
        None => (mantissa, &[][..]),
    };
    if fraction.contains(&b'.') {
        return None;
    }
    // The first and last significant digits, counted across the point.
    let is_significant = |&b: &u8| b != b'0';
    let first = match whole.iter().position(is_significant) {
        Some(index) => index,
        None => whole.len() + fraction.iter().position(is_significant)?,
    };
    let last = match fraction.iter().rposition(is_significant) {
        Some(index) => whole.len() + index,
        None => whole.iter().rposition(is_significant)?,
    };
    let count = last + 1 - first;
    if count > 15 {
        return None;
    }
    let from_whole = whole.get(first..whole.len().min(last + 1)).unwrap_or_default();
    let from_fraction = fraction
        .get(first.saturating_sub(whole.len())..(last + 1).saturating_sub(whole.len()))
        .unwrap_or_default();
    let mut digits = Digits { count, ..Digits::EMPTY };
    let (whole_digits, fraction_digits) = digits.bytes[..count].split_at_mut(from_whole.len());
    whole_digits.copy_from_slice(from_whole);
    fraction_digits.copy_from_slice(from_fraction);
    let first_place = i64::try_from(whole.len()).ok()? - 1 - i64::try_from(first).ok()?;
    digits.exponent = first_place + written_exponent;
    Some(digits)
}

/// The exponent written after an `e`, or `None` for one malformed or long
/// enough to overflow, which the search spells as well.
fn read_exponent(text: &[u8]) -> Option<i64> {
    let (sign, digits) = match text {
        [b'-', digits @ ..] => (-1, digits),
        [b'+', digits @ ..] => (1, digits),
        _ => (1, text),
    };
    if digits.is_empty() || digits.len() > 4 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(sign * digits.iter().fold(0, |value, &digit| value * 10 + i64::from(digit - b'0')))
}

/// How many digits `value` takes in decimal.
fn decimal_length(value: u64) -> usize {
    let mut length = 1;
    let mut rest = value;
    while rest >= 10 {
        rest /= 10;
        length += 1;
    }
    length
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

/// The shortest layout of `digits` of `value`. Earlier layouts win a tie:
/// plain, then exponents with the most digits before the point, then hex.
fn shortest_layout(value: f64, digits: &Digits) -> Layout {
    let count = digits.count;
    let exponent = digits.exponent;
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

fn write_layout(layout: Layout, value: f64, digits: &Digits, spelling: &mut NumberSpelling) {
    let exponent = digits.exponent;
    let digit_bytes = digits.as_bytes();
    let count = i64::try_from(digits.count).expect("a double has at most 17 digits");
    let zeros = [b'0'; 32];
    match layout {
        Layout::Plain => {
            if let Ok(zero_count) = usize::try_from(exponent - count + 1) {
                spelling.push_all(digit_bytes);
                spelling.push_all(&zeros[..zero_count]);
            } else if let Ok(whole) = usize::try_from(exponent + 1) {
                let (whole_digits, fraction_digits) = digit_bytes.split_at(whole);
                spelling.push_all(whole_digits);
                spelling.push(b'.');
                spelling.push_all(fraction_digits);
            } else {
                spelling.push(b'.');
                let zero_count = usize::try_from(-exponent - 1).expect("the exponent is below -1");
                spelling.push_all(&zeros[..zero_count]);
                spelling.push_all(digit_bytes);
            }
        }
        Layout::Exponent(before_point) => {
            let (whole_digits, fraction_digits) = digit_bytes.split_at(before_point);
            spelling.push_all(whole_digits);
            if !fraction_digits.is_empty() {
                spelling.push(b'.');
                spelling.push_all(fraction_digits);
            }
            let shifted =
                exponent + 1 - i64::try_from(before_point).expect("a double has at most 17 digits");
            spelling.push(b'e');
            if shifted < 0 {
                spelling.push(b'-');
            }
            spelling.push_decimal(shifted.unsigned_abs());
        }
        Layout::Hex => {
            spelling.push_hex(float_to_u64(value).expect("only an integer takes the hex layout"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spelling [`spell_number`] lays out from the digits it searches
    /// for, which every shortcut has to match.
    fn searched_spelling(value: f64) -> String {
        let digits = searched_digits(value);
        let mut spelling = NumberSpelling::EMPTY;
        write_layout(shortest_layout(value, &digits), value, &digits, &mut spelling);
        spelling.as_str().to_owned()
    }

    fn random_words() -> impl Iterator<Item = u64> {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        std::iter::repeat_with(move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        })
    }

    #[test]
    fn an_exact_integer_spells_as_its_searched_digits_do() {
        let random = random_words()
            .take(4000)
            .enumerate()
            .map(|(i, word)| (word % EXACT_INTEGER_LIMIT) >> (i % 53));
        let near_powers_of_ten = (0..16).flat_map(|exponent| {
            let power = 10u64.pow(exponent);
            [power, power - 1, power + 1, 7 * power, 15 * power, 123 * power]
        });
        let near_powers_of_two = (0..53).flat_map(|bits| {
            let power = 1u64 << bits;
            [power, power - 1, power + 1, power * 3 / 2]
        });
        let integers = random
            .chain(near_powers_of_ten)
            .chain(near_powers_of_two)
            .chain([EXACT_INTEGER_LIMIT - 1])
            .filter(|&integer| integer != 0 && integer < EXACT_INTEGER_LIMIT);
        for integer in integers {
            let value = unsigned_to_f64(integer);
            assert_eq!(
                spell_exact_integer(integer).as_str(),
                searched_spelling(value),
                "{integer}"
            );
        }
    }

    /// Decimal numerals spelled as floats every way the lexer takes: digits
    /// on either side of a point, leading and trailing zeros, separators, and
    /// exponents both near and far.
    #[test]
    fn a_short_decimal_spells_from_its_written_digits_as_from_its_searched_ones() {
        let mut numerals = Vec::new();
        for (i, word) in random_words().take(6000).enumerate() {
            let significant = (word >> 12) % 10u64.pow(u32::try_from(i % 15 + 1).unwrap());
            let digits = format!("{significant:0width$}", width = i % 18 + 1);
            let point = usize::try_from(word % 20).unwrap().min(digits.len());
            let (whole, fraction) = digits.split_at(point);
            let range: i64 = if i % 2 == 0 { 700 } else { 60 };
            let exponent = i64::try_from((word >> 5) % range.unsigned_abs()).unwrap() - range / 2;
            numerals.push(format!("{whole}.{fraction}"));
            numerals.push(format!("{whole}.{fraction}e{exponent}"));
            numerals.push(format!("{whole}{fraction}E+{}", exponent.unsigned_abs()));
            numerals.push(format!("{whole}_.{fraction}_e-0_{}", exponent.unsigned_abs()));
        }
        numerals.extend(
            [
                ".5",
                "5.",
                "0.1",
                "0.0",
                "00.000e5",
                "007.5",
                "1_0.5_0",
                "1e22",
                "1e23",
                "1e-22",
                "1e-23",
                "999999999999999e22",
                "9007199254740993.0",
                "1e999",
                "1e-400",
                "4.9e-324",
                "2.2250738585072014e-308",
            ]
            .map(String::from),
        );
        for numeral in numerals {
            let Ok(value) = numeral.replace('_', "").parse::<f64>() else {
                continue;
            };
            let spelled = spell_number(value, Some(&numeral));
            let expected = if value.is_finite()
                && value != 0.0
                && float_to_u64(value).is_none_or(|integer| integer >= EXACT_INTEGER_LIMIT)
            {
                searched_spelling(value)
            } else {
                spell_number(value, None).as_str().to_owned()
            };
            assert_eq!(spelled.as_str(), expected, "{numeral}");
        }
    }
}
