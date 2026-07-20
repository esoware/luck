//! Table constructor layout: `impl Format` for `TableConstructor` and `Field`.
//!
//! Simple all-positional tables pack with `fill` (as many entries per line as
//! fit); tables with keyed fields group + indent and break one field per line.
//! Comments inside a table are drained at the block level after the statement.

use luck_ast::expr::{Expression, TableConstructor};
use luck_ast::shared::Field;

use crate::ir::*;
use crate::tokens::write_token;

/// Fill packing is only for tables whose fields are all positional simple
/// values; a keyed or nested/compound value wants one-per-line breaking.
fn use_fill_mode(table: &TableConstructor) -> bool {
    table.fields.iter().all(|field| match field {
        Field::Positional { value, .. } => matches!(
            value,
            Expression::Nil(_)
                | Expression::False(_)
                | Expression::True(_)
                | Expression::Number(_)
                | Expression::StringLiteral(_)
                | Expression::VarArg(_)
                | Expression::Var(_)
        ),
        Field::Named { .. } | Field::Bracketed { .. } => false,
    })
}

impl Format for TableConstructor {
    fn fmt(&self, f: &mut Formatter) {
        if self.fields.is_empty() {
            // Any dangling comments were drained at the block level.
            token("{}").fmt(f);
            return;
        }

        let use_fill = use_fill_mode(self);
        // A trailing comma/semicolon after the last field is the user asking
        // for multi-line layout (Black/Prettier magic trailing comma).
        let force_expand = f.options.magic_trailing_comma && self.fields.has_trailing_separator;
        let field_count = self.fields.len();

        let group_id = f.group_id();
        group_with_id(
            group_id,
            format_with(|f| {
                token("{").fmt(f);
                indent(format_with(|f| {
                    soft_line_or_space().fmt(f);

                    if use_fill && !force_expand {
                        // Each entry carries its own trailing comma (all but the
                        // last) so the fill measures comma-inclusive widths.
                        let entries: Vec<_> = self
                            .fields
                            .iter()
                            .enumerate()
                            .map(|(index, field)| {
                                format_with(move |f| {
                                    field.fmt(f);
                                    if index + 1 < field_count {
                                        token(",").fmt(f);
                                    }
                                })
                            })
                            .collect();
                        let refs: Vec<&dyn Format> =
                            entries.iter().map(|entry| entry as &dyn Format).collect();
                        fill(LineMode::SoftOrSpace, &refs).fmt(f);
                    } else {
                        for (index, field) in self.fields.iter().enumerate() {
                            field.fmt(f);
                            if index + 1 < field_count {
                                token(",").fmt(f);
                                soft_line_or_space().fmt(f);
                            }
                        }
                    }

                    if force_expand {
                        expand_parent().fmt(f);
                    }
                    // Trailing comma only when the table breaks across lines.
                    if_group_breaks(group_id, token(",")).fmt(f);
                }))
                .fmt(f);
                soft_line_or_space().fmt(f);
                line_suffix_boundary().fmt(f);
                token("}").fmt(f);
            }),
        )
        .fmt(f);
    }
}

impl Format for Field {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Field::Named { name, value, .. } => {
                write_token(f, name);
                crate::write!(f, [space(), token("="), space()]);
                value.fmt(f);
            }
            Field::Bracketed { key, value, .. } => {
                token("[").fmt(f);
                if crate::format_expr::starts_with_bracket(key) {
                    crate::write!(f, [space()]);
                    key.fmt(f);
                    crate::write!(f, [space()]);
                } else {
                    key.fmt(f);
                }
                crate::write!(f, [token("]"), space(), token("="), space()]);
                value.fmt(f);
            }
            Field::Positional { value, .. } => value.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comments::Comments;
    use crate::printer::{PrinterOptions, print};
    use luck_ast::stmt::LastStatement;
    use luck_token::LuaVersion;

    fn print_expr(source: &str, line_width: u16, magic_trailing_comma: bool) -> String {
        let wrapped = format!("return {source}\n");
        let parsed = luck_parser::parse(&wrapped, LuaVersion::Luau);
        let last = parsed.block.last_stmt.expect("wrapped return statement");
        let LastStatement::Return(ret) = last.as_ref() else {
            panic!("expected a return statement");
        };
        let expr = &ret.exprs.items[0];

        let options = crate::FormatOptions {
            line_width,
            magic_trailing_comma,
            ..crate::FormatOptions::default()
        };
        let mut formatter = Formatter::with_context(options, Comments::none());
        expr.fmt(&mut formatter);
        let group_count = formatter.group_count();
        let elements = formatter.into_elements();
        print(
            &elements,
            group_count,
            &PrinterOptions {
                line_width,
                use_tabs: false,
                indent_width: 4,
            },
        )
    }

    #[test]
    fn empty_table() {
        assert_eq!(print_expr("{}", 80, false), "{}");
    }

    #[test]
    fn positional_table_packs_flat() {
        assert_eq!(print_expr("{1, 2, 3}", 80, false), "{ 1, 2, 3 }");
    }

    #[test]
    fn keyed_table_flat() {
        assert_eq!(print_expr("{a = 1, b = 2}", 80, false), "{ a = 1, b = 2 }");
    }

    #[test]
    fn bracketed_field_flat() {
        assert_eq!(print_expr("{[1] = 2}", 80, false), "{ [1] = 2 }");
    }

    #[test]
    fn positional_table_fills_when_wide() {
        // Narrow width forces the fill to wrap; the group breaks so a trailing
        // comma appears and the braces sit on their own lines.
        let output = print_expr("{11, 22, 33, 44, 55, 66}", 12, false);
        assert!(output.starts_with("{\n"));
        assert!(output.contains('\n'));
        assert!(output.trim_end().ends_with("\n}"));
    }

    #[test]
    fn magic_trailing_comma_forces_break() {
        // Source trailing comma with the option on breaks even though it fits.
        let output = print_expr("{a = 1, b = 2,}", 80, true);
        assert!(output.starts_with("{\n"));
        assert!(output.contains("a = 1,"));
        assert!(output.contains("b = 2,"));
    }
}
