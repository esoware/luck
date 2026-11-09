use luck_ast::{
    Block, Expression, Field, FieldAccess, Literal, TableConstructor, Transform, Var,
    is_multi_value, is_valid_identifier_in,
};
use luck_token::{LuaVersion, Span, short_string_body};

use crate::binding::identifier_token;

/// Converts bracket indexing to dot notation and simplifies table
/// constructors.
pub(crate) fn simplify(block: Block, version: LuaVersion) -> Block {
    IndexSimplifier { version }.transform_block(block)
}

struct IndexSimplifier {
    version: LuaVersion,
}

impl IndexSimplifier {
    /// `{["foo"] = val}` becomes `{foo = val}`. A key that isn't a plain
    /// quoted string, such as a long-bracket one, leaves the field as it is.
    fn simplify_string_key(&mut self, span: Span, literal: Literal, value: Expression) -> Field {
        let Some(name) = short_string_body(&literal.text) else {
            return Field::Bracketed { span, key: Expression::StringLiteral(literal), value };
        };
        let new_value = self.transform_expr(value);
        if is_valid_identifier_in(name, self.version) {
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
                    && is_valid_identifier_in(name, self.version)
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
                Field::Positional { span: Span::default(), value: self.transform_expr(value) }
            }
            Field::Bracketed { span, key: Expression::StringLiteral(literal), value } => {
                self.simplify_string_key(span, literal, value)
            }
            Field::Bracketed { span, key, value } => Field::Bracketed {
                span,
                key: self.transform_expr(key),
                value: self.transform_expr(value),
            },
            Field::Named { span, name, value } => {
                Field::Named { span, name, value: self.transform_expr(value) }
            }
            Field::Positional { span, value } => {
                Field::Positional { span, value: self.transform_expr(value) }
            }
        });
        table
    }
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
        Some(Field::Bracketed { value, .. }) if is_multi_value(value)
    );
    !fields.is_empty() && has_sequential_keys && !has_multi_value_tail
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{assert_reparses, run_pass};

    fn apply_version(source: &str, version: LuaVersion) -> String {
        run_pass(source, version, |block| simplify(block, version))
    }

    fn apply(source: &str) -> String {
        apply_version(source, LuaVersion::Lua54)
    }

    #[test]
    fn identifier_keys_become_fields_and_other_keys_stay_bracketed() {
        for key in ["foo", "_bar", "x1", "_", "camelCase"] {
            assert_eq!(apply(&format!("return t[\"{key}\"]\n")), format!("return t.{key}"));
        }
        for key in ["if", "end", "and", "or", "a-b", "123abc", "", "foo bar"] {
            let source = format!("return t[\"{key}\"]\n");
            assert_eq!(apply(&source), format!("return t[\"{key}\"]"), "source: {source}");
        }
    }

    fn fields_for(key: &str, version: LuaVersion) -> (String, String) {
        let index = apply_version(&format!("return t[\"{key}\"]\n"), version);
        let constructor = apply_version(&format!("return {{[\"{key}\"] = 1}}\n"), version);
        assert_reparses(&index, version);
        assert_reparses(&constructor, version);
        (index, constructor)
    }

    fn assert_field_spelling(key: &str, is_plain: bool, versions: &[LuaVersion]) {
        let expected = if is_plain {
            (format!("return t.{key}"), format!("return{{{key}=1}}"))
        } else {
            (format!("return t[\"{key}\"]"), format!("return{{[\"{key}\"]=1}}"))
        };
        for &version in versions {
            assert_eq!(fields_for(key, version), expected, "{key} on {version:?}");
        }
    }

    #[test]
    fn global_key_stays_bracketed_only_on_lua55() {
        assert_field_spelling("global", false, &[LuaVersion::Lua55]);
        assert_field_spelling(
            "global",
            true,
            &[
                LuaVersion::Lua51,
                LuaVersion::Lua52,
                LuaVersion::Lua53,
                LuaVersion::Lua54,
                LuaVersion::Luau,
            ],
        );
    }

    #[test]
    fn goto_key_stays_bracketed_only_where_reserved() {
        assert_field_spelling(
            "goto",
            false,
            &[LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55],
        );
        assert_field_spelling("goto", true, &[LuaVersion::Lua51, LuaVersion::Luau]);
    }

    #[test]
    fn continue_key_is_a_plain_field_on_every_target() {
        assert_field_spelling("continue", true, &LuaVersion::ALL);
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
