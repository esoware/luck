//! Types format from the [`Type`] tree alone, never from source, so synthetic
//! types format the same as parsed ones.

use luck_ast::{
    FunctionType, FunctionTypeParam, GenericTypeList, GenericTypeParam, IntersectionType,
    NamedType, TableType, Type, TypeArgs, TypeField, TypeofType, UnionType,
};
use luck_token::TokenKind;

use crate::ir::{
    Format, Formatter, format_with, group, group_with_id, if_group_breaks, indent, separated,
    soft_line, soft_line_or_space, space, token,
};
use crate::quotes::normalize_quote;

impl Format for Type {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Self::Name { name, .. } => f.write_text(name.clone()),
            Self::Named(named) => named.fmt(f),
            Self::Typeof(typeof_type) => typeof_type.fmt(f),
            Self::Table(table) => table.fmt(f),
            Self::Function(function) => function.fmt(f),
            Self::Optional(optional) => {
                (&optional.type_value, token("?")).fmt(f);
            }
            Self::Union(union) => union.fmt(f),
            Self::Intersection(intersection) => intersection.fmt(f),
            Self::Parenthesized(parenthesized) => {
                (token("("), &parenthesized.type_value, token(")")).fmt(f);
            }
            Self::Pack(pack) => write_delimited(f, "(", ")", &pack.types.items),
            Self::Singleton(literal) => {
                if let TokenKind::StringLiteral(text) = &literal.kind {
                    let quote_style = f.options().quote_style;
                    f.write_text(normalize_quote(text, quote_style));
                } else {
                    literal.fmt(f);
                }
            }
            // Luau: `...T`.
            Self::Variadic(variadic) => (token("..."), &variadic.type_value).fmt(f),
            // Luau: `T...`.
            Self::GenericPack(generic_pack) => (&generic_pack.name, token("...")).fmt(f),
            // A parse-recovery placeholder has no text to print.
            Self::Error(_) => {}
        }
    }
}

/// Qualification and generics are tight against the name.
impl Format for NamedType {
    fn fmt(&self, f: &mut Formatter) {
        if let Some(module) = &self.prefix {
            (module, token(".")).fmt(f);
        }
        (&self.name, &self.generics).fmt(f);
    }
}

impl Format for TypeofType {
    fn fmt(&self, f: &mut Formatter) {
        (token("typeof"), token("("), &self.expr, token(")")).fmt(f);
    }
}

/// When the group breaks, each field gets its own line and a trailing comma,
/// except the array shorthand `{ T }`, since Luau's grammar has no `{ T, }`.
impl Format for TableType {
    fn fmt(&self, f: &mut Formatter) {
        if self.fields.is_empty() {
            (token("{"), token("}")).fmt(f);
            return;
        }

        let is_array_shorthand = matches!(self.fields.items.as_slice(), [TypeField::Array { .. }]);

        let group_id = f.group_id();
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
        .fmt(f);
    }
}

/// The parameter list breaks like a call argument list.
impl Format for FunctionType {
    fn fmt(&self, f: &mut Formatter) {
        self.generics.fmt(f);
        write_delimited(f, "(", ")", &self.params.items);
        (space(), token("->"), space(), &self.return_type).fmt(f);
    }
}

impl Format for UnionType {
    fn fmt(&self, f: &mut Formatter) {
        write_alternation(f, &self.types.items, "|", false);
    }
}

impl Format for IntersectionType {
    fn fmt(&self, f: &mut Formatter) {
        write_alternation(f, &self.types.items, "&", false);
    }
}

/// Flat reads `A | B | C`.
///
/// Broken, each member gets its own line and a leading
/// `| ` (or `& `), as Prettier does for TypeScript unions. The break decision
/// alone places the leading operator, so the AST's `has_leading_pipe` and
/// `has_leading_ampersand` are ignored, except that `should_lead` keeps it
/// when flat too.
fn write_alternation(f: &mut Formatter, items: &[Type], op: &'static str, should_lead: bool) {
    let group_id = f.group_id();
    let separator = (soft_line_or_space(), token(op), space());
    let lead = format_with(|lead_f| {
        if should_lead {
            (if_group_breaks(group_id, soft_line_or_space()), token(op), space()).fmt(lead_f);
        } else {
            if_group_breaks(group_id, &separator).fmt(lead_f);
        }
    });
    group_with_id(group_id, indent((lead, separated(items, &separator)))).fmt(f);
}

/// The name `ty` prints first when flat, where an alternation drops its
/// leading `|` or `&`.
fn flat_leading_name(ty: &Type) -> Option<&str> {
    let mut current = ty;
    loop {
        current = match current {
            Type::Name { name, .. } => return Some(name),
            Type::Named(named) => return named.prefix.as_ref().unwrap_or(&named.name).identifier(),
            Type::GenericPack(pack) => return pack.name.identifier(),
            Type::Optional(optional) => &optional.type_value,
            Type::Union(union) => union.types.first()?,
            Type::Intersection(intersection) => intersection.types.first()?,
            Type::Typeof(_)
            | Type::Table(_)
            | Type::Function(_)
            | Type::Parenthesized(_)
            | Type::Pack(_)
            | Type::Singleton(_)
            | Type::Variadic(_)
            | Type::Error(_) => return None,
        };
    }
}

impl Format for TypeField {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Self::Named { access, name, value, .. } => {
                // Luau `read`/`write` access modifier sits before the name.
                if let Some(modifier) = access {
                    (modifier, space()).fmt(f);
                }
                (name, token(":"), space(), value).fmt(f);
            }
            Self::Indexer { access, key, value, .. } => {
                if let Some(modifier) = access {
                    (modifier, space()).fmt(f);
                }
                (token("["), key, token("]"), token(":"), space(), value).fmt(f);
            }
            Self::Array { access, value, .. } => {
                if let Some(modifier) = access {
                    (modifier, space()).fmt(f);
                }
                // A `read` or `write` that opens the field would read back
                // as its access modifier, so the leading operator that keeps
                // it from opening the field stays.
                let should_lead = access.is_none()
                    && flat_leading_name(value)
                        .is_some_and(|name| matches!(name, "read" | "write"));
                if should_lead && let Type::Union(union) = value {
                    write_alternation(f, &union.types.items, "|", true);
                } else if should_lead && let Type::Intersection(intersection) = value {
                    write_alternation(f, &intersection.types.items, "&", true);
                } else {
                    value.fmt(f);
                }
            }
        }
    }
}

impl Format for TypeArgs {
    fn fmt(&self, f: &mut Formatter) {
        write_delimited(f, "<", ">", &self.args.items);
    }
}

/// Luau expression-side explicit type arguments use doubled delimiters.
pub(crate) struct ExplicitTypeArgs<'args>(pub &'args TypeArgs);

impl Format for ExplicitTypeArgs<'_> {
    fn fmt(&self, f: &mut Formatter) {
        write_delimited(f, "<<", ">>", &self.0.args.items);
    }
}

impl Format for GenericTypeList {
    fn fmt(&self, f: &mut Formatter) {
        write_delimited(f, "<", ">", &self.params.items);
    }
}

impl Format for GenericTypeParam {
    fn fmt(&self, f: &mut Formatter) {
        self.name.fmt(f);
        // Luau: `T...` marks a generic pack parameter.
        if self.is_pack {
            token("...").fmt(f);
        }
        // `= T` default - only legal in `type` declarations.
        if let Some(default) = &self.default {
            (space(), token("="), space(), default).fmt(f);
        }
    }
}

impl Format for FunctionTypeParam {
    fn fmt(&self, f: &mut Formatter) {
        if let Some(name) = &self.name {
            (name, token(":"), space()).fmt(f);
        }
        self.type_value.fmt(f);
    }
}

/// Nested angle brackets close with adjacent `>` tokens. The lexer reads them
/// as one `>>`, which the parser splits again.
fn write_delimited<T>(f: &mut Formatter, open: &'static str, close: &'static str, items: &[T])
where
    T: Format,
{
    if items.is_empty() {
        (token(open), token(close)).fmt(f);
        return;
    }
    group((
        token(open),
        indent((soft_line(), separated(items, (token(","), soft_line_or_space())))),
        soft_line(),
        token(close),
    ))
    .fmt(f);
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
