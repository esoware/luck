//! `t["foo"]` becomes `t.foo`, `{["foo"] = 1}` becomes `{foo = 1}`, and
//! `{[1] = a, [2] = b}` becomes `{a, b}`.

use luck_ast::{
    Expression, Field, Name, Suffix, TableConstructor, is_multi_value, is_valid_identifier_in,
};
use luck_token::{LuaNumber, LuaVersion};

/// The name a `["name"]` key's value is, when it's an identifier, however
/// the key is spelled.
fn key_name<'ast>(key: &Expression<'ast>, version: LuaVersion) -> Option<Name<'ast>> {
    let Expression::StringLiteral(literal) = key else {
        return None;
    };
    let name = literal.as_str()?;
    is_valid_identifier_in(name, version).then_some(Name::new(name.into(), None, literal.span))
}

/// Says whether it rewrote the suffix.
pub(super) fn index_to_field(suffix: &mut Suffix<'_>, version: LuaVersion) -> bool {
    let Suffix::Index { index, .. } = suffix else {
        return false;
    };
    let Some(name) = key_name(index, version) else {
        return false;
    };
    *suffix = Suffix::Field(name);
    true
}

/// Says whether it rewrote any field.
pub(super) fn simplify_table(table: &mut TableConstructor<'_>, version: LuaVersion) -> bool {
    let can_use_implicit = is_sequential_from_one(&table.fields);
    let mut has_changed = false;
    for field in &mut table.fields {
        let Field::Bracketed { key, value, .. } = field else {
            continue;
        };
        if can_use_implicit {
            *field = Field::Positional(value.take());
            has_changed = true;
            continue;
        }
        if let Some(name) = key_name(key, version) {
            *field = Field::Named { name, value: value.take() };
            has_changed = true;
        }
    }
    has_changed
}

/// A float key with an integer's value is that integer's key (Lua 5.3+), so
/// `[1.0]` is positional too.
fn is_sequential_from_one(fields: &[Field<'_>]) -> bool {
    let has_sequential_keys = (1i64..).zip(fields).all(|(expected, field)| {
        matches!(
            field,
            Field::Bracketed { key: Expression::Number(literal), .. }
                if literal.value == LuaNumber::Int(expected)
                    || literal.value == LuaNumber::Float(LuaNumber::Int(expected).to_f64())
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
    use crate::fixtures::{NO_PASSES, minify_with};
    use crate::transform_config::TransformConfig;

    const INDEXES: TransformConfig = TransformConfig { simplify_indexes: true, ..NO_PASSES };

    fn apply_version(source: &str, version: LuaVersion) -> String {
        minify_with(source, version, &INDEXES)
    }

    fn apply(source: &str) -> String {
        apply_version(source, LuaVersion::Lua54)
    }

    #[test]
    fn identifier_keys_become_fields_and_other_keys_stay_bracketed() {
        for k in ["foo", "_bar", "x1", "_", "camelCase"] {
            assert_eq!(apply(&format!("return t[\"{k}\"]\n")), format!("return t.{k}"));
            assert_eq!(apply(&format!("return {{[\"{k}\"] = 1}}\n")), format!("return{{{k}=1}}"));
        }
        for k in ["if", "end", "and", "or", "a-b", "123abc", "", "foo bar"] {
            let source = format!("return t[\"{k}\"]\n");
            assert_eq!(apply(&source), format!("return t[\"{k}\"]"), "source: {source}");
        }
    }

    fn assert_field_spelling(key: &str, is_plain: bool, versions: &[LuaVersion]) {
        let expected = if is_plain {
            (format!("return t.{key}"), format!("return{{{key}=1}}"))
        } else {
            (format!("return t[\"{key}\"]"), format!("return{{[\"{key}\"]=1}}"))
        };
        for &version in versions {
            let index = apply_version(&format!("return t[\"{key}\"]\n"), version);
            let constructor = apply_version(&format!("return {{[\"{key}\"] = 1}}\n"), version);
            assert_eq!((index, constructor), expected, "{key} on {version:?}");
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
    fn sequential_integer_keys_become_positional() {
        assert_eq!(
            apply("return {[1]=\"a\", [2]=\"b\", [3]=\"c\"}\n"),
            "return{\"a\",\"b\",\"c\"}"
        );
        // `{[1] = f()}` truncates, and `{f()}` wouldn't.
        assert_eq!(apply("return {[1]=f()}\n"), "return{[1]=f()}");
        assert_eq!(apply("return {[1]=a, [3]=b}\n"), "return{[1]=a,[3]=b}");
    }
}
