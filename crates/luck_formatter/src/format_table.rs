//! A table of simple positional values packs with `fill`, as many per line as
//! fit.
//!
//! Any other table stays flat if it fits and otherwise breaks one field
//! per line. A table whose comments all sit between fields claims them and
//! breaks one field per line; any other comments are left to the block level.

use luck_ast::{Expression, Field, TableConstructor};
use luck_token::{Span, has_line_break};

use crate::comments::ListKind;
use crate::ir::{Formatter, LineMode};

fn is_fillable(table: &TableConstructor<'_>) -> bool {
    table.fields.iter().all(|field| match field {
        Field::Positional(value) => matches!(
            value,
            Expression::Nil(_)
                | Expression::False(_)
                | Expression::True(_)
                | Expression::Number(_)
                | Expression::Integer(_)
                | Expression::StringLiteral(_)
                | Expression::VarArg(_)
                | Expression::Name(_)
        ),
        Field::Named { .. } | Field::Bracketed { .. } => false,
    })
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the formatter is one type with its node families split by file"
)]
impl<'ast> Formatter<'ast> {
    /// Whether the source spreads the table over several lines with two
    /// fields sharing one of them. A synthetic table has no source and never
    /// does.
    fn is_packed_in_source(&self, table: &TableConstructor<'_>) -> bool {
        let Some((source, version)) = self.comments().source() else {
            return false;
        };
        let fields = &table.fields;
        let (Some(first), Some(last)) = (fields.first(), fields.last()) else {
            return false;
        };
        let has_break =
            |start: u32, end: u32| has_line_break(Span::new(start, end).text(source), version);

        let mut has_shared_line = false;
        let mut is_multi_line = has_break(table.span.start, first.span().start)
            || has_break(last.span().end, table.span.end);
        for (field, next) in fields.iter().zip(fields.iter().skip(1)) {
            if has_break(field.span().end, next.span().start) {
                is_multi_line = true;
            } else {
                has_shared_line = true;
            }
        }
        has_shared_line && is_multi_line
    }

    /// A hugged table breaks in its call's argument list's place.
    pub(crate) fn write_table(&mut self, table: &TableConstructor<'ast>, is_hugged: bool) {
        if self.comments().has_claimable_comments(table.span, table.fields.iter().map(Field::span))
        {
            let list = |f: &mut Self| {
                f.write_commented_list(
                    ("{", "}"),
                    table.span,
                    &table.fields,
                    ListKind::TableFields,
                    Field::span,
                    Self::write_field,
                );
            };
            if is_hugged {
                let group_id = self.group_id();
                self.hugged_group_with_id(group_id, list);
            } else {
                list(self);
            }
            return;
        }

        if table.fields.is_empty() {
            self.token("{}");
            return;
        }

        // A trailing `,` or `;` after the last field keeps the table broken, one
        // field per line unless its values were packed.
        let has_magic_comma = self.options().magic_trailing_comma && table.has_trailing_separator;
        let should_fill =
            is_fillable(table) && (!has_magic_comma || self.is_packed_in_source(table));
        let last_index = table.fields.len().saturating_sub(1);

        let group_id = self.group_id();
        let content = |grouped: &mut Self| {
            grouped.token("{");
            grouped.indent(|indented| {
                indented.soft_line_or_space();

                // A blank line the source has above a field is kept.
                if should_fill {
                    // Every entry but the last carries its own comma, so
                    // the fill measures widths with the comma included.
                    indented.fill(|f| {
                        for (i, field) in table.fields.iter().enumerate() {
                            let separator = (i > 0).then(|| {
                                if f.has_blank_line_above(field.span().start) {
                                    LineMode::Empty
                                } else {
                                    LineMode::SoftOrSpace
                                }
                            });
                            f.fill_entry(separator, |entry| {
                                entry.write_field(field);
                                if i < last_index {
                                    entry.token(",");
                                }
                            });
                        }
                    });
                } else {
                    for (i, field) in table.fields.iter().enumerate() {
                        if i > 0 {
                            indented.token(",");
                            let is_blank = indented.has_blank_line_above(field.span().start);
                            indented.line(if is_blank {
                                LineMode::Empty
                            } else {
                                LineMode::SoftOrSpace
                            });
                        }
                        indented.write_field(field);
                    }
                }

                if has_magic_comma {
                    indented.expand_parent();
                }
                indented.if_group_breaks(group_id, |f| f.token(","));
            });
            grouped.soft_line_or_space();
            grouped.line_suffix_boundary();
            grouped.token("}");
        };
        if is_hugged {
            self.hugged_group_with_id(group_id, content);
        } else {
            self.group_with_id(group_id, content);
        }
    }

    fn write_field(&mut self, field: &Field<'ast>) {
        match field {
            Field::Named { name, value } => {
                self.text(name.text);
                self.space();
                self.token("=");
                self.space();
                self.write_expr(value);
            }
            Field::Bracketed { key, value, .. } => {
                self.write_bracketed_key(key);
                self.space();
                self.token("=");
                self.space();
                self.write_expr(value);
            }
            Field::Positional(value) => self.write_expr(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::render_expr;
    use crate::{FormatOptions, IndentStyle};

    fn print_expr(source: &str, line_width: u16, magic_trailing_comma: bool) -> String {
        let options = FormatOptions {
            line_width,
            indent_style: IndentStyle::Spaces,
            magic_trailing_comma,
            ..FormatOptions::default()
        };
        render_expr(source, options)
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
        // The fill wraps and the group breaks, so a trailing comma appears
        // and the braces sit on their own lines.
        assert_eq!(
            print_expr("{11, 22, 33, 44, 55, 66}", 12, false),
            "\
{
    11, 22,
    33, 44,
    55, 66,
}"
        );
    }

    #[test]
    fn magic_trailing_comma_forces_break() {
        // A source trailing comma with the option on breaks even though it
        // fits.
        assert_eq!(
            print_expr("{a = 1, b = 2,}", 80, true),
            "\
{
    a = 1,
    b = 2,
}"
        );
    }
}
