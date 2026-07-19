use luck_ast::expr::Expression;
use luck_ast::shared::Block;
use luck_ast::transform::AstTransform;
use luck_token::token::{Token, TokenKind};
use luck_token::{LuaVersion, Span};

use crate::expr::{decode_string_literal, encode_string_literal};

/// Shorten string literals by re-encoding their decoded byte value in
/// canonical quoted form and keeping whichever spelling is shorter.
///
/// Decode -> encode is exact (full escape semantics, UTF-8 and arbitrary
/// bytes preserved), replacing the old textual scanner that reinterpreted
/// bytes as chars and mis-tracked escape state.
pub fn shorten(block: Block, version: LuaVersion) -> Block {
    StringShortener { version }.transform_block(block)
}

struct StringShortener {
    version: LuaVersion,
}

impl AstTransform for StringShortener {
    fn transform_expression(&mut self, expr: Expression) -> Expression {
        let expr = self.walk_expression(expr);

        match expr {
            Expression::StringLiteral(ref token) => {
                if let TokenKind::StringLiteral(ref raw) = token.kind
                    && let Some(bytes) = decode_string_literal(raw, self.version)
                {
                    let candidate = encode_string_literal(&bytes);
                    if candidate.len() < raw.len() {
                        return Expression::StringLiteral(Token::new(
                            TokenKind::StringLiteral(candidate.into()),
                            Span::default(),
                        ));
                    }
                }
                expr
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        assert!(
            result.errors.is_empty(),
            "parse failed: {:?}",
            result.errors
        );
        let block = shorten(result.block, luck_token::LuaVersion::Lua54);
        luck_codegen::compact(&block, source)
    }

    fn reparses(output: &str) {
        let reparse = luck_parser::parse(output, luck_token::LuaVersion::Lua54);
        assert!(reparse.errors.is_empty(), "output must reparse: {output}");
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
        reparses(&result);
    }

    #[test]
    fn long_bracket_with_quotes_kept_when_not_shorter() {
        // Escaping the quotes makes the candidate the same length -
        // no rewrite, no churn.
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
    fn numeric_escape_simplified_in_full_transform() {
        let result = apply("local x = \"\\097\\098\\099\"");
        assert_eq!(result, "local x=\"abc\"");
    }

    #[test]
    fn nul_escape_shortened_but_exact() {
        let result = apply("local x = \"\\000\"");
        assert_eq!(result, "local x=\"\\0\"");
        reparses(&result);
    }

    #[test]
    fn escaped_backslash_before_digits_stays_escaped() {
        // `\\097` is a literal backslash followed by "097" - the old
        // scanner ate it down to "a".
        let result = apply("local x = \"\\\\097\"");
        assert!(
            result.contains("\\\\097"),
            "backslash + digits must survive: {result}"
        );
        reparses(&result);
    }

    #[test]
    fn utf8_survives_with_escapes() {
        let result = apply("local x = \"\u{e9}\\097\\098\\099\"");
        assert_eq!(result, "local x=\"\u{e9}abc\"");
        reparses(&result);
    }

    #[test]
    fn single_quoted_string_unchanged() {
        // Same length re-encoded - no churn.
        let result = apply("local x = 'hello'");
        assert_eq!(result, "local x='hello'");
    }

    #[test]
    fn nested_string_in_table() {
        let result = apply("local t = {[[hello]], [[world]]}");
        assert!(result.contains("\"hello\""));
        assert!(result.contains("\"world\""));
    }
}
