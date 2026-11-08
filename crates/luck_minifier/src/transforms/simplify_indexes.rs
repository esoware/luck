use luck_ast::{Block, Expression, Field, FieldAccess, Literal, TableConstructor, Transform, Var};
use luck_token::{Span, TokenKind, short_string_body};

use crate::binding::identifier_token;

/// Converts bracket indexing to dot notation and simplifies table
/// constructors.
pub(crate) fn simplify(block: Block) -> Block {
    IndexSimplifier.transform_block(block)
}

struct IndexSimplifier;

impl IndexSimplifier {
    /// `{["foo"] = val}` becomes `{foo = val}`. A key that isn't a plain
    /// quoted string, such as a long-bracket one, leaves the field as it is.
    fn simplify_string_key(&mut self, span: Span, literal: Literal, value: Expression) -> Field {
        let Some(name) = short_string_body(&literal.text) else {
            return Field::Bracketed { span, key: Expression::StringLiteral(literal), value };
        };
        let new_value = self.transform_expression(value);
        if is_valid_identifier(name) {
            Field::Named {
                span: Span::default(),
                name: identifier_token(name.into()),
                value: new_value,
            }
        } else {
            Field::Bracketed {
                span: Span::default(),
                key: Expression::StringLiteral(literal),
                value: new_value,
            }
        }
    }
}

impl Transform for IndexSimplifier {
    fn transform_var(&mut self, var: Var) -> Var {
        match self.walk_var(var) {
            // t["foo"] becomes t.foo when "foo" is a valid identifier.
            Var::Index(index_expr) => {
                if let Expression::StringLiteral(literal) = &index_expr.index
                    && let Some(name) = short_string_body(&literal.text)
                    && is_valid_identifier(name)
                {
                    return Var::FieldAccess(Box::new(FieldAccess {
                        span: Span::default(),
                        prefix: index_expr.prefix,
                        name: identifier_token(name.into()),
                    }));
                }
                Var::Index(index_expr)
            }
            other @ (Var::Name(_) | Var::FieldAccess(_)) => other,
        }
    }

    fn transform_table_constructor(&mut self, mut table: TableConstructor) -> TableConstructor {
        let can_use_implicit = is_sequential_from_one(&table.fields.items);

        table.fields = table.fields.map_items(|field| match field {
            // {[1]="a", [2]="b"} becomes {"a", "b"} when the keys run
            // sequentially from 1.
            Field::Bracketed { value, .. } if can_use_implicit => {
                Field::Positional { span: Span::default(), value: self.transform_expression(value) }
            }
            Field::Bracketed { span, key: Expression::StringLiteral(literal), value } => {
                self.simplify_string_key(span, literal, value)
            }
            Field::Bracketed { span, key, value } => Field::Bracketed {
                span,
                key: self.transform_expression(key),
                value: self.transform_expression(value),
            },
            Field::Named { span, name, value } => {
                Field::Named { span, name, value: self.transform_expression(value) }
            }
            Field::Positional { span, value } => {
                Field::Positional { span, value: self.transform_expression(value) }
            }
        });
        table
    }
}

/// The 21 keywords every target reserves, plus `goto` and Luau's
/// `continue`. One list serves every target, so those two stay bracketed even
/// where they're plain names.
const LUA_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
    "continue", // Luau
];

fn is_valid_identifier(name: &str) -> bool {
    TokenKind::is_identifier_shaped(name) && !LUA_KEYWORDS.contains(&name)
}

fn is_sequential_from_one(fields: &[Field]) -> bool {
    let has_sequential_keys = (1usize..).zip(fields).all(|(expected, field)| {
        matches!(
            field,
            Field::Bracketed { key: Expression::Number(literal), .. }
                if literal.text.parse::<usize>() == Ok(expected)
        )
    });
    // `{[1] = f()}` truncates f() to one value, while `{f()}` in final
    // position expands every return, so refuse when the last field's value is
    // a call or `...`.
    let has_multi_value_tail = matches!(
        fields.last(),
        Some(Field::Bracketed { value: Expression::FunctionCall(_) | Expression::VarArg(_), .. })
    );
    !fields.is_empty() && has_sequential_keys && !has_multi_value_tail
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::run_pass;

    fn apply(source: &str) -> String {
        run_pass(source, LuaVersion::Lua54, simplify)
    }

    #[test]
    fn validates_identifiers() {
        assert!(is_valid_identifier("foo"));
        assert!(is_valid_identifier("_bar"));
        assert!(is_valid_identifier("x1"));
        assert!(is_valid_identifier("_"));
        assert!(is_valid_identifier("camelCase"));

        assert!(!is_valid_identifier("if"));
        assert!(!is_valid_identifier("end"));
        assert!(!is_valid_identifier("and"));
        assert!(!is_valid_identifier("or"));

        assert!(!is_valid_identifier("a-b"));
        assert!(!is_valid_identifier("123abc"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("foo bar"));
    }

    #[test]
    fn bracket_string_to_dot() {
        let result = apply("return t[\"foo\"]\n");
        assert!(result.contains("t.foo"), "Expected t.foo, got: {result}");
    }

    #[test]
    fn table_key_string_to_name() {
        let result = apply("return {[\"foo\"] = 1}\n");
        assert!(
            result.contains("foo") && !result.contains("[\"foo\"]"),
            "Expected name key, got: {result}"
        );
    }

    #[test]
    fn table_key_keyword_stays_bracketed() {
        let result = apply("return {[\"if\"] = 1}\n");
        assert!(result.contains("[\"if\"]"), "Keyword key should stay bracketed, got: {result}");
    }

    #[test]
    fn sequential_integer_keys_to_implicit() {
        let result = apply("return {[1]=\"a\", [2]=\"b\", [3]=\"c\"}\n");
        assert!(
            !result.contains("[1]") && !result.contains("[2]") && !result.contains("[3]"),
            "Integer keys should be removed, got: {result}"
        );
    }
}
