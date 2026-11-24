use std::borrow::Cow;

use luck_diagnostics::Diagnostic;
use luck_token::{EscapeError, LuaVersion, Span, StringKind, Token, TokenKind, read_escape};
use memchr::memmem;

use super::Lexer;
use crate::search::{ByteMatchTable, byte_match_table, find_match};

// Stop bytes for raw interpolated-string text: escape lead, interpolation
// opener, closing backtick, and the line breaks Luau rejects.
static INTERPOLATED_STRING_STOP: ByteMatchTable =
    byte_match_table!(|b| matches!(b, b'\\' | b'{' | b'`' | b'\n' | b'\r'));

#[derive(Clone, Copy, PartialEq, Eq)]
enum SegmentEnd {
    OpenBrace,
    Backtick,
    /// The line or the source ended before the closing backtick.
    Unterminated,
}

/// What a long bracket delimits, which names it in the error a missing closer
/// reports.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LongBracketKind {
    /// A string, whose scan also looks for a CR, which its value rewrites.
    String,
    Comment,
}

/// What a long bracket's body held, as its lexer reads it.
pub(super) struct LongBracketBody {
    /// Whether its closer came.
    pub is_closed: bool,
    /// Whether a CR came in a string's body. A comment's scan never looks.
    pub has_cr: bool,
}

impl Lexer<'_> {
    /// Lex a single- or double-quoted string that opened with `quote` at
    /// `start`, with the cursor past it.
    ///
    /// A malformed escape goes to `errors` and stays text, so the string ends
    /// where it would have without it. A line break or the end of the source
    /// before the closing quote ends the string there, unterminated, so what
    /// follows on later lines lexes as it would have.
    pub(super) fn lex_short_string(&mut self, start: usize, quote: u8) -> Token {
        loop {
            self.cursor.eat_short_string_run(quote);
            match self.cursor.peek() {
                None | Some(b'\n' | b'\r') => {
                    self.reject(start, "unterminated string");
                    return self.unterminated_token(TokenKind::StringLiteral, start);
                }
                Some(b'\\') => self.scan_escape(StringKind::Quoted),
                // The scan stops only at those bytes and at `quote`.
                Some(_) => {
                    self.cursor.bump();
                    return self.token(TokenKind::StringLiteral, start);
                }
            }
        }
    }

    /// Lex a long-bracket string of `level` that opened at `start`, with the
    /// cursor past its `[=*[` opener. A missing closer runs the string to the
    /// end of the source.
    pub(super) fn lex_long_string(&mut self, start: usize, level: usize) -> Token {
        let body = self.scan_long_bracket_body(start, level, LongBracketKind::String);
        self.has_escape = body.has_cr;
        if body.is_closed {
            self.token(TokenKind::StringLiteral, start)
        } else {
            self.unterminated_token(TokenKind::StringLiteral, start)
        }
    }

    /// Lex an interpolated string's first piece, with the cursor past its
    /// opening backtick: `InterpolatedStringBegin` up to an interpolation's
    /// `{`, or the whole of a string with none as `InterpolatedStringSimple`.
    pub(super) fn lex_interpolated_string(&mut self, start: usize) -> Token {
        match self.scan_interpolated_segment(start) {
            SegmentEnd::OpenBrace => {
                self.interpolation_brace_depths.push(0);
                self.token(TokenKind::InterpolatedStringBegin, start)
            }
            SegmentEnd::Backtick => self.token(TokenKind::InterpolatedStringSimple, start),
            SegmentEnd::Unterminated => {
                self.unterminated_token(TokenKind::InterpolatedStringSimple, start)
            }
        }
    }

    /// Resume an interpolated string at the `}` that closes an expression,
    /// with the cursor past it, producing `InterpolatedStringMiddle` up to the
    /// next `{` or `InterpolatedStringEnd` through the closing backtick.
    ///
    /// An unterminated string ends at its line break, so the `}` always
    /// yields one, and the lexer never closes a string the parser doesn't see
    /// close.
    pub(super) fn lex_interpolated_string_continuation(&mut self, start: usize) -> Token {
        self.interpolation_brace_depths.pop();
        match self.scan_interpolated_segment(start) {
            SegmentEnd::OpenBrace => {
                self.interpolation_brace_depths.push(0);
                self.token(TokenKind::InterpolatedStringMiddle, start)
            }
            SegmentEnd::Backtick => self.token(TokenKind::InterpolatedStringEnd, start),
            SegmentEnd::Unterminated => {
                self.unterminated_token(TokenKind::InterpolatedStringEnd, start)
            }
        }
    }

    /// The token of a string from `start` that ends at the cursor without its
    /// closing delimiter.
    #[cold]
    #[inline(never)]
    fn unterminated_token(&mut self, kind: TokenKind, start: usize) -> Token {
        let mut token = self.token(kind, start);
        token.is_unterminated = true;
        token
    }

    /// Consume the escape sequence at the cursor's `\`, as [`read_escape`]
    /// reads it for every stage. A malformed one goes to `errors`, and one
    /// only Lua 5.1 and Luau read, as the character it escapes, to
    /// `lenient_escapes`. At the end of the source it consumes only the `\`,
    /// and the string's scan reports the string unterminated.
    fn scan_escape(&mut self, kind: StringKind) {
        let escape_start = self.cursor.position();
        self.cursor.bump();
        let Some(escaped_byte) = self.cursor.peek() else {
            return;
        };
        self.has_escape = true;
        let character_start = self.cursor.position();
        let (length, meaning) = read_escape(self.cursor.rest(), kind, self.version);
        self.cursor.advance_by(length);
        // A lenient or undefined escape takes the first byte of a multi-byte
        // character, and its span and its error take the whole character.
        match meaning {
            // Lua 5.1, Luau
            Ok(escaped) if escaped.is_lenient() => {
                self.cursor.eat_rest_of_char(character_start);
                self.lenient_escapes.push(self.cursor.span_since(escape_start));
            }
            Ok(_) => {}
            // Lua 5.2+
            Err(EscapeError::Undefined) => {
                let message = match self.cursor.eat_rest_of_char(character_start) {
                    Some(character) => format!("invalid escape sequence `\\{character}`"),
                    None => {
                        format!("invalid escape sequence, `\\` before byte 0x{escaped_byte:02X}")
                    }
                };
                self.reject(escape_start, message);
            }
            Err(error) => self.reject(escape_start, escape_message(error, self.version)),
        }
    }

    /// Scan one piece of raw interpolated-string text that began at `start`,
    /// at its backtick or the `}` that resumed the string.
    ///
    /// The piece ends at an interpolation opener `{`, the closing backtick, or
    /// the line break that leaves it unterminated, consuming the `{` or the
    /// backtick. A malformed escape or a `{{` goes to `errors` and stays text,
    /// so a piece ends where it would have without it.
    fn scan_interpolated_segment(&mut self, start: usize) -> SegmentEnd {
        loop {
            match self.cursor.peek() {
                // Luau rejects unescaped line breaks in backtick strings, same
                // as in short strings.
                None | Some(b'\n' | b'\r') => {
                    self.reject(start, "unterminated interpolated string");
                    return SegmentEnd::Unterminated;
                }
                // Consuming `\u{...}` whole also keeps its `{` from opening
                // an interpolation.
                Some(b'\\') => self.scan_escape(StringKind::Interpolated),
                Some(b'{') if self.cursor.peek_next() == Some(b'{') => {
                    let position = self.cursor.position();
                    self.cursor.advance_by(2);
                    self.reject(
                        position,
                        "`{{` is not allowed in interpolated strings, use `\\{` for a literal brace",
                    );
                }
                Some(b'{') => {
                    self.cursor.bump();
                    return SegmentEnd::OpenBrace;
                }
                Some(b'`') => {
                    self.cursor.bump();
                    return SegmentEnd::Backtick;
                }
                Some(_) => {
                    self.cursor.bump();
                    self.cursor
                        .advance_by(find_match(self.cursor.rest(), &INTERPOLATED_STRING_STOP));
                }
            }
        }
    }

    /// Consume the rest of a long-bracket opener `=*[` after its first `[`,
    /// which the cursor is past, and return its level (`[==[` is level 2).
    /// Consumes nothing and returns `None` when no opener follows.
    pub(super) fn eat_long_bracket_level(&mut self) -> Option<usize> {
        let rest = self.cursor.rest();
        let level = rest.iter().take_while(|&&b| b == b'=').count();
        if rest.get(level) != Some(&b'[') {
            return None;
        }
        self.cursor.advance_by(level + 1);
        Some(level)
    }

    /// Advance from just past the opener of a long bracket of `level`, which
    /// began at `start`, to past its matching `]=*]` closer, and say whether
    /// the closer came, and for a string, whether a CR came before it, in the
    /// same pass.
    ///
    /// A missing closer leaves the cursor at the end of the source and goes
    /// to `errors`, spanning from `start`. A level-0 body holding a `[[` that
    /// the version rejects goes there too, at its first `[[`, where Lua 5.1
    /// stops.
    pub(super) fn scan_long_bracket_body(
        &mut self,
        start: usize,
        level: usize,
        kind: LongBracketKind,
    ) -> LongBracketBody {
        let body_start = self.cursor.position();
        let should_find_cr = kind == LongBracketKind::String;
        let mut has_cr = false;
        loop {
            let rest = self.cursor.rest();
            let stop = if should_find_cr && !has_cr {
                memchr::memchr2(b']', b'\r', rest)
            } else {
                memchr::memchr(b']', rest)
            };
            let Some(stop_offset) = stop else {
                self.cursor.advance_by(rest.len());
                let message = match kind {
                    LongBracketKind::String => "unterminated long bracket string",
                    LongBracketKind::Comment => "unterminated block comment",
                };
                self.reject(start, message);
                return LongBracketBody { is_closed: false, has_cr };
            };
            self.cursor.advance_by(stop_offset + 1);
            if rest[stop_offset] == b'\r' {
                has_cr = true;
                continue;
            }
            let after_bracket = self.cursor.rest();
            let closing_level = after_bracket.iter().take_while(|&&b| b == b'=').count();
            if closing_level == level && after_bracket.get(level) == Some(&b']') {
                self.cursor.advance_by(level + 1);
                if level == 0
                    && self.version.has_long_bracket_nesting_error()
                    && let Some(offset) = memmem::find(self.cursor.text_since(body_start), b"[[")
                {
                    let nested_open = body_start + offset;
                    self.errors.push(Diagnostic::e008(
                        Span::from_indices(nested_open, nested_open + 2),
                        "nesting of `[[...]]` is deprecated",
                    ));
                }
                return LongBracketBody { is_closed: true, has_cr };
            }
        }
    }
}

fn escape_message(error: EscapeError, version: LuaVersion) -> Cow<'static, str> {
    match error {
        EscapeError::DecimalTooLarge(value) => {
            format!("decimal escape too large (`\\{value}`), maximum is `\\255`").into()
        }
        EscapeError::HexDigits(count) => {
            format!("`\\x` escape requires exactly 2 hex digits, got {count}").into()
        }
        EscapeError::UnicodeBrace => "`\\u` escape requires `{`".into(),
        EscapeError::UnicodeDigits => "`\\u{}` requires at least one hex digit".into(),
        EscapeError::UnicodeTooLarge => {
            let limit = if version.has_extended_unicode_escape() { "2^31" } else { "0x110000" };
            format!("`\\u` codepoint too large (must be less than {limit})").into()
        }
        EscapeError::UnicodeUnclosed => "`\\u` escape missing closing `}`".into(),
        EscapeError::Undefined => "invalid escape sequence".into(),
    }
}
