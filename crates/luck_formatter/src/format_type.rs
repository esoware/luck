//! Types format from the [`Type`] tree alone, never from source, so synthetic
//! types format the same as parsed ones.

use luck_ast::{FunctionTypeParam, GenericTypeList, Type, TypeField};

use crate::ir::Formatter;

/// Where an alternation stands, which decides the lines its members take
/// when it breaks.
#[derive(Debug, Clone, Copy)]
enum Placement {
    /// After other text on its line, such as `type T =` or `name:`. The
    /// members move to the lines after it, one indent deeper.
    AfterText,
    /// Where a list or a table that breaks opens a line for it. The members
    /// start on that line, at its indent, and `should_lead` keeps the leading
    /// operator when flat too.
    LineStart { should_lead: bool },
}

/// The name `ty` prints first when flat, where an alternation drops its
/// leading `|` or `&`.
fn flat_leading_name<'ast>(ty: &Type<'ast>) -> Option<&'ast str> {
    let mut current = ty;
    loop {
        current = match current {
            Type::Name(name) => return Some(name.text.as_str()),
            Type::Named(named) => return Some(named.prefix.unwrap_or(named.name).text.as_str()),
            Type::GenericPack(pack) => return Some(pack.name.text.as_str()),
            Type::Optional(optional) => &optional.ty,
            Type::Union(union) => union.types.first()?,
            Type::Intersection(intersection) => intersection.types.first()?,
            Type::Typeof(_)
            | Type::Table(_)
            | Type::Function(_)
            | Type::Parenthesized(_)
            | Type::Pack(_)
            | Type::Nil(_)
            | Type::True(_)
            | Type::False(_)
            | Type::StringLiteral(_)
            | Type::Number(_)
            | Type::Variadic(_)
            | Type::Error(_) => return None,
        };
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the formatter is one type with its node families split by file"
)]
impl<'ast> Formatter<'ast> {
    pub(crate) fn write_type(&mut self, ty: &Type<'ast>) {
        match ty {
            Type::Name(name) => self.name(**name),
            // Qualification and generics are tight against the name.
            Type::Named(named) => {
                if let Some(module) = &named.prefix {
                    self.name(*module);
                    self.token(".");
                }
                self.name(named.name);
                if let Some(generics) = &named.generics {
                    self.write_delimited_types("<", ">", &generics.args);
                }
            }
            Type::Typeof(typeof_type) => {
                self.token("typeof");
                self.token("(");
                self.write_expr(&typeof_type.expr);
                self.token(")");
            }
            Type::Table(table) => self.write_table_type(&table.fields),
            // The parameter list breaks like a call argument list.
            Type::Function(function) => {
                if let Some(generics) = &function.generics {
                    self.write_generic_type_list(generics);
                }
                self.write_delimited("(", ")", &function.params, Self::write_function_type_param);
                self.space();
                self.token("->");
                self.space();
                self.write_type(&function.return_type);
            }
            Type::Optional(optional) => {
                self.write_type(&optional.ty);
                for _ in 0..optional.question_count {
                    self.token("?");
                }
            }
            Type::Union(union) => self.write_alternation(&union.types, "|", Placement::AfterText),
            Type::Intersection(intersection) => {
                self.write_alternation(&intersection.types, "&", Placement::AfterText);
            }
            Type::Parenthesized(parenthesized) => {
                self.token("(");
                self.write_type(&parenthesized.ty);
                self.token(")");
            }
            Type::Pack(pack) => self.write_delimited_types("(", ")", &pack.types),
            Type::Nil(_) => self.token("nil"),
            Type::True(_) => self.token("true"),
            Type::False(_) => self.token("false"),
            Type::StringLiteral(literal) => self.write_string(literal.text),
            Type::Number(literal) => self.ascii(literal.text),
            // Luau: `...T`.
            Type::Variadic(variadic) => {
                self.token("...");
                self.write_type(&variadic.ty);
            }
            // Luau: `T...`.
            Type::GenericPack(generic_pack) => {
                self.name(generic_pack.name);
                self.token("...");
            }
            // A parse-recovery placeholder has no text to print.
            Type::Error(_) => {}
        }
    }

    /// When the group breaks, each field gets its own line and a trailing
    /// comma, except the array shorthand `{ T }`, since Luau's grammar has no
    /// `{ T, }`.
    fn write_table_type(&mut self, fields: &[TypeField<'ast>]) {
        if fields.is_empty() {
            self.token("{");
            self.token("}");
            return;
        }

        let is_array_shorthand = matches!(fields, [TypeField::Array { .. }]);

        let group_id = self.group_id();
        self.group_with_id(group_id, |grouped| {
            grouped.token("{");
            grouped.indent(|indented| {
                indented.soft_line_or_space();
                for (i, field) in fields.iter().enumerate() {
                    if i > 0 {
                        indented.token(",");
                        indented.soft_line_or_space();
                    }
                    indented.write_type_field(field);
                }
                if !is_array_shorthand {
                    indented.if_group_breaks(group_id, |f| f.token(","));
                }
            });
            grouped.soft_line_or_space();
            grouped.token("}");
        });
    }

    fn write_type_field(&mut self, field: &TypeField<'ast>) {
        match field {
            TypeField::Named { access, name, value, .. } => {
                // Luau `read`/`write` access modifier sits before the name.
                if let Some(modifier) = access {
                    self.token(modifier.as_str());
                    self.space();
                }
                self.name(*name);
                self.token(":");
                self.space();
                self.write_type(value);
            }
            TypeField::Indexer { access, key, value, .. } => {
                if let Some(modifier) = access {
                    self.token(modifier.as_str());
                    self.space();
                }
                self.token("[");
                self.write_type(key);
                self.token("]");
                self.token(":");
                self.space();
                self.write_type(value);
            }
            TypeField::Array { access: Some(modifier), value, .. } => {
                self.token(modifier.as_str());
                self.space();
                self.write_type(value);
            }
            TypeField::Array { access: None, value, .. } => {
                // A `read` or `write` that opens the field would read back
                // as its access modifier, so the leading operator that keeps
                // it from opening the field stays.
                let should_lead =
                    flat_leading_name(value).is_some_and(|name| matches!(name, "read" | "write"));
                self.write_at_line_start(value, should_lead);
            }
        }
    }

    /// Flat reads `A | B | C`.
    ///
    /// Broken, each member gets its own line and a leading `| ` (or `& `), as
    /// Prettier does for TypeScript unions. The break decision alone places
    /// the leading operator, so the AST's `has_leading_pipe` and
    /// `has_leading_ampersand` are ignored.
    fn write_alternation(&mut self, items: &[Type<'ast>], op: &'static str, placement: Placement) {
        let group_id = self.group_id();
        let separator = |f: &mut Self| {
            f.soft_line_or_space();
            f.token(op);
            f.space();
        };
        let members = |f: &mut Self| {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    separator(f);
                }
                f.write_type(item);
            }
        };
        match placement {
            Placement::AfterText => self.group_with_id(group_id, |grouped| {
                grouped.indent(|f| {
                    f.if_group_breaks(group_id, separator);
                    members(f);
                });
            }),
            Placement::LineStart { should_lead: true } => self.group_with_id(group_id, |f| {
                f.token(op);
                f.space();
                members(f);
            }),
            Placement::LineStart { should_lead: false } => {
                self.group_with_id(group_id, |grouped| {
                    grouped.if_group_breaks(group_id, |f| {
                        f.token(op);
                        f.space();
                    });
                    members(grouped);
                });
            }
        }
    }

    /// `ty` where a list or a table that breaks opens a line for it.
    fn write_at_line_start(&mut self, ty: &Type<'ast>, should_lead: bool) {
        let placement = Placement::LineStart { should_lead };
        if let Type::Union(union) = ty {
            self.write_alternation(&union.types, "|", placement);
        } else if let Type::Intersection(intersection) = ty {
            self.write_alternation(&intersection.types, "&", placement);
        } else {
            self.write_type(ty);
        }
    }

    fn write_list_type(&mut self, ty: &Type<'ast>) {
        self.write_at_line_start(ty, false);
    }

    pub(crate) fn write_generic_type_list(&mut self, generics: &GenericTypeList<'ast>) {
        self.write_delimited("<", ">", &generics.params, |f, param| {
            f.name(param.name);
            // Luau: `T...` marks a generic pack parameter.
            if param.is_pack {
                f.token("...");
            }
            // `= T` default - only legal in `type` declarations.
            if let Some(default) = &param.default {
                f.space();
                f.token("=");
                f.space();
                f.write_type(default);
            }
        });
    }

    fn write_function_type_param(&mut self, param: &FunctionTypeParam<'ast>) {
        if let Some(name) = &param.name {
            self.name(*name);
            self.token(":");
            self.space();
            self.write_type(&param.ty);
        } else {
            self.write_list_type(&param.ty);
        }
    }

    /// Type arguments, `<T, U>`, and a Luau instantiation's doubled
    /// `<<T, U>>`.
    pub(crate) fn write_delimited_types(
        &mut self,
        open: &'static str,
        close: &'static str,
        types: &[Type<'ast>],
    ) {
        self.write_delimited(open, close, types, Self::write_list_type);
    }

    /// A broken list opens a line for each item, where `write_item` writes it.
    ///
    /// Nested angle brackets close with adjacent `>` tokens. The lexer reads
    /// them as one `>>`, which the parser splits again.
    fn write_delimited<T>(
        &mut self,
        open: &'static str,
        close: &'static str,
        items: &[T],
        write_item: impl Fn(&mut Self, &T),
    ) {
        if items.is_empty() {
            self.token(open);
            self.token(close);
            return;
        }
        self.group(|grouped| {
            grouped.token(open);
            grouped.indent(|f| {
                f.soft_line();
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.token(",");
                        f.soft_line_or_space();
                    }
                    write_item(f, item);
                }
            });
            grouped.soft_line();
            grouped.token(close);
        });
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_ast::{SpanCounter, Synth, Type};

    use crate::FormatOptions;
    use crate::fixtures::render;

    fn render_type<'ast>(
        allocator: &'ast Allocator,
        build: impl FnOnce(Synth<'ast>) -> Type<'ast>,
    ) -> String {
        let spans = allocator.alloc(SpanCounter::new());
        let ty = build(Synth::new(allocator, spans));
        render(allocator, FormatOptions::default(), |f| f.write_type(&ty))
    }

    #[test]
    fn named_type() {
        let allocator = Allocator::new();
        assert_eq!(render_type(&allocator, |synth| synth.ty_named("number")), "number");
    }

    #[test]
    fn qualified_type() {
        let allocator = Allocator::new();
        let rendered = render_type(&allocator, |synth| synth.ty_qualified("module", "Type"));
        assert_eq!(rendered, "module.Type");
    }

    #[test]
    fn optional_is_tight() {
        let allocator = Allocator::new();
        let rendered = render_type(&allocator, |synth| synth.ty_optional(synth.ty_named("string")));
        assert_eq!(rendered, "string?");
    }

    #[test]
    fn union_flat_has_spaced_pipes() {
        let allocator = Allocator::new();
        let rendered = render_type(&allocator, |synth| {
            synth.ty_union([synth.ty_named("string"), synth.ty_named("number")])
        });
        assert_eq!(rendered, "string | number");
    }

    #[test]
    fn intersection_flat_has_spaced_ampersands() {
        let allocator = Allocator::new();
        let rendered = render_type(&allocator, |synth| {
            synth.ty_intersection([synth.ty_named("A"), synth.ty_named("B")])
        });
        assert_eq!(rendered, "A & B");
    }

    #[test]
    fn generic_args() {
        let allocator = Allocator::new();
        let rendered = render_type(&allocator, |synth| {
            synth.ty_generic("Map", [synth.ty_named("string"), synth.ty_named("number")])
        });
        assert_eq!(rendered, "Map<string, number>");
    }

    #[test]
    fn table_type_flat_has_inner_spaces() {
        let allocator = Allocator::new();
        let rendered = render_type(&allocator, |synth| {
            synth.ty_record([("name", synth.ty_named("string")), ("age", synth.ty_named("number"))])
        });
        assert_eq!(rendered, "{ name: string, age: number }");
    }

    #[test]
    fn array_shorthand_has_no_trailing_comma() {
        let allocator = Allocator::new();
        let rendered =
            render_type(&allocator, |synth| synth.ty_table_array(synth.ty_named("string")));
        assert_eq!(rendered, "{ string }");
    }

    #[test]
    fn function_type() {
        let allocator = Allocator::new();
        let rendered = render_type(&allocator, |synth| {
            synth.ty_function([synth.ty_named("number")], synth.ty_named("boolean"))
        });
        assert_eq!(rendered, "(number) -> boolean");
    }

    #[test]
    fn variadic_pack_is_tight() {
        let allocator = Allocator::new();
        let rendered = render_type(&allocator, |synth| synth.ty_variadic(synth.ty_named("number")));
        assert_eq!(rendered, "...number");
    }
}
