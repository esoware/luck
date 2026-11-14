use luck_token::{BinOp, CompactString, LuaVersion, TokenKind, UnOp, encode_string};

use crate::expr::{Expression, InterpolatedString, InterpolatedStringSegment, Literal};
use crate::synth::Synth;

/// One piece of a [`Synth::interpolated_string`]: unescaped literal text or
/// an interpolated `{expr}`.
#[derive(Debug)]
pub enum InterpolatedStringPart<'text> {
    Text(&'text str),
    Expr(Expression),
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the synthesizer is one type with its constructors split by node family"
)]
impl Synth {
    /// Prefer [`Synth::number_f64`] / [`Synth::number_i64`] for runtime values.
    #[must_use]
    pub fn number(&self, text: &str) -> Expression {
        Expression::Number(Literal { text: CompactString::from(text), span: self.next_span() })
    }

    /// A Luau integer literal from source-form text including the `i` suffix.
    #[must_use]
    pub fn integer_literal(&self, text: &str) -> Expression {
        Expression::Integer(Literal { text: CompactString::from(text), span: self.next_span() })
    }

    /// A literal can't carry a sign, so negative values use their two's-complement hex form.
    #[must_use]
    pub fn integer_i64(&self, value: i64) -> Expression {
        if value < 0 {
            self.integer_literal(&format!("0x{value:x}i"))
        } else {
            self.integer_literal(&format!("{value}i"))
        }
    }

    /// An `f64` as a numeric expression, assuming the target uses IEEE-754
    /// doubles.
    ///
    /// Finite values and infinities round-trip, signed zero included. NaN
    /// becomes `0/0` and deliberately loses its sign, payload, and signaling
    /// state, since only a target-specific runtime construction can rebuild
    /// a bit-exact NaN.
    ///
    /// Integral floats get a `.0` suffix so they stay floats on Lua 5.3+,
    /// unless the version is pinned to a single-number dialect (5.1, 5.2,
    /// Luau). Exponent notation replaces the plain form when it's shorter.
    /// Negatives use unary minus and infinities use `1/0`.
    #[must_use]
    pub fn number_f64(&self, value: f64) -> Expression {
        if value.is_nan() {
            return self.binary_op(self.number("0"), BinOp::Div, self.number("0"));
        }
        if value.is_infinite() {
            let infinity = self.binary_op(self.number("1"), BinOp::Div, self.number("0"));
            return if value < 0.0 { self.unary_op(UnOp::Neg, infinity) } else { infinity };
        }
        if value.is_sign_negative() {
            // Lua has no negative literals; this also keeps -0.0's sign bit.
            return self.unary_op(UnOp::Neg, self.number_f64(value.abs()));
        }
        // `Display` never uses exponent notation, so a missing `.` means the
        // value is integral.
        let mut text = value.to_string();
        let should_mark_float_subtype = self.version.is_none_or(LuaVersion::has_integer_subtype);
        if !text.contains('.') && should_mark_float_subtype {
            text.push_str(".0");
        }
        // Both forms carry shortest round-trip digits; an exponent implies
        // the float subtype on 5.3+ just as `.0` does.
        let exponent = format!("{value:e}");
        if exponent.len() < text.len() {
            text = exponent;
        }
        self.number(&text)
    }

    /// Exact on Lua 5.3+ with 64-bit integers, which is also what an
    /// unpinned synthesizer assumes. On single-number dialects (5.1, 5.2,
    /// Luau), integers outside `[-2^53, 2^53]` can round to the nearest
    /// double. For Luau's separate exact integer type, use
    /// [`Synth::integer_i64`].
    #[must_use]
    pub fn number_i64(&self, value: i64) -> Expression {
        if value == i64::MIN {
            // On 5.3+ the decimal magnitude overflows into a float, while
            // hex wraps to the intended integer. On a single-number dialect
            // hex reads as +2^63, but the decimal magnitude is an exact
            // double, so negating it works there.
            if self.version.is_some_and(|version| !version.has_integer_subtype()) {
                return self.unary_op(UnOp::Neg, self.number("9223372036854775808"));
            }
            return self.number("0x8000000000000000");
        }
        if value < 0 {
            return self.unary_op(UnOp::Neg, self.number(&value.unsigned_abs().to_string()));
        }
        self.number(&value.to_string())
    }

    /// A string literal from unescaped `content`, stored as escaped,
    /// double-quoted token text.
    #[must_use]
    pub fn string(&self, content: &str) -> Expression {
        Expression::StringLiteral(Literal {
            text: encode_string(content.as_bytes()),
            span: self.next_span(),
        })
    }

    /// A string literal from arbitrary bytes, such as a bytecode constant
    /// that isn't UTF-8. Bytes that aren't valid UTF-8 are escaped, so the
    /// token text is always valid UTF-8.
    #[must_use]
    pub fn string_bytes(&self, content: &[u8]) -> Expression {
        Expression::StringLiteral(Literal { text: encode_string(content), span: self.next_span() })
    }

    /// A `[[...]]` long-bracket string at the lowest `=` level whose closer
    /// neither appears in the content nor straddles its end, and which any
    /// `[[` in the content can't nest inside on a dialect that rejects that.
    /// Content with carriage returns (which Lua normalizes) or NUL bytes
    /// (which Luau rejects) falls back to [`Synth::string`].
    #[must_use]
    pub fn long_string(&self, content: &str) -> Expression {
        if content.contains(['\r', '\0']) {
            return self.string(content);
        }
        let must_avoid_level_zero = content.contains("[[")
            && self.version.is_none_or(LuaVersion::has_long_bracket_nesting_error);
        let mut level = usize::from(must_avoid_level_zero);
        for taken_level in taken_closer_levels(content) {
            if taken_level == level {
                level = level.saturating_add(1);
            }
        }
        let equals = "=".repeat(level);
        // Lua swallows a newline directly after the opening bracket.
        let leading_newline = if content.starts_with('\n') { "\n" } else { "" };
        Expression::StringLiteral(Literal {
            text: CompactString::from(format!("[{equals}[{leading_newline}{content}]{equals}]")),
            span: self.next_span(),
        })
    }

    #[must_use]
    pub fn nil(&self) -> Expression {
        Expression::Nil(self.next_span())
    }

    #[must_use]
    pub fn boolean(&self, value: bool) -> Expression {
        if value { Expression::True(self.next_span()) } else { Expression::False(self.next_span()) }
    }

    #[must_use]
    pub fn vararg(&self) -> Expression {
        Expression::VarArg(self.next_span())
    }

    /// A Luau backtick string from text and expressions in source order:
    /// `` `hi {name}!` `` is `vec![Text("hi "), Expr(name), Text("!")]`.
    ///
    /// Text is unescaped; this escapes `` ` ``, `{`, `\`, and control
    /// characters. Adjacent text parts merge, and the lexer's
    /// `InterpolatedStringBegin`/`InterpolatedStringMiddle`/`InterpolatedStringEnd`
    /// segments are built for you.
    #[must_use]
    pub fn interpolated_string(&self, parts: Vec<InterpolatedStringPart<'_>>) -> Expression {
        let span = self.next_span();
        let mut segments = Vec::new();
        let mut text = String::new();
        for part in parts {
            match part {
                InterpolatedStringPart::Text(more) => text.push_str(more),
                InterpolatedStringPart::Expr(expr) => {
                    let escaped = escape_interpolated_text(&text);
                    let kind = if segments.is_empty() {
                        TokenKind::InterpolatedStringBegin(escaped)
                    } else {
                        TokenKind::InterpolatedStringMiddle(escaped)
                    };
                    segments.push(InterpolatedStringSegment {
                        literal: self.token(kind),
                        expr: Some(expr),
                    });
                    text.clear();
                }
            }
        }
        if segments.is_empty() {
            // The lexer gives plain text as `InterpolatedStringBegin("")` followed by
            // `InterpolatedStringEnd(text)`.
            let begin = TokenKind::InterpolatedStringBegin(CompactString::default());
            segments.push(InterpolatedStringSegment { literal: self.token(begin), expr: None });
        }
        let end = TokenKind::InterpolatedStringEnd(escape_interpolated_text(&text));
        segments.push(InterpolatedStringSegment { literal: self.token(end), expr: None });
        Expression::InterpolatedString(Box::new(InterpolatedString { span, segments }))
    }
}

/// The `=` count of every closer in `content`, ascending.
///
/// A closer is a `]=*]`, or a `]=*` the content ends with, which the
/// literal's own closer would finish. One pass finds them all, where asking
/// about each level in turn rereads the content once per level.
fn taken_closer_levels(content: &str) -> Vec<usize> {
    let mut pieces = content.split(']');
    // What comes before the first `]` belongs to no closer.
    pieces.next();
    let mut levels: Vec<usize> =
        pieces.filter(|piece| piece.trim_start_matches('=').is_empty()).map(str::len).collect();
    levels.sort_unstable();
    levels.dedup();
    levels
}

/// Escapes interpolated-string segment text. The lexer stores segment text
/// as written, escapes included, so synthesized text needs them too.
fn escape_interpolated_text(text: &str) -> CompactString {
    let mut raw = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => raw.push_str("\\\\"),
            '`' => raw.push_str("\\`"),
            '{' => raw.push_str("\\{"),
            '\n' => raw.push_str("\\n"),
            '\r' => raw.push_str("\\r"),
            '\t' => raw.push_str("\\t"),
            '\0'..='\x1f' | '\x7f' => raw.push_str(&decimal_escape(u32::from(c))),
            _ => raw.push(c),
        }
    }
    CompactString::from(raw)
}

/// Always three digits: a shorter escape followed by a literal digit
/// (`\57`) would be read as a different byte.
fn decimal_escape(byte_value: u32) -> String {
    format!("\\{byte_value:03}")
}

#[cfg(test)]
mod tests {
    use core::iter;
    use core::time::Duration;
    use std::time::Instant;

    use super::*;

    fn number_text(expr: Expression) -> CompactString {
        let Expression::Number(literal) = expr else {
            panic!("expected number");
        };
        literal.text
    }

    fn string_text(expr: Expression) -> CompactString {
        let Expression::StringLiteral(literal) = expr else {
            panic!("expected string literal");
        };
        literal.text
    }

    #[test]
    fn number_f64_handles_special_values() {
        let synth = Synth::new();

        assert_eq!(number_text(synth.number_f64(1.5)), "1.5");

        // Integral floats keep the float subtype on 5.3+.
        assert_eq!(number_text(synth.number_f64(3.0)), "3.0");

        // Negatives become a unary-minus node (Lua has no negative literals).
        assert!(matches!(synth.number_f64(-2.5), Expression::UnaryOp(_)));
        assert!(matches!(synth.number_f64(-0.0), Expression::UnaryOp(_)));

        // inf -> 1/0, nan -> 0/0, -inf -> -(1/0).
        assert!(matches!(synth.number_f64(f64::INFINITY), Expression::BinaryOp(_)));
        assert!(matches!(synth.number_f64(f64::NAN), Expression::BinaryOp(_)));
        let Expression::UnaryOp(negative_infinity) = synth.number_f64(f64::NEG_INFINITY) else {
            panic!("expected unop");
        };
        assert!(matches!(negative_infinity.operand, Expression::Parenthesized(_)));
    }

    #[test]
    fn number_f64_prefers_exponent_when_shorter() {
        let synth = Synth::new();
        assert_eq!(number_text(synth.number_f64(1e300)), "1e300");

        assert_eq!(number_text(synth.number_f64(100.0)), "1e2");

        // Plain forms win ties.
        assert_eq!(number_text(synth.number_f64(3.0)), "3.0");
        assert_eq!(number_text(synth.number_f64(1.5)), "1.5");
    }

    #[test]
    fn number_i64_handles_extremes() {
        let synth = Synth::new();
        assert_eq!(number_text(synth.number_i64(42)), "42");

        assert!(matches!(synth.number_i64(-7), Expression::UnaryOp(_)));

        // i64::MIN has no decimal literal; the hex form wraps to it.
        assert_eq!(number_text(synth.number_i64(i64::MIN)), "0x8000000000000000");
    }

    #[test]
    fn number_rendering_respects_pinned_number_model() {
        // Plain digits win ties; exponent notation wins when shorter.
        let luau = Synth::new().with_version(LuaVersion::Luau);
        let renderings: [(f64, &str); 6] = [
            (100.0, "100"),
            (1000.0, "1e3"),
            (10000.0, "1e4"),
            (3.0, "3"),
            (1.5, "1.5"),
            (1e300, "1e300"),
        ];
        for (v, expected) in renderings {
            assert_eq!(number_text(luau.number_f64(v)), expected);
        }
        assert!(matches!(luau.number_f64(-0.0), Expression::UnaryOp(_)));

        // Hex would lex as +2^63 on Luau; the decimal magnitude is exact
        // as a float, so i64::MIN becomes its negation.
        let Expression::UnaryOp(mut negated) = luau.number_i64(i64::MIN) else {
            panic!("expected unop");
        };
        assert_eq!(number_text(negated.operand.take()), "9223372036854775808");

        // The integer/float split keeps the markers on 5.3+.
        let lua54 = Synth::new().with_version(LuaVersion::Lua54);
        assert_eq!(number_text(lua54.number_f64(3.0)), "3.0");
        assert_eq!(number_text(lua54.number_f64(100.0)), "1e2");
        assert_eq!(number_text(lua54.number_i64(i64::MIN)), "0x8000000000000000");
    }

    #[test]
    fn nan_encoding_is_deliberately_not_bit_exact() {
        let synth = Synth::new();
        for bits in [
            0x7ff8_0000_0000_0000,
            0xfff8_0000_0000_0000,
            0x7ff8_0000_dead_beef,
            0x7ff0_0000_0000_0001,
        ] {
            let Expression::BinaryOp(division) = synth.number_f64(f64::from_bits(bits)) else {
                panic!("expected NaN expression");
            };
            let [link] = &*division.links else {
                panic!("expected one division");
            };
            assert_eq!(link.op, BinOp::Div);
            assert!(matches!(&division.first, Expression::Number(literal) if literal.text == "0"));
            assert!(matches!(&link.operand, Expression::Number(literal) if literal.text == "0"));
        }
    }

    #[test]
    fn ordinary_luau_numbers_and_exact_integers_are_distinct() {
        let synth = Synth::new().with_version(LuaVersion::Luau);
        // 2^53 + 1, the first integer a double can't hold.
        let value = 0x20_0000_0000_0001;
        let number = number_text(synth.number_i64(value));
        assert_eq!(number, "9007199254740993");
        assert_eq!(number.parse::<f64>().expect("double").to_string(), "9007199254740992");
        let Expression::Integer(integer) = synth.integer_i64(value) else {
            panic!("exact integer")
        };
        assert_eq!(integer.text, "9007199254740993i");
    }

    #[test]
    fn string_stores_escaped_token_text() {
        let synth = Synth::new();
        assert_eq!(string_text(synth.string("a\"b\n")), "\"a\\\"b\\n\"");
    }

    #[test]
    fn string_bytes_escapes_non_utf8() {
        let synth = Synth::new();
        // Escapes are zero-padded to three digits so the following literal
        // digit `7` can't extend them.
        assert_eq!(string_text(synth.string_bytes(&[0xff, b'a', 0x01, b'7'])), "\"\\255a\\0017\"");
    }

    #[test]
    fn long_string_picks_collision_free_level() {
        let synth = Synth::new();
        assert_eq!(string_text(synth.long_string("plain text")), "[[plain text]]");

        assert_eq!(string_text(synth.long_string("has ]] inside")), "[=[has ]] inside]=]");

        // A leading newline would be swallowed; it gets doubled.
        assert_eq!(string_text(synth.long_string("\ntext")), "[[\n\ntext]]");

        // A trailing `]` at level 0 would merge with the closer.
        assert_eq!(string_text(synth.long_string("x]")), "[=[x]]=]");
    }

    #[test]
    fn long_string_skips_every_level_a_closer_in_the_content_takes() {
        let synth = Synth::new();
        for (content, expected) in [
            ("]]", "[=[]]]=]"),
            ("]=]", "[==[]=]]==]"),
            ("]] ]=] ]===]", "[==[]] ]=] ]===]]==]"),
            ("]]]=]==]", "[===[]]]=]==]]===]"),
            ("a]=", "[[a]=]]"),
            ("]] a]=", "[==[]] a]=]==]"),
            ("]=x] ]", "[=[]=x] ]]=]"),
            ("=]", "[=[=]]=]"),
        ] {
            assert_eq!(string_text(synth.long_string(content)), expected, "{content}");
        }
        let lua51 = Synth::new().with_version(LuaVersion::Lua51);
        assert_eq!(string_text(lua51.long_string("[[ ]=]")), "[==[[[ ]=]]==]");
    }

    #[test]
    fn long_string_reads_its_content_once_however_many_levels_it_takes() {
        // A closer at every level up to about 2,800.
        let mut content = String::new();
        let mut level = 0usize;
        while content.len() < 4_000_000 {
            content.push(']');
            content.push_str(&"=".repeat(level));
            content.push(']');
            level = level.saturating_add(1);
        }
        let plain = "x".repeat(content.len());
        let synth = Synth::new();
        let fastest = |text: &str| {
            iter::repeat_with(|| {
                let start = Instant::now();
                let literal = string_text(synth.long_string(text));
                assert!(literal.len() > text.len(), "the literal wraps its content");
                start.elapsed()
            })
            .take(3)
            .min()
            .unwrap_or_default()
        };
        // Asking about each level in turn reads this content 2,800 times.
        let budget = fastest(&plain).saturating_mul(40).saturating_add(Duration::from_millis(20));
        let taken = fastest(&content);
        assert!(taken < budget, "{taken:?} against a budget of {budget:?}");
        let expected_open = format!("[{}[", "=".repeat(level));
        assert!(string_text(synth.long_string(&content)).starts_with(&expected_open));
    }

    #[test]
    fn long_string_carriage_return_falls_back_to_quoted() {
        let synth = Synth::new();
        let Expression::StringLiteral(literal) = synth.long_string("a\r\nb") else {
            panic!("expected string literal");
        };
        assert_eq!(literal.text.as_str(), "\"a\\r\\nb\"");
    }

    #[test]
    fn builds_interpolated_string_with_escapes() {
        let synth = Synth::new();
        let inner = synth.name_expr("value");
        // InterpolatedStringBegin("a") carries the expression and InterpolatedStringEnd the rest.
        let string = synth.interpolated_string(vec![
            InterpolatedStringPart::Text("a"),
            InterpolatedStringPart::Expr(inner),
            InterpolatedStringPart::Text("b{`"),
        ]);

        let Expression::InterpolatedString(interpolated) = &string else {
            panic!("expected interpolated string");
        };
        assert_eq!(interpolated.segments.len(), 2);
        assert!(matches!(
            interpolated.segments[0].literal.kind,
            TokenKind::InterpolatedStringBegin(_)
        ));
        assert!(interpolated.segments[0].expr.is_some());
        let TokenKind::InterpolatedStringEnd(text) = &interpolated.segments[1].literal.kind else {
            panic!("expected InterpolatedStringEnd");
        };
        // `{` and backtick carry their escapes, as the lexer stores them.
        assert_eq!(text.as_str(), "b\\{\\`");
        assert!(interpolated.segments[1].expr.is_none());
    }

    #[test]
    fn interpolated_string_derives_segment_shape() {
        let synth = Synth::new();

        // Plain text keeps the lexer's Begin("") + End(text) pair; adjacent
        // text parts merge.
        let plain = synth.interpolated_string(vec![
            InterpolatedStringPart::Text("a"),
            InterpolatedStringPart::Text("b"),
        ]);
        let Expression::InterpolatedString(plain_node) = &plain else {
            panic!("expected interpolated string");
        };
        assert_eq!(plain_node.segments.len(), 2);
        let TokenKind::InterpolatedStringBegin(begin) = &plain_node.segments[0].literal.kind else {
            panic!("expected InterpolatedStringBegin");
        };
        assert_eq!(begin.as_str(), "");
        let TokenKind::InterpolatedStringEnd(end) = &plain_node.segments[1].literal.kind else {
            panic!("expected InterpolatedStringEnd");
        };
        assert_eq!(end.as_str(), "ab");

        // Two expressions produce a Mid segment between Begin and End.
        let multi = synth.interpolated_string(vec![
            InterpolatedStringPart::Expr(synth.name_expr("x")),
            InterpolatedStringPart::Text("-"),
            InterpolatedStringPart::Expr(synth.name_expr("y")),
        ]);
        let Expression::InterpolatedString(multi_node) = &multi else {
            panic!("expected interpolated string");
        };
        assert_eq!(multi_node.segments.len(), 3);
        assert!(matches!(
            multi_node.segments[1].literal.kind,
            TokenKind::InterpolatedStringMiddle(_)
        ));
        assert!(multi_node.segments[2].expr.is_none());
    }

    #[test]
    fn integer_i64_writes_negatives_in_twos_complement_hex() {
        let synth = Synth::new().with_version(LuaVersion::Luau);
        let Expression::Integer(integer) = synth.integer_i64(-1) else {
            panic!("expected integer literal");
        };
        assert_eq!(integer.text.as_str(), "0xffffffffffffffffi");
    }

    #[test]
    fn long_string_does_not_nest_level_zero_brackets_on_lua51() {
        let synth = Synth::new().with_version(LuaVersion::Lua51);
        let text = string_text(synth.long_string("a[[b"));
        assert!(
            !text.starts_with("[["),
            "stock Lua 5.1 rejects `[[` inside a level-0 long string (\"nesting of [[...]] is \
             deprecated\"), so {text} must use a higher `=` level or the quoted form"
        );
    }
}
