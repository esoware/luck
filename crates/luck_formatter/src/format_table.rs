//! A table of simple positional values packs with `fill`, as many per line as
//! fit.
//!
//! Any other table stays flat if it fits and otherwise breaks one field
//! per line. A table whose comments all sit between fields claims them and
//! breaks one field per line; any other comments are left to the block level.

use luck_ast::{Expression, Field, TableConstructor};

use crate::comments::{ListSeparator, write_commented_list};
use crate::format_expr::BracketedKey;
use crate::ir::{
    Format, Formatter, expand_parent, fill, format_with, group_with_id, if_group_breaks, indent,
    line_suffix_boundary, separated, soft_line_or_space, space, token,
};

fn is_fillable(table: &TableConstructor) -> bool {
    table.fields.iter().all(|field| match field {
        Field::Positional { value, .. } => matches!(
            value,
            Expression::Nil(_)
                | Expression::False(_)
                | Expression::True(_)
                | Expression::Number(_)
                | Expression::Integer(_)
                | Expression::StringLiteral(_)
                | Expression::VarArg(_)
                | Expression::Var(_)
        ),
        Field::Named { .. } | Field::Bracketed { .. } => false,
    })
}

impl Format for TableConstructor {
    fn fmt(&self, f: &mut Formatter) {
        if f.comments().has_claimable_comments(self.span, self.fields.items.iter().map(Field::span))
        {
            write_commented_list(
                f,
                ("{", "}"),
                self.span,
                &self.fields.items,
                Field::span,
                ListSeparator::AfterEachItem,
            );
            return;
        }

        if self.fields.is_empty() {
            token("{}").fmt(f);
            return;
        }

        let is_magic = f.options().magic_trailing_comma;
        // A trailing `,` or `;` after the last field asks for the expanded
        // layout.
        let should_force_expand = is_magic && self.fields.has_trailing_separator;
        let should_fill = is_fillable(self) && !should_force_expand;
        let last_index = self.fields.len().saturating_sub(1);

        let group_id = f.group_id();
        group_with_id(
            group_id,
            (
                token("{"),
                indent(format_with(|indented| {
                    soft_line_or_space().fmt(indented);

                    if should_fill {
                        // Every entry but the last carries its own comma, so
                        // the fill measures widths with the comma included.
                        let entries: Vec<_> = self
                            .fields
                            .iter()
                            .enumerate()
                            .map(|(i, field)| (field, (i < last_index).then_some(token(","))))
                            .collect();
                        fill(&entries).fmt(indented);
                    } else {
                        separated(&self.fields.items, (token(","), soft_line_or_space()))
                            .fmt(indented);
                    }

                    if should_force_expand {
                        expand_parent().fmt(indented);
                    }
                    // A trailing comma means one field per line under the
                    // magic comma option, so a packed fill can't carry one.
                    if !(should_fill && is_magic) {
                        if_group_breaks(group_id, token(",")).fmt(indented);
                    }
                })),
                soft_line_or_space(),
                line_suffix_boundary(),
                token("}"),
            ),
        )
        .fmt(f);
    }
}

impl Format for Field {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Self::Named { name, value, .. } => {
                (name, space(), token("="), space(), value).fmt(f);
            }
            Self::Bracketed { key, value, .. } => {
                (BracketedKey(key), space(), token("="), space(), value).fmt(f);
            }
            Self::Positional { value, .. } => value.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::{parse_expr, render};
    use crate::{FormatOptions, IndentStyle};

    fn print_expr(source: &str, line_width: u16, magic_trailing_comma: bool) -> String {
        let options = FormatOptions {
            line_width,
            indent_style: IndentStyle::Spaces,
            magic_trailing_comma,
            ..FormatOptions::default()
        };
        render(&parse_expr(source), options)
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
