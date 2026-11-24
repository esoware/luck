use std::fmt::{self, Write as _};
use std::iter;

use crate::LuaVersion;

/// A Lua number. Lua 5.3+ tells an integer from a float, as `math.type`
/// does, and on 5.1, 5.2, and Luau every number is a float.
///
/// Two numbers are equal when they are the same value of the same subtype,
/// bit for bit, as two literals are: `Int(1)` and `Float(1.0)` differ, and
/// so do `0.0` and `-0.0`.
#[derive(Debug, Clone, Copy)]
pub enum LuaNumber {
    Int(i64),
    Float(f64),
}

impl PartialEq for LuaNumber {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Int(lhs), Self::Int(rhs)) => lhs == rhs,
            (Self::Float(lhs), Self::Float(rhs)) => lhs.to_bits() == rhs.to_bits(),
            (Self::Int(_), Self::Float(_)) | (Self::Float(_), Self::Int(_)) => false,
        }
    }
}

impl Eq for LuaNumber {}

impl LuaNumber {
    /// The value as a double, an integer rounded to the nearest one as the
    /// C cast Lua uses rounds it.
    pub const fn to_f64(self) -> f64 {
        match self {
            Self::Int(value) => int_to_f64(value),
            Self::Float(value) => value,
        }
    }

    /// The integer the number equals, which a float does when it has no
    /// fraction and lies in `[-2^63, 2^63)`, as Lua 5.3+ converts a float
    /// for a bitwise operator, a table key, or a comparison with an integer.
    pub fn to_integer(self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(value),
            Self::Float(value) => float_to_integer(value),
        }
    }

    /// The magnitude a literal of the number holds, and whether a `-` goes
    /// before it, or `None` for NaN, which no literal holds.
    ///
    /// No numeral spells a negative number, so -1 is `-` before 1, and a
    /// negative zero keeps its sign that way. `i64::MIN` has no magnitude to
    /// negate, so it's a literal of its own, which prints as the hex that
    /// wraps to it on Lua 5.3+.
    pub const fn literal_magnitude(self) -> Option<(bool, Self)> {
        match self {
            Self::Int(value) => Some(match value.checked_neg() {
                Some(magnitude) if value < 0 => (true, Self::Int(magnitude)),
                Some(_) | None => (false, self),
            }),
            Self::Float(value) if value.is_nan() => None,
            Self::Float(value) => Some((value.is_sign_negative(), Self::Float(value.abs()))),
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "every float the range and the fraction check let through is an integer's exact value"
)]
fn float_to_integer(value: f64) -> Option<i64> {
    let range = -9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0;
    (value.fract() == 0.0 && range.contains(&value)).then_some(value as i64)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "the C cast Lua itself uses, and i64 has no From into f64"
)]
const fn int_to_f64(value: i64) -> f64 {
    value as f64
}

#[expect(
    clippy::cast_precision_loss,
    reason = "the C cast of an unsigned 64-bit integer to double, which rounds to the nearest"
)]
const fn unsigned_to_f64(value: u64) -> f64 {
    value as f64
}

/// 2^64, which Luau reads a hex or binary numeral past 64 bits as.
const TWO_TO_64: f64 = 18_446_744_073_709_551_616.0;

/// What a numeral means: its value, and whether that is the integer it
/// spells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Numeral {
    pub value: LuaNumber,
    /// False for a numeral spelled as an integer that doesn't read as that
    /// integer: one a double can't hold, a decimal past `i64::MAX` that Lua
    /// 5.3+ reads as a float, or hex digits past 64 bits, which wrap on 5.3+
    /// and saturate on Luau. True for a numeral spelled as a float.
    pub is_exact: bool,
}

/// The number `text` spells under `version`, as its reference reader
/// converts it.
///
/// `text` is a numeral the lexer took for `version`, and any other text
/// reads as some number. A decimal reads as the nearest double, or on Lua
/// 5.3+ as an integer when it has no `.` or exponent and fits an `i64`. A hex
/// integer wraps past 64 bits on 5.3+, and is the nearest double elsewhere,
/// but Luau's saturate at 2^64, as its binary ones do. A hex float (5.2+) is
/// the nearest double. Luau drops every `_` first.
pub fn read_numeral(text: &str, version: LuaVersion) -> Numeral {
    if let Some(integer) = short_decimal_integer(text.as_bytes()) {
        let value = if version.has_integer_subtype() {
            LuaNumber::Int(integer)
        } else {
            LuaNumber::Float(int_to_f64(integer))
        };
        return Numeral { value, is_exact: true };
    }
    read_other_numeral(text, version)
}

/// The integer a decimal of at most 15 digits spells, as most numerals are,
/// which neither an `i64` nor a double rounds.
#[inline]
fn short_decimal_integer(digits: &[u8]) -> Option<i64> {
    if digits.is_empty() || digits.len() > 15 {
        return None;
    }
    digits.iter().try_fold(0, |value, &digit| {
        let digit_value = digit.wrapping_sub(b'0');
        (digit_value <= 9).then(|| value * 10 + i64::from(digit_value))
    })
}

/// Out of line, so the short path inlines into its caller alone.
#[cold]
#[inline(never)]
fn read_other_numeral(text: &str, version: LuaVersion) -> Numeral {
    let read = |separated: &str| match separated.as_bytes() {
        [b'0', b'x' | b'X', digits @ ..] => read_hex(digits, version),
        // Luau
        [b'0', b'b' | b'B', digits @ ..] if version.has_binary_literals() => {
            digits_value(digits, 2).map_or(SATURATED, float_numeral)
        }
        _ => read_decimal(separated, version),
    };
    if version.has_underscore_separators() { without_separators(text, read) } else { read(text) }
}

/// What Luau reads a hex or binary numeral past 64 bits as.
const SATURATED: Numeral = Numeral { value: LuaNumber::Float(TWO_TO_64), is_exact: false };

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

fn read_decimal(text: &str, version: LuaVersion) -> Numeral {
    let nearest = || LuaNumber::Float(text.parse::<f64>().unwrap_or_default());
    if text.contains(['.', 'e', 'E']) {
        return Numeral { value: nearest(), is_exact: true };
    }
    let wide = decimal_integer(text.as_bytes());
    if !version.has_integer_subtype() {
        return Numeral { value: nearest(), is_exact: wide.is_some_and(is_exact_in_double) };
    }
    // Lua 5.3+ reads a decimal past `i64::MAX` as a float.
    match wide.and_then(|value| i64::try_from(value).ok()) {
        Some(integer) => Numeral { value: LuaNumber::Int(integer), is_exact: true },
        None => Numeral { value: nearest(), is_exact: false },
    }
}

/// `digits` follow the `0x`.
fn read_hex(digits: &[u8], version: LuaVersion) -> Numeral {
    let (mantissa, exponent) = split_at_marker(digits, |b| matches!(b, b'p' | b'P'));
    let (whole, fraction) = split_at_marker(mantissa, |b| b == b'.');
    if exponent.is_none() && fraction.is_none() {
        return read_hex_integer(whole, version);
    }
    // Lua 5.2+
    let binary_exponent = exponent.map_or(0, read_exponent);
    let (value, _) = hex_to_f64(whole, fraction.unwrap_or_default(), binary_exponent);
    Numeral { value: LuaNumber::Float(value), is_exact: true }
}

/// `bytes` before the first byte `is_marker` picks out, and the bytes after
/// it when there is one.
fn split_at_marker(bytes: &[u8], is_marker: impl Fn(u8) -> bool) -> (&[u8], Option<&[u8]>) {
    bytes
        .iter()
        .position(|&b| is_marker(b))
        .map_or((bytes, None), |marker| (&bytes[..marker], Some(&bytes[marker + 1..])))
}

fn read_hex_integer(digits: &[u8], version: LuaVersion) -> Numeral {
    if version.has_integer_subtype() {
        let wrapped =
            digits.iter().fold(0u64, |value, &digit| (value << 4) | u64::from(hex_value(digit)));
        let significant_digits = digits.iter().skip_while(|&&digit| digit == b'0').count();
        return Numeral {
            value: LuaNumber::Int(wrapped.cast_signed()),
            is_exact: significant_digits <= 16,
        };
    }
    if let Some(integer) = digits_value(digits, 16) {
        return float_numeral(integer);
    }
    if version.has_saturating_hex_numerals() {
        return SATURATED;
    }
    let (value, is_exact) = hex_to_f64(digits, &[], 0);
    Numeral { value: LuaNumber::Float(value), is_exact }
}

/// The nearest double to an integer that fits 64 bits, which is exact when
/// a double can hold it.
fn float_numeral(integer: u64) -> Numeral {
    Numeral {
        value: LuaNumber::Float(unsigned_to_f64(integer)),
        is_exact: is_exact_in_double(u128::from(integer)),
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

fn hex_value(digit: u8) -> u32 {
    char::from(digit).to_digit(16).unwrap_or_default()
}

/// A hex float's `p` exponent: an optional sign, then decimal digits.
fn read_exponent(exponent: &[u8]) -> i64 {
    let (is_negative, digits) = match exponent {
        [b'-', digits @ ..] => (true, digits),
        [b'+', digits @ ..] | digits => (false, digits),
    };
    // Past this every value is zero or infinite, and the sums the reader
    // makes with it can't overflow.
    let magnitude = digits.iter().fold(0i64, |value, &digit| {
        (value * 10 + i64::from(digit.wrapping_sub(b'0'))).min(1 << 40)
    });
    if is_negative { -magnitude } else { magnitude }
}

/// The double nearest to the hex digits `whole` and `fraction` spell, with a
/// point between them, times `2^binary_exponent`, as a correctly rounding
/// C99 `strtod` reads them, and whether it is exact.
fn hex_to_f64(whole: &[u8], fraction: &[u8], binary_exponent: i64) -> (f64, bool) {
    // `mantissa * 2^exponent` is the value of the digits read so far, and
    // `is_sticky` whether any digit past the 60 bits it keeps was nonzero.
    let mut mantissa = 0u64;
    let mut exponent = binary_exponent;
    let mut is_sticky = false;
    let whole_digits = whole.iter().map(|&digit| (digit, false));
    let digits = whole_digits.chain(fraction.iter().map(|&digit| (digit, true)));
    for (digit, is_fraction) in digits {
        let value = hex_value(digit);
        if mantissa >> 60 == 0 {
            mantissa = (mantissa << 4) | u64::from(value);
            if is_fraction {
                exponent -= 4;
            }
        } else {
            is_sticky |= value != 0;
            if !is_fraction {
                exponent += 4;
            }
        }
    }
    round_to_f64(mantissa, exponent, is_sticky)
}

/// The double nearest to `mantissa * 2^exponent`, plus a little more when
/// `is_sticky`, rounding half to even, and whether it is exact.
fn round_to_f64(mantissa: u64, exponent: i64, is_sticky: bool) -> (f64, bool) {
    if mantissa == 0 {
        return (0.0, !is_sticky);
    }
    let leading_zeros = mantissa.leading_zeros();
    let normalized = mantissa << leading_zeros;
    // The power of two the top bit of `normalized` stands for.
    let top_exponent = exponent - i64::from(leading_zeros) + 63;
    if top_exponent > 1023 {
        return (f64::INFINITY, false);
    }
    // How many low bits fall below the double's last significand bit: 11 for
    // a normal double, and more the further a subnormal falls below 2^-1022.
    let dropped_bits = if top_exponent >= -1022 { 11 } else { -1011 - top_exponent };
    let Some(dropped) = u32::try_from(dropped_bits).ok().filter(|&dropped| dropped <= 64) else {
        return (0.0, false);
    };
    let (kept, remainder, half) = if dropped == 64 {
        (0, normalized, 1 << 63)
    } else {
        let low_mask = (1 << dropped) - 1;
        (normalized >> dropped, normalized & low_mask, 1 << (dropped - 1))
    };
    let is_above_half = remainder > half || (remainder == half && is_sticky);
    let is_odd_tie = remainder == half && !is_sticky && kept & 1 == 1;
    let rounded = kept + u64::from(is_above_half || is_odd_tie);
    let is_exact = remainder == 0 && !is_sticky;
    let bits = match u64::try_from(top_exponent + 1023) {
        // `rounded` holds the implicit bit, so adding it to the exponent
        // field one below counts it, and a carry out of it raises the
        // exponent.
        Ok(biased) if biased > 0 => ((biased - 1) << 52) + rounded,
        // A subnormal's exponent field is zero, and a carry into it gives the
        // least normal double.
        Ok(_) | Err(_) => rounded,
    };
    let value = f64::from_bits(bits);
    (value, is_exact && value.is_finite())
}

/// The integer a Luau `i` literal such as `0xFFi` spells, its `_`s dropped.
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

/// The shortest numeral that reads back as `value` under `version`.
///
/// A float stays a float on Lua 5.3+, with a `.` or an exponent, and a
/// negative integer, which a numeral spells only as hex that wraps, is
/// written in hex. `value` is one a literal holds: a float that's neither
/// negative nor NaN. Infinity is `1e999`, which every reader overflows.
pub fn spell_number(value: LuaNumber, version: LuaVersion) -> NumberSpelling {
    match value {
        LuaNumber::Int(integer) => spell_int(integer, ""),
        LuaNumber::Float(float) => spell_float(float, version.has_integer_subtype()),
    }
}

/// The shortest Luau integer literal for `value`, which is in hex when it's
/// negative.
pub fn spell_integer(value: i64) -> NumberSpelling {
    spell_int(value, "i")
}

fn spell_int(value: i64, suffix: &str) -> NumberSpelling {
    let mut spelling = NumberSpelling::EMPTY;
    let bits = value.cast_unsigned();
    let decimal_length = (value >= 0).then(|| decimal_length(bits));
    // Plain digits win a tie.
    let _ = if decimal_length.is_some_and(|length| length <= hex_length(bits)) {
        write!(spelling, "{value}{suffix}")
    } else {
        write!(spelling, "0x{bits:x}{suffix}")
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

/// The ways [`spell_float`] can lay out a float's digits.
#[derive(Clone, Copy)]
enum Layout {
    /// No exponent: `12300`, `12300.`, `1.23`, or `.00123`.
    Plain,
    /// The first `n` digits before the point, then an exponent: `123e2`,
    /// `1.23e4`, or `.123e5`.
    Exponent(usize),
    /// A hex integer, which reads as a float where every number is one.
    Hex,
}

fn spell_float(value: f64, is_float_marked: bool) -> NumberSpelling {
    debug_assert!(value.is_sign_positive() && !value.is_nan(), "a literal holds {value}");
    let mut spelling = NumberSpelling::EMPTY;
    if value.is_nan() {
        spelling.push_all(*b"(0/0)");
        return spelling;
    }
    let magnitude = value.abs();
    if magnitude == 0.0 {
        let _ = spelling.write_str(if is_float_marked { "0." } else { "0" });
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
    let layout = shortest_layout(magnitude, digits, exponent, is_float_marked);
    write_layout(layout, magnitude, digits, exponent, is_float_marked, &mut spelling);
    spelling
}
/// The shortest layout of `digits`, the first worth `10^exponent`. Earlier
/// layouts win a tie: plain, then exponents with the most digits before the
/// point, then hex.
fn shortest_layout(value: f64, digits: &[u8], exponent: i64, is_float_marked: bool) -> Layout {
    let count = digits.len();
    let count_wide = i64::try_from(count).expect("a double has at most 17 digits");
    let last_exponent = exponent - count_wide + 1;
    let to_length = |wide: i64| usize::try_from(wide).expect("each branch checks the sign");
    let plain = if last_exponent >= 0 {
        count + to_length(last_exponent) + usize::from(is_float_marked)
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
    if !is_float_marked
        && let Some(integer) = float_to_u64(value)
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
    is_float_marked: bool,
    spelling: &mut NumberSpelling,
) {
    let count = i64::try_from(digits.len()).expect("a double has at most 17 digits");
    match layout {
        Layout::Plain => {
            if let Ok(zeros) = usize::try_from(exponent - count + 1) {
                spelling.push_all(digits.iter().copied());
                spelling.push_all(iter::repeat_n(b'0', zeros));
                if is_float_marked {
                    spelling.push(b'.');
                }
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
