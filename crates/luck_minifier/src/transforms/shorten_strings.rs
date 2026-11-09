use luck_ast::{Block, Expression, Literal, Transform};
use luck_token::{LuaVersion, Span, decode_string, encode_string};

/// Going through the decoded bytes keeps every escape, UTF-8 sequence, and
/// arbitrary byte exact.
pub(crate) fn shorten(block: Block, version: LuaVersion) -> Block {
    StringShortener { version }.transform_block(block)
}

struct StringShortener {
    version: LuaVersion,
}

impl Transform for StringShortener {
    fn transform_expr(&mut self, expr: Expression) -> Expression {
        match self.walk_expr(expr) {
            Expression::StringLiteral(literal) => {
                if let Some(bytes) = decode_string(&literal.text, self.version) {
                    let candidate = encode_string(&bytes);
                    if candidate.len() < literal.text.len() {
                        return Expression::StringLiteral(Literal {
                            text: candidate,
                            span: Span::default(),
                        });
                    }
                }
                Expression::StringLiteral(literal)
            }
            other @ (Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Var(_)
            | Expression::FunctionCall(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::TypeInstantiation(_)
            | Expression::Error(_)) => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{assert_reparses, run_pass};

    fn apply(source: &str) -> String {
        run_pass(source, LuaVersion::Lua54, |block| shorten(block, LuaVersion::Lua54))
    }

    #[test]
    fn long_bracket_to_quoted() {
        let result = apply("local x = [[hello]]");
        assert_eq!(result, "local x=\"hello\"");
    }

    #[test]
    fn long_bracket_with_newline_becomes_escaped() {
        // The escaped form is both shorter and exact.
        let result = apply("local x = [[hello\nworld]]");
        assert_eq!(result, "local x=\"hello\\nworld\"");
        assert_reparses(&result, LuaVersion::Lua54);
    }

    #[test]
    fn long_bracket_with_quotes_kept_when_not_shorter() {
        // Escaping the quotes makes the candidate the same length, so there
        // is no rewrite and no churn.
        let result = apply("local x = [[say \"hi\"]]");
        assert_eq!(result, "local x=[[say \"hi\"]]");
    }

    #[test]
    fn leveled_bracket_to_quoted() {
        let result = apply("local x = [=[hello]=]");
        assert_eq!(result, "local x=\"hello\"");
    }

    #[test]
    fn already_optimal_quoted_string_unchanged() {
        let result = apply("local x = \"hello\"");
        assert_eq!(result, "local x=\"hello\"");
    }

    #[test]
    fn numeric_escapes_decode_to_plain_text() {
        let result = apply("local x = \"\\097\\098\\099\"");
        assert_eq!(result, "local x=\"abc\"");
    }

    #[test]
    fn nul_escape_shortened_but_exact() {
        let result = apply("local x = \"\\000\"");
        assert_eq!(result, "local x=\"\\0\"");
        assert_reparses(&result, LuaVersion::Lua54);
    }

    #[test]
    fn escaped_backslash_before_digits_stays_escaped() {
        // `\\097` is a literal backslash followed by "097", not the byte 97.
        let result = apply("local x = \"\\\\097\"");
        assert!(result.contains("\\\\097"), "backslash + digits must survive: {result}");
        assert_reparses(&result, LuaVersion::Lua54);
    }

    #[test]
    fn utf8_survives_with_escapes() {
        let result = apply("local x = \"\u{e9}\\097\\098\\099\"");
        assert_eq!(result, "local x=\"\u{e9}abc\"");
        assert_reparses(&result, LuaVersion::Lua54);
    }

    #[test]
    fn single_quoted_string_unchanged() {
        // The re-encoded form is the same length, so nothing changes.
        let result = apply("local x = 'hello'");
        assert_eq!(result, "local x='hello'");
    }

    #[test]
    fn shortens_strings_nested_in_a_table() {
        let result = apply("local t = {[[hello]], [[world]]}");
        assert!(result.contains("\"hello\""));
        assert!(result.contains("\"world\""));
    }
}
