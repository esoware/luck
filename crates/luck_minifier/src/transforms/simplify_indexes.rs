use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::transform::AstTransform;

use crate::expr::is_valid_identifier;
use crate::tokens::{default_span as sp, make_ident};

/// Convert bracket indexing to dot notation and simplify table constructors.
pub fn simplify(block: Block) -> Block {
    IndexSimplifier.transform_block(block)
}

struct IndexSimplifier;

impl AstTransform for IndexSimplifier {
    fn transform_var(&mut self, var: Var) -> Var {
        let var = self.walk_var(var);
        match var {
            // t["foo"] -> t.foo when "foo" is a valid identifier
            Var::Index(index_expr) => {
                if let Expression::StringLiteral(ref literal) = index_expr.index
                    && let Some(s) = strip_simple_quotes(&literal.text)
                    && is_valid_identifier(&s)
                {
                    return Var::FieldAccess(Box::new(FieldAccess {
                        span: sp(),
                        prefix: index_expr.prefix,
                        name: make_ident(&s),
                    }));
                }
                Var::Index(index_expr)
            }
            other => other,
        }
    }

    fn walk_table_constructor(&mut self, mut table: TableConstructor) -> TableConstructor {
        let can_use_implicit = is_sequential_from_one(&table.fields.items);

        let mut implicit_idx = 0usize;
        table.fields.items = table
            .fields
            .items
            .into_iter()
            .map(|field| {
                match field {
                    // {[1]="a", [2]="b"} -> {"a", "b"} when sequential from 1
                    Field::Bracketed {
                        ref key, ref value, ..
                    } if can_use_implicit => {
                        if let Expression::Number(literal) = key {
                            if let Ok(n) = literal.text.parse::<usize>() {
                                if n == implicit_idx + 1 {
                                    implicit_idx = n;
                                    Field::Positional {
                                        span: sp(),
                                        value: self.transform_expression(value.clone()),
                                    }
                                } else {
                                    field
                                }
                            } else {
                                field
                            }
                        } else {
                            field
                        }
                    }
                    // {["foo"] = val} -> {foo = val}
                    Field::Bracketed {
                        key: Expression::StringLiteral(ref literal),
                        ref value,
                        ..
                    } => {
                        if let Some(s) = strip_simple_quotes(&literal.text) {
                            if is_valid_identifier(&s) {
                                Field::Named {
                                    span: sp(),
                                    name: make_ident(&s),
                                    value: self.transform_expression(value.clone()),
                                }
                            } else {
                                let new_val = self.transform_expression(value.clone());
                                Field::Bracketed {
                                    span: sp(),
                                    key: Expression::StringLiteral(literal.clone()),
                                    value: new_val,
                                }
                            }
                        } else {
                            field
                        }
                    }
                    Field::Bracketed { span, key, value } => Field::Bracketed {
                        span,
                        key: self.transform_expression(key),
                        value: self.transform_expression(value),
                    },
                    Field::Named { span, name, value } => Field::Named {
                        span,
                        name,
                        value: self.transform_expression(value),
                    },
                    Field::Positional { span, value } => Field::Positional {
                        span,
                        value: self.transform_expression(value),
                    },
                }
            })
            .collect();
        table
    }
}

fn strip_simple_quotes(raw: &str) -> Option<String> {
    if (raw.starts_with('"') && raw.ends_with('"'))
        || (raw.starts_with('\'') && raw.ends_with('\''))
    {
        Some(raw[1..raw.len() - 1].to_string())
    } else {
        None
    }
}

fn is_sequential_from_one(fields: &[Field]) -> bool {
    let mut expected = 1usize;
    for (position, field) in fields.iter().enumerate() {
        match field {
            Field::Bracketed {
                key: Expression::Number(literal),
                value,
                ..
            } => {
                if let Ok(n) = literal.text.parse::<usize>() {
                    if n != expected {
                        return false;
                    }
                    expected += 1;
                } else {
                    return false;
                }
                // `{[1] = f()}` truncates f() to one value; `{f()}` in
                // final position expands every return - refuse when the
                // LAST field's value is a call or `...`.
                if position == fields.len() - 1
                    && matches!(value, Expression::FunctionCall(_) | Expression::VarArg(_))
                {
                    return false;
                }
            }
            _ => return false,
        }
    }
    expected > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        assert!(result.errors.is_empty(), "parse failed");
        let block = simplify(result.block);
        luck_codegen::compact(&block, source)
    }

    #[test]
    fn bracket_string_to_dot() {
        let r = apply("return t[\"foo\"]\n");
        assert!(r.contains("t.foo"), "Expected t.foo, got: {r}");
    }

    #[test]
    fn table_key_string_to_name() {
        let r = apply("return {[\"foo\"] = 1}\n");
        assert!(
            r.contains("foo") && !r.contains("[\"foo\"]"),
            "Expected name key, got: {r}"
        );
    }

    #[test]
    fn table_key_keyword_stays_bracketed() {
        let r = apply("return {[\"if\"] = 1}\n");
        assert!(
            r.contains("[\"if\"]"),
            "Keyword key should stay bracketed, got: {r}"
        );
    }

    #[test]
    fn sequential_integer_keys_to_implicit() {
        let r = apply("return {[1]=\"a\", [2]=\"b\", [3]=\"c\"}\n");
        assert!(
            !r.contains("[1]") && !r.contains("[2]") && !r.contains("[3]"),
            "Integer keys should be removed, got: {r}"
        );
    }
}
