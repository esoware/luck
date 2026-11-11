//! A table of simple positional values packs with `fill`, as many per line as
//! fit.
//!
//! Any other table stays flat if it fits and otherwise breaks one field
//! per line. A table whose comments all sit between fields claims them and
//! breaks one field per line; any other comments are left to the block level.

use luck_ast::{Expression, Field, TableConstructor};
use luck_token::{Span, has_line_break};

use crate::comments::{ListKind, write_commented_list};
use crate::format_block::has_blank_line_above;
use crate::format_expr::BracketedKey;
use crate::ir::{
    Format, Formatter, LineMode, empty_line, expand_parent, fill, format_with, group_with_id,
    hugged_group_with_id, if_group_breaks, indent, line_suffix_boundary, soft_line_or_space, space,
    token,
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

/// Whether the source spreads the table over several lines with two fields
/// sharing one of them. A synthetic table has no source and never does.
fn is_packed_in_source(f: &Formatter, table: &TableConstructor) -> bool {
    let comments = f.comments();
    let Some((source, version)) = comments.source_text().zip(comments.source_version()) else {
        return false;
    };
    let fields = &table.fields.items;
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

impl Format for TableConstructor {
    fn fmt(&self, f: &mut Formatter) {
        write_table(f, self, false);
    }
}

/// A table argument that breaks in its call's argument list's place.
pub(crate) struct HuggedTable<'table>(pub &'table TableConstructor);

impl Format for HuggedTable<'_> {
    fn fmt(&self, f: &mut Formatter) {
        write_table(f, self.0, true);
    }
}

fn write_table(f: &mut Formatter, table: &TableConstructor, is_hugged: bool) {
    if f.comments().has_claimable_comments(table.span, table.fields.items.iter().map(Field::span)) {
        let list = format_with(|commented| {
            write_commented_list(
                commented,
                ("{", "}"),
                table.span,
                &table.fields.items,
                Field::span,
                ListKind::TableFields,
            );
        });
        if is_hugged {
            let group_id = f.group_id();
            hugged_group_with_id(group_id, list).fmt(f);
        } else {
            list.fmt(f);
        }
        return;
    }

    if table.fields.is_empty() {
        token("{}").fmt(f);
        return;
    }

    // A trailing `,` or `;` after the last field keeps the table broken, one
    // field per line unless its values were packed.
    let has_magic_comma = f.options().magic_trailing_comma && table.fields.has_trailing_separator;
    let should_fill = is_fillable(table) && (!has_magic_comma || is_packed_in_source(f, table));
    let last_index = table.fields.len().saturating_sub(1);

    let group_id = f.group_id();
    let content = (
        token("{"),
        indent(format_with(|indented| {
            soft_line_or_space().fmt(indented);

            // A blank line the source has above a field is kept.
            if should_fill {
                // Every entry but the last carries its own comma, so
                // the fill measures widths with the comma included.
                let entries: Vec<_> = table
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, field)| {
                        let separator = if has_blank_line_above(indented, field.span().start) {
                            LineMode::Empty
                        } else {
                            LineMode::SoftOrSpace
                        };
                        (separator, (field, (i < last_index).then_some(token(","))))
                    })
                    .collect();
                fill(&entries).fmt(indented);
            } else {
                for (i, field) in table.fields.iter().enumerate() {
                    if i > 0 {
                        token(",").fmt(indented);
                        if has_blank_line_above(indented, field.span().start) {
                            empty_line().fmt(indented);
                        } else {
                            soft_line_or_space().fmt(indented);
                        }
                    }
                    field.fmt(indented);
                }
            }

            if has_magic_comma {
                expand_parent().fmt(indented);
            }
            if_group_breaks(group_id, token(",")).fmt(indented);
        })),
        soft_line_or_space(),
        line_suffix_boundary(),
        token("}"),
    );
    if is_hugged {
        hugged_group_with_id(group_id, content).fmt(f);
    } else {
        group_with_id(group_id, content).fmt(f);
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
