//! Luau type-AST emission.
//!
//! Types are formatted directly from the parsed [`Type`] tree - no source
//! slicing, no re-tokenizing. Every leaf goes through [`crate::tokens`] so
//! synthetic (decompiler-built) types format identically to parsed ones.

use luck_ast::types::*;
use luck_token::Span;

use crate::ir::*;
use crate::tokens::FormatToken;

impl Format for Type {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Type::Named(named) => write_named(f, named),
            Type::Typeof(typeof_type) => write_typeof(f, typeof_type),
            Type::Table(table) => write_table(f, table),
            Type::Function(function) => write_function_type(f, function),
            Type::Optional(optional) => {
                optional.type_value.fmt(f);
                // `T?` is tight - no space before the question mark.
                token("?").fmt(f);
            }
            Type::Union(union) => write_alternation(f, &union.types.items, "|"),
            Type::Intersection(intersection) => {
                write_alternation(f, &intersection.types.items, "&");
            }
            Type::Parenthesized(paren) => {
                token("(").fmt(f);
                paren.type_value.fmt(f);
                token(")").fmt(f);
            }
            Type::Pack(pack) => write_delimited(f, "(", ")", &pack.types.items),
            Type::Singleton(literal) => FormatToken(literal).fmt(f),
            // Luau: `...T` variadic pack element - tight.
            Type::Variadic(variadic) => {
                token("...").fmt(f);
                variadic.type_value.fmt(f);
            }
            // Luau: `T...` generic pack reference - tight.
            Type::GenericPack(generic_pack) => {
                FormatToken(&generic_pack.name).fmt(f);
                token("...").fmt(f);
            }
            // A parse error placeholder emits nothing; the diagnostic already
            // reported the failure and there is no faithful text to print.
            Type::Error(_) => {}
        }
    }
}

/// `Name`, `module.Name`, `Name<args>`. Qualification and generics are tight
/// against the name.
fn write_named(f: &mut Formatter, named: &NamedType) {
    if let Some((module, _dot)) = &named.prefix {
        FormatToken(module).fmt(f);
        token(".").fmt(f);
    }
    FormatToken(&named.name).fmt(f);
    if let Some(generics) = &named.generics {
        generics.fmt(f);
    }
}

/// `typeof(expr)` - the inner expression formats through its own impl.
fn write_typeof(f: &mut Formatter, typeof_type: &TypeofType) {
    token("typeof").fmt(f);
    token("(").fmt(f);
    typeof_type.expr.fmt(f);
    token(")").fmt(f);
}

/// `{ name: T, [K]: V }`. Flat prints with inner spaces; when the group
/// breaks, each field lands on its own line with a trailing comma. The
/// single-element array shorthand `{ T }` never gains a trailing comma -
/// that would change how it parses.
fn write_table(f: &mut Formatter, table: &TableType) {
    if table.fields.is_empty() {
        token("{").fmt(f);
        token("}").fmt(f);
        return;
    }

    let is_array_shorthand =
        table.fields.len() == 1 && matches!(table.fields[0].0, TypeField::Array { .. });

    let group_id = f.group_id();
    group_with_id(
        group_id,
        format_with(move |f| {
            token("{").fmt(f);
            indent(format_with(move |f| {
                soft_line_or_space().fmt(f);
                for (index, (field, _separator)) in table.fields.iter().enumerate() {
                    if index > 0 {
                        token(",").fmt(f);
                        soft_line_or_space().fmt(f);
                    }
                    field.fmt(f);
                }
                if !is_array_shorthand {
                    if_group_breaks(group_id, token(",")).fmt(f);
                }
            }))
            .fmt(f);
            soft_line_or_space().fmt(f);
            token("}").fmt(f);
        }),
    )
    .fmt(f);
}

/// `<T>(a: T) -> R`. The parameter list breaks like a call argument list.
fn write_function_type(f: &mut Formatter, function: &FunctionType) {
    if let Some(generics) = &function.generics {
        generics.fmt(f);
    }
    write_delimited(f, "(", ")", &function.params.items);
    space().fmt(f);
    token("->").fmt(f);
    space().fmt(f);
    function.return_type.fmt(f);
}

/// N-ary union/intersection. Flat reads `A | B | C`; when the group breaks
/// each member gets a leading `| ` (`& ` for intersections) on its own line,
/// matching how Prettier lays out TypeScript unions. The AST's
/// `leading_pipe`/`leading_ampersand` is intentionally normalized away - the
/// leading operator is driven by the break decision, not by the source.
fn write_alternation(f: &mut Formatter, items: &[(Type, Option<Span>)], operator: &'static str) {
    let group_id = f.group_id();
    group_with_id(
        group_id,
        indent(format_with(move |f| {
            if_group_breaks(group_id, (soft_line_or_space(), token(operator), space())).fmt(f);
            for (index, (element, _separator)) in items.iter().enumerate() {
                if index > 0 {
                    soft_line_or_space().fmt(f);
                    token(operator).fmt(f);
                    space().fmt(f);
                }
                element.fmt(f);
            }
        })),
    )
    .fmt(f);
}

impl Format for TypeField {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            TypeField::Named {
                access,
                name,
                value,
                ..
            } => {
                // Luau `read`/`write` access modifier sits before the name.
                if let Some(access) = access {
                    FormatToken(access).fmt(f);
                    space().fmt(f);
                }
                FormatToken(name).fmt(f);
                token(":").fmt(f);
                space().fmt(f);
                value.fmt(f);
            }
            TypeField::Indexer {
                access, key, value, ..
            } => {
                if let Some(access) = access {
                    FormatToken(access).fmt(f);
                    space().fmt(f);
                }
                token("[").fmt(f);
                key.fmt(f);
                token("]").fmt(f);
                token(":").fmt(f);
                space().fmt(f);
                value.fmt(f);
            }
            // Array shorthand `{ T }` - a bare element type.
            TypeField::Array { value, .. } => {
                value.fmt(f);
            }
        }
    }
}

impl Format for TypeArgs {
    fn fmt(&self, f: &mut Formatter) {
        write_delimited(f, "<", ">", &self.args.items);
    }
}

impl Format for GenericTypeList {
    fn fmt(&self, f: &mut Formatter) {
        write_delimited(f, "<", ">", &self.params.items);
    }
}

impl Format for GenericTypeParam {
    fn fmt(&self, f: &mut Formatter) {
        FormatToken(&self.name).fmt(f);
        // Luau: `T...` marks a generic pack parameter.
        if self.dots.is_some() {
            token("...").fmt(f);
        }
        // `= T` default - only legal in `type` declarations.
        if let Some((_equal, default)) = &self.default {
            space().fmt(f);
            token("=").fmt(f);
            space().fmt(f);
            default.fmt(f);
        }
    }
}

impl Format for FunctionTypeParam {
    fn fmt(&self, f: &mut Formatter) {
        // (name, colon) when the parameter is named: `x: number`.
        if let Some((name, _colon)) = &self.name {
            FormatToken(name).fmt(f);
            token(":").fmt(f);
            space().fmt(f);
        }
        self.type_value.fmt(f);
    }
}

/// Emit a comma-separated punctuated list, breaking one item per line when the
/// enclosing group expands. Source separators are normalized to `,`.
fn write_punctuated<T: Format>(f: &mut Formatter, items: &[(T, Option<Span>)]) {
    for (index, (item, _separator)) in items.iter().enumerate() {
        if index > 0 {
            token(",").fmt(f);
            soft_line_or_space().fmt(f);
        }
        item.fmt(f);
    }
}

/// Wrap a punctuated list in `open`/`close` delimiters with a breakable group.
/// Used for `<...>`, `(...)` argument/pack lists - angle brackets emit as
/// separate `>` tokens per nesting level (the printer never merges them; a
/// `>>` run re-parses via the lexer's `ShiftRight` splitting).
fn write_delimited<T: Format>(
    f: &mut Formatter,
    open: &'static str,
    close: &'static str,
    items: &[(T, Option<Span>)],
) {
    if items.is_empty() {
        token(open).fmt(f);
        token(close).fmt(f);
        return;
    }
    group(format_with(move |f| {
        token(open).fmt(f);
        indent(format_with(move |f| {
            soft_line().fmt(f);
            write_punctuated(f, items);
        }))
        .fmt(f);
        soft_line().fmt(f);
        token(close).fmt(f);
    }))
    .fmt(f);
}

#[cfg(test)]
mod tests {
    use luck_ast::synth::Synth;
    use luck_ast::types::Type;

    use crate::ir::{Format, Formatter};
    use crate::printer::{PrinterOptions, print};

    fn render(ty: &Type) -> String {
        let mut formatter = Formatter::new();
        ty.fmt(&mut formatter);
        let group_count = formatter.group_count();
        let elements = formatter.into_elements();
        print(
            &elements,
            group_count,
            &PrinterOptions {
                line_width: 80,
                use_tabs: false,
                indent_width: 2,
            },
        )
    }

    #[test]
    fn named_type() {
        let synth = Synth::new();
        let ty = synth.ty_named("number");
        assert_eq!(render(&ty), "number");
    }

    #[test]
    fn qualified_type() {
        let synth = Synth::new();
        let ty = synth.ty_qualified("module", "Type");
        assert_eq!(render(&ty), "module.Type");
    }

    #[test]
    fn optional_is_tight() {
        let synth = Synth::new();
        let inner = synth.ty_named("string");
        let ty = synth.ty_optional(inner);
        assert_eq!(render(&ty), "string?");
    }

    #[test]
    fn union_flat_has_spaced_pipes() {
        let synth = Synth::new();
        let a = synth.ty_named("string");
        let b = synth.ty_named("number");
        let ty = synth.ty_union(vec![a, b]);
        assert_eq!(render(&ty), "string | number");
    }

    #[test]
    fn intersection_flat_has_spaced_ampersands() {
        let synth = Synth::new();
        let a = synth.ty_named("A");
        let b = synth.ty_named("B");
        let ty = synth.ty_intersection(vec![a, b]);
        assert_eq!(render(&ty), "A & B");
    }

    #[test]
    fn generic_args() {
        let synth = Synth::new();
        let key = synth.ty_named("string");
        let value = synth.ty_named("number");
        let ty = synth.ty_generic("Map", vec![key, value]);
        assert_eq!(render(&ty), "Map<string, number>");
    }

    #[test]
    fn table_type_flat_has_inner_spaces() {
        let synth = Synth::new();
        let name_type = synth.ty_named("string");
        let age_type = synth.ty_named("number");
        let ty = synth.ty_record(vec![("name", name_type), ("age", age_type)]);
        assert_eq!(render(&ty), "{ name: string, age: number }");
    }

    #[test]
    fn array_shorthand_has_no_trailing_comma() {
        let synth = Synth::new();
        let element = synth.ty_named("string");
        let ty = synth.ty_table_array(element);
        assert_eq!(render(&ty), "{ string }");
    }

    #[test]
    fn function_type() {
        let synth = Synth::new();
        let param = synth.ty_named("number");
        let return_type = synth.ty_named("boolean");
        let ty = synth.ty_function(vec![param], return_type);
        assert_eq!(render(&ty), "(number) -> boolean");
    }

    #[test]
    fn variadic_pack_is_tight() {
        let synth = Synth::new();
        let element = synth.ty_named("number");
        let ty = synth.ty_variadic(element);
        assert_eq!(render(&ty), "...number");
    }
}
