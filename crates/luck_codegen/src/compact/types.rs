use luck_ast::{
    FunctionTypeParam, GenericTypeList, GenericTypeParam, Type, TypeArgs, TypeField,
    TypeFieldAccess,
};

use super::Printer;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the printer is one type with its node families split by file"
)]
impl Printer {
    pub(super) fn print_type(&mut self, ty: &Type<'_>) {
        match ty {
            Type::Name(name) => self.name(name.text.as_str()),
            Type::Named(named) => {
                if let Some(prefix) = &named.prefix {
                    self.name(prefix.text.as_str());
                    self.piece(".");
                }
                self.name(named.name.text.as_str());
                if let Some(generics) = &named.generics {
                    self.piece("<");
                    self.list(&generics.args, ",", Self::print_type);
                    self.piece(">");
                }
            }
            Type::Typeof(typeof_type) => {
                self.piece("typeof(");
                self.print_expr(&typeof_type.expr);
                self.piece(")");
            }
            Type::Table(table) => {
                self.piece("{");
                self.list(&table.fields, ",", Self::print_type_field);
                self.piece("}");
            }
            Type::Function(function) => {
                if let Some(generics) = &function.generics {
                    self.print_generic_type_list(generics);
                }
                self.piece("(");
                self.list(&function.params, ",", Self::print_function_type_param);
                self.piece(")->");
                self.print_type(&function.return_type);
            }
            Type::Optional(optional) => {
                self.print_type(&optional.ty);
                for _ in 0..optional.question_count {
                    self.piece("?");
                }
            }
            Type::Union(union) => {
                if union.has_leading_pipe {
                    self.piece("|");
                }
                self.list(&union.types, "|", Self::print_type);
            }
            Type::Intersection(intersection) => {
                if intersection.has_leading_ampersand {
                    self.piece("&");
                }
                self.list(&intersection.types, "&", Self::print_type);
            }
            Type::Parenthesized(parenthesized) => {
                self.piece("(");
                self.print_type(&parenthesized.ty);
                self.piece(")");
            }
            Type::Pack(pack) => {
                self.piece("(");
                self.list(&pack.types, ",", Self::print_type);
                self.piece(")");
            }
            Type::Nil(_) => self.piece("nil"),
            Type::True(_) => self.piece("true"),
            Type::False(_) => self.piece("false"),
            Type::StringLiteral(literal) => self.string(literal.text),
            Type::Number(literal) => self.number(literal.text),
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

    fn print_type_field(&mut self, field: &TypeField<'_>) {
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

    /// `read` and `write` lex as names.
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
        self.list(&generics.params, ",", Self::print_generic_type_param);
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
        self.list(&type_args.args, ",", Self::print_type);
        self.piece(">>");
    }
}
