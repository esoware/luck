use luck_ast::{
    FunctionType, FunctionTypeParam, GenericPackType, GenericTypeList, GenericTypeParam,
    IntersectionType, NamedType, OptionalType, ParenthesizedType, TableType, Type, TypeArgs,
    TypeField, TypePack, TypeofType, UnionType, VariadicType, access_spelling, check_type_field,
    error_node, name_spelling, singleton_spelling, type_name_spelling,
};
use luck_token::{Token, TokenKind};

use super::CompactPrinter;
use crate::separator::PieceClass;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the printer is one type with its emitters split by node family"
)]
impl CompactPrinter {
    pub(super) fn emit_type(&mut self, ty: &Type) {
        match ty {
            Type::Name { name, span } => {
                self.emit_leaf(type_name_spelling(name, *span), PieceClass::Word);
            }
            Type::Named(named) => self.emit_named_type(named),
            Type::Typeof(typeof_type) => self.emit_typeof_type(typeof_type),
            Type::Table(table) => self.emit_table_type(table),
            Type::Function(function) => self.emit_function_type(function),
            Type::Optional(optional) => self.emit_optional_type(optional),
            Type::Union(union) => self.emit_union_type(union),
            Type::Intersection(intersection) => self.emit_intersection_type(intersection),
            Type::Parenthesized(parenthesized) => self.emit_parenthesized_type(parenthesized),
            Type::Pack(pack) => self.emit_type_pack(pack),
            Type::Singleton(token) => self.emit_singleton(token),
            Type::Variadic(variadic) => self.emit_variadic_type(variadic),
            Type::GenericPack(generic_pack) => self.emit_generic_pack_type(generic_pack),
            Type::Error(span) => self.refuse(error_node(*span)),
        }
    }

    fn emit_named_type(&mut self, named: &NamedType) {
        if self.descend() {
            if let Some(module) = &named.prefix {
                self.emit_name(module);
                self.emit_fixed(".");
                self.emit_name(&named.name);
            } else {
                self.emit_type_name(&named.name);
            }
            if let Some(generics) = &named.generics {
                self.emit_type_args(generics);
            }
        }
        self.ascend();
    }

    fn emit_type_args(&mut self, args: &TypeArgs) {
        self.emit_fixed("<");
        self.emit_punctuated(&args.args, ",", Self::emit_type);
        self.emit_fixed(">");
    }

    fn emit_typeof_type(&mut self, typeof_type: &TypeofType) {
        if self.descend() {
            self.emit_fixed("typeof");
            self.emit_fixed("(");
            self.emit_expr(&typeof_type.expr);
            self.emit_fixed(")");
        }
        self.ascend();
    }

    fn emit_table_type(&mut self, table: &TableType) {
        if self.descend() {
            self.emit_fixed("{");
            // Trailing separators are dropped, as in table constructors.
            self.emit_joined(&table.fields.items, ",", Self::emit_type_field);
            self.emit_fixed("}");
        }
        self.ascend();
    }

    fn emit_type_field(&mut self, field: &TypeField) {
        if let Err(error) = check_type_field(field) {
            self.refuse(error);
        }
        match field {
            TypeField::Named { access, name, value, .. } => {
                // Luau `read`/`write` is a word, so the separator spaces it
                // from the name.
                if let Some(access_token) = access {
                    self.emit_access(access_token);
                }
                self.emit_name(name);
                self.emit_fixed(":");
                self.emit_type(value);
            }
            TypeField::Indexer { access, key, value, .. } => {
                if let Some(access_token) = access {
                    self.emit_access(access_token);
                }
                self.emit_fixed("[");
                self.emit_type(key);
                self.emit_fixed("]");
                self.emit_fixed(":");
                self.emit_type(value);
            }
            TypeField::Array { access, value, .. } => {
                if let Some(access_token) = access {
                    self.emit_access(access_token);
                }
                self.emit_type(value);
            }
        }
    }

    fn emit_function_type(&mut self, function: &FunctionType) {
        if self.descend() {
            if let Some(generics) = &function.generics {
                self.emit_generic_type_list(generics);
            }
            self.emit_fixed("(");
            self.emit_punctuated(&function.params, ",", Self::emit_function_type_param);
            self.emit_fixed(")");
            self.emit_fixed("->");
            self.emit_type(&function.return_type);
        }
        self.ascend();
    }

    fn emit_function_type_param(&mut self, param: &FunctionTypeParam) {
        if let Some(name) = &param.name {
            self.emit_name(name);
            self.emit_fixed(":");
        }
        self.emit_type(&param.type_value);
    }

    fn emit_optional_type(&mut self, optional: &OptionalType) {
        if self.descend() {
            self.emit_type(&optional.type_value);
            self.emit_fixed("?");
        }
        self.ascend();
    }

    fn emit_union_type(&mut self, union: &UnionType) {
        if self.descend() {
            if union.has_leading_pipe {
                self.emit_fixed("|");
            }
            self.emit_punctuated(&union.types, "|", Self::emit_type);
        }
        self.ascend();
    }

    fn emit_intersection_type(&mut self, intersection: &IntersectionType) {
        if self.descend() {
            if intersection.has_leading_ampersand {
                self.emit_fixed("&");
            }
            self.emit_punctuated(&intersection.types, "&", Self::emit_type);
        }
        self.ascend();
    }

    fn emit_parenthesized_type(&mut self, parenthesized: &ParenthesizedType) {
        if self.descend() {
            self.emit_fixed("(");
            self.emit_type(&parenthesized.type_value);
            self.emit_fixed(")");
        }
        self.ascend();
    }

    fn emit_type_pack(&mut self, pack: &TypePack) {
        if self.descend() {
            self.emit_fixed("(");
            self.emit_punctuated(&pack.types, ",", Self::emit_type);
            self.emit_fixed(")");
        }
        self.ascend();
    }

    fn emit_variadic_type(&mut self, variadic: &VariadicType) {
        if self.descend() {
            self.emit_fixed("...");
            self.emit_type(&variadic.type_value);
        }
        self.ascend();
    }

    fn emit_generic_pack_type(&mut self, generic_pack: &GenericPackType) {
        self.emit_type_name(&generic_pack.name);
        self.emit_fixed("...");
    }

    pub(super) fn emit_generic_type_list(&mut self, generics: &GenericTypeList) {
        self.emit_fixed("<");
        self.emit_punctuated(&generics.params, ",", Self::emit_generic_type_param);
        self.emit_fixed(">");
    }

    fn emit_generic_type_param(&mut self, param: &GenericTypeParam) {
        self.emit_name(&param.name);
        if param.is_pack {
            self.emit_fixed("...");
        }
        if let Some(default) = &param.default {
            self.emit_fixed("=");
            self.emit_type(default);
        }
    }

    /// A name that starts a type, where `typeof` can't stand.
    fn emit_type_name(&mut self, token: &Token) {
        let spelling = name_spelling(token).and_then(|name| type_name_spelling(name, token.span));
        self.emit_leaf(spelling, PieceClass::Word);
    }

    fn emit_access(&mut self, token: &Token) {
        self.emit_leaf(access_spelling(token), PieceClass::Word);
    }

    fn emit_singleton(&mut self, token: &Token) {
        let class = if matches!(token.kind, TokenKind::Number(_)) {
            PieceClass::Number
        } else if matches!(token.kind, TokenKind::StringLiteral(_)) {
            PieceClass::Other
        } else {
            PieceClass::Word
        };
        self.emit_leaf(singleton_spelling(token), class);
    }
}
