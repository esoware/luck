use luck_allocator::ThinBytes;
use luck_token::{BinOp, LuaNumber, UnOp, write_long_string};

use crate::expr::{Expression, InterpolatedSegment};
use crate::leaf::{IntegerLiteral, NumberLiteral, StringLiteral};
use crate::synth::Synth;

/// One piece of a [`Synth::interpolated_string`]: literal text or an
/// interpolated `{expr}`.
#[derive(Debug)]
pub enum InterpolatedStringPart<'text, 'ast> {
    Text(&'text str),
    Expr(Expression<'ast>),
}

impl<'ast> Synth<'ast> {
    /// An `f64` as a numeric expression, assuming the target uses IEEE-754
    /// doubles.
    ///
    /// Finite values and infinities round-trip, signed zero included.
    /// Negatives use unary minus. NaN becomes `0/0` and deliberately loses
    /// its sign, payload, and signaling state, since only a target-specific
    /// runtime construction can rebuild a bit-exact NaN.
    #[must_use]
    pub fn number_f64(&self, value: f64) -> Expression<'ast> {
        self.number_value(LuaNumber::Float(value))
    }

    /// An integer on Lua 5.3+, which is also what an unpinned synthesizer
    /// assumes, and a float on a version pinned to a single-number dialect
    /// (5.1, 5.2, Luau), where one outside `[-2^53, 2^53]` rounds to the
    /// nearest double. For Luau's separate exact integer type, use
    /// [`Synth::integer_i64`].
    ///
    /// Negatives use unary minus, but for `i64::MIN`, which no integer
    /// negates to: it's a literal on 5.3+, which prints as the hex that
    /// wraps to it.
    #[must_use]
    pub fn number_i64(&self, value: i64) -> Expression<'ast> {
        if self.version.is_some_and(|version| !version.has_integer_subtype()) {
            return self.number_f64(LuaNumber::Int(value).to_f64());
        }
        self.number_value(LuaNumber::Int(value))
    }

    /// The literal of `value`, under a `-` when it's negative, or `0/0` for
    /// NaN.
    fn number_value(&self, value: LuaNumber) -> Expression<'ast> {
        let Some((is_negative, magnitude)) = value.literal_magnitude() else {
            let zero = || self.number(LuaNumber::Float(0.0));
            return self.binary_op(zero(), BinOp::Div, zero());
        };
        let literal = self.number(magnitude);
        if is_negative { self.unary_op(UnOp::Neg, literal) } else { literal }
    }

    /// Luau: an exact 64-bit integer literal, which prints as hex when it's
    /// negative, as the bit pattern a literal can't carry a sign for.
    #[must_use]
    pub fn integer_i64(&self, value: i64) -> Expression<'ast> {
        Expression::Integer(self.boxed(IntegerLiteral::new(value, None, self.next_span())))
    }

    fn number(&self, value: LuaNumber) -> Expression<'ast> {
        Expression::Number(self.boxed(NumberLiteral::new(value, None, self.next_span(), true)))
    }

    /// A string literal whose value is `content`.
    #[must_use]
    pub fn string(&self, content: &str) -> Expression<'ast> {
        self.string_bytes(content.as_bytes())
    }

    /// A string literal whose value is arbitrary bytes, such as a bytecode
    /// constant that isn't UTF-8.
    #[must_use]
    pub fn string_bytes(&self, content: &[u8]) -> Expression<'ast> {
        Expression::StringLiteral(self.boxed(self.string_literal(content)))
    }

    /// `content` copied into the arena as a literal's value, at a fresh
    /// span, which the printers spell.
    pub(super) fn string_literal(&self, content: &[u8]) -> StringLiteral<'ast> {
        let value = ThinBytes::new(self.allocator.alloc_slice_copy(content));
        StringLiteral::new(value, None, self.next_span())
    }

    /// A string literal spelled in long brackets, `[[...]]`, at the lowest
    /// level every version reads back as `content`. Content with a carriage
    /// return, which Lua normalizes, or a NUL byte, which ends a Luau source,
    /// is [`Synth::string`] instead.
    #[must_use]
    pub fn long_string(&self, content: &str) -> Expression<'ast> {
        let mut spelling = Vec::new();
        let mut literal = self.string_literal(content.as_bytes());
        if write_long_string(content, &mut spelling) {
            literal.raw = Some(ThinBytes::new(self.allocator.alloc_slice_copy(&spelling)));
        }
        Expression::StringLiteral(self.boxed(literal))
    }

    #[must_use]
    pub fn nil(&self) -> Expression<'ast> {
        Expression::Nil(self.next_span())
    }

    #[must_use]
    pub fn boolean(&self, value: bool) -> Expression<'ast> {
        if value { Expression::True(self.next_span()) } else { Expression::False(self.next_span()) }
    }

    #[must_use]
    pub fn vararg(&self) -> Expression<'ast> {
        Expression::VarArg(self.next_span())
    }

    /// A Luau backtick string from text and expressions in source order:
    /// `` `hi {name}!` `` is `[Text("hi "), Expr(name), Text("!")]`.
    ///
    /// Each text is a value, which the printers escape. Adjacent text parts
    /// merge.
    #[must_use]
    pub fn interpolated_string<'text, Parts>(&self, parts: Parts) -> Expression<'ast>
    where
        Parts: IntoIterator<Item = InterpolatedStringPart<'text, 'ast>>,
    {
        let span = self.next_span();
        let mut scratch = Vec::new();
        let mut text = String::new();
        for part in parts {
            match part {
                InterpolatedStringPart::Text(more) => text.push_str(more),
                InterpolatedStringPart::Expr(expr) => {
                    let piece = self.string_literal(text.as_bytes());
                    scratch.push(InterpolatedSegment::new(piece, expr));
                    text.clear();
                }
            }
        }
        let tail = self.string_literal(text.as_bytes());
        Expression::new_interpolated_string(span, self.list(scratch), tail, self.allocator)
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::Fixture;

    fn number_value(expr: &Expression<'_>) -> LuaNumber {
        let Expression::Number(literal) = expr else {
            panic!("expected number, found {expr:?}");
        };
        assert!(literal.raw.is_none(), "{literal:?}");
        literal.value
    }

    fn string_literal<'ast>(expr: &'ast Expression<'ast>) -> &'ast StringLiteral<'ast> {
        let Expression::StringLiteral(literal) = expr else {
            panic!("expected string literal, found {expr:?}");
        };
        literal
    }

    #[test]
    fn number_f64_handles_special_values() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        assert_eq!(number_value(&synth.number_f64(1.5)), LuaNumber::Float(1.5));
        assert_eq!(number_value(&synth.number_f64(3.0)), LuaNumber::Float(3.0));
        assert_eq!(number_value(&synth.number_f64(f64::INFINITY)), LuaNumber::Float(f64::INFINITY));
        for negative in [-2.5f64, -0.0f64, f64::NEG_INFINITY] {
            let Expression::UnaryOp(negation) = synth.number_f64(negative) else {
                panic!("a literal carries no sign");
            };
            assert_eq!(number_value(&negation.operand), LuaNumber::Float(negative.abs()));
        }
    }

    #[test]
    fn nan_encoding_is_deliberately_not_bit_exact() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
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
            assert_eq!(number_value(&division.first), LuaNumber::Float(0.0));
            assert_eq!(number_value(&link.operand), LuaNumber::Float(0.0));
        }
    }

    #[test]
    fn number_i64_builds_what_the_pinned_dialect_reads() {
        let fixture = Fixture::default();
        let unpinned = fixture.synth();
        assert_eq!(number_value(&unpinned.number_i64(42)), LuaNumber::Int(42));
        assert_eq!(number_value(&unpinned.number_i64(i64::MIN)), LuaNumber::Int(i64::MIN));
        let Expression::UnaryOp(negated) = unpinned.number_i64(-7) else {
            panic!("expected a negation");
        };
        assert_eq!(number_value(&negated.operand), LuaNumber::Int(7));

        let luau = fixture.synth().with_version(LuaVersion::Luau);
        assert_eq!(number_value(&luau.number_i64(42)), LuaNumber::Float(42.0));
        let Expression::UnaryOp(negated_min) = luau.number_i64(i64::MIN) else {
            panic!("expected a negation");
        };
        let two_to_63 = LuaNumber::Float(f64::from_bits(0x43E0_0000_0000_0000));
        assert_eq!(number_value(&negated_min.operand), two_to_63);
        // 2^53 + 1, the first integer a double can't hold.
        let rounded = number_value(&luau.number_i64(0x20_0000_0000_0001));
        assert_eq!(rounded, LuaNumber::Float(9_007_199_254_740_992.0));
    }

    #[test]
    fn integer_i64_keeps_every_bit_pattern() {
        let fixture = Fixture::default();
        let synth = fixture.synth().with_version(LuaVersion::Luau);
        for value in [0, 0x20_0000_0000_0001, -1, i64::MIN] {
            let Expression::Integer(integer) = synth.integer_i64(value) else {
                panic!("expected integer literal");
            };
            assert_eq!(integer.value, value);
        }
    }

    #[test]
    fn strings_hold_their_value_and_no_spelling() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let quoted = synth.string("a\"b\n");
        let literal = string_literal(&quoted);
        assert_eq!(literal.value, &b"a\"b\n"[..]);
        assert_eq!(literal.raw, None);
        let bytes = synth.string_bytes(&[0xff, b'a']);
        assert_eq!(string_literal(&bytes).value, &[0xff, b'a'][..]);
        assert_eq!(string_literal(&bytes).as_str(), None);
    }

    #[test]
    fn a_long_string_keeps_its_long_bracket_spelling() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        for (content, spelling) in [("plain", Some(&b"[[plain]]"[..])), ("a\r\nb", None)] {
            let expr = synth.long_string(content);
            let literal = string_literal(&expr);
            assert_eq!(literal.value, content.as_bytes(), "{content:?}");
            assert_eq!(literal.raw.map(ThinBytes::as_bytes), spelling, "{content:?}");
        }
    }

    #[test]
    fn builds_interpolated_string_pieces_as_values() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let string = synth.interpolated_string([
            InterpolatedStringPart::Text("a"),
            InterpolatedStringPart::Expr(synth.name_expr("value")),
            InterpolatedStringPart::Text("b{`"),
        ]);
        let Expression::InterpolatedString(interpolated) = &string else {
            panic!("expected interpolated string");
        };
        let [segment] = &*interpolated.segments else {
            panic!("expected one expression, found {:?}", interpolated.segments);
        };
        assert_eq!(segment.text.value, &b"a"[..]);
        assert!(matches!(segment.expr, Expression::Name(_)));
        assert_eq!(interpolated.tail.value, &b"b{`"[..]);
    }

    #[test]
    fn interpolated_string_merges_adjacent_text() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let plain = synth.interpolated_string([
            InterpolatedStringPart::Text("a"),
            InterpolatedStringPart::Text("b"),
        ]);
        let Expression::InterpolatedString(plain_node) = &plain else {
            panic!("expected interpolated string");
        };
        assert!(plain_node.segments.is_empty());
        assert_eq!(plain_node.tail.value, &b"ab"[..]);

        let multi = synth.interpolated_string([
            InterpolatedStringPart::Expr(synth.name_expr("x")),
            InterpolatedStringPart::Text("-"),
            InterpolatedStringPart::Expr(synth.name_expr("y")),
        ]);
        let Expression::InterpolatedString(multi_node) = &multi else {
            panic!("expected interpolated string");
        };
        let texts: Vec<&[u8]> =
            multi_node.segments.iter().map(|segment| segment.text.value.as_bytes()).collect();
        assert_eq!(texts, [&b""[..], b"-"]);
        assert!(multi_node.tail.value.is_empty());
    }
}
