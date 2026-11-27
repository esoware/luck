use luck_ast::{
    Expression, FunctionType, FunctionTypeParam, GenericTypeList, GenericTypeParam, NamedType,
    ParenthesizedType, Type, TypeArgs, TypeField, TypeFieldAccess,
};

use super::Printer;

impl Printer {
    pub(super) fn print_type(&mut self, ty: &Type<'_>) {
        match ty {
            Type::Name(name) => self.name(name.text.as_str()),
            Type::Named(named) => self.print_named_type(named),
            Type::Typeof(typeof_type) => self.print_typeof(&typeof_type.expr),
            Type::Table(table) => self.print_table_type(&table.fields),
            Type::Function(function) => self.print_function_type(function),
            Type::Optional(optional) => {
                self.print_type(&optional.ty);
                for _ in 0..optional.question_count {
                    self.piece("?");
                }
            }
            Type::Union(union) => {
                self.print_type_run(union.has_leading_pipe, &union.types, "|");
            }
            Type::Intersection(intersection) => {
                self.print_type_run(intersection.has_leading_ampersand, &intersection.types, "&");
            }
            Type::Parenthesized(parenthesized) => self.print_parenthesized_type(parenthesized),
            Type::Pack(pack) => self.print_type_pack(&pack.types),
            Type::Nil(_) => self.piece("nil"),
            Type::True(_) => self.piece("true"),
            Type::False(_) => self.piece("false"),
            Type::StringLiteral(literal) => self.print_string(literal),
            Type::Variadic(variadic) => {
                self.piece("...");
                self.print_type(&variadic.ty);
            }
            Type::GenericPack(pack) => {
                self.name(pack.name.text.as_str());
                self.piece("...");
            }
            Type::Error(_) => {}
        }
    }

    fn print_named_type(&mut self, named: &NamedType<'_>) {
        if let Some(prefix) = &named.prefix {
            self.name(prefix.text.as_str());
            self.piece(".");
        }
        self.name(named.name.text.as_str());
        if let Some(generics) = &named.generics {
            self.piece("<");
            self.list(&generics.args, Self::print_type);
            self.piece(">");
        }
    }

    fn print_typeof(&mut self, expr: &Expression<'_>) {
        self.piece("typeof(");
        self.print_free_expr(expr);
        self.piece(")");
    }

    fn print_table_type(&mut self, fields: &[TypeField<'_>]) {
        self.piece("{");
        self.list(fields, Self::print_type_field);
        self.piece("}");
    }

    fn print_function_type(&mut self, function: &FunctionType<'_>) {
        if !function.attributes.is_empty() {
            self.print_function_attributes(&function.attributes);
        }
        if let Some(generics) = &function.generics {
            self.print_generic_type_list(generics);
        }
        self.piece("(");
        self.list(&function.params, Self::print_function_type_param);
        self.piece(")->");
        self.print_type(&function.return_type);
    }

    fn print_type_run(
        &mut self,
        has_leading_operator: bool,
        types: &[Type<'_>],
        operator: &'static str,
    ) {
        if has_leading_operator {
            self.piece(operator);
        }
        for (index, ty) in types.iter().enumerate() {
            if index != 0 {
                self.piece(operator);
            }
            self.print_type(ty);
        }
    }

    fn print_parenthesized_type(&mut self, parenthesized: &ParenthesizedType<'_>) {
        if !parenthesized.attributes.is_empty() {
            self.print_function_attributes(&parenthesized.attributes);
        }
        self.piece("(");
        self.print_type(&parenthesized.ty);
        self.piece(")");
    }

    fn print_type_pack(&mut self, types: &[Type<'_>]) {
        self.piece("(");
        self.list(types, Self::print_type);
        self.piece(")");
    }

    pub(super) fn print_type_field(&mut self, field: &TypeField<'_>) {
        match field {
            TypeField::Named { access, name, value, .. } => {
                self.print_access(*access);
                self.name(name.text.as_str());
                self.piece(":");
                self.print_type(value);
            }
            TypeField::Indexer { access, key, value, .. } => {
                self.print_access(*access);
                self.piece("[");
                self.print_type(key);
                self.piece("]:");
                self.print_type(value);
            }
            TypeField::Array { access, value, .. } => {
                self.print_access(*access);
                self.print_type(value);
            }
        }
    }

    /// `read` and `write` are words, which merge as names do.
    fn print_access(&mut self, access: Option<TypeFieldAccess>) {
        match access {
            Some(TypeFieldAccess::Read) => self.name("read"),
            Some(TypeFieldAccess::Write) => self.name("write"),
            None => {}
        }
    }

    fn print_function_type_param(&mut self, param: &FunctionTypeParam<'_>) {
        if let Some(name) = &param.name {
            self.name(name.text.as_str());
            self.piece(":");
        }
        self.print_type(&param.ty);
    }

    pub(super) fn print_generic_type_list(&mut self, generics: &GenericTypeList<'_>) {
        self.piece("<");
        self.list(&generics.params, Self::print_generic_type_param);
        self.piece(">");
    }

    fn print_generic_type_param(&mut self, param: &GenericTypeParam<'_>) {
        self.name(param.name.text.as_str());
        if param.is_pack {
            self.piece("...");
        }
        if let Some(default) = &param.default {
            self.piece("=");
            self.print_type(default);
        }
    }

    pub(super) fn print_type_instantiation(&mut self, type_args: &TypeArgs<'_>) {
        self.piece("<<");
        self.list(&type_args.args, Self::print_type);
        self.piece(">>");
    }
}
