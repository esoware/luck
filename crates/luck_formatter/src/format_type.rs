//! Luau type emission.
//!
//! Types format from the [`Type`] tree alone, never from source, so synthetic
//! types format the same as parsed ones.

use luck_ast::{
    FunctionType, FunctionTypeParam, GenericTypeList, GenericTypeParam, IntersectionType,
    NamedType, TableType, Type, TypeArgs, TypeField, TypeofType, UnionType,
};

use crate::ir::{
    Format, Formatter, group, group_with_id, if_group_breaks, indent, separated, soft_line,
    soft_line_or_space, space, token,
};

impl Format for Type {
    fn fmt(&self, formatter: &mut Formatter) {
        match self {
            Self::Name { name, .. } => formatter.write_text(name.clone()),
            Self::Named(named) => named.fmt(formatter),
            Self::Typeof(typeof_type) => typeof_type.fmt(formatter),
            Self::Table(table) => table.fmt(formatter),
            Self::Function(function) => function.fmt(formatter),
            Self::Optional(optional) => (&optional.type_value, token("?")).fmt(formatter),
            Self::Union(union) => union.fmt(formatter),
            Self::Intersection(intersection) => intersection.fmt(formatter),
            Self::Parenthesized(parenthesized) => {
                (token("("), &parenthesized.type_value, token(")")).fmt(formatter);
            }
            Self::Pack(pack) => write_delimited(formatter, "(", ")", &pack.types.items),
            Self::Singleton(literal) => literal.fmt(formatter),
            // Luau: `...T`.
            Self::Variadic(variadic) => (token("..."), &variadic.type_value).fmt(formatter),
            // Luau: `T...`.
            Self::GenericPack(generic_pack) => (&generic_pack.name, token("...")).fmt(formatter),
            // A parse-recovery placeholder has no text to print.
            Self::Error(_) => {}
        }
    }
}

/// `Name`, `module.Name`, `Name<args>`. Qualification and generics are tight
/// against the name.
impl Format for NamedType {
    fn fmt(&self, formatter: &mut Formatter) {
        if let Some(module) = &self.prefix {
            (module, token(".")).fmt(formatter);
        }
        (&self.name, &self.generics).fmt(formatter);
    }
}

/// `typeof(expr)`.
impl Format for TypeofType {
    fn fmt(&self, formatter: &mut Formatter) {
        (token("typeof"), token("("), &self.expr, token(")")).fmt(formatter);
    }
}

/// `{ name: T, [K]: V }`.
///
/// When the group breaks, each field gets its own line and a trailing comma,
/// except the array shorthand `{ T }`, since Luau's grammar has no `{ T, }`.
impl Format for TableType {
    fn fmt(&self, formatter: &mut Formatter) {
        if self.fields.is_empty() {
            (token("{"), token("}")).fmt(formatter);
            return;
        }

        let is_array_shorthand = matches!(self.fields.items.as_slice(), [TypeField::Array { .. }]);

        let group_id = formatter.group_id();
        group_with_id(
            group_id,
            (
                token("{"),
                indent((
                    soft_line_or_space(),
                    separated(&self.fields.items, (token(","), soft_line_or_space())),
                    (!is_array_shorthand).then(|| if_group_breaks(group_id, token(","))),
                )),
                soft_line_or_space(),
                token("}"),
            ),
        )
        .fmt(formatter);
    }
}

/// `<T>(a: T) -> R`. The parameter list breaks like a call argument list.
impl Format for FunctionType {
    fn fmt(&self, formatter: &mut Formatter) {
        self.generics.fmt(formatter);
        write_delimited(formatter, "(", ")", &self.params.items);
        (space(), token("->"), space(), &self.return_type).fmt(formatter);
    }
}

impl Format for UnionType {
    fn fmt(&self, formatter: &mut Formatter) {
        write_alternation(formatter, &self.types.items, "|");
    }
}

impl Format for IntersectionType {
    fn fmt(&self, formatter: &mut Formatter) {
        write_alternation(formatter, &self.types.items, "&");
    }
}

/// Union or intersection.
///
/// Flat reads `A | B | C`. Broken, each member gets its own line and a leading
/// `| ` (or `& `), as Prettier does for TypeScript unions. The break decision
/// alone places the leading operator, so the AST's `has_leading_pipe` and
/// `has_leading_ampersand` are ignored.
fn write_alternation(formatter: &mut Formatter, items: &[Type], op: &'static str) {
    let group_id = formatter.group_id();
    let separator = (soft_line_or_space(), token(op), space());
    group_with_id(
        group_id,
        indent((if_group_breaks(group_id, &separator), separated(items, &separator))),
    )
    .fmt(formatter);
}

impl Format for TypeField {
    fn fmt(&self, formatter: &mut Formatter) {
        match self {
            Self::Named { access, name, value, .. } => {
                // Luau `read`/`write` access modifier sits before the name.
                if let Some(modifier) = access {
                    (modifier, space()).fmt(formatter);
                }
                (name, token(":"), space(), value).fmt(formatter);
            }
            Self::Indexer { access, key, value, .. } => {
                if let Some(modifier) = access {
                    (modifier, space()).fmt(formatter);
                }
                (token("["), key, token("]"), token(":"), space(), value).fmt(formatter);
            }
            // Array shorthand `{ T }` - a bare element type.
            Self::Array { value, .. } => value.fmt(formatter),
        }
    }
}

impl Format for TypeArgs {
    fn fmt(&self, formatter: &mut Formatter) {
        write_delimited(formatter, "<", ">", &self.args.items);
    }
}

/// Luau expression-side explicit type arguments use doubled delimiters.
pub(crate) struct ExplicitTypeArgs<'args>(pub &'args TypeArgs);

impl Format for ExplicitTypeArgs<'_> {
    fn fmt(&self, formatter: &mut Formatter) {
        write_delimited(formatter, "<<", ">>", &self.0.args.items);
    }
}

impl Format for GenericTypeList {
    fn fmt(&self, formatter: &mut Formatter) {
        write_delimited(formatter, "<", ">", &self.params.items);
    }
}

impl Format for GenericTypeParam {
    fn fmt(&self, formatter: &mut Formatter) {
        self.name.fmt(formatter);
        // Luau: `T...` marks a generic pack parameter.
        if self.is_pack {
            token("...").fmt(formatter);
        }
        // `= T` default - only legal in `type` declarations.
        if let Some(default) = &self.default {
            (space(), token("="), space(), default).fmt(formatter);
        }
    }
}

impl Format for FunctionTypeParam {
    fn fmt(&self, formatter: &mut Formatter) {
        // Present when the parameter is named: `x: number`.
        if let Some(name) = &self.name {
            (name, token(":"), space()).fmt(formatter);
        }
        self.type_value.fmt(formatter);
    }
}

/// Wraps a list in `open` and `close` inside a breakable group, one item per
/// line when it breaks. Source separators are normalized to `,`.
///
/// Nested angle brackets close with adjacent `>` tokens. The lexer reads them
/// as one `>>`, which the parser splits again.
fn write_delimited<T>(
    formatter: &mut Formatter,
    open: &'static str,
    close: &'static str,
    items: &[T],
) where
    T: Format,
{
    if items.is_empty() {
        (token(open), token(close)).fmt(formatter);
        return;
    }
    group((
        token(open),
        indent((soft_line(), separated(items, (token(","), soft_line_or_space())))),
        soft_line(),
        token(close),
    ))
    .fmt(formatter);
}

#[cfg(test)]
mod tests {
    use luck_ast::{Synth, Type};

    use crate::{FormatOptions, fixtures};

    fn render(ty: &Type) -> String {
        fixtures::render(ty, FormatOptions::default())
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
        let string_ty = synth.ty_named("string");
        let number_ty = synth.ty_named("number");
        let ty = synth.ty_union(vec![string_ty, number_ty]);
        assert_eq!(render(&ty), "string | number");
    }

    #[test]
    fn intersection_flat_has_spaced_ampersands() {
        let synth = Synth::new();
        let left = synth.ty_named("A");
        let right = synth.ty_named("B");
        let ty = synth.ty_intersection(vec![left, right]);
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
        let name_ty = synth.ty_named("string");
        let age_ty = synth.ty_named("number");
        let ty = synth.ty_record(vec![("name", name_ty), ("age", age_ty)]);
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
