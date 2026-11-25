use std::mem::MaybeUninit;

use luck_simd::DynCpu;
use luck_token::{Span, Token, TokenKind};

use super::Scanner;
use crate::search::are_hex_digits;
use crate::window::{Classify, Planes, WINDOW_BYTES, WindowBytes};

/// A batch's tokens, which stay in the cache while the parser reads them.
pub(super) const BUFFER_TOKENS: usize = 256;

pub(super) const UNREAD_TOKEN: Token = Token {
    span: Span::new(0, 0),
    kind: TokenKind::Eof,
    has_line_break_before: false,
    is_unterminated: false,
    has_escape: false,
};

/// Where a batch puts a token: the lexer's buffer, or the spare capacity of
/// the vector `lex` returns.
pub(super) trait Slot {
    fn put(&mut self, token: Token);
}

impl Slot for Token {
    #[inline]
    fn put(&mut self, token: Token) {
        *self = token;
    }
}

impl Slot for MaybeUninit<Token> {
    #[inline]
    fn put(&mut self, token: Token) {
        self.write(token);
    }
}

impl Scanner<'_> {
    /// Fill the first slots of `batch` with the next tokens, through the `Eof`
    /// if the source ends, and return how many. Every slot below the count
    /// is written, which `lex` relies on.
    pub(super) fn fill<S: Slot>(&mut self, batch: &mut [S; BUFFER_TOKENS]) -> usize {
        match self.cpu {
            DynCpu::Avx2(cpu) => cpu.run(|cpu| self.fill_windows(cpu, batch)),
            DynCpu::Neon(cpu) => self.fill_windows(cpu, batch),
            DynCpu::Generic(_) => self.fill_scanned(batch),
        }
    }

    fn fill_scanned<S: Slot>(&mut self, batch: &mut [S; BUFFER_TOKENS]) -> usize {
        for (count, slot) in batch.iter_mut().enumerate() {
            let token = self.scan_token();
            slot.put(token);
            if token.kind == TokenKind::Eof {
                return count + 1;
            }
        }
        BUFFER_TOKENS
    }

    /// Lex the simple tokens a window at a time, and each other with
    /// `scan_token`, until `batch` has no room for another window's tokens or
    /// holds the `Eof`. A window stays classified while the cursor is in it,
    /// so the tokens after one `scan_token` lexes come off the same bits.
    #[expect(
        clippy::inline_always,
        reason = "inlined into `Avx2::run`, which enables the feature set"
    )]
    #[inline(always)]
    fn fill_windows<S: Slot>(
        &mut self,
        cpu: impl Classify,
        batch: &mut [S; BUFFER_TOKENS],
    ) -> usize {
        let mut count = 0;
        let mut window = Window::SPENT;
        let mut is_spent = true;
        // Room for a window's tokens and the one `scan_token` lexes after them.
        while count + WINDOW_BYTES < BUFFER_TOKENS {
            // An interpolation's braces, which `scan_token` counts, take it.
            if self.interpolation_brace_depths.is_empty() {
                let position = self.cursor.position();
                if is_spent || position - window.base >= WINDOW_BYTES {
                    window = Window::new(cpu, position, self.cursor.rest());
                }
                let (slots, _) = batch[count..].split_first_chunk_mut().expect("room for a window");
                let (emitted, should_scan) =
                    self.emit_window(&window, position - window.base, slots);
                count += emitted;
                is_spent = !should_scan;
                if is_spent {
                    continue;
                }
            }
            let token = self.scan_complex_token();
            batch[count].put(token);
            count += 1;
            if token.kind == TokenKind::Eof {
                break;
            }
        }
        count
    }

    #[inline(never)]
    fn scan_complex_token(&mut self) -> Token {
        self.scan_token()
    }

    /// Emit the simple tokens of `window` from `skip` bytes into it, the
    /// cursor, into `slots`: names, keywords, decimal integers that touch
    /// nothing and the other numerals [`Window::numeral`] takes, punctuation
    /// and the operators it joins into, and quoted strings that hold no
    /// escape and close in the window, each spelled as `scan_token` reads it. Returns how many, which are the slots it wrote,
    /// and whether the next token needs `scan_token`, because it's another
    /// kind, rather than a new window, because the window ended or ended
    /// inside it.
    #[expect(clippy::inline_always, reason = "the batch loop's body")]
    #[inline(always)]
    fn emit_window<S: Slot>(
        &mut self,
        window: &Window,
        skip: usize,
        slots: &mut [S; WINDOW_BYTES],
    ) -> (usize, bool) {
        let limit = window.limit;
        let from_cursor = u64::MAX << skip;
        let mut first_complex = (window.complex & from_cursor).trailing_zeros();
        let mut end = first_complex.min(limit);
        let mut starts = window.starts & from_cursor;
        let line_breaks = window.line_breaks & from_cursor & below(limit);
        // A carry from each line break runs through the bytes after it to the
        // next start, the token whose gap holds it. A string's body holds no
        // line break, so the starts in one take no carry.
        let reachable = starts & below(limit);
        let mut has_break_before = reachable & (!reachable).wrapping_add(line_breaks);
        if self.has_line_break {
            has_break_before |= reachable & reachable.wrapping_neg();
        }
        let mut pending = starts & below(end);
        let mut stopped_at = None;
        for slot in slots {
            if pending == 0 {
                break;
            }
            let offset = pending.trailing_zeros();
            pending &= pending - 1;
            let index = offset as usize;
            let first = window.bytes[index];
            let kind = BYTE_KINDS[usize::from(first)];
            let (kind, length) = if (window.slow >> offset) & 1 == 0 {
                (kind, 1)
            } else if kind == TokenKind::Identifier {
                let length = (window.ends >> offset).trailing_zeros() + 1;
                let (first_eight, _) =
                    window.bytes[index..].split_first_chunk().expect("8 bytes of slack");
                (TokenKind::of_name(u64::from_le_bytes(*first_eight), length as usize), length)
            } else if kind == TokenKind::Number {
                let run = (window.ends >> offset).trailing_zeros() + 1;
                // No numeral reaches the window's last byte, so `run` is
                // under 64, and the bits from its first byte through the one
                // past it fit.
                if (window.numeral_breaks >> offset) << (63 - run) == 0 {
                    (kind, run)
                } else {
                    let Some(length) = window.numeral(offset, run) else {
                        stopped_at = Some(offset);
                        break;
                    };
                    if length > run {
                        // A fraction's point and digits start no token, and
                        // its point stops none.
                        let past = !below(offset + length);
                        starts &= past | below(offset + 1);
                        first_complex = (window.complex & past).trailing_zeros();
                        end = first_complex.min(limit);
                        pending = starts & past & below(end);
                    }
                    (kind, length)
                }
            } else if kind == TokenKind::StringLiteral {
                let closing =
                    if first == b'"' { window.double_quotes } else { window.single_quotes };
                let close =
                    ((closing | window.string_breaks) & (u64::MAX << offset << 1)).trailing_zeros();
                if close >= limit || (closing >> close) & 1 == 0 {
                    stopped_at = Some(offset);
                    break;
                }
                // What the string holds starts no token, and stops none.
                let past = u64::MAX << close << 1;
                starts &= past | below(offset + 1);
                first_complex = (window.complex & past).trailing_zeros();
                end = first_complex.min(limit);
                pending = starts & past & below(end);
                (kind, close + 1 - offset)
            } else {
                let following = [window.bytes[index + 1], window.bytes[index + 2]];
                let Some((kind, length)) = operator(first, following, kind) else {
                    stopped_at = Some(offset);
                    break;
                };
                if offset + length > end {
                    stopped_at = Some(offset);
                    break;
                }
                // The bytes an operator joins start no token.
                let past = !below(offset + length);
                starts &= past | below(offset + 1);
                pending &= past;
                (kind, length)
            };
            let start = window.base + index;
            slot.put(Token {
                span: span(start, start + length as usize),
                kind,
                has_line_break_before: (has_break_before >> offset) & 1 == 1,
                is_unterminated: false,
                has_escape: false,
            });
        }
        // The loop takes the starts in order, writes a token at each but the
        // one it stops at, and drops a token's inner starts before it gets to
        // them, so the starts left below `stop` are the tokens it wrote. Each
        // is past the last and below 64, so they never outrun the slots.
        let stop = stopped_at.unwrap_or(end);
        let emitted = starts & below(stop);
        if emitted == 0 {
            self.has_line_break |= line_breaks & below(stop) != 0;
        } else {
            let last = emitted.ilog2();
            self.has_line_break = (line_breaks & below(stop)) >> last != 0;
            self.last_token_start = source_offset(window.base + last as usize);
            if self.attached_comment_count < self.comments.len() {
                let first = window.base + emitted.trailing_zeros() as usize;
                self.attach_leading_comments(source_offset(first));
            }
        }
        self.cursor.advance_by(stop as usize - skip);
        let should_scan =
            stopped_at.is_some() || first_complex < 64 && first_complex <= limit || end == 0;
        (emitted.count_ones() as usize, should_scan)
    }
}

/// A classified window, and where its tokens lie in it, a bit per byte, byte
/// 0's lowest.
struct Window {
    /// Where the window's first byte is in the source.
    base: usize,
    bytes: WindowBytes,
    /// The first byte of each name and numeral, and every punctuation byte
    /// and quote, where a token may start.
    starts: u64,
    /// The last byte of each name and numeral.
    ends: u64,
    /// The first byte of each token that takes more than its first byte to
    /// read: a name, a numeral, a quoted string, or punctuation a
    /// continuation follows.
    slow: u64,
    /// What makes a numeral more than a decimal integer: a letter or a `_`
    /// in it, or a `.` after it.
    numeral_breaks: u64,
    double_quotes: u64,
    single_quotes: u64,
    /// What ends a run of a short string's body short of its quote.
    string_breaks: u64,
    /// The bytes no window token starts at or runs through, outside a
    /// string: a byte in no class, and a `.` before a digit.
    complex: u64,
    line_breaks: u64,
    /// Where the window's tokens stop: the start of a name or punctuation
    /// in its last byte, which may go on past it, or else 64.
    limit: u32,
}

impl Window {
    /// What the batch loop holds before it classifies its first window.
    const SPENT: Self = Self {
        base: 0,
        bytes: [0; WINDOW_BYTES + 8],
        starts: 0,
        ends: 0,
        slow: 0,
        numeral_breaks: 0,
        double_quotes: 0,
        single_quotes: 0,
        string_breaks: 0,
        complex: 0,
        line_breaks: 0,
        limit: 0,
    };

    /// The length of the numeral at `offset`, whose first `run` bytes of
    /// name make more than a decimal integer, where it's one `scan_token`
    /// reads with no error and the window holds whole: a hexadecimal integer
    /// of 1 to 8 digits, or a decimal fraction with digits on both sides of
    /// its point. `None` for any other, which `scan_token` reads.
    #[inline]
    fn numeral(&self, offset: u32, run: u32) -> Option<u32> {
        let index = offset as usize;
        let after = self.bytes[index + run as usize];
        if after == b'.' && (self.numeral_breaks >> offset) & below(run) == 0 {
            let fraction = offset + run + 1;
            if fraction >= 64 || !self.bytes[fraction as usize].is_ascii_digit() {
                return None;
            }
            let digits = (self.ends >> fraction).trailing_zeros() + 1;
            let length = run + 1 + digits;
            let is_whole = offset + length <= self.limit
                && (self.numeral_breaks >> fraction) << (63 - digits) == 0;
            return is_whole.then_some(length);
        }
        let digits = run.checked_sub(2)? as usize;
        let (chunk, _) = self.bytes[index + 2..].split_first_chunk()?;
        let is_hex = self.bytes[index] == b'0'
            && matches!(self.bytes[index + 1], b'x' | b'X')
            && (1..=8).contains(&digits)
            && are_hex_digits(*chunk, digits)
            && after != b'.';
        is_hex.then_some(run)
    }

    /// The window at `base`, whose source runs on with `rest`.
    #[expect(clippy::inline_always, reason = "the batch loop's body")]
    #[inline(always)]
    fn new(cpu: impl Classify, base: usize, rest: &[u8]) -> Self {
        let bytes = window_bytes(rest);
        let (head, _) = bytes.split_first_chunk().expect("72 bytes");
        let Planes {
            whitespace,
            name,
            digit,
            line_break,
            punct,
            continuation,
            dot,
            double_quote,
            single_quote,
            string_break,
        } = cpu.classify(head);
        // The window opens at the cursor, and the cursor stays where no name
        // runs on from before it, so a name starts where the byte before it
        // isn't one.
        let name_starts = name & !(name << 1);
        let quotes = double_quote | single_quote;
        let starts = name_starts | punct | quotes;
        Self {
            base,
            bytes,
            starts,
            ends: name & !(name >> 1),
            slow: name_starts | quotes | (punct & (continuation >> 1)),
            numeral_breaks: (name & !digit) | dot,
            double_quotes: double_quote,
            single_quotes: single_quote,
            string_breaks: string_break | line_break,
            complex: !(whitespace | name | punct | quotes) | (dot & (digit >> 1)),
            line_breaks: line_break,
            limit: if (name | punct) >> 63 == 0 { 64 } else { starts.ilog2() },
        }
    }
}

/// The bits below bit `count`.
#[inline]
fn below(count: u32) -> u64 {
    u64::MAX.checked_shr(64 - count).unwrap_or(0)
}

#[inline]
fn span(start: usize, end: usize) -> Span {
    Span::new(source_offset(start), source_offset(end))
}

/// A plain cast, as the cursor's spans are.
#[inline]
#[expect(
    clippy::cast_possible_truncation,
    reason = "`Lexer::new` refuses a source over `u32::MAX` bytes, so a position fits a `u32`"
)]
const fn source_offset(position: usize) -> u32 {
    position as u32
}

#[inline]
fn window_bytes(rest: &[u8]) -> WindowBytes {
    match rest.first_chunk() {
        Some(bytes) => *bytes,
        None => tail_window_bytes(rest),
    }
}

/// Only the last windows of a source run past its end.
#[cold]
#[inline(never)]
fn tail_window_bytes(rest: &[u8]) -> WindowBytes {
    let mut bytes = [0; WINDOW_BYTES + 8];
    bytes[..rest.len()].copy_from_slice(rest);
    bytes
}

/// The kind of the token each byte that starts one in a window starts:
/// `Identifier` for a name, whose keyword its spelling decides, `Number` for
/// a numeral, `StringLiteral` for a quoted string, and for punctuation that
/// no continuation follows, the token it spells alone. Any other byte starts
/// none there.
static BYTE_KINDS: [TokenKind; 256] = {
    let mut kinds = [TokenKind::Identifier; 256];
    let mut byte: u8 = 0;
    loop {
        kinds[byte as usize] = match byte {
            b'0'..=b'9' => TokenKind::Number,
            b'"' | b'\'' => TokenKind::StringLiteral,
            b'#' => TokenKind::Hash,
            b'%' => TokenKind::Percent,
            b'&' => TokenKind::Ampersand,
            b'(' => TokenKind::LeftParen,
            b')' => TokenKind::RightParen,
            b'*' => TokenKind::Star,
            b'+' => TokenKind::Plus,
            b',' => TokenKind::Comma,
            b'-' => TokenKind::Minus,
            b'.' => TokenKind::Dot,
            b'/' => TokenKind::Slash,
            b':' => TokenKind::Colon,
            b';' => TokenKind::Semicolon,
            b'<' => TokenKind::Less,
            b'=' => TokenKind::Equal,
            b'>' => TokenKind::Greater,
            b'?' => TokenKind::Question,
            b'@' => TokenKind::At,
            b'[' => TokenKind::LeftBracket,
            b']' => TokenKind::RightBracket,
            b'^' => TokenKind::Caret,
            b'{' => TokenKind::LeftBrace,
            b'|' => TokenKind::Pipe,
            b'}' => TokenKind::RightBrace,
            _ => TokenKind::Identifier,
        };
        if byte == u8::MAX {
            break kinds;
        }
        byte += 1;
    }
};

/// The token that punctuation `first` starts with the two bytes `following`
/// it, the first a continuation, as `scan_token` reads it: an operator they
/// join it into, with its length, or `single` alone, which spells `first`.
/// `None` where they open what only `scan_token` reads, a comment or a long
/// bracket.
#[inline]
const fn operator(first: u8, following: [u8; 2], single: TokenKind) -> Option<(TokenKind, u32)> {
    let [second, third] = following;
    Some(match (first, second) {
        (b'=', b'=') => (TokenKind::EqualEqual, 2),
        (b'<', b'=') => (TokenKind::LessEqual, 2),
        (b'>', b'=') => (TokenKind::GreaterEqual, 2),
        (b':', b':') => (TokenKind::DoubleColon, 2),
        (b'-', b'>') => (TokenKind::Arrow, 2),
        (b'-', b'=') => (TokenKind::MinusEqual, 2),
        (b'+', b'=') => (TokenKind::PlusEqual, 2),
        (b'*', b'=') => (TokenKind::StarEqual, 2),
        (b'/', b'=') => (TokenKind::SlashEqual, 2),
        (b'%', b'=') => (TokenKind::PercentEqual, 2),
        (b'^', b'=') => (TokenKind::CaretEqual, 2),
        (b'/', b'/') => {
            if third == b'=' {
                (TokenKind::FloorDivEqual, 3)
            } else {
                (TokenKind::FloorDiv, 2)
            }
        }
        (b'.', b'.') => match third {
            b'.' => (TokenKind::DotDotDot, 3),
            b'=' => (TokenKind::DotDotEqual, 3),
            _ => (TokenKind::DotDot, 2),
        },
        (b'-', b'-') | (b'[', b'[' | b'=') => return None,
        _ => (single, 1),
    })
}
