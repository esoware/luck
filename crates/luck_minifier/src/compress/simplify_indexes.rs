//! `t["foo"]` becomes `t.foo`, `{["foo"] = 1}` becomes `{foo = 1}`, and
//! `{[1] = a, [2] = b}` becomes `{a, b}`.

use luck_ast::{Expression, Field, Name, Suffix, TableConstructor, is_valid_identifier};

/// The name a `["name"]` key's value is, when it's an identifier, however
/// the key is spelled.
fn key_name<'ast>(key: &Expression<'ast>) -> Option<Name<'ast>> {
    let Expression::StringLiteral(literal) = key else {
        return None;
    };
    let name = literal.as_str()?;
    is_valid_identifier(name).then_some(Name::new(name.into(), None, literal.span))
}

/// Says whether it rewrote the suffix.
pub(super) fn index_to_field(suffix: &mut Suffix<'_>) -> bool {
    let Suffix::Index { index, .. } = suffix else {
        return false;
    };
    let Some(name) = key_name(index) else {
        return false;
    };
    *suffix = Suffix::Field(name);
    true
}

/// Says whether it rewrote any field.
pub(super) fn simplify_table(table: &mut TableConstructor<'_>) -> bool {
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
        if let Some(name) = key_name(key) {
            *field = Field::Named { name, value: value.take() };
            has_changed = true;
        }
    }
    has_changed
}

/// A key is a number however it's spelled, so `[1.0]` is positional too.
fn is_sequential_from_one(fields: &[Field<'_>]) -> bool {
    let has_sequential_keys = (1_u32..).zip(fields).all(|(expected, field)| {
        matches!(
            field,
            Field::Bracketed { key: Expression::Number(literal), .. }
                if literal.value.to_bits() == f64::from(expected).to_bits()
        )
    });
    // `{[1] = f()}` truncates f() to one value, while `{f()}` in final
    // position expands every return, so refuse when the last field's value is
    // a call or `...`.
    let has_multi_value_tail = matches!(
        fields.last(),
        Some(Field::Bracketed { value, .. }) if value.is_multi_value()
    );
    !fields.is_empty() && has_sequential_keys && !has_multi_value_tail
}

#[cfg(test)]
mod tests {
    use crate::fixtures::{NO_PASSES, minify_with};
    use crate::transform_config::TransformConfig;

    const INDEXES: TransformConfig = TransformConfig { simplify_indexes: true, ..NO_PASSES };

    fn apply(source: &str) -> String {
        minify_with(source, &INDEXES)
    }

    #[test]
    fn identifier_keys_become_fields_and_other_keys_stay_bracketed() {
        // `continue`, `export`, `const`, and `type` are keywords only where a
        // statement starts.
        for k in ["foo", "_bar", "x1", "_", "camelCase", "continue", "export", "const", "type"] {
            assert_eq!(apply(&format!("return t[\"{k}\"]\n")), format!("return t.{k}"));
            assert_eq!(apply(&format!("return {{[\"{k}\"] = 1}}\n")), format!("return{{{k}=1}}"));
        }
        for k in ["if", "end", "and", "or", "a-b", "123abc", "", "foo bar"] {
            let source = format!("return t[\"{k}\"]\n");
            assert_eq!(apply(&source), format!("return t[\"{k}\"]"), "source: {source}");
        }
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
        assert_eq!(apply("return {[1.0]=a, [2]=b}\n"), "return{a,b}");
    }
}
