use luck_allocator::ThinBytes;
use luck_token::{BinOp, UnOp, write_long_string};

use crate::expr::{Expression, InterpolatedSegment};
use crate::leaf::{NumberLiteral, StringLiteral};
use crate::synth::Synth;

/// One piece of a [`Synth::interpolated_string`]: literal text or an
/// interpolated `{expr}`.
#[derive(Debug)]
pub enum InterpolatedStringPart<'text, 'ast> {
    Text(&'text str),
    Expr(Expression<'ast>),
}

impl<'ast> Synth<'ast> {
    /// An `f64` as a numeric expression.
    ///
    /// Finite values and infinities round-trip, signed zero included.
    /// Negatives use unary minus. NaN becomes `0/0` and deliberately loses
    /// its sign, payload, and signaling state, which no constant expression
    /// can rebuild.
    pub fn number_f64(&self, value: f64) -> Expression<'ast> {
        if value.is_nan() {
            let zero = || self.number(0.0);
            return self.binary_op(zero(), BinOp::Div, zero());
        }
        let literal = self.number(value.abs());
        if value.is_sign_negative() { self.unary_op(UnOp::Neg, literal) } else { literal }
    }

    fn number(&self, value: f64) -> Expression<'ast> {
        Expression::Number(self.boxed(NumberLiteral::new(value, None, self.next_span(), true)))
    }

    /// A string literal whose value is `content`.
    pub fn string(&self, content: &str) -> Expression<'ast> {
        self.string_bytes(content.as_bytes())
    }

    /// A string literal whose value is arbitrary bytes, such as a bytecode
    /// constant that isn't UTF-8.
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
    /// level that reads back as `content`. Content with a carriage return,
    /// which a long bracket's CRLF loses, or a NUL byte, which ends a source,
    /// is [`Synth::string`] instead.
    pub fn long_string(&self, content: &str) -> Expression<'ast> {
        let mut spelling = Vec::new();
        let mut literal = self.string_literal(content.as_bytes());
        if write_long_string(content, &mut spelling) {
            literal.raw = Some(ThinBytes::new(self.allocator.alloc_slice_copy(&spelling)));
        }
        Expression::StringLiteral(self.boxed(literal))
    }

    pub fn nil(&self) -> Expression<'ast> {
        Expression::Nil(self.next_span())
    }

    pub fn boolean(&self, value: bool) -> Expression<'ast> {
        if value { Expression::True(self.next_span()) } else { Expression::False(self.next_span()) }
    }

    pub fn vararg(&self) -> Expression<'ast> {
        Expression::VarArg(self.next_span())
    }

    /// A backtick string from text and expressions in source order:
    /// `` `hi {name}!` `` is `[Text("hi "), Expr(name), Text("!")]`.
    ///
    /// Each text is a value, which the printers escape. Adjacent text parts
    /// merge.
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
    use super::*;
    use crate::fixtures::Fixture;

    fn number_value(expr: &Expression<'_>) -> f64 {
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
    fn number_f64_puts_a_negative_value_under_a_minus() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        assert_eq!(number_value(&synth.number_f64(1.5)), 1.5);
        assert_eq!(number_value(&synth.number_f64(3.0)), 3.0);
        assert_eq!(number_value(&synth.number_f64(f64::INFINITY)), f64::INFINITY);
        for negative in [-2.5, -0.0, f64::NEG_INFINITY] {
            let Expression::UnaryOp(negation) = synth.number_f64(negative) else {
                panic!("a literal carries no sign");
            };
            assert_eq!(number_value(&negation.operand).to_bits(), negative.abs().to_bits());
        }
    }

    #[test]
    fn every_nan_builds_zero_over_zero() {
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
            assert_eq!(number_value(&division.first).to_bits(), 0.0f64.to_bits());
            assert_eq!(number_value(&link.operand).to_bits(), 0.0f64.to_bits());
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
